//! Blocking platform work runs on one worker, never in the SDL/UI thread.
//! TV calls originate from this native executable's LS2 connection, not a CLI
//! process trying to impersonate its application identity.
use crate::{
    app_artwork,
    model::{App, Input, Inventory, demo_inventory, valid_app_id},
};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::mpsc, thread};

#[path = "platform-install.rs"]
mod installation;

pub const APP_ID: &str = "com.archbtw.luma";
pub const HOME_ID: &str = "com.webos.app.home";
pub const FULL_HOME_ID: &str = "com.webos.app.fullhome";

pub fn is_home_id(id: &str) -> bool {
    matches!(id, HOME_ID | FULL_HOME_ID)
}

/// Keep the window surface and SAM lifecycle identity aligned with the executed Home app.
fn home_identity(executable: Option<&std::path::Path>, metadata: Option<&Value>) -> &'static str {
    if let Some(executable) = executable {
        if executable == std::path::Path::new("/usr/bin/com.webos.app.fullhome") {
            return FULL_HOME_ID;
        }
        if executable == std::path::Path::new(LG_HOME_EXE) {
            return HOME_ID;
        }
    }
    if metadata.is_some_and(|value| {
        ["nid", "appId"]
            .iter()
            .any(|key| value.get(key).and_then(Value::as_str) == Some(FULL_HOME_ID))
    }) {
        FULL_HOME_ID
    } else {
        HOME_ID
    }
}

/// Derive the identity from SAM's actual launch, never from a saved preference.
pub fn home_id() -> &'static str {
    let executable = std::env::current_exe().ok();
    let metadata = std::env::args()
        .nth(1)
        .and_then(|arg| serde_json::from_str::<Value>(&arg).ok());
    home_identity(executable.as_deref(), metadata.as_ref())
}

/// LG's Home executable. The root integration may bind-mount Luma over it (webOS 5/6/24), so
/// sam starts Luma under LG's own Home manifest and LS2 role (lifecycle v2, keep-alive).
pub const LG_HOME_EXE: &str = "/usr/bin/com.webos.app.home";

/// Whether this process runs in place of LG's Home executable.
pub fn runs_as_lg_home() -> bool {
    std::env::current_exe().is_ok_and(|exe| {
        exe == std::path::Path::new(LG_HOME_EXE)
            || exe == std::path::Path::new("/usr/bin/com.webos.app.fullhome")
    })
}

/// While this file exists, a staged Home (webOS 10/11) does not register with sam and is
/// restarted per Home key as before. Luma creates it when a relaunch did not bring it to the
/// front; delete it to try registering again.
#[cfg_attr(
    not(all(feature = "webos", target_os = "linux", target_arch = "arm")),
    allow(dead_code)
)]
pub const HOME_LIFECYCLE_OFF: &str = "/var/lib/luma-home/no-lifecycle";

/// Whether a relaunch actually brought a staged Home (webOS 10/11) to the front. Only the
/// compositor's focus or remote input counts: SDL_ShowWindow emits Shown by itself, so Shown
/// and Restored say nothing about whether the raise reached the screen.
#[derive(Debug, Default)]
pub struct ForegroundCheck {
    deadline: Option<std::time::Instant>,
}

impl ForegroundCheck {
    pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

    /// A relaunch arrived. `in_front`: Luma was already focused and visible, so nothing to check.
    #[cfg_attr(
        not(all(feature = "webos", target_os = "linux", target_arch = "arm")),
        allow(dead_code)
    )]
    pub fn relaunched(&mut self, now: std::time::Instant, in_front: bool) {
        if !in_front {
            self.deadline = Some(now + Self::TIMEOUT);
        }
    }

    /// The compositor focused Luma, or remote input reached it.
    pub fn confirmed(&mut self) {
        self.deadline = None;
    }

    /// Whether a pending relaunch has failed by `now`. Reports each failure once.
    pub fn failed(&mut self, now: std::time::Instant) -> bool {
        let failed = self.deadline.is_some_and(|deadline| now >= deadline);
        if failed {
            self.deadline = None;
        }
        failed
    }
}

/// What sam asks of a registered Home, from one lifecycle reply. Only TV builds register.
#[cfg_attr(
    not(all(feature = "webos", target_os = "linux", target_arch = "arm")),
    allow(dead_code)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// Bring the running instance to the front instead of a restart.
    Relaunch,
    /// Exit.
    Close,
}

impl Lifecycle {
    /// Lifecycle v2 (`registerApp`) names the event in `event`; v1 (`registerNativeApp`) in
    /// `message`. Other replies (`registered`, `pause`, keep-alive) need no action.
    #[cfg_attr(
        not(all(feature = "webos", target_os = "linux", target_arch = "arm")),
        allow(dead_code)
    )]
    pub fn from_reply(reply: &Value) -> Option<Self> {
        let name = reply
            .get("event")
            .or_else(|| reply.get("message"))
            .and_then(Value::as_str)?;
        match name {
            "relaunch" => Some(Self::Relaunch),
            "close" => Some(Self::Close),
            _ => None,
        }
    }
}

/// Directory holding assets/, lib/ and splash/: next to the executable, or the installed
/// package when the executable runs in place of LG's Home.
pub fn install_dir() -> Result<std::path::PathBuf, String> {
    static DIRECTORY: std::sync::OnceLock<Result<std::path::PathBuf, String>> =
        std::sync::OnceLock::new();
    DIRECTORY
        .get_or_init(|| {
            let executable = std::env::current_exe().map_err(|error| error.to_string())?;
            installation::find_directory(&executable, installation::directories())
        })
        .clone()
}
/// LG's web browser; `target` in its launch parameters is the page to open.
pub const BROWSER_ID: &str = "com.webos.app.browser";

pub fn redirect_to_home() -> Result<bool, String> {
    // The root-only integration mounts this marker with its native Home
    // manifest, or writes it to /var/run on webOS 5/6/24, where it mounts this
    // executable over LG's (a tile's jail sees /usr/bin without that mount).
    // A normal tile launch then opens that same instance instead of starting
    // a second renderer writing the same preferences.
    let marked = |file| std::fs::read_to_string(file).is_ok_and(|marker| marker.trim() == APP_ID);
    if marked("/usr/palm/applications/com.webos.app.home/.luma-home")
        || marked("/var/run/luma-home")
    {
        thread::spawn(|| {
            let target = std::fs::read_to_string("/var/run/luma-home-target")
                .ok()
                .filter(|id| is_home_id(id.trim()))
                .map_or_else(|| HOME_ID.to_owned(), |id| id.trim().to_owned());
            call(
                "luna://com.webos.applicationManager/launch",
                json!({"id": target}),
            )
        })
        .join()
        .map_err(|_| "Home launch worker failed".to_string())??;
        return Ok(true);
    }
    Ok(false)
}

pub enum Request {
    Refresh,
    Launch(String),
    /// Open this http(s) address in the TV's web browser.
    OpenUrl(String),
    MoonlightLaunch(crate::moonlight::MoonlightLink, i64),
    /// Close another app (best effort; webOS may refuse without root).
    CloseApp(String),
    /// Open Luma's web player app with these launch parameters. They hold
    /// signed URLs and a token, so they are never logged.
    WebPlayer(Value),
    /// Read root features, after applying a change when one is given.
    Root(Option<RootSet>),
    /// Which HDMI ports carry a picture right now.
    Signals,
}
pub enum Response {
    Inventory(Result<(Inventory, Option<String>), String>),
    Launched(Result<(), String>),
    /// `None` when Homebrew Channel has no root: Luma runs as a plain app.
    Root(Result<Option<RootStatus>, String>),
    /// (HDMI port number, picture present).
    Signals(Vec<(u8, bool)>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RootStatus {
    /// Luma opens when the TV turns on (boot and Quick Start+ wake).
    pub wake: bool,
    /// LG «Home Auto Launch»; `None` when the TV did not answer.
    pub home_auto_launch: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RootSet {
    Wake(bool),
    HomeAutoLaunch(bool),
}

pub struct Platform {
    sender: mpsc::Sender<Request>,
    pub receiver: mpsc::Receiver<Response>,
    pub webos: bool,
}

impl Platform {
    pub fn new(webos: bool) -> Self {
        let (sender, requests) = mpsc::channel();
        let (responses, receiver) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(request) = requests.recv() {
                let response = match request {
                    Request::Refresh => Response::Inventory(if webos {
                        inventory()
                    } else {
                        Ok((demo_inventory(), None))
                    }),
                    Request::Signals => Response::Signals(if webos {
                        hdmi_signals().unwrap_or_default()
                    } else {
                        vec![]
                    }),
                    Request::Root(change) => Response::Root(if webos {
                        root_features(change)
                    } else {
                        Ok(std::env::var_os("LUMA_ROOT_PREVIEW").map(|_| RootStatus {
                            wake: true,
                            home_auto_launch: Some(false),
                        }))
                    }),
                    Request::WebPlayer(params) => Response::Launched(if webos {
                        // Launch directly. A getAppInfo preflight may require extra
                        // permissions on some webOS versions even when launching works.
                        // Report the missing companion clearly without masking other errors.
                        call(
                            "luna://com.webos.applicationManager/launch",
                            json!({"id": WEB_PLAYER_ID, "params": params}),
                        )
                        .map(|_| ())
                        .map_err(|launch_error| {
                            if player_not_installed(&launch_error)
                                || call(
                                    "luna://com.webos.applicationManager/getAppInfo",
                                    json!({"id": WEB_PLAYER_ID}),
                                )
                                .err()
                                .is_some_and(|error| player_not_installed(&error))
                            {
                                INSTALL_PLAYER_MESSAGE.to_owned()
                            } else {
                                launch_error
                            }
                        })
                    } else {
                        Ok(())
                    }),
                    Request::MoonlightLaunch(link, id) => Response::Launched(if webos {
                        call("luna://com.webos.applicationManager/launch",
                            json!({"id":"com.limelight.webos","params":crate::moonlight::launch_params(&link,id)}))
                        .map(|_| ())
                    } else {
                        Ok(())
                    }),
                    Request::CloseApp(id) => {
                        if webos && valid_app_id(&id) {
                            let _ = call(
                                "luna://com.webos.service.applicationmanager/closeByAppId",
                                json!({ "id": id }),
                            );
                        }
                        continue;
                    }
                    Request::OpenUrl(url) => Response::Launched(if !webos {
                        Ok(())
                    } else if !(url.starts_with("http://") || url.starts_with("https://"))
                        || url.len() > 2048
                        || url.chars().any(char::is_control)
                    {
                        Err("Invalid web address".into())
                    } else {
                        call(
                            "luna://com.webos.applicationManager/launch",
                            json!({"id": BROWSER_ID, "params": {"target": url}}),
                        )
                        .map(|_| ())
                    }),
                    Request::Launch(id) => Response::Launched(if webos {
                        if crate::scroll_preview::is_placeholder(&id) {
                            Err("Scroll preview cards cannot be launched".into())
                        } else if !valid_app_id(&id) {
                            Err("Invalid application identifier".into())
                        } else {
                            call(
                                "luna://com.webos.applicationManager/launch",
                                json!({"id": id}),
                            )
                            .map(|_| ())
                        }
                    } else {
                        Ok(())
                    }),
                };
                if responses.send(response).is_err() {
                    break;
                }
            }
        });
        Self {
            sender,
            receiver,
            webos,
        }
    }

    pub fn send(&self, request: Request) {
        let _ = self.sender.send(request);
    }
}

/// Luma's own player: a small web app around the TV's web video element.
pub const WEB_PLAYER_ID: &str = "com.archbtw.luma.player";
pub const INSTALL_PLAYER_MESSAGE: &str = "Install Luma Player from Homebrew Channel";

// webOS versions use slightly different texts for unknown application IDs.
fn player_not_installed(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "not installed",
        "not found",
        "does not exist",
        "unknown application",
        "invalid appid",
    ]
    .iter()
    .any(|part| error.contains(part))
}

fn call(uri: &str, payload: Value) -> Result<Value, String> {
    #[cfg(all(feature = "webos", target_os = "linux", target_arch = "arm"))]
    let response = crate::luna::call(uri, &payload);
    #[cfg(not(all(feature = "webos", target_os = "linux", target_arch = "arm")))]
    let response: Result<Value, String> = {
        let _ = payload;
        Err("Native Luna services are only available in an ARM webOS build".into())
    };
    response.and_then(checked_response).map_err(|error| {
        let error = format!("{uri}: {error}");
        eprintln!("Luma TV service: {error}");
        error
    })
}

fn checked_response(value: Value) -> Result<Value, String> {
    if value.get("returnValue").and_then(Value::as_bool) != Some(true) {
        return Err(value
            .get("errorText")
            .and_then(Value::as_str)
            .unwrap_or("The TV denied this action")
            .chars()
            .take(240)
            .collect());
    }
    Ok(value)
}

fn inventory_read(uri: &str, field: &str) -> Result<Value, String> {
    // Without root, ask the service as Luma itself; keep the root error when
    // the TV refuses that too.
    root_inventory_read(uri, field).or_else(|root_error| {
        call(uri, json!({}))
            .ok()
            .and_then(|reply| reply.get(field).cloned())
            .ok_or(root_error)
    })
}

fn root_inventory_read(uri: &str, field: &str) -> Result<Value, String> {
    // Only these fixed inventory commands use Homebrew's elevated service.
    // Use the same one-reply invocation as the web launcher, without guessing
    // CLI behavior from a firmware-version string.
    let command = format!(
        "if [ \"$(id -u)\" = 0 ]; then /usr/bin/luna-send -n 1 -w 4000 {uri} '{{}}'; else printf '%s\\n' '{{\"returnValue\":false,\"errorText\":\"Homebrew Channel needs root access\"}}'; fi"
    );
    let envelope = call(
        "luna://org.webosbrew.hbchannel.service/exec",
        json!({"command": command}),
    )?;
    inventory_payload(&envelope, field).map_err(|error| {
        let error = format!("{uri}: {error}");
        eprintln!("Luma TV inventory: {error}");
        error
    })
}

pub fn read_app_icons(source: &app_artwork::Source) -> Result<Vec<Vec<u8>>, String> {
    if !cfg!(feature = "webos") {
        return Err("Installed artwork is only read on TV builds".into());
    }
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let command = format!(
        "/usr/bin/node -e {} {}",
        quote(include_str!("../tv/read-app-icon.cjs")),
        quote(&serde_json::to_string(source).map_err(|e| e.to_string())?)
    );
    let envelope = call(
        "luna://org.webosbrew.hbchannel.service/exec",
        json!({"command": command}),
    )?;
    let icons = inventory_payload(&envelope, "icons")?;
    let mut remaining = app_artwork::MAX_FILE;
    icons
        .as_array()
        .ok_or("Invalid installed icon reply")?
        .iter()
        .map(|value| {
            let hex = value.as_str().ok_or("Invalid installed icon data")?;
            if hex.len() % 2 != 0 || hex.len() / 2 > remaining {
                return Err("Installed icon reply exceeds its size limit".into());
            }
            remaining -= hex.len() / 2;
            hex.as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    let digit = |b: u8| {
                        (b as char)
                            .to_digit(16)
                            .ok_or("Invalid installed icon encoding")
                    };
                    Ok((digit(pair[0])? * 16 + digit(pair[1])?) as u8)
                })
                .collect()
        })
        .collect()
}

fn output_excerpt(output: &str) -> String {
    output
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .take(320)
        .collect()
}

fn inventory_payload(envelope: &Value, field: &str) -> Result<Value, String> {
    let stdout = envelope
        .get("stdoutString")
        .and_then(Value::as_str)
        .unwrap_or("");
    let stderr = envelope
        .get("stderrString")
        .and_then(Value::as_str)
        .unwrap_or("");
    let failure = |reason: &str| {
        format!(
            "{reason}; stdout: {}; stderr: {}",
            output_excerpt(stdout),
            output_excerpt(stderr),
        )
    };
    for key in ["returnCode", "exitCode"] {
        if let Some(code) = envelope.get(key)
            && code != 0
            && code != "0"
        {
            return Err(failure(&format!(
                "Homebrew inventory command failed ({key}={code})"
            )));
        }
    }
    if stdout.trim().is_empty() {
        return Err(failure("Homebrew returned no inventory"));
    }
    if stdout.len() > 2 * 1024 * 1024 {
        return Err("Homebrew inventory exceeds the 2 MiB limit".into());
    }
    // Parse an entire JSON object, including pretty-printed multi-line replies.
    // A stream parser permits CLI timing text after that object; a preamble can
    // precede its opening brace. Never mistake a nested one-line app for a reply.
    let start = stdout
        .find('{')
        .ok_or_else(|| failure("Homebrew inventory contains no JSON object"))?;
    let response = serde_json::Deserializer::from_str(&stdout[start..])
        .into_iter::<Value>()
        .next()
        .ok_or_else(|| failure("Homebrew inventory contains no JSON reply"))?
        .map_err(|error| {
            failure(&format!(
                "Homebrew inventory JSON could not be parsed: {error}"
            ))
        })?;
    if !response.is_object() || !response.get("returnValue").is_some_and(Value::is_boolean) {
        return Err(failure("Homebrew inventory JSON is not a Luna response"));
    }
    let response = checked_response(response)?;
    response
        .get(field)
        .cloned()
        .ok_or_else(|| format!("Missing TV inventory field: {field}"))
}

const WAKE_SCRIPT: &str = "/var/lib/webosbrew/init.d/71-luma-wake";
const WAKE_OFF: &str = "/var/lib/webosbrew/luma-autostart-disabled";

fn root_features(change: Option<RootSet>) -> Result<Option<RootStatus>, String> {
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    // Stop the running watcher and its subscription before switching.
    let stop = "p=$(cat /tmp/luma-wake.pid 2>/dev/null); if [ -n \"$p\" ]; then pkill -P \"$p\"; kill \"$p\"; fi; rm -f /tmp/luma-wake.pid";
    let apply = match change {
        None => String::new(),
        Some(RootSet::Wake(true)) => format!(
            "{stop}; printf '%s' {} > {WAKE_SCRIPT} && chmod 755 {WAKE_SCRIPT} && rm -f {WAKE_OFF} && {WAKE_SCRIPT}; ",
            quote(include_str!("../tv/luma-wake.sh"))
        ),
        Some(RootSet::Wake(false)) => format!("touch {WAKE_OFF}; {stop}; "),
        Some(RootSet::HomeAutoLaunch(on)) => format!(
            "/usr/bin/luna-send -n 1 -w 4000 luna://com.webos.settingsservice/setSystemSettings '{{\"category\":\"general\",\"settings\":{{\"homeAutoLaunch\":\"{}\"}}}}' >/dev/null; ",
            if on { "on" } else { "off" }
        ),
    };
    // Gamepad watcher for a double Start+Select during a stream (pad_watch).
    // Restarted when Luma was updated, so it always runs the current binary.
    let bin = "/media/developer/apps/usr/palm/applications/com.archbtw.luma/luma";
    let pid = crate::pad_watch::PIDFILE;
    let pad = format!(
        "p=$(cat {pid} 2>/dev/null); if [ -n \"$p\" ] && kill -0 \"$p\" 2>/dev/null; then \
         [ {bin} -nt {pid} ] && kill \"$p\" && p=; else p=; fi; \
         if [ -z \"$p\" ] && [ -x {bin} ]; then nohup {bin} --pad-watch > /tmp/luma-pad-watch.log 2>&1 < /dev/null & fi; "
    );
    let command = format!(
        "if [ \"$(id -u)\" != 0 ]; then echo noroot; exit 0; fi; {apply}{pad}echo root; \
         if [ -f {WAKE_SCRIPT} ] && [ ! -e {WAKE_OFF} ]; then echo wake; fi; \
         /usr/bin/luna-send -n 1 -w 4000 luna://com.webos.settingsservice/getSystemSettings '{{\"category\":\"general\",\"keys\":[\"homeAutoLaunch\"]}}'"
    );
    // Without Homebrew Channel the exec service does not exist: no root.
    let Ok(envelope) = call(
        "luna://org.webosbrew.hbchannel.service/exec",
        json!({"command": command}),
    ) else {
        return Ok(None);
    };
    let stdout = envelope
        .get("stdoutString")
        .and_then(Value::as_str)
        .unwrap_or("");
    Ok(parse_root_status(stdout))
}

pub fn parse_root_status(stdout: &str) -> Option<RootStatus> {
    let lines: Vec<&str> = stdout.lines().map(str::trim).collect();
    if !lines.contains(&"root") {
        return None;
    }
    let compact: String = stdout.chars().filter(|c| !c.is_whitespace()).collect();
    Some(RootStatus {
        wake: lines.contains(&"wake"),
        home_auto_launch: if compact.contains("\"homeAutoLaunch\":\"on\"") {
            Some(true)
        } else if compact.contains("\"homeAutoLaunch\":\"off\"") {
            Some(false)
        } else {
            None
        },
    })
}

/// Picture present per HDMI port. The reply lists every port whatever id is
/// asked for. Root first (always allowed), then Luma's own call.
fn hdmi_signals() -> Result<Vec<(u8, bool)>, String> {
    let uri = "luna://com.webos.service.eim/getHDMISignalInfo";
    let command = format!(
        "if [ \"$(id -u)\" = 0 ]; then /usr/bin/luna-send -n 1 -w 3000 {uri} '{{\"id\":\"HDMI_1\"}}'; fi"
    );
    let root = call(
        "luna://org.webosbrew.hbchannel.service/exec",
        json!({"command": command}),
    )
    .ok()
    .and_then(|envelope| {
        let stdout = envelope.get("stdoutString")?.as_str()?.to_owned();
        let start = stdout.find('{')?;
        serde_json::Deserializer::from_str(&stdout[start..])
            .into_iter::<Value>()
            .next()?
            .ok()
    });
    let reply = match root {
        Some(reply) => reply,
        None => call(uri, json!({"id": "HDMI_1"}))?,
    };
    Ok(parse_signals(&reply))
}

pub fn parse_signals(reply: &Value) -> Vec<(u8, bool)> {
    (1..=4u8)
        .filter_map(|n| {
            let signal = reply.get(format!("HDMI{n}"))?.get("signal")?.as_str()?;
            Some((n, signal.eq_ignore_ascii_case("true")))
        })
        .collect()
}

fn inventory() -> Result<(Inventory, Option<String>), String> {
    let apps = normalize_apps(&inventory_read(
        "luna://com.webos.applicationManager/listApps",
        "apps",
    )?)?;
    let inputs = inventory_read("luna://com.webos.service.eim/getAllInputStatus", "devices")
        .and_then(|raw| normalize_inputs(&raw));
    match inputs {
        Ok(mut inputs) => {
            crate::model::apply_signals(&mut inputs, &hdmi_signals().unwrap_or_default());
            eprintln!(
                "Luma TV inventory: {} apps, {} inputs",
                apps.len(),
                inputs.len()
            );
            Ok((Inventory { apps, inputs }, None))
        }
        Err(error) => Ok((
            Inventory {
                apps,
                inputs: vec![],
            },
            Some(format!("Inputs could not be refreshed: {error}")),
        )),
    }
}

fn title(value: Option<&str>, fallback: &str) -> String {
    let clean: String = value
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    if clean.trim().is_empty() {
        fallback.into()
    } else {
        clean.trim().into()
    }
}

pub fn normalize_apps(raw: &Value) -> Result<Vec<App>, String> {
    let entries = raw
        .as_array()
        .filter(|a| a.len() <= 1000)
        .ok_or("Invalid app inventory")?;
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for entry in entries {
        let Some(id) = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| valid_app_id(id))
        else {
            continue;
        };
        let live = id == "com.webos.app.livetv";
        let visible = entry.get("visible");
        if id == APP_ID
            || id == "com.archbtw.luma"
            || id == "com.luma.native"
            || id == "com.luma.launcher"
            || id == "com.luma.launcher.native"
            || id == "com.webos.app.home"
            || id.starts_with("com.webos.app.hdmi")
            || id.starts_with("com.webos.app.externalinput.")
            || (!live && (visible == Some(&json!(false)) || visible == Some(&json!("false"))))
            || (!live
                && ["com.webos.", "com.palm.", "com.lge."]
                    .iter()
                    .any(|prefix| id.starts_with(prefix))
                && visible != Some(&json!(true)))
            || !seen.insert(id)
        {
            continue;
        }
        let mut app = App::new(id, &title(entry.get("title").and_then(Value::as_str), id));
        app.icon = Some(app_artwork::Source::new(id, entry));
        apps.push(app);
    }
    apps.sort_by_key(|a| a.title.to_lowercase());
    Ok(apps)
}

pub fn normalize_inputs(raw: &Value) -> Result<Vec<Input>, String> {
    let entries = raw
        .as_array()
        .filter(|a| a.len() <= 128)
        .ok_or("Invalid input inventory")?;
    let mut inputs = Vec::new();
    for entry in entries {
        let Some(id) = entry.get("appId").and_then(Value::as_str) else {
            continue;
        };
        let Some(number) = id
            .strip_prefix("com.webos.app.hdmi")
            .and_then(|s| s.parse::<u8>().ok())
            .filter(|n| *n > 0 && *n < 100)
        else {
            continue;
        };
        if id != format!("com.webos.app.hdmi{number}")
            || entries
                .iter()
                .filter(|e| e.get("appId").and_then(Value::as_str) == Some(id))
                .count()
                != 1
        {
            continue;
        }
        if entry
            .get("id")
            .is_some_and(|v| v != &format!("HDMI_{number}"))
        {
            continue;
        }
        if entry
            .get("port")
            .is_some_and(|v| v != &json!(number) && v != &number.to_string())
        {
            continue;
        }
        let port = format!("HDMI {number}");
        inputs.push(Input {
            id: id.into(),
            title: title(entry.get("label").and_then(Value::as_str), &port),
            port,
            // Some firmware reports `connected: true` for every existing port;
            // `hdmiPlugIn` is the actual cable state where it is provided.
            connected: entry
                .get("hdmiPlugIn")
                .and_then(Value::as_bool)
                .or_else(|| entry.get("connected").and_then(Value::as_bool)),
            signal: None,
        });
    }
    Ok(inputs)
}
