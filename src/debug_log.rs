//! Debug log: everything Luma prints, plus remote presses, page changes and
//! network failures, written to a file on the TV that survives a reboot.
//!
//! Switched on by Settings › System › Debug Log, by a file named `debug-log`
//! next to preferences.json (to catch a problem at startup), or by
//! `--debug-log`. Logs go to `logs/` next to preferences.json; the newest
//! few are kept. No tokens, passwords or secrets are ever written.
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use time::{OffsetDateTime, UtcOffset};

/// Logs kept, the current one included.
const KEEP: usize = 5;
/// Name of the file that switches the log on before Luma starts.
pub const FLAG: &str = "debug-log";

static ENABLED: AtomicBool = AtomicBool::new(false);
static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
static OFFSET: OnceLock<UtcOffset> = OnceLock::new();

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The file this run writes to, once the log is on.
pub fn current() -> Option<PathBuf> {
    FILE.lock().ok()?.clone()
}

/// Where logs go for these preferences.
pub fn directory(preferences: &Path) -> PathBuf {
    preferences
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("logs")
}

/// The switch file a person can create over SSH to log from the start.
pub fn flag(preferences: &Path) -> PathBuf {
    preferences
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(FLAG)
}

/// The TV's time zone, read once at startup (later reads can fail once
/// threads run).
pub fn set_offset(offset: UtcOffset) {
    let _ = OFFSET.set(offset);
}

/// "2026-11-11T19:04:05.123+01:00": local time, as the TV shows it.
pub fn timestamp() -> String {
    let offset = *OFFSET.get().unwrap_or(&UtcOffset::UTC);
    let now = OffsetDateTime::now_utc().to_offset(offset);
    let (h, m, _) = offset.as_hms();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}{}{:02}:{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond(),
        if h < 0 || m < 0 { '-' } else { '+' },
        h.unsigned_abs(),
        m.unsigned_abs(),
    )
}

/// Turns the log on: a new file in `dir`, and stderr (everything Luma
/// already prints) goes into it. Does nothing when it is already on.
pub fn start(dir: &Path, offset: UtcOffset, why: &str) -> Result<PathBuf, String> {
    let _ = OFFSET.set(offset);
    if let Some(path) = current() {
        ENABLED.store(true, Ordering::Relaxed);
        return Ok(path);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    prune(dir, KEEP - 1);
    // Colons are not welcome in every file system: 2026-11-11T19-04-05.
    let name = format!("luma-{}.log", timestamp()[..19].replace(':', "-"));
    let path = dir.join(name);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    redirect_stderr(file);
    *FILE.lock().map_err(|_| "log lock")? = Some(path.clone());
    ENABLED.store(true, Ordering::Relaxed);
    install_panic_hook();
    eprintln!("{} Luma debug log ({why})", timestamp());
    eprintln!("  version  {}", env!("CARGO_PKG_VERSION"));
    eprintln!("  app id   {}", crate::platform::APP_ID);
    if let Ok(release) = std::fs::read_to_string("/etc/starfish-release") {
        eprintln!("  webOS    {}", release.trim());
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    eprintln!("  args     {}", args.join(" "));
    Ok(path)
}

/// Stops writing new debug lines. Luma's own messages keep going to the
/// file until the next start.
pub fn stop() {
    if enabled() {
        eprintln!("{} Debug log switched off", timestamp());
    }
    ENABLED.store(false, Ordering::Relaxed);
}

/// Removes all but the newest `keep` logs.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("luma-") && n.ends_with(".log"))
        })
        .collect();
    // The names sort by time.
    logs.sort();
    let extra = logs.len().saturating_sub(keep);
    for old in &logs[..extra] {
        let _ = std::fs::remove_file(old);
    }
}

#[cfg(unix)]
fn redirect_stderr(file: std::fs::File) {
    use std::os::unix::io::IntoRawFd;
    // SAFETY: duplicates a valid, owned descriptor onto stderr.
    unsafe {
        libc_dup2(file.into_raw_fd(), 2);
    }
}
#[cfg(not(unix))]
fn redirect_stderr(_file: std::fs::File) {}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "dup2"]
    fn libc_dup2(old: i32, new: i32) -> i32;
}

/// A crash leaves its message and where it happened in the log.
fn install_panic_hook() {
    static HOOKED: AtomicBool = AtomicBool::new(false);
    if HOOKED.swap(true, Ordering::Relaxed) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("{} CRASH {info}", timestamp());
        eprintln!("{}", std::backtrace::Backtrace::force_capture());
        previous(info);
    }));
}

/// Only the path of a URL: query strings can carry tokens.
pub fn path_only(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// A line in the debug log, with the time in front. Free when the log is off.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::debug_log::enabled() {
            eprintln!("{} {}", $crate::debug_log::timestamp(), format_args!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strings_never_reach_the_log() {
        assert_eq!(path_only("/Items?userId=1&api_key=secret"), "/Items");
        assert_eq!(
            path_only("/library/sections#X-Plex-Token=t"),
            "/library/sections"
        );
        assert_eq!(path_only("/System/Info"), "/System/Info");
    }

    #[test]
    fn only_the_newest_logs_are_kept() {
        let dir = std::env::temp_dir().join(format!("luma-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for day in 1..=7 {
            std::fs::write(dir.join(format!("luma-2026-11-0{day}T10-00-00.log")), "x").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "keep").unwrap();
        prune(&dir, 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "luma-2026-11-05T10-00-00.log",
                "luma-2026-11-06T10-00-00.log",
                "luma-2026-11-07T10-00-00.log",
                "notes.txt"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn times_are_rfc3339_with_milliseconds_and_offset() {
        let _ = OFFSET.set(UtcOffset::from_hms(1, 0, 0).unwrap());
        let t = timestamp();
        assert_eq!(t.len(), "2026-11-11T19:04:05.123+01:00".len(), "{t}");
        assert_eq!(&t[10..11], "T");
        assert!(t.ends_with("+01:00") || t.ends_with("+00:00"), "{t}");
    }
}
