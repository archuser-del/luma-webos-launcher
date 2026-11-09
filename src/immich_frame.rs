//! ImmichFrame wallpaper: photos from an Immich library, served by an
//! ImmichFrame server (https://immichframe.dev). A worker asks the server for
//! the next picture, fits it to the screen and hands the file to the view.
//! Nothing here runs on the SDL/GL thread, and the secret is never logged.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, GenericImageView, imageops::FilterType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

/// Seconds a photo stays on screen.
pub const INTERVALS: [u32; 5] = [15, 30, 60, 300, 900];
/// A base64 photo inside JSON; previews are a few megabytes at most.
const MAX_REPLY: u64 = 64 * 1024 * 1024;
pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;

/// Saved ImmichFrame server. The secret is the server's optional
/// `AuthenticationSecret`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ImmichFrameLink {
    pub url: String,
    pub secret: String,
    pub interval_seconds: u32,
    /// Show where and when the photo was taken.
    pub show_info: bool,
}
impl Default for ImmichFrameLink {
    fn default() -> Self {
        Self {
            url: String::new(),
            secret: String::new(),
            interval_seconds: 30,
            show_info: true,
        }
    }
}
impl ImmichFrameLink {
    pub fn linked(&self) -> bool {
        !self.url.is_empty()
    }
    pub fn normalize(&mut self) {
        self.url = normalize_url(&self.url).unwrap_or_default();
        if self.secret.len() > 512 || self.secret.chars().any(char::is_control) {
            self.secret.clear();
        }
        if !INTERVALS.contains(&self.interval_seconds) {
            self.interval_seconds = 30;
        }
    }
    /// The next longer interval, wrapping to the shortest.
    pub fn next_interval(&self) -> u32 {
        let at = INTERVALS
            .iter()
            .position(|s| *s == self.interval_seconds)
            .unwrap_or(0);
        INTERVALS[(at + 1) % INTERVALS.len()]
    }
    /// Host shown in Settings, without scheme or path.
    pub fn host(&self) -> &str {
        self.url
            .split_once("://")
            .map_or(self.url.as_str(), |p| p.1)
            .split('/')
            .next()
            .unwrap_or("")
    }
}

pub fn interval_label(seconds: u32) -> String {
    match seconds {
        s if s < 60 => format!("{s} seconds"),
        60 => "1 minute".into(),
        s => format!("{} minutes", s / 60),
    }
}

/// "192.168.1.20:8080" or a full URL, as an http(s) base without a trailing slash.
pub fn normalize_url(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() || value.len() > 300 || value.chars().any(|c| c.is_control() || c == ' ') {
        return None;
    }
    let full = if value.starts_with("http://") || value.starts_with("https://") {
        value.to_owned()
    } else if value.contains("://") {
        return None;
    } else {
        format!("http://{value}")
    };
    let parsed = url::Url::parse(&full).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.query().is_some()
    {
        return None;
    }
    Some(full)
}

/// One picture ready for the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub path: String,
    /// "Lisbon, Portugal · 12 May 2023", or empty.
    pub caption: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    // ImmichFrame answers in camelCase; older builds used PascalCase.
    value
        .as_object()
        .and_then(|o| {
            o.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .and_then(|(_, v)| v.as_str())
        })
        .unwrap_or("")
}

/// Ask the server for its next photo: (encoded image, caption).
fn fetch(link: &ImmichFrameLink) -> Result<(Vec<u8>, String), String> {
    let request = agent()
        .get(&format!(
            "{}/api/Asset/RandomImageAndInfo?clientIdentifier=Luma",
            link.url
        ))
        .header("Accept", "application/json");
    let request = if link.secret.is_empty() {
        request
    } else {
        request.header("Authorization", &format!("Bearer {}", link.secret))
    };
    let mut response = request
        .call()
        .map_err(|_| "ImmichFrame did not answer. Check the address.".to_owned())?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err("ImmichFrame refused the secret.".into()),
        404 => return Err("No ImmichFrame at this address.".into()),
        code => return Err(format!("ImmichFrame answered with error {code}.")),
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_REPLY)
        .read_to_end(&mut bytes)
        .map_err(|_| "The photo did not arrive completely.".to_owned())?;
    let reply: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "This is not an ImmichFrame server.".to_owned())?;
    let picture = STANDARD
        .decode(field(&reply, "randomImageBase64"))
        .ok()
        .filter(|p| !p.is_empty())
        .ok_or("ImmichFrame sent no photo.")?;
    // ImmichFrame joins city, state and country with bare commas.
    let place = field(&reply, "imageLocation")
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let caption = [place.as_str(), field(&reply, "photoDate").trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    Ok((picture, caption.chars().take(120).collect()))
}

/// Fit a photo to the 16:9 screen. Landscape photos fill it; portrait and
/// narrow ones are shown whole over a blurred copy of themselves.
pub fn compose(picture: &DynamicImage) -> image::RgbImage {
    let (w, h) = picture.dimensions();
    let aspect = w as f32 / h.max(1) as f32;
    let screen = WIDTH as f32 / HEIGHT as f32;
    // Close to 16:9: crop a little rather than letterbox.
    if aspect >= screen * 0.8 && aspect <= screen * 1.35 {
        return picture
            .resize_to_fill(WIDTH, HEIGHT, FilterType::Triangle)
            .to_rgb8();
    }
    // Cheap heavy blur: shrink, blur, enlarge.
    let mut canvas = picture
        .resize_to_fill(96, 54, FilterType::Triangle)
        .blur(2.5)
        .resize_exact(WIDTH, HEIGHT, FilterType::Triangle)
        .to_rgb8();
    for pixel in canvas.pixels_mut() {
        pixel.0 = pixel.0.map(|v| (v as u16 * 55 / 100) as u8);
    }
    let whole = picture
        .resize(WIDTH, HEIGHT, FilterType::Triangle)
        .to_rgb8();
    let x = (WIDTH - whole.width()) / 2;
    let y = (HEIGHT - whole.height()) / 2;
    image::imageops::replace(&mut canvas, &whole, x as i64, y as i64);
    canvas
}

/// Fetch, fit and save the next photo as `<folder>/frame-<n>.jpg`.
pub fn next_frame(link: &ImmichFrameLink, folder: &Path, n: u64) -> Result<Frame, String> {
    let (bytes, caption) = fetch(link)?;
    let picture = image::load_from_memory(&bytes)
        .map_err(|_| "ImmichFrame sent a picture Luma cannot read.".to_owned())?;
    let fitted = compose(&picture);
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let file = folder.join(format!("frame-{n}.jpg"));
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 90)
        .encode_image(&fitted)
        .map_err(|e| e.to_string())?;
    std::fs::write(&file, encoded).map_err(|e| e.to_string())?;
    // The view may still crossfade from the two before this one.
    if n >= 3 {
        let _ = std::fs::remove_file(folder.join(format!("frame-{}.jpg", n - 3)));
    }
    Ok(Frame {
        path: file.to_string_lossy().into_owned(),
        caption,
    })
}

/// Check an address typed on the phone and load its first photo.
pub fn check(
    folder: &Path,
    address: &str,
    secret: &str,
) -> Result<(ImmichFrameLink, Frame), String> {
    let url = normalize_url(address).ok_or("Type the address you open ImmichFrame with.")?;
    let link = ImmichFrameLink {
        url,
        secret: secret.trim().to_owned(),
        ..Default::default()
    };
    let frame = next_frame(&link, folder, first_number())?;
    Ok((link, frame))
}

/// File numbers start at the clock, so a restart never reuses a cached texture path.
fn first_number() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        * 10
}

pub enum Command {
    /// The wallpaper is (or is not) on screen with this server.
    Show(Option<ImmichFrameLink>),
    /// Load another photo now.
    Skip,
}
pub enum Update {
    Frame(Frame),
    Failed(String),
}
pub struct Service {
    sender: Sender<Command>,
    pub updates: Mutex<Receiver<Update>>,
}
impl Service {
    pub fn new(folder: PathBuf) -> Self {
        let (sender, commands) = mpsc::channel::<Command>();
        let (reply, updates) = mpsc::channel();
        thread::spawn(move || {
            // Leftovers of the last run are no longer referenced.
            if let Ok(entries) = std::fs::read_dir(&folder) {
                for entry in entries.flatten() {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            let mut link: Option<ImmichFrameLink> = None;
            let mut number = first_number();
            let mut due = Instant::now();
            loop {
                let wait = match &link {
                    Some(_) => due.saturating_duration_since(Instant::now()),
                    None => Duration::from_secs(3600),
                };
                match commands.recv_timeout(wait) {
                    Ok(Command::Show(next)) => {
                        // A new server or a returning wallpaper loads at once;
                        // the same server keeps its rhythm.
                        if next.as_ref().map(|l| (&l.url, &l.secret))
                            != link.as_ref().map(|l| (&l.url, &l.secret))
                        {
                            due = Instant::now();
                        }
                        link = next;
                        continue;
                    }
                    Ok(Command::Skip) => due = Instant::now(),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                let Some(current) = link.clone() else {
                    continue;
                };
                if Instant::now() < due {
                    continue;
                }
                number += 1;
                let update = match next_frame(&current, &folder, number) {
                    Ok(frame) => {
                        due = Instant::now() + Duration::from_secs(current.interval_seconds as u64);
                        Update::Frame(frame)
                    }
                    Err(error) => {
                        // A server that is down is asked again, but not hammered.
                        due = Instant::now() + Duration::from_secs(20);
                        Update::Failed(error)
                    }
                };
                if reply.send(update).is_err() {
                    break;
                }
            }
        });
        Self {
            sender,
            updates: Mutex::new(updates),
        }
    }
    pub fn send(&self, command: Command) {
        let _ = self.sender.send(command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn addresses_are_normalized_and_bad_ones_refused() {
        assert_eq!(
            normalize_url(" 192.168.1.20:8080/ ").as_deref(),
            Some("http://192.168.1.20:8080")
        );
        assert_eq!(
            normalize_url("https://frame.example.com/").as_deref(),
            Some("https://frame.example.com")
        );
        for bad in [
            "",
            "ftp://x",
            "http://user:pw@host",
            "http://a b",
            "http://h/?q=1",
        ] {
            assert_eq!(normalize_url(bad), None, "{bad}");
        }
        let link = ImmichFrameLink {
            url: "http://192.168.1.20:8080".into(),
            interval_seconds: 900,
            ..Default::default()
        };
        assert_eq!(link.host(), "192.168.1.20:8080");
        assert_eq!(link.next_interval(), 15);
        assert_eq!(interval_label(300), "5 minutes");
    }

    #[test]
    fn landscape_fills_and_portrait_keeps_the_whole_photo() {
        let wide = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            400,
            225,
            image::Rgb([200, 10, 10]),
        ));
        let fitted = compose(&wide);
        assert_eq!((fitted.width(), fitted.height()), (WIDTH, HEIGHT));
        assert_eq!(fitted.get_pixel(5, 5).0, [200, 10, 10]);
        let tall = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            300,
            600,
            image::Rgb([10, 200, 10]),
        ));
        let fitted = compose(&tall);
        assert_eq!((fitted.width(), fitted.height()), (WIDTH, HEIGHT));
        // The photo itself in the middle, a darkened blur at the sides.
        assert_eq!(fitted.get_pixel(WIDTH / 2, HEIGHT / 2).0, [10, 200, 10]);
        assert!(fitted.get_pixel(20, HEIGHT / 2).0[1] < 150);
    }

    /// A one-shot ImmichFrame stand-in; returns the request it received.
    fn serve(status: &str, body: String) -> (u16, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let status = status.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0u8; 4096];
            let n = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8_lossy(&request[..n]).into_owned()
        });
        (port, handle)
    }

    #[test]
    fn a_photo_is_fetched_with_the_secret_and_saved_for_the_screen() {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&image::RgbImage::from_pixel(
                64,
                36,
                image::Rgb([30, 60, 90]),
            ))
            .unwrap();
        let body = serde_json::json!({
            "randomImageBase64": STANDARD.encode(&jpeg),
            "thumbHashImageBase64": "",
            "photoDate": "12 May 2023",
            "imageLocation": "Lisbon,Portugal",
        })
        .to_string();
        let (port, server) = serve("200 OK", body);
        let folder = std::env::temp_dir().join(format!("luma-immich-{port}"));
        let (link, frame) = check(&folder, &format!("127.0.0.1:{port}"), " s3cret ").unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /api/Asset/RandomImageAndInfo?clientIdentifier=Luma "));
        assert!(request.contains("Bearer s3cret"), "{request}");
        assert_eq!(link.url, format!("http://127.0.0.1:{port}"));
        assert_eq!(frame.caption, "Lisbon, Portugal · 12 May 2023");
        let saved = image::open(&frame.path).unwrap();
        assert_eq!(saved.dimensions(), (WIDTH, HEIGHT));
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_wrong_secret_and_a_wrong_server_say_so() {
        let (port, server) = serve("401 Unauthorized", String::new());
        let folder = std::env::temp_dir().join(format!("luma-immich-{port}"));
        let error = check(&folder, &format!("127.0.0.1:{port}"), "x").unwrap_err();
        assert!(error.contains("secret"), "{error}");
        server.join().unwrap();
        let (port, server) = serve("200 OK", "<html>hello</html>".into());
        let error = check(&folder, &format!("127.0.0.1:{port}"), "").unwrap_err();
        assert!(error.contains("not an ImmichFrame"), "{error}");
        server.join().unwrap();
    }
}
