//! Tech headlines written by a small language model on the TV.
//!
//! `luma-llm` (native/llm) runs only when Luma needs a new headline and has
//! none waiting for the current weather: it loads SmolLM2-360M, writes a few
//! lines and exits, so its memory (about 280 MB) is returned straight away.
//! Each line is steered by one tech word and filtered here; a title that
//! passes keeps the model's subtitle only when that is short and about the
//! weather, otherwise it gets one of the built-in subtitles.
use crate::weather_phrases::Category;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

pub const MODEL: &str = "smollm2-360m-instruct-q4_k_m.gguf";
/// Lines asked for in one run (about half pass the filter).
const REQUESTS: usize = 6;
/// Lines kept waiting per weather.
pub const KEEP: usize = 6;
/// The model and its working memory need this much available memory.
const NEEDS_MIB: u64 = 380;
/// A run that takes longer than this is stopped.
const TIMEOUT: Duration = Duration::from_secs(240);
/// After a failed or skipped run, wait this long before trying again.
const RETRY: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// `Category::label()` of the weather it was written for.
    pub category: String,
    pub title: String,
    pub subtitle: String,
}

const EXAMPLES: &str = "\
Funny weather headlines for a TV home screen, written like computer status messages. Each line gives the weather and a tech word. The title is a short tech joke that uses the tech word. The subtitle says plainly what the weather is like.

sunny (cache): Sky cache: fully cleared. | Sunny and clear. Go outside.
cold (boot): Cold boot conditions. | It's cold out. Let your fingers warm up.
hot (fans): Fans at 100%. | It's hot out there. Drink some water.
fog (resolution): Low resolution mode. | Foggy. Visibility like 240p.
snow (pixel): Pixel-perfect snowfall. | It's snowing. Enjoy the view.
night (sleep): Entering sleep mode. | It's late. One more episode?
wind (airflow): Airflow: excessive. | Strong winds. Hold on to your hat.
cloudy (dll): Sun.dll not found. | Overcast. Try again later.
frost (kernel): Kernel frozen. | Below zero. Watch for ice.
storm (surge): Power surge warning. | Thunderstorms. Unplug the expensive stuff.
chilly (thermal): Thermal throttling: off. | It's chilly. Grab a hoodie.
rain (packet): Packet loss: rain. | It's raining. Stay on the wired connection.
rain (liquid): Liquid cooling online. | Rain all day. Umbrella recommended.
";

/// Tech words a headline is built around; one per line asked for.
const WORDS: &[&str] = &[
    "firewall",
    "buffer",
    "driver",
    "router",
    "firmware",
    "sync",
    "reboot",
    "bandwidth",
    "server",
    "compiler",
    "debug",
    "backup",
    "render",
    "frame rate",
    "shader",
    "GPU",
    "CPU",
    "battery",
    "Wi-Fi",
    "Bluetooth",
    "modem",
    "cursor",
    "keyboard",
    "download",
    "upload",
    "stream",
    "codec",
    "HDR",
    "screensaver",
    "beta",
    "version",
    "deploy",
    "merge",
    "crash",
    "404",
    "bug",
    "glitch",
    "lag",
    "latency",
    "overclock",
    "cooling",
    "voltage",
    "uptime",
    "log file",
    "terminal",
    "script",
    "loop",
    "bitrate",
    "refresh rate",
    "storage",
    "SSD",
    "RAM",
    "swap",
    "cookie",
    "autocorrect",
    "notification",
    "patch",
    "cache",
    "ping",
    "pixel",
];

/// How the prompt names each weather (as in the examples).
fn weather_name(category: Category) -> &'static str {
    match category {
        Category::ClearNice => "sunny",
        Category::Chilly => "chilly",
        Category::Cold => "cold",
        Category::Frost => "frost",
        Category::Hot => "hot",
        Category::Cloudy => "cloudy",
        Category::Rain => "rain",
        Category::Thunder => "storm",
        Category::Snow => "snow",
        Category::Wind => "wind",
        Category::Fog => "fog",
        Category::Night => "night",
        Category::Still | Category::Change => "",
    }
}

/// Words a subtitle must use to count as being about this weather.
fn weather_words(category: Category) -> &'static [&'static str] {
    match category {
        Category::ClearNice => &[
            "sun", "sunny", "sunshine", "clear", "blue", "nice", "pleasant", "bright", "warm",
        ],
        Category::Chilly => &[
            "cool", "chilly", "brisk", "crisp", "jacket", "hoodie", "nippy", "sweater",
        ],
        Category::Cold => &[
            "cold", "chilly", "freezing", "coat", "scarf", "gloves", "frosty", "icy",
        ],
        Category::Frost => &[
            "freezing", "frost", "frosty", "ice", "icy", "frozen", "zero",
        ],
        Category::Hot => &[
            "hot",
            "heat",
            "oven",
            "scorching",
            "sweltering",
            "sweat",
            "boiling",
            "sun",
        ],
        Category::Cloudy => &[
            "cloud", "cloudy", "clouds", "grey", "gray", "overcast", "dull",
        ],
        Category::Rain => &[
            "rain", "raining", "rainy", "wet", "umbrella", "drizzle", "pouring", "showers",
        ],
        Category::Thunder => &["storm", "storms", "stormy", "thunder", "lightning"],
        Category::Snow => &[
            "snow",
            "snowing",
            "snowy",
            "snowflakes",
            "flurries",
            "sleet",
        ],
        Category::Wind => &[
            "wind", "windy", "gust", "gusty", "gusts", "breeze", "blustery",
        ],
        Category::Fog => &[
            "fog",
            "foggy",
            "mist",
            "misty",
            "haze",
            "hazy",
            "visibility",
        ],
        Category::Night => &["night", "late", "bed", "sleep", "moon", "dark", "midnight"],
        Category::Still | Category::Change => &[],
    }
}

/// Words that would contradict the weather if they appeared in a headline.
const OTHER_WEATHER: &[&str] = &[
    "snow",
    "snowing",
    "rain",
    "raining",
    "sunny",
    "sunshine",
    "fog",
    "foggy",
    "thunder",
    "lightning",
    "freezing",
    "scorching",
    "overcast",
    "cloudy",
    "windy",
    "storm",
];
/// Things a launcher headline has no business saying.
const BLOCKED: &[&str] = &[
    "password", "credit", "card", "vpn", "torrent", "torrents", "diaper", "kill", "die", "dead",
    "sex", "drunk", "drugs", "gun", "hate", "stupid", "idiot", "http", "www", "com",
];

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn tidy(text: &str) -> String {
    let mut text = text.trim().trim_matches('"').trim().to_owned();
    if let Some(first) = text.chars().next()
        && first.is_lowercase()
    {
        text = first.to_uppercase().collect::<String>() + &text[first.len_utf8()..];
    }
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

fn clean_characters(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_ascii_alphanumeric() || " .,:;!?'%-°".contains(c) || c == '’')
        && !text.contains("..")
}

/// The model's line for one request, or None when it does not make a usable
/// headline. The subtitle is None when only the title is good.
pub fn accept(category: Category, word: &str, raw: &str) -> Option<(String, Option<String>)> {
    let raw = raw.trim();
    let mut parts = raw.split('|');
    let title = tidy(parts.next()?);
    let subtitle = parts.next().map(tidy);
    if parts.next().is_some() {
        return None;
    }
    let title_words = words(&title);
    let stem: String = word
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(4)
        .collect();
    let own = weather_words(category);
    let contradicts = |list: &[String]| {
        list.iter()
            .any(|w| OTHER_WEATHER.contains(&w.as_str()) && !own.contains(&w.as_str()))
    };
    let blocked = |list: &[String]| list.iter().any(|w| BLOCKED.contains(&w.as_str()));
    let title_ok = (2..=5).contains(&title_words.len())
        && (6..=32).contains(&title.chars().count())
        && clean_characters(&title)
        && title_words.iter().any(|w| w.starts_with(&stem))
        && !title.to_lowercase().contains("your computer")
        && !title_words
            .iter()
            .any(|w| w.len() == 4 && w.parse::<u32>().is_ok_and(|n| n > 1900))
        && !contradicts(&title_words)
        && !blocked(&title_words)
        && !crate::weather_phrases::known_title(&title)
        && !EXAMPLES.contains(&format!(": {title} |"));
    if !title_ok {
        return None;
    }
    let subtitle = subtitle.filter(|subtitle| {
        let list = words(subtitle);
        (3..=8).contains(&list.len())
            && subtitle.chars().count() <= 48
            && clean_characters(subtitle)
            && list.iter().any(|w| own.contains(&w.as_str()))
            && !list.iter().any(|w| w == "not" || w.ends_with("n't"))
            && !list.iter().any(|w| w.chars().any(|c| c.is_ascii_digit()))
            && !contradicts(&list)
            && !blocked(&list)
            && !EXAMPLES.contains(&format!("| {subtitle}\n"))
    });
    Some((title, subtitle))
}

/// The helper and model, when this install has them. On a desktop build,
/// LUMA_LLM_DIR points at a folder holding both (native/llm/build.sh host).
pub fn installed() -> Option<(PathBuf, PathBuf)> {
    let dir = std::env::var_os("LUMA_LLM_DIR")
        .map(PathBuf::from)
        .or_else(|| crate::platform::install_dir().ok())?;
    let helper = dir.join("luma-llm");
    let model = if dir.join(MODEL).is_file() {
        dir.join(MODEL)
    } else {
        dir.join("llm").join(MODEL)
    };
    (helper.is_file() && model.is_file()).then_some((helper, model))
}

/// MemAvailable from /proc/meminfo, in MiB.
pub fn available_mib() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))
        .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
        .map(|kib| kib / 1024)
}

pub struct Outcome {
    pub category: Category,
    /// (tech word, the model's raw line)
    pub lines: Vec<(String, String)>,
    /// Timings and memory reported by the helper.
    pub stats: Vec<String>,
    pub error: Option<String>,
    pub elapsed: Duration,
}

#[derive(Default)]
pub struct Writer {
    running: Option<Receiver<Outcome>>,
    rest_until: Option<Instant>,
    /// Recently used tech words, so consecutive runs differ.
    recent: Vec<&'static str>,
}

impl Writer {
    /// Starts a run for this weather unless one is running, a recent run
    /// failed, or memory is short. Returns a line for the debug log.
    pub fn request(&mut self, category: Category, seed: u64) -> Option<String> {
        if self.running.is_some() || weather_name(category).is_empty() {
            return None;
        }
        if self.rest_until.is_some_and(|until| Instant::now() < until) {
            return None;
        }
        let Some((helper, model)) = installed() else {
            self.rest_until = Some(Instant::now() + RETRY * 4);
            return Some("AI headlines: luma-llm or its model is not installed".into());
        };
        if let Some(mib) = available_mib()
            && mib < NEEDS_MIB
        {
            self.rest_until = Some(Instant::now() + RETRY);
            return Some(format!(
                "AI headlines: skipped, only {mib} MiB available (needs {NEEDS_MIB})"
            ));
        }
        let mut words = Vec::new();
        let mut state = seed;
        while words.len() < REQUESTS {
            // splitmix64
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            let word = WORDS[(z % WORDS.len() as u64) as usize];
            if !words.contains(&word) && !self.recent.contains(&word) {
                words.push(word);
            }
        }
        self.recent.extend(words.iter().copied());
        if self.recent.len() > 24 {
            self.recent.drain(..self.recent.len() - 24);
        }
        let threads = std::thread::available_parallelism()
            .map_or(2, |n| n.get())
            .clamp(1, 4)
            .saturating_sub(1)
            .max(1);
        let (sender, receiver) = channel();
        self.running = Some(receiver);
        let name = weather_name(category);
        let available = available_mib();
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut outcome = Outcome {
                category,
                lines: Vec::new(),
                stats: Vec::new(),
                error: None,
                elapsed: Duration::ZERO,
            };
            if let Err(error) = run(&helper, &model, threads, seed, name, &words, &mut outcome) {
                outcome.error = Some(error);
            }
            outcome.elapsed = started.elapsed();
            let _ = sender.send(outcome);
        });
        Some(format!(
            "AI headlines: writing {REQUESTS} for {name} ({} MiB available, {threads} threads)",
            available.map_or_else(|| "?".into(), |m| m.to_string())
        ))
    }

    /// A finished run, once.
    pub fn poll(&mut self) -> Option<Outcome> {
        let outcome = self.running.as_ref()?.try_recv().ok()?;
        self.running = None;
        if outcome.error.is_some() || outcome.lines.is_empty() {
            self.rest_until = Some(Instant::now() + RETRY);
        }
        Some(outcome)
    }
}

fn run(
    helper: &std::path::Path,
    model: &std::path::Path,
    threads: usize,
    seed: u64,
    weather: &str,
    words: &[&str],
    outcome: &mut Outcome,
) -> Result<(), String> {
    let mut command = Command::new(helper);
    command
        .arg(model)
        .arg(threads.to_string())
        .arg((seed as u32).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(all(target_os = "linux", feature = "webos"))]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
    }
    let mut child = command.spawn().map_err(|e| format!("start: {e}"))?;
    let mut input = String::from(EXAMPLES);
    input.push_str("%%\n");
    for word in words {
        input.push_str(&format!("{weather} ({word}):\n"));
    }
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .map_err(|e| format!("write: {e}"))?;
    }
    let stdout = child.stdout.take().ok_or("no output")?;
    let stderr = child.stderr.take().ok_or("no output")?;
    let errors = std::thread::spawn(move || {
        BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<_>>()
    });
    let (line_sender, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line_sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + TIMEOUT;
    let mut answers = Vec::new();
    loop {
        while let Ok(line) = lines.try_recv() {
            if let Some(text) = line.strip_prefix("= ") {
                answers.push(text.to_owned());
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                while let Ok(line) = lines.recv_timeout(Duration::from_millis(200)) {
                    if let Some(text) = line.strip_prefix("= ") {
                        answers.push(text.to_owned());
                    }
                }
                let errors = errors.join().unwrap_or_default();
                outcome.stats = errors
                    .iter()
                    .filter_map(|l| l.strip_prefix("stat ").map(str::to_owned))
                    .collect();
                if !status.success() && answers.is_empty() {
                    return Err(format!(
                        "luma-llm exited with {status}: {}",
                        errors.last().cloned().unwrap_or_default()
                    ));
                }
                break;
            }
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("stopped after {} s", TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(error) => return Err(format!("wait: {error}")),
        }
    }
    outcome.lines = words
        .iter()
        .zip(answers)
        .map(|(word, answer)| ((*word).to_owned(), answer))
        .collect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_lines_pass_and_odd_subtitles_are_replaced() {
        assert_eq!(
            accept(
                Category::Rain,
                "buffer",
                " Buffer overflow: raining. | Rainy day. Stay safe."
            ),
            Some((
                "Buffer overflow: raining.".into(),
                Some("Rainy day. Stay safe.".into())
            ))
        );
        // A good title keeps; a rambling subtitle does not.
        assert_eq!(
            accept(
                Category::Hot,
                "driver",
                "Driver getting hot | It's hot. Turn off the fan and change a diaper."
            ),
            Some(("Driver getting hot.".into(), None))
        );
        // The subtitle has to be about this weather.
        assert_eq!(
            accept(
                Category::ClearNice,
                "firmware",
                "Firmware update: not available. | It's not sunny."
            ),
            Some(("Firmware update: not available.".into(), None))
        );
    }

    #[test]
    fn bad_titles_are_dropped() {
        // Missing the tech word.
        assert_eq!(
            accept(Category::Rain, "router", "Rainy mess. | It's raining."),
            None
        );
        // Generic, an example, a built-in title, or about other weather.
        assert_eq!(
            accept(
                Category::Snow,
                "reboot",
                "Reboot your computer. | It's snowing."
            ),
            None
        );
        assert_eq!(
            accept(
                Category::Rain,
                "packet",
                "Packet loss: rain. | It's raining."
            ),
            None
        );
        assert_eq!(
            accept(Category::Fog, "blur", "Blur filter enabled. | Foggy."),
            None
        );
        assert_eq!(
            accept(
                Category::ClearNice,
                "sync",
                "Synchronize: no rain. | Sunny."
            ),
            None
        );
        // Too long, odd characters, or more than one separator.
        assert_eq!(
            accept(
                Category::Rain,
                "buffer",
                "Buffer is very very full of the rain water today | Wet."
            ),
            None
        );
        assert_eq!(
            accept(Category::Snow, "cache", "Cache/cache-to-cache. | Snow."),
            None
        );
        assert_eq!(accept(Category::Rain, "buffer", "Buffer. | a | b"), None);
        assert_eq!(
            accept(
                Category::Snow,
                "driver",
                "Driver 1 hit the roof 2016. | Snow."
            ),
            None
        );
    }

    /// Runs the real model: LUMA_LLM_DIR=.build/llm-host cargo test -- --ignored model_writes
    #[test]
    #[ignore]
    fn model_writes_usable_headlines() {
        let mut kept = 0;
        for (n, category) in [Category::Rain, Category::ClearNice, Category::Night]
            .into_iter()
            .enumerate()
        {
            let mut writer = Writer::default();
            let note = writer.request(category, 0x9e37_79b9 + n as u64);
            println!("{note:?}");
            let outcome = loop {
                if let Some(outcome) = writer.poll() {
                    break outcome;
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            println!(
                "{:?} {:?} {:?}",
                outcome.elapsed, outcome.error, outcome.stats
            );
            for (word, raw) in &outcome.lines {
                let verdict = accept(category, word, raw);
                println!("{word:>12}: {raw}\n{:>12}  {verdict:?}", "");
                kept += usize::from(verdict.is_some());
            }
        }
        assert!(kept >= 3, "only {kept} usable lines");
    }

    #[test]
    fn every_weather_has_words_and_a_name() {
        for category in [
            Category::ClearNice,
            Category::Chilly,
            Category::Cold,
            Category::Frost,
            Category::Hot,
            Category::Cloudy,
            Category::Rain,
            Category::Thunder,
            Category::Snow,
            Category::Wind,
            Category::Fog,
            Category::Night,
        ] {
            assert!(!weather_name(category).is_empty());
            assert!(!weather_words(category).is_empty());
            assert!(crate::weather_phrases::subtitles(category).next().is_some());
        }
    }
}
