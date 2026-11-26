//! Video wallpapers kept on a USB drive. With Settings › Wallpaper › Save to
//! USB Drive on, a background thread copies the selected catalog clips into a
//! `LumaWallpapers` folder on the first writable USB drive, one at a time, and
//! the hardware wallpaper plays a saved clip from the drive instead of
//! streaming it. Downloads pause while a film plays, stop when the drive is
//! nearly full, and pick up where they left off after the drive comes back.
//!
//! The drive has to be visible and writable to Luma: as the Home screen it
//! runs as root, and root tools that pass USB mounts to apps work as well.
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// The folder Luma keeps on the drive.
pub const FOLDER: &str = "LumaWallpapers";
const PART: &str = ".part";
/// Room left on the drive for other things.
const RESERVE_BYTES: u64 = 1024 * 1024 * 1024;
/// A clip larger than this is not a wallpaper (and vfat stops at 4 GiB).
const MAX_CLIP_BYTES: u64 = 3 * 1024 * 1024 * 1024;
/// How often the worker looks for a drive that came or went.
const RESCAN: Duration = Duration::from_secs(20);

/// One catalog clip the drive should hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub enabled: bool,
    /// The saved-clips folder on the drive in use.
    pub folder: Option<PathBuf>,
    /// Selected clips already on the drive, and how many are selected.
    pub saved: usize,
    pub wanted: usize,
    /// The clip being copied: name, bytes so far, size (0 = unknown).
    pub current: Option<(String, u64, u64)>,
    pub free: Option<u64>,
    pub paused: bool,
    pub full: bool,
    pub error: Option<String>,
}

impl Status {
    /// The value Settings › Wallpaper shows next to Save to USB Drive.
    pub fn summary(&self) -> String {
        if !self.enabled {
            return "Off".into();
        }
        if self.folder.is_none() {
            return "No drive".into();
        }
        let count = format!("{} of {}", self.saved, self.wanted);
        if self.wanted > 0 && self.saved >= self.wanted {
            format!("{count} saved")
        } else if self.full {
            format!("Drive full · {count}")
        } else if self.paused {
            format!("Paused · {count}")
        } else if let Some((_, done, total)) = &self.current {
            match (*done * 100).checked_div(*total) {
                Some(percent) => format!("Saving {count} · {percent}%"),
                None => format!("Saving {count}"),
            }
        } else if self.error.is_some() {
            format!("Retrying · {count}")
        } else {
            count
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Wish {
    enabled: bool,
    paused: bool,
    jobs: Vec<Job>,
}

struct Shared {
    wish: Mutex<(Wish, u64)>,
    wake: Condvar,
}

static SHARED: Mutex<Option<Arc<Shared>>> = Mutex::new(None);
static STATUS: Mutex<Option<Status>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The folder in use and the clip ids saved in it.
static SAVED: RwLock<Option<(PathBuf, HashSet<String>)>> = RwLock::new(None);
/// What `configure` last sent, so an unchanged frame costs a compare.
static LAST_KEY: Mutex<Option<(bool, bool, u64, usize)>> = Mutex::new(None);

/// Called every frame from the main loop; wakes the worker only on a change.
/// `first` is the clip on screen, copied before the others.
pub fn configure(enabled: bool, paused: bool, excluded: &BTreeSet<String>, first: usize) {
    let key = (
        enabled,
        paused,
        crate::video_catalog::selection_version(excluded),
        crate::video_catalog::all().len(),
    );
    {
        let mut last = LAST_KEY.lock().unwrap();
        if last.as_ref() == Some(&key) {
            return;
        }
        *last = Some(key);
    }
    let selected = crate::video_catalog::selected(excluded);
    let start = selected.iter().position(|&i| i == first).unwrap_or(0);
    let jobs = selected[start..]
        .iter()
        .chain(&selected[..start])
        .filter_map(|&i| crate::video_catalog::all().get(i))
        .filter(|v| !v.is_custom() && v.url.starts_with("https://") && clip_name_ok(&v.id))
        .map(|v| Job {
            id: v.id.clone(),
            name: v.name.clone(),
            url: v.url.clone(),
        })
        .collect();
    let wish = Wish {
        enabled,
        paused,
        jobs,
    };
    let mut shared = SHARED.lock().unwrap();
    if shared.is_none() {
        if !enabled {
            return;
        }
        let new = Arc::new(Shared {
            wish: Mutex::new((Wish::default(), 0)),
            wake: Condvar::new(),
        });
        let worker = new.clone();
        let spawned = thread::Builder::new()
            .name("luma-usb-videos".into())
            .spawn(move || run(&worker, &DriveFinder::System));
        if spawned.is_err() {
            return;
        }
        *shared = Some(new);
    }
    if let Some(shared) = shared.as_ref() {
        let mut slot = shared.wish.lock().unwrap();
        slot.0 = wish;
        slot.1 += 1;
        shared.wake.notify_all();
    }
}

pub fn status() -> Status {
    STATUS.lock().unwrap().clone().unwrap_or_default()
}
/// Changes whenever the status does, so Settings can repaint.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}
fn publish(status: &Status) {
    let mut current = STATUS.lock().unwrap();
    if current.as_ref() != Some(status) {
        *current = Some(status.clone());
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

/// The saved copy of a catalog clip, when the drive holds one.
#[cfg_attr(
    not(all(feature = "webos", target_os = "linux", target_arch = "arm")),
    allow(dead_code)
)]
pub fn local_path(id: &str) -> Option<String> {
    let saved = SAVED.read().ok()?;
    let (folder, ids) = saved.as_ref()?;
    if !ids.contains(id) {
        return None;
    }
    let path = folder.join(id);
    path.is_file().then(|| path.to_string_lossy().into_owned())
}
/// True for a path the hardware wallpaper may open: a clip saved on the drive.
#[cfg_attr(
    not(all(feature = "webos", target_os = "linux", target_arch = "arm")),
    allow(dead_code)
)]
pub fn is_saved_path(path: &str) -> bool {
    let Ok(saved) = SAVED.read() else {
        return false;
    };
    let Some((folder, ids)) = saved.as_ref() else {
        return false;
    };
    let path = Path::new(path);
    path.parent() == Some(folder.as_path())
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| ids.contains(n))
}

/// Catalog ids are file names already ("comp_..._AVC.mov"); refuse anything
/// that could leave the folder or upset a FAT drive.
fn clip_name_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 120
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && [".mov", ".mp4", ".m4v"]
            .iter()
            .any(|ext| id.to_ascii_lowercase().ends_with(ext))
}

/// Mounted USB drives from `/proc/mounts`, writable ones only. webOS mounts
/// them under /tmp/usb; root tools may bind them elsewhere.
fn usb_mounts(mounts: &str) -> Vec<PathBuf> {
    const SKIP: [&str; 9] = [
        "proc", "sysfs", "tmpfs", "devtmpfs", "squashfs", "overlay", "cgroup", "cgroup2", "devpts",
    ];
    let mut found = Vec::new();
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let (Some(source), Some(target), Some(kind), Some(options)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let target = unescape(target);
        let usb_source = source.starts_with("/dev/sd");
        let usb_target = ["/tmp/usb/", "/media/usb", "/mnt/usb", "/run/media/"]
            .iter()
            .any(|p| target.starts_with(p));
        let writable = options.split(',').any(|o| o == "rw");
        if (usb_source || usb_target)
            && writable
            && !SKIP.contains(&kind)
            && target != "/"
            && !found
                .iter()
                .any(|p: &PathBuf| p.as_os_str() == target.as_str())
        {
            found.push(PathBuf::from(target));
        }
    }
    found
}
/// `/proc/mounts` writes spaces and the like as `\040`.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value =
                (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(value);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

enum DriveFinder {
    System,
    #[cfg(test)]
    Fixed(PathBuf),
}
impl DriveFinder {
    /// The saved-clips folder on the first drive Luma can write to. A drive
    /// that already has the folder wins.
    fn folder(&self) -> Option<PathBuf> {
        let mounts = match self {
            #[cfg(test)]
            Self::Fixed(path) => vec![path.clone()],
            Self::System => match std::env::var_os("LUMA_USB_DIR") {
                Some(dir) => vec![PathBuf::from(dir)],
                None => usb_mounts(&fs::read_to_string("/proc/mounts").unwrap_or_default()),
            },
        };
        let (with, without): (Vec<_>, Vec<_>) =
            mounts.into_iter().partition(|m| m.join(FOLDER).is_dir());
        with.into_iter()
            .chain(without)
            .map(|m| m.join(FOLDER))
            .find(|folder| writable(folder))
    }
}
fn writable(folder: &Path) -> bool {
    if fs::create_dir_all(folder).is_err() {
        return false;
    }
    let probe = folder.join(".luma-write-test");
    let ok = fs::write(&probe, b"ok").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// Free bytes on the drive holding `folder`.
#[cfg(feature = "webos")]
fn free_bytes(folder: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(folder.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}
#[cfg(not(feature = "webos"))]
fn free_bytes(_folder: &Path) -> Option<u64> {
    None
}

/// Finished clips in the folder; leftovers from a cut-off copy go.
fn scan(folder: &Path) -> HashSet<String> {
    let mut saved = HashSet::new();
    for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.ends_with(PART) {
            let _ = fs::remove_file(entry.path());
        } else if clip_name_ok(&name) && entry.metadata().is_ok_and(|m| m.is_file() && m.len() > 0)
        {
            saved.insert(name);
        }
    }
    saved
}

enum Outcome {
    Saved,
    /// The wish changed (turned off, paused, other clips); try again later.
    Interrupted,
    Full,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_body(None)
        .timeout_global(None)
        .build()
        .into()
}

/// Copies one clip into `folder` through a `.part` file, so a cut-off copy
/// never looks finished. `progress` gets (bytes so far, size) and answers
/// whether to carry on.
fn download(
    agent: &ureq::Agent,
    job: &Job,
    folder: &Path,
    free: impl Fn() -> Option<u64>,
    mut progress: impl FnMut(u64, u64) -> bool,
) -> Result<Outcome, String> {
    let response = agent.get(&job.url).call().map_err(|e| e.to_string())?;
    let total = response.body().content_length().unwrap_or(0);
    if total > MAX_CLIP_BYTES {
        return Err(format!("{} is too large", job.name));
    }
    if free().is_some_and(|free| free < total.max(200 * 1024 * 1024) + RESERVE_BYTES) {
        return Ok(Outcome::Full);
    }
    let part = folder.join(format!("{}{PART}", job.id));
    let finished = folder.join(&job.id);
    let result = (|| {
        let mut file = fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut reader = response.into_body().into_reader();
        let mut buffer = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        let mut last_report = Instant::now();
        loop {
            let n = reader.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if done > MAX_CLIP_BYTES {
                return Err(format!("{} is too large", job.name));
            }
            if last_report.elapsed() >= Duration::from_millis(500) {
                last_report = Instant::now();
                if !progress(done, total) {
                    return Ok(Outcome::Interrupted);
                }
                if free().is_some_and(|free| free < RESERVE_BYTES / 2) {
                    return Ok(Outcome::Full);
                }
            }
        }
        if total > 0 && done != total {
            return Err(format!(
                "{} ended early ({done} of {total} bytes)",
                job.name
            ));
        }
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&part, &finished).map_err(|e| e.to_string())?;
        Ok(Outcome::Saved)
    })();
    if !matches!(result, Ok(Outcome::Saved)) {
        let _ = fs::remove_file(&part);
    }
    result
}

fn run(shared: &Shared, finder: &DriveFinder) {
    let agent = agent();
    let mut seen = u64::MAX;
    let mut wish = Wish::default();
    let mut failures: u32 = 0;
    let mut retry_at: Option<Instant> = None;
    let mut status = Status::default();
    let mut logged_folder: Option<PathBuf> = None;
    loop {
        // Wait for a change, a rescan or the end of a retry pause.
        {
            let mut slot = shared.wish.lock().unwrap();
            let busy = wish.enabled
                && !wish.paused
                && retry_at.is_none_or(|at| Instant::now() >= at)
                && status.folder.is_some()
                && !status.full
                && status.saved < status.wanted;
            if slot.1 == seen && !busy {
                let wait = retry_at
                    .map(|at| at.saturating_duration_since(Instant::now()).min(RESCAN))
                    .unwrap_or(RESCAN);
                slot = shared.wake.wait_timeout(slot, wait).unwrap().0;
            }
            if slot.1 != seen {
                seen = slot.1;
                wish = slot.0.clone();
                retry_at = None;
                failures = 0;
            }
        }
        status.enabled = wish.enabled;
        status.paused = wish.paused;
        status.wanted = wish.jobs.len();
        if !wish.enabled {
            *SAVED.write().unwrap() = None;
            status = Status::default();
            publish(&status);
            continue;
        }
        let folder = finder.folder();
        if folder != logged_folder {
            match &folder {
                Some(f) => crate::debug_log!("USB wallpapers: using {}", f.display()),
                None => crate::debug_log!("USB wallpapers: no writable USB drive"),
            }
            logged_folder = folder.clone();
        }
        let Some(folder) = folder else {
            *SAVED.write().unwrap() = None;
            status.folder = None;
            status.saved = 0;
            status.current = None;
            publish(&status);
            continue;
        };
        let on_drive = scan(&folder);
        status.saved = wish
            .jobs
            .iter()
            .filter(|j| on_drive.contains(&j.id))
            .count();
        status.folder = Some(folder.clone());
        status.free = free_bytes(&folder);
        status.full = status
            .free
            .is_some_and(|free| free < RESERVE_BYTES + 200 * 1024 * 1024);
        *SAVED.write().unwrap() = Some((folder.clone(), on_drive.clone()));
        publish(&status);
        if wish.paused || status.full || retry_at.is_some_and(|at| Instant::now() < at) {
            continue;
        }
        let Some(job) = wish
            .jobs
            .iter()
            .find(|j| !on_drive.contains(&j.id))
            .cloned()
        else {
            continue;
        };
        status.current = Some((job.name.clone(), 0, 0));
        publish(&status);
        let started = Instant::now();
        let outcome = download(
            &agent,
            &job,
            &folder,
            || free_bytes(&folder),
            |done, total| {
                status.current = Some((job.name.clone(), done, total));
                publish(&status);
                let slot = shared.wish.lock().unwrap();
                slot.1 == seen
            },
        );
        status.current = None;
        match outcome {
            Ok(Outcome::Saved) => {
                failures = 0;
                status.error = None;
                let size = fs::metadata(folder.join(&job.id)).map_or(0, |m| m.len());
                crate::debug_log!(
                    "USB wallpapers: saved {} ({} MB in {} s)",
                    job.name,
                    size / 1_000_000,
                    started.elapsed().as_secs()
                );
                if let Ok(mut saved) = SAVED.write()
                    && let Some((_, ids)) = saved.as_mut()
                {
                    ids.insert(job.id.clone());
                }
            }
            Ok(Outcome::Interrupted) => {}
            Ok(Outcome::Full) => {
                status.full = true;
                crate::debug_log!("USB wallpapers: the drive is full");
            }
            Err(error) => {
                failures += 1;
                let pause = Duration::from_secs(30 * u64::from(failures.min(20)));
                crate::debug_log!(
                    "USB wallpapers: {} failed: {error}; next try in {} s",
                    job.name,
                    pause.as_secs()
                );
                status.error = Some(error);
                retry_at = Some(Instant::now() + pause);
            }
        }
        publish(&status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn usb_drives_come_from_writable_usb_mounts() {
        let mounts = "\
/dev/root / ext4 ro,relatime 0 0
tmpfs /tmp tmpfs rw,nosuid 0 0
/dev/sda1 /tmp/usb/sda/sda1 vfat rw,relatime,fmask=0022 0 0
/dev/sdb1 /tmp/usb/sdb/sdb1 exfat ro,relatime 0 0
/dev/sdc1 /tmp/usb/sdc/My\\040Drive tntfs rw,relatime 0 0
/dev/sda1 /media/developer/usb vfat rw,relatime 0 0
/dev/mmcblk0p40 /media/developer ext4 rw,relatime 0 0
";
        assert_eq!(
            usb_mounts(mounts),
            vec![
                PathBuf::from("/tmp/usb/sda/sda1"),
                PathBuf::from("/tmp/usb/sdc/My Drive"),
                PathBuf::from("/media/developer/usb"),
            ]
        );
    }

    #[test]
    fn only_catalog_style_file_names_are_saved() {
        assert!(clip_name_ok(
            "comp_A105_C002_v06_SDR_FINAL_25062018_SDR_2K_AVC.mov"
        ));
        assert!(clip_name_ok(
            "content-d1b2090f-f2ad-49fa-bbc7-4ec64957680b.mp4"
        ));
        assert!(clip_name_ok("robinfrcd_reunion_CapMechant_1080p_h264.m4v"));
        for bad in [
            "",
            "../x.mov",
            "a/b.mov",
            ".hidden.mov",
            "clip.mov.part",
            "x.txt",
        ] {
            assert!(!clip_name_ok(bad), "{bad}");
        }
        assert!(
            crate::video_catalog::all()
                .iter()
                .filter(|v| !v.is_custom())
                .all(|v| clip_name_ok(&v.id)),
            "every catalog clip can be saved"
        );
    }

    #[test]
    fn summary_reads_like_a_settings_value() {
        let mut status = Status::default();
        assert_eq!(status.summary(), "Off");
        status.enabled = true;
        assert_eq!(status.summary(), "No drive");
        status.folder = Some(PathBuf::from("/tmp/usb/sda/sda1/LumaWallpapers"));
        status.wanted = 289;
        status.saved = 12;
        status.current = Some(("London".into(), 50, 200));
        assert_eq!(status.summary(), "Saving 12 of 289 · 25%");
        status.paused = true;
        assert_eq!(status.summary(), "Paused · 12 of 289");
        status.full = true;
        assert_eq!(status.summary(), "Drive full · 12 of 289");
        status.saved = 289;
        assert_eq!(status.summary(), "289 of 289 saved");
    }

    /// Serves `body` once per connection over plain HTTP.
    fn serve(body: Vec<u8>, connections: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming().take(connections).flatten() {
                let mut stream = stream;
                let mut head = [0u8; 2048];
                let _ = stream.read(&mut head);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: video/mp4\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}/clip.mov")
    }

    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "luma-usb-{name}-{}-{}",
            std::process::id(),
            GENERATION.load(Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_download_lands_complete_and_leftovers_are_cleared() {
        let folder = temp_folder("download");
        let body: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let job = Job {
            id: "clip_test.mov".into(),
            name: "Test".into(),
            url: serve(body.clone(), 1),
        };
        let outcome = download(&agent(), &job, &folder, || None, |_, _| true).unwrap();
        assert!(matches!(outcome, Outcome::Saved));
        assert_eq!(fs::read(folder.join("clip_test.mov")).unwrap(), body);
        assert!(!folder.join("clip_test.mov.part").exists());

        fs::write(folder.join("other.mov.part"), b"half").unwrap();
        fs::write(folder.join("notes.txt"), b"x").unwrap();
        let saved = scan(&folder);
        assert_eq!(saved, HashSet::from(["clip_test.mov".to_owned()]));
        assert!(!folder.join("other.mov.part").exists());
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_nearly_full_drive_takes_nothing() {
        let folder = temp_folder("full");
        let job = Job {
            id: "clip_full.mov".into(),
            name: "Full".into(),
            url: serve(vec![1; 1000], 1),
        };
        let outcome = download(&agent(), &job, &folder, || Some(10_000_000), |_, _| true).unwrap();
        assert!(matches!(outcome, Outcome::Full));
        assert!(fs::read_dir(&folder).unwrap().next().is_none());
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_worker_fills_the_drive_and_serves_saved_paths() {
        let drive = temp_folder("worker");
        let folder = drive.join(FOLDER);
        let shared = Arc::new(Shared {
            wish: Mutex::new((
                Wish {
                    enabled: true,
                    paused: false,
                    jobs: vec![
                        Job {
                            id: "one.mov".into(),
                            name: "One".into(),
                            url: serve(vec![7; 4000], 1),
                        },
                        Job {
                            id: "two.mp4".into(),
                            name: "Two".into(),
                            url: serve(vec![9; 5000], 1),
                        },
                    ],
                },
                1,
            )),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        let finder = DriveFinder::Fixed(drive.clone());
        thread::spawn(move || run(&worker, &finder));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !(folder.join("one.mov").is_file() && folder.join("two.mp4").is_file()) {
            assert!(Instant::now() < deadline, "the worker saved both clips");
            thread::sleep(Duration::from_millis(20));
        }
        // SAVED and STATUS are process-wide; another test may not touch them.
        while status().saved < 2 {
            assert!(Instant::now() < deadline, "status counts both clips");
            thread::sleep(Duration::from_millis(20));
        }
        let path = local_path("two.mp4").unwrap();
        assert_eq!(Path::new(&path), folder.join("two.mp4"));
        assert!(is_saved_path(&path));
        assert!(!is_saved_path(&folder.join("three.mp4").to_string_lossy()));
        assert_eq!(status().summary(), "2 of 2 saved");
        {
            let mut slot = shared.wish.lock().unwrap();
            slot.0.enabled = false;
            slot.1 += 1;
            shared.wake.notify_all();
        }
        while status().enabled {
            assert!(Instant::now() < deadline, "the worker stands down");
            thread::sleep(Duration::from_millis(20));
        }
        let _ = fs::remove_dir_all(&drive);
    }
}
