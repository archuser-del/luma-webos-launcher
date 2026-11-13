mod aerial_video;
mod app_artwork;
#[cfg(all(feature = "webos", unix))]
#[path = "app-subscription.rs"]
mod app_subscription;
mod branding;
mod custom_videos;
mod debug_log;
mod diagnostics;
mod font;
#[cfg(all(feature = "webos", target_os = "linux", target_arch = "arm"))]
mod hw_wallpaper;
mod i18n;
mod immich_frame;
mod jellyfin;
mod layout;
mod library;
#[cfg(all(feature = "webos", target_os = "linux", target_arch = "arm"))]
mod luna;
mod media;
mod model;
mod moonlight;
mod motion;
mod music;
mod pad_watch;
mod phone_link;
mod platform;
mod player;
mod player_backend;
mod plex;
mod raster;
mod renderer;
#[path = "scroll-preview.rs"]
mod scroll_preview;
mod smart_home;
mod smart_home_pairing;
mod smart_home_ws;
mod stacks;
mod state;
mod steam;
mod storage;
mod tech_writer;
mod typography;
mod ui_sound;
mod vector;
mod video_catalog;
mod view;
mod weather;
mod weather_phrases;
#[path = "weather-service.rs"]
mod weather_service;
mod weather_sky;
#[cfg(all(feature = "webos", any(target_os = "linux", test)))]
#[path = "webos-compat.rs"]
mod webos_compat;
mod widgets;
mod widgets_state;

#[cfg(test)]
mod tests;

use platform::{Platform, Request, Response};
use raster::Size;
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::Keycode,
    mouse::MouseButton,
    video::GLProfile,
};
use state::{Direction, Effect, Overlay, State, Target};
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};
use time::OffsetDateTime;

#[derive(Default)]
struct Options {
    webos_home: bool,
    assets: Option<PathBuf>,
    preferences: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    dump: Option<PathBuf>,
    frames: Option<u64>,
    script: VecDeque<String>,
    fullscreen: bool,
    time: Option<OffsetDateTime>,
    resolution: Option<Size>,
    capture_size: Option<Size>,
    diagnostics: Option<PathBuf>,
    /// Write the debug log from the start (also: Settings › System).
    debug_log: bool,
    profile_frames: Option<usize>,
    weather_test_server: Option<String>,
    test_extra_apps: usize,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut result = Self::default();
        let mut args = std::env::args().skip(1).peekable();
        // SAM/jailer puts native launch metadata (e.g. @system_native_app and
        // nid) in argv[1]. It is not a Luma CLI option. Keep the original argv
        // intact for SDL/libhelpers, which also read the process launch data.
        if cfg!(feature = "webos")
            && args.peek().is_some_and(|argument| {
                serde_json::from_str::<serde_json::Value>(argument)
                    .is_ok_and(|value| value.is_object())
            })
        {
            if let Some(argument) = args.next() {
                let metadata: serde_json::Value =
                    serde_json::from_str(&argument).map_err(|error| error.to_string())?;
                result.webos_home = ["nid", "appId"].iter().any(|key| {
                    metadata
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(platform::is_home_id)
                });
            }
            eprintln!("Luma webOS launch parameters accepted");
        }
        while let Some(arg) = args.next() {
            if arg == "--help" {
                println!(
                    "Luma native\n\n  --assets PATH       Generated artwork directory\n  --preferences PATH  Isolated preferences file\n  --fullscreen        Fullscreen desktop preview\n  --resolution SIZE   Request 1080p, 4k, or WIDTHxHEIGHT; verify actual drawable\n  --frames N          Exit after N event-loop frames\n  --profile-frames N  Force N presentations (20+) and report pacing after warmup\n  --script ACTIONS    Comma-separated home/apps/settings/up/down/left/right/enter/back/menu/hold/type:TEXT/click:X:Y/wait\n  --time RFC3339      Fix the preview's clock and date\n  --screenshot PATH   Save an offscreen PNG\n  --capture-scale N   Capture at 1x/1080p (default) or 2x/4k\n  --diagnostics PATH  Save actual drawable, GPU, font-cache and timing information\n  --dump-state PATH   Save final UI state as JSON\n  --debug-log         Write a debug log next to the preferences file\n\nCompile with --features webos for the TV backend. Desktop builds never call TV services."
                );
                println!(
                    "\n  --test-extra-apps N  Explicit scroll preview with up to 300 non-launchable extra cards; separate preferences"
                );
                std::process::exit(0);
            }
            if arg == "--fullscreen" {
                result.fullscreen = true;
                continue;
            }
            if arg == "--debug-log" {
                result.debug_log = true;
                continue;
            }
            let value = args
                .next()
                .ok_or_else(|| format!("Missing value for {arg}"))?;
            match arg.as_str() {
                "--assets" => result.assets = Some(value.into()),
                "--preferences" => result.preferences = Some(value.into()),
                "--screenshot" => result.screenshot = Some(value.into()),
                "--dump-state" => result.dump = Some(value.into()),
                "--resolution" => result.resolution = Some(value.parse()?),
                "--capture-scale" => {
                    result.capture_size = Some(match value.as_str() {
                        "1" => Size::HD,
                        "2" => Size::UHD,
                        _ => return Err("--capture-scale must be 1 or 2".into()),
                    })
                }
                "--diagnostics" => result.diagnostics = Some(value.into()),
                "--weather-test-server" => result.weather_test_server = Some(value),
                "--test-extra-apps" => {
                    result.test_extra_apps = value
                        .parse::<usize>()
                        .ok()
                        .filter(|count| *count <= 300)
                        .ok_or("--test-extra-apps must be between 0 and 300")?;
                }
                "--profile-frames" => {
                    result.profile_frames = Some(
                        value
                            .parse::<usize>()
                            .ok()
                            .filter(|n| (20..=1000000).contains(n))
                            .ok_or("--profile-frames must be between 20 and 1000000")?,
                    )
                }
                "--time" => {
                    result.time = Some(
                        OffsetDateTime::parse(
                            &value,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .map_err(|e| e.to_string())?,
                    )
                }
                "--frames" => {
                    result.frames = Some(
                        value
                            .parse::<u64>()
                            .ok()
                            .filter(|n| *n > 0)
                            .ok_or("--frames must be positive")?,
                    )
                }
                "--script" => result.script = value.split(',').map(str::to_owned).collect(),
                _ => return Err(format!("Unknown option: {arg}")),
            }
        }
        if result.screenshot.is_some() && result.frames.is_none() && result.profile_frames.is_none()
        {
            result.frames = Some(90);
        }
        Ok(result)
    }
}

struct Press {
    target: Target,
    start: Instant,
    held: bool,
    pointer: bool,
}

/// What a pointer gesture on the title card moves: the side panel list when
/// it is open, else "More like this" sideways or the page up and down.
fn movie_scroll(state: &state::State, sideways: bool) -> state::movie::Scroll {
    use state::movie::Scroll;
    if state.movie.as_ref().is_some_and(|m| m.panel.is_some()) {
        Scroll::Panel
    } else if sideways {
        Scroll::Similar
    } else {
        Scroll::Page
    }
}

struct Drag {
    previous: (f32, f32),
    started: (f32, f32),
    previous_at: Instant,
    velocity: f32,
    moved: bool,
    horizontal: bool,
    /// What a sideways drag moves: Favorites, a stack row, or nothing (page only).
    lane: Option<Lane>,
}
#[derive(Clone, Copy, PartialEq)]
enum Lane {
    Favorites,
    Stack(usize),
    /// "More like this" on the title card.
    Similar,
}
const SCRIPT_POINTER: u32 = u32::MAX - 1;
// SDL-webOS's Back scancode is outside rust-sdl2's standard Scancode enum.
// Keycode retains the raw value: SDL_SCANCODE_TO_KEYCODE(SDL_SCANCODE_WEBOS_BACK).
const WEBOS_BACK_KEYCODE: i32 = (1 << 30) | 482;
// TV network-input/sendSpecialKey("BACK") emits this verified native keycode,
// with scancode=None. Proven on LG C1 on 2026-10-24 in settings-v240-key-diagnostic.log.
const WEBOS_NATIVE_BACK_KEYCODE: i32 = 2_097_155;
fn is_back_keycode(code: i32, webos: bool) -> bool {
    code == 461 || (webos && matches!(code, WEBOS_BACK_KEYCODE | WEBOS_NATIVE_BACK_KEYCODE))
}

fn effects(
    effects: Vec<Effect>,
    state: &mut State,
    platform: &Platform,
    preferences: &std::path::Path,
    weather: &weather_service::Service,
    home: &smart_home::Service,
) -> bool {
    for effect in effects {
        let name = effect_name(&effect);
        debug_log!("effect {name}");
        if matches!(name.as_str(), "MediaDetails" | "MediaPlay") || name.starts_with("Launch") {
            HEAVY_EFFECT.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        match effect {
            Effect::DebugLog(on) => {
                if on {
                    // The zone read at startup; set_offset keeps the first one.
                    start_debug_log(preferences, time::UtcOffset::UTC, "Settings › System");
                } else {
                    debug_log::stop();
                }
                state.debug_log_file = debug_log::current()
                    .filter(|_| debug_log::enabled())
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
            }
            Effect::Save => {
                if let Err(error) = storage::save(preferences, &state.preferences) {
                    eprintln!("{error}");
                    state.notify(state.preferences.language.t("savedError"));
                }
            }
            Effect::Root(change) => platform.send(Request::Root(change)),
            effect @ (Effect::Refresh | Effect::Reload) => {
                state.reload_artwork |= effect == Effect::Reload;
                if !state.loading {
                    state.loading = true;
                    state.dirty = true;
                    platform.send(Request::Refresh);
                }
            }
            Effect::OpenUrl(url) => {
                if platform.webos {
                    if !state.launching {
                        state.launching = true;
                        platform.send(Request::OpenUrl(url));
                    }
                } else {
                    state.notify(format!("Would open {url} in the TV browser"));
                }
            }
            Effect::Launch(id) => {
                if scroll_preview::is_placeholder(&id) {
                    state.notify(state.preferences.language.t("scrollTestOnly"));
                    continue;
                }
                if !state.launching {
                    if platform.webos {
                        state.launching = true;
                        platform.send(Request::Launch(id));
                    } else {
                        let title = state
                            .inventory
                            .apps
                            .iter()
                            .find(|a| a.id == id)
                            .map_or(id.as_str(), |app| app.title.as_str());
                        state.notify(
                            state
                                .preferences
                                .language
                                .format("desktopPreview", &[("app", title)]),
                        );
                    }
                }
            }
            Effect::Quit => return false,
            Effect::Weather(command) => weather.send(command),
            Effect::HomeAssistant(command) => home.send(command),
            Effect::MoonlightProbe(ip) => moonlight_service().send(moonlight::Command::Probe(ip)),
            Effect::MoonlightDiscover(ip) => {
                moonlight_service().send(moonlight::Command::Discover(ip))
            }
            Effect::MoonlightPair(host) => {
                moonlight_service().send(moonlight::Command::Pair(
                    host,
                    state.preferences.moonlight.clone(),
                ));
            }
            Effect::MoonlightCancel => moonlight_service().send(moonlight::Command::Cancel),
            Effect::MoonlightWake => {
                moonlight_service().send(moonlight::Command::Wake(
                    state.preferences.moonlight.mac.clone(),
                ));
                state.moonlight_status = Some("Sending Wake-on-LAN…".into());
            }
            Effect::MoonlightRefilter => moonlight_refilter(state),
            Effect::GameTrailer => {
                let trailer = state.game.as_ref().and_then(|g| {
                    let info = state.game_info.get(&g.id)?;
                    (!info.trailer.is_empty()).then(|| {
                        (
                            info.trailer.clone(),
                            format!("{} · Trailer", g.title),
                            info.hero.clone(),
                        )
                    })
                });
                if let Some((url, title, poster)) = trailer {
                    let request = player::OpenRequest {
                        url,
                        title,
                        source: player::Source::Youtube,
                        resume_ms: 0,
                        duration_ms: 0,
                        poster,
                        audio: vec![],
                        subtitles: vec![],
                        qualities: vec![],
                        chapters: vec![],
                        item_id: None,
                        episode_line: None,
                        prev_episode: None,
                        next_episode: None,
                        selected_subtitle: None,
                        report: None,
                        music: None,
                        jellyfin: None,
                    };
                    if let Ok(request) = request.sanitize() {
                        open_player(state, platform, request);
                    }
                }
            }
            Effect::GameInfo(id, title) => {
                if !state.game_info.contains_key(&id) {
                    steam_service().request(id, &title);
                }
            }
            Effect::MoonlightQuit => {
                if let Some(game) = state.game.as_mut() {
                    game.running = false;
                }
                state.moonlight_pending = None;
                state.moonlight_status = Some("Closing the game…".into());
                moonlight_service().send(moonlight::Command::QuitGame(
                    state.preferences.moonlight.clone(),
                ));
            }
            Effect::MoonlightPlay(id) => {
                if state.preferences.moonlight.paired {
                    if state.moonlight_online {
                        if platform.webos {
                            state.launching = true;
                            state.moonlight_launched = true;
                            platform.send(Request::MoonlightLaunch(
                                state.preferences.moonlight.clone(),
                                id,
                            ));
                        } else {
                            state.notify("Desktop preview: launch Moonlight");
                        }
                    } else {
                        state.moonlight_pending = Some(id);
                        state.moonlight_status = Some("Waking PC…".into());
                        moonlight_service().send(moonlight::Command::WakeAndWait(
                            state.preferences.moonlight.mac.clone(),
                            state.preferences.moonlight.address.clone(),
                        ));
                    }
                } else {
                    state.notify("Connect Luma to Sunshine first");
                }
            }
            Effect::StacksRefresh => stacks_refresh(state),
            Effect::JellyfinDiscover => {
                let saved = state.preferences.jellyfin.server.clone();
                jellyfin_service().send(jellyfin::Command::Discover(
                    (!saved.is_empty()).then_some(saved),
                ));
            }
            Effect::JellyfinConnect(address) => {
                if state.preferences.jellyfin.device_id.is_empty() {
                    state.preferences.jellyfin.device_id =
                        phone_link::random_hex()[..32].to_owned();
                }
                jellyfin_service().send(jellyfin::Command::Connect {
                    address,
                    device_id: state.preferences.jellyfin.device_id.clone(),
                });
            }
            Effect::JellyfinCancel => {
                jellyfin_service().send(jellyfin::Command::Cancel);
                jellyfin_form_stop();
            }
            Effect::JellyfinAddress => jellyfin_form_start(),
            Effect::VideoUploadStart => {
                *VIDEO_UPLOAD.lock().unwrap() = Some(custom_videos::start());
            }
            Effect::VideoUploadStop => {
                VIDEO_UPLOAD.lock().unwrap().take();
            }
            Effect::ImmichFormStart => immich_form_start(preferences),
            Effect::ImmichFormStop => immich_form_stop(),
            Effect::ImmichSkip => {
                if let Some(service) = IMMICH.get() {
                    service.send(immich_frame::Command::Skip);
                }
            }
            Effect::PlexLink => {
                if state.preferences.plex.client_id.is_empty() {
                    state.preferences.plex.client_id = phone_link::random_hex()[..32].to_owned();
                }
                plex_service().send(plex::Command::Link {
                    client_id: state.preferences.plex.client_id.clone(),
                });
            }
            Effect::PlexCancel => plex_service().send(plex::Command::Cancel),
            Effect::PlexViews => {
                if state.preferences.plex.linked() {
                    plex_service().send(plex::Command::Views(state.preferences.plex.clone()));
                }
            }
            Effect::JellyfinViews => {
                if state.preferences.jellyfin.linked() {
                    jellyfin_service()
                        .send(jellyfin::Command::Views(state.preferences.jellyfin.clone()));
                }
            }
            Effect::MediaPlay(request) if plex::owns(request.id) => {
                if !state.launching {
                    state.launching = true;
                    plex_service()
                        .send(plex::Command::Play(state.preferences.plex.clone(), request));
                }
            }
            Effect::MediaPlay(request) if jellyfin::owns(request.id) => {
                if !state.launching {
                    state.launching = true;
                    jellyfin_service().send(jellyfin::Command::Play(
                        state.preferences.jellyfin.clone(),
                        request,
                    ));
                }
            }
            Effect::MediaPlay(_) => state.notify("Unknown media source"),
            Effect::Player(player::Command::Episode(id, season, number)) if plex::owns(id) => {
                state.launching = true;
                plex_service().send(plex::Command::Play(
                    state.preferences.plex.clone(),
                    media::PlayRequest {
                        id,
                        episode: Some((season, number)),
                        ..Default::default()
                    },
                ));
            }
            Effect::Player(player::Command::Episode(id, season, number)) if jellyfin::owns(id) => {
                state.launching = true;
                jellyfin_service().send(jellyfin::Command::Play(
                    state.preferences.jellyfin.clone(),
                    media::PlayRequest {
                        id,
                        episode: Some((season, number)),
                        ..Default::default()
                    },
                ));
            }
            Effect::Player(command) => player_backend::send(command),
            Effect::MediaDetails(id) if plex::owns(id) => {
                plex_service().send(plex::Command::Details(state.preferences.plex.clone(), id));
            }
            Effect::MediaDetails(id) if jellyfin::owns(id) => {
                jellyfin_service().send(jellyfin::Command::Details(
                    state.preferences.jellyfin.clone(),
                    id,
                ));
            }
            Effect::MediaDetails(_) => state.notify("Unknown media source"),
            Effect::AlbumDetails(id) => {
                jellyfin_service().send(jellyfin::Command::Album(
                    state.preferences.jellyfin.clone(),
                    id,
                ));
            }
            Effect::AlbumPlay(id, start, shuffle) => {
                state.launching = true;
                jellyfin_service().send(jellyfin::Command::PlayAlbum(
                    state.preferences.jellyfin.clone(),
                    id,
                    start,
                    shuffle,
                ));
            }
            Effect::MediaThumbs(id, list) => media::thumbs_request(id, list),
            // Trailers play in Luma's own player too, never the TV's.
            Effect::PlayUrl(url, title) => {
                let request = player::OpenRequest {
                    url,
                    title,
                    source: player::Source::Movie,
                    resume_ms: 0,
                    duration_ms: 0,
                    poster: None,
                    audio: vec![],
                    subtitles: vec![],
                    qualities: vec![],
                    chapters: vec![],
                    item_id: None,
                    episode_line: None,
                    prev_episode: None,
                    next_episode: None,
                    selected_subtitle: None,
                    report: None,
                    music: None,
                    jellyfin: None,
                };
                match request.sanitize() {
                    Ok(request) => open_player(state, platform, request),
                    Err(error) => state.notify(error),
                }
            }
        }
    }
    true
}

fn script(
    action: &str,
    state: &mut State,
    controls: &[state::Control],
) -> Result<Vec<Effect>, String> {
    Ok(match action {
        "smart-demo" | "smart-detail" | "smart-camera" | "smart-settings" | "smart-connect"
        | "smart-devices" | "smart-signin" | "smart-hide" | "smart-thermostat" | "smart-lock" => {
            state.preferences.smart_home.server_url = "http://demo.homeassistant.local:8123".into();
            state.preferences.smart_home.show_tab = true;
            let names = [
                ("camera.front_door", "Front Door", "idle", None),
                ("camera.backyard", "Backyard", "unavailable", None),
                ("scene.movie_night", "Movie Night", "scening", None),
                ("scene.good_morning", "Good Morning", "scening", None),
                ("scene.arrive_home", "Arrive Home", "scening", None),
                ("scene.all_off", "All Off", "scening", None),
                ("light.ceiling", "Ceiling Light", "on", Some(80)),
                ("light.floor", "Floor Lamp", "on", Some(40)),
                ("light.kitchen", "Kitchen Strip", "off", None),
                ("light.tv", "TV Backlight", "on", Some(60)),
                ("lock.front", "Front Door", "locked", None),
                ("alarm_control_panel.alarmo", "Alarmo", "armed_home", None),
                ("sensor.temperature", "Temperature", "22.5", None),
                ("binary_sensor.motion", "Hallway Motion", "off", None),
                ("fan.air", "Air Purifier", "off", None),
                ("switch.soundbar", "Sony Soundbar", "on", None),
                (
                    "switch.led_indicator",
                    "Mini Switch LED Indicator",
                    "on",
                    None,
                ),
                (
                    "sensor.mini_switch_battery",
                    "Mini Switch Battery",
                    "88",
                    None,
                ),
                ("binary_sensor.config_dirty", "Config Dirty", "off", None),
                ("light.garden", "Garden Lights", "unavailable", None),
                (
                    "sensor.outdoor_camera_signal",
                    "Outdoor Camera Signal",
                    "-61",
                    None,
                ),
            ];
            state.ha_entities = names
                .into_iter()
                .map(|(id, name, status, brightness)| smart_home::Entity {
                    id: id.into(),
                    name: name.into(),
                    state: status.into(),
                    domain: id.split_once('.').unwrap().0.into(),
                    area: "Living Room".into(),
                    unit: if id == "sensor.temperature" {
                        "°C"
                    } else {
                        ""
                    }
                    .into(),
                    brightness,
                    adjustable: brightness.is_some(),
                    hidden: false,
                    device_class: String::new(),
                    ..Default::default()
                })
                .collect();
            // Devices with a dial of their own: target temperature, volume, position.
            state.ha_entities.extend([
                smart_home::Entity {
                    id: "climate.living".into(),
                    name: "Thermostat".into(),
                    state: "heat".into(),
                    domain: "climate".into(),
                    area: "Living Room".into(),
                    level: Some(21.5),
                    level_min: Some(7.),
                    level_max: Some(30.),
                    level_step: Some(0.5),
                    adjustable: true,
                    current: Some(20.3),
                    ..Default::default()
                },
                smart_home::Entity {
                    id: "media_player.speaker".into(),
                    name: "Kitchen Speaker".into(),
                    state: "playing".into(),
                    domain: "media_player".into(),
                    area: "Kitchen".into(),
                    level: Some(35.),
                    adjustable: true,
                    media: "Lo-fi Radio".into(),
                    ..Default::default()
                },
                smart_home::Entity {
                    id: "cover.blinds".into(),
                    name: "Blinds".into(),
                    state: "open".into(),
                    domain: "cover".into(),
                    area: "Living Room".into(),
                    level: Some(70.),
                    adjustable: true,
                    ..Default::default()
                },
            ]);
            state.ha_online = true;
            match action {
                "smart-demo" => {
                    state.open(2);
                    state.focus = Target::SmartCamera("camera.front_door".into());
                }
                "smart-hide" => {
                    state.open(2);
                    state.preferences.smart_home.hidden_cameras = vec!["camera.front_door".into()];
                    state.ha_camera_menu = Some("camera.front_door".into());
                    state.focus = Target::SmartCameraHide;
                }
                "smart-detail" => {
                    state.open(2);
                    state.ha_detail = Some("light.ceiling".into());
                    state.focus = Target::SmartBrightness(0);
                }
                "smart-thermostat" => {
                    state.open(2);
                    state.ha_detail = Some("climate.living".into());
                    state.focus = Target::SmartBrightness(0);
                }
                "smart-lock" => {
                    state.open(2);
                    state.ha_detail = Some("lock.front".into());
                    state.focus = Target::SmartBrightness(0);
                }
                "smart-camera" => {
                    state.open(2);
                    state.ha_camera = Some("camera.front_door".into());
                    state.focus = Target::SmartClose;
                }
                "smart-settings" => {
                    state.open_settings();
                    state.settings_level = state::SettingsLevel::Section;
                    state.section = state::SMARTHOME_SECTION;
                    state.focus = Target::SmartShowTab;
                }
                "smart-devices" => {
                    state.open_settings();
                    state.settings_level = state::SettingsLevel::Section;
                    state.section = state::SMARTHOME_SECTION;
                    state.subpage = Some(state::Subpage::SmartSelect(0));
                    state.focus = Target::SmartBulk(0, 0);
                }
                "smart-signin" => {
                    state.preferences.smart_home.server_url.clear();
                    state.open_settings();
                    state.settings_level = state::SettingsLevel::Section;
                    state.section = state::SMARTHOME_SECTION;
                    state.subpage = Some(state::Subpage::SmartConnect);
                    state.ha_address_draft = "http://homeassistant.local:8123".into();
                    state.ha_servers = vec![smart_home::DiscoveredServer {
                        name: "Home".into(),
                        url: "http://homeassistant.local:8123".into(),
                        version: "2026.10.2".into(),
                    }];
                    state.ha_qr = smart_home::qr_matrix(
                        "http://homeassistant.local:8123/auth/authorize?demo",
                    );
                    state.ha_fallback_qr =
                        smart_home::qr_matrix("http://192.168.1.206:47821/token?n=demo");
                    state.ha_auth_status = "Waiting for sign-in…".into();
                    state.focus = Target::SmartServer(0);
                }
                "smart-connect" => {
                    state.open_settings();
                    state.settings_level = state::SettingsLevel::Section;
                    state.section = state::SMARTHOME_SECTION;
                    state.subpage = Some(state::Subpage::SmartConnect);
                    state.ha_address_draft = "http://homeassistant.local:8123".into();
                    state.focus = Target::SmartAddress;
                }
                _ => {}
            }
            state.dirty = true;
            vec![]
        }
        "home" => {
            state.open(0);
            vec![]
        }
        "apps" => {
            state.open(1);
            vec![]
        }
        "smart" => {
            state.open(2);
            vec![]
        }
        "settings" => {
            state.open_settings();
            vec![]
        }
        "up" => state.direction(Direction::Up, controls),
        "down" => state.direction(Direction::Down, controls),
        "left" => state.direction(Direction::Left, controls),
        "right" => state.direction(Direction::Right, controls),
        "enter" => state.activate(),
        "back" => state.back(),
        "menu" => {
            state.options();
            vec![]
        }
        "hold" => state.hold(),
        "wait" => vec![],
        action if action.starts_with("notice:") => {
            state.notify(&action["notice:".len()..]);
            vec![]
        }
        "error" => {
            state.error = Some("luna://org.webosbrew.hbchannel.service/exec failed".into());
            state.inventory.apps.clear();
            vec![]
        }
        action if action.starts_with("type:") => {
            let field = if state.text_input_active() {
                state.keyboard_field
            } else {
                match state.subpage {
                    Some(state::Subpage::Greeting) => state::TextField::Name,
                    Some(state::Subpage::WeatherLocation) => state::TextField::Location,
                    Some(state::Subpage::Input(_)) => state::TextField::InputName,
                    _ if state.library.is_some() => state::TextField::Library,
                    _ => state::TextField::Apps,
                }
            };
            if !state.text_input_active() {
                state.open_keyboard(field);
            }
            state.text(&action[5..]);
            vec![]
        }
        "await-search" => vec![],
        "await-weather" => vec![],
        _ => return Err(format!("Unknown scripted action: {action}")),
    })
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--pad-watch") {
        pad_watch::run();
    }
    if let Err(error) = run() {
        eprintln!("Luma native: {error}");
        std::process::exit(1);
    }
}

fn compose(
    state: &mut State,
    assets: &view::Assets,
    pill: f32,
    width: f32,
    now: OffsetDateTime,
) -> view::Scene {
    let current_page = state.page;
    if current_page != state.rendered_page {
        let direction = if state.page_index()
            > match state.rendered_page {
                state::Page::Home => 0,
                state::Page::Apps => 1,
                state::Page::SmartHome => 2,
                state::Page::Settings => state.settings_nav_index(),
            } {
            1.
        } else {
            -1.
        };
        if !state.last_body.is_empty() {
            state.previous_page_layer = Some((
                std::mem::take(&mut state.last_body),
                Instant::now(),
                direction,
            ));
        }
        state.rendered_page = current_page;
        state.previous_panel_layer = None;
        state.previous_sidebar_layer = None;
        state.last_panel.clear();
        state.last_sidebar.clear();
    } else if current_page == state::Page::Settings
        && ((state.rendered_section != state.section
            && state.settings_level != state::SettingsLevel::Root)
            || state.rendered_subpage != state.subpage
            || state.rendered_settings_level != state.settings_level)
    {
        state.previous_panel_layer = None;
        state.previous_sidebar_layer = None;
        if !state.last_panel.is_empty() {
            state.previous_panel_layer = Some((
                std::mem::take(&mut state.last_panel),
                Instant::now(),
                (state.rendered_subpage != state.subpage
                    || state.rendered_settings_level != state.settings_level)
                    && state.settings_forward,
            ));
        }
        // Outgoing panel already contains its preview. No second exit animation.
        state.panel_started = Some(Instant::now());
    }
    // Preview contents alone crossfade on row focus; panel geometry never animates.
    if current_page == state::Page::Settings
        && state.rendered_settings_level == state.settings_level
        && state.rendered_subpage == state.subpage
        && state.rendered_preview_focus != state.focus
        && !matches!(state.focus, Target::Nav(_) | Target::SettingsBack)
        && !state.last_sidebar.is_empty()
    {
        state.previous_sidebar_layer =
            Some((std::mem::take(&mut state.last_sidebar), Instant::now()));
        state.previous_sidebar_key = state.last_sidebar_key;
    }
    state.rendered_preview_focus = state.focus.clone();
    state.rendered_settings_level = state.settings_level;
    state.rendered_section = state.section;
    state.rendered_subpage = state.subpage.clone();
    let overlay_open = matches!(
        state.overlay,
        Some(state::Overlay::Keyboard | state::Overlay::Options { .. })
    );
    if !overlay_open && !state.last_overlay.is_empty() {
        state.previous_overlay_layer =
            Some((std::mem::take(&mut state.last_overlay), Instant::now()));
    }
    let mut scene = view::scene_with_pill(state, assets, pill, width, now);
    if !state.pointer && state.overlay.is_none() && state.reveal_focus(&scene.controls) {
        scene = view::scene_with_pill(state, assets, pill, width, now);
    }
    state.last_body = scene.draw[scene.body_range.clone()].to_vec();
    state.last_panel = scene.draw[scene.settings_panel_range.clone()].to_vec();
    state.last_sidebar = scene.draw[scene.settings_sidebar_range.clone()].to_vec();
    state.last_sidebar_key = scene.settings_sidebar_key;
    state.settings_art_live = scene.settings_sidebar_key != 0;
    state.last_overlay = scene.draw[scene.overlay_range.clone()].to_vec();
    scene
}

fn jellyfin_service() -> &'static jellyfin::Service {
    static SERVICE: std::sync::OnceLock<jellyfin::Service> = std::sync::OnceLock::new();
    SERVICE.get_or_init(|| {
        let cache = if std::path::Path::new("/media/developer/com.archbtw.luma").exists() {
            std::path::PathBuf::from("/media/developer/com.archbtw.luma/posters")
        } else {
            std::env::temp_dir().join("luma-posters")
        };
        jellyfin::Service::new(cache)
    })
}

/// The phone form that takes a Jellyfin server address (nothing is typed
/// with the remote): link to show as QR, checked servers, form errors.
struct JellyfinForm {
    ready: std::sync::mpsc::Receiver<Result<String, String>>,
    servers: std::sync::mpsc::Receiver<jellyfin::Server>,
    errors: std::sync::mpsc::Receiver<String>,
    canceled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
static JELLYFIN_FORM: std::sync::Mutex<Option<JellyfinForm>> = std::sync::Mutex::new(None);
/// Settings › Wallpaper › Upload Videos page server, while the page is open.
static VIDEO_UPLOAD: std::sync::Mutex<Option<custom_videos::Server>> = std::sync::Mutex::new(None);

/// ImmichFrame wallpaper worker, created with the preferences folder at startup.
static IMMICH: std::sync::OnceLock<immich_frame::Service> = std::sync::OnceLock::new();
/// Server last told to the worker, so it hears only about changes.
static IMMICH_SHOWN: std::sync::Mutex<Option<immich_frame::ImmichFrameLink>> =
    std::sync::Mutex::new(None);
type ImmichLinked = (immich_frame::ImmichFrameLink, immich_frame::Frame);
/// The phone form that takes an ImmichFrame address and optional secret.
struct ImmichForm {
    ready: std::sync::mpsc::Receiver<Result<String, String>>,
    linked: std::sync::mpsc::Receiver<ImmichLinked>,
    errors: std::sync::mpsc::Receiver<String>,
    canceled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    folder: std::path::PathBuf,
}
static IMMICH_FORM: std::sync::Mutex<Option<ImmichForm>> = std::sync::Mutex::new(None);

fn immich_folder(preferences: &std::path::Path) -> std::path::PathBuf {
    preferences
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."))
        .join("immich")
}

fn immich_form_stop() {
    if let Some(form) = IMMICH_FORM.lock().unwrap().take() {
        form.canceled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn immich_form_start(preferences: &std::path::Path) {
    immich_form_stop();
    let folder = immich_folder(preferences);
    let form = phone_link::Form {
        title: "ImmichFrame".into(),
        hint: "Type the address you open ImmichFrame with, for example 192.168.1.20:8080".into(),
        help: Some((
            "What is ImmichFrame?".into(),
            "https://immichframe.dev".into(),
        )),
        fields: vec![
            phone_link::Field {
                label: "ImmichFrame address".into(),
                name: "address".into(),
                kind: phone_link::FieldType::Text,
                max_len: 300,
                placeholder: "http://192.168.1.20:8080".into(),
                optional: false,
            },
            phone_link::Field {
                label: "Authentication secret (only if your server has one)".into(),
                name: "secret".into(),
                kind: phone_link::FieldType::Secret,
                max_len: 512,
                placeholder: "Leave empty if none".into(),
                optional: true,
            },
        ],
        submit_label: "Connect".into(),
        success_title: "Connected".into(),
        success_message: "Look at your TV: your photos are the wallpaper now.".into(),
        route: "/immich".into(),
    };
    let (linked_tx, linked) = std::sync::mpsc::channel();
    let (error_tx, errors) = std::sync::mpsc::channel();
    let canceled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let check_folder = folder.clone();
    let ready = phone_link::spawn_form(
        form,
        move |fields| {
            immich_frame::check(
                &check_folder,
                fields.get("address").map(String::as_str).unwrap_or(""),
                fields.get("secret").map(String::as_str).unwrap_or(""),
            )
        },
        linked_tx,
        error_tx,
        canceled.clone(),
    );
    *IMMICH_FORM.lock().unwrap() = Some(ImmichForm {
        ready,
        linked,
        errors,
        canceled,
        folder,
    });
}

/// Keep the ImmichFrame worker in step with the wallpaper, and apply its
/// photos and the phone form's results.
fn immich_tick(state: &mut State, preferences_path: &std::path::Path, visible: bool) {
    let wanted =
        (visible && state.preferences.background == "immich" && state.preferences.immich.linked())
            .then(|| state.preferences.immich.clone());
    {
        let mut shown = IMMICH_SHOWN.lock().unwrap();
        if *shown != wanted {
            if let Some(service) = IMMICH.get() {
                service.send(immich_frame::Command::Show(wanted.clone()));
            }
            *shown = wanted;
        }
    }
    if let Some(service) = IMMICH.get()
        && let Ok(updates) = service.updates.try_lock()
    {
        while let Ok(update) = updates.try_recv() {
            match update {
                immich_frame::Update::Frame(frame) => state.immich_show(frame),
                immich_frame::Update::Failed(error) => {
                    debug_log!("ImmichFrame failed: {error}");
                    state.immich_status = Some((error, true));
                    state.dirty = true;
                }
            }
        }
    }
    let mut linked = vec![];
    let mut restart = None;
    if let Some(form) = IMMICH_FORM.lock().unwrap().as_ref() {
        while let Ok(ready) = form.ready.try_recv() {
            state.dirty = true;
            match ready {
                Ok(link) => state.immich_qr = phone_link::qr_matrix(&link),
                Err(error) => state.immich_status = Some((error, true)),
            }
        }
        while let Ok(error) = form.errors.try_recv() {
            state.immich_status = Some((error, true));
            state.dirty = true;
        }
        while let Ok(result) = form.linked.try_recv() {
            linked.push(result);
            restart = Some(form.folder.clone());
        }
    }
    for (link, frame) in linked {
        state.immich_linked(link, frame);
        let _ = storage::save(preferences_path, &state.preferences);
    }
    // The form's code is used up; show a fresh one while the page stays open.
    if restart.is_some() {
        state.immich_qr.clear();
        if state.settings_level == state::SettingsLevel::WallpaperImmich {
            immich_form_start(preferences_path);
        } else {
            immich_form_stop();
        }
    }
}

/// Apply what the video upload page did: its address, new and deleted clips.
fn video_upload_tick(state: &mut State, preferences_path: &std::path::Path) {
    let mut events = vec![];
    if let Some(server) = VIDEO_UPLOAD.lock().unwrap().as_ref() {
        while let Ok(event) = server.events.try_recv() {
            events.push(event);
        }
    }
    for event in events {
        state.dirty = true;
        match event {
            custom_videos::Event::Ready(Ok(link)) => {
                state.video_upload_qr = phone_link::qr_matrix(&link);
                state.video_upload_link = Some(link);
            }
            custom_videos::Event::Ready(Err(error)) => {
                state.video_upload_status = Some((error, true));
            }
            custom_videos::Event::Added(id) => {
                custom_videos::reload();
                if let Some(index) = custom_videos::catalog_index(&id) {
                    let name = video_catalog::all()[index].name.clone();
                    state.video_uploaded(index, &name);
                    let _ = storage::save(preferences_path, &state.preferences);
                }
            }
            custom_videos::Event::Removed => {
                custom_videos::reload();
                state.refresh_video_catalog();
                state.video_upload_status = Some(("Video deleted.".into(), false));
                let _ = storage::save(preferences_path, &state.preferences);
            }
        }
    }
}

fn jellyfin_form_stop() {
    if let Some(form) = JELLYFIN_FORM.lock().unwrap().take() {
        form.canceled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn jellyfin_form_start() {
    jellyfin_form_stop();
    let form = phone_link::Form {
        title: "Jellyfin server".into(),
        hint: "Type the address you open Jellyfin with, for example 192.168.1.20:8096".into(),
        help: None,
        fields: vec![phone_link::Field {
            optional: false,
            label: "Server address".into(),
            name: "address".into(),
            kind: phone_link::FieldType::Text,
            max_len: 200,
            placeholder: "http://192.168.1.20:8096".into(),
        }],
        submit_label: "Connect".into(),
        success_title: "Server found".into(),
        success_message: "Look at your TV: Luma shows a Quick Connect code.".into(),
        route: "/jellyfin".into(),
    };
    let (server_tx, servers) = std::sync::mpsc::channel();
    let (error_tx, errors) = std::sync::mpsc::channel();
    let canceled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ready = phone_link::spawn_form(
        form,
        |fields| jellyfin::check(fields.get("address").map(String::as_str).unwrap_or("")),
        server_tx,
        error_tx,
        canceled.clone(),
    );
    *JELLYFIN_FORM.lock().unwrap() = Some(JellyfinForm {
        ready,
        servers,
        errors,
        canceled,
    });
}

/// Apply Jellyfin replies; stacks refresh with the media ones (30 min).
fn jellyfin_tick(state: &mut State, preferences_path: &std::path::Path, platform: &Platform) {
    let mut found = vec![];
    if let Some(form) = JELLYFIN_FORM.lock().unwrap().as_ref() {
        while let Ok(ready) = form.ready.try_recv() {
            state.dirty = true;
            match ready {
                Ok(link) => {
                    state.jellyfin_qr = phone_link::qr_matrix(&link);
                    state.jellyfin_qr_caption = (
                        "Scan with your phone".into(),
                        "Type your Jellyfin server address there".into(),
                    );
                }
                Err(error) => {
                    state.jellyfin_error = true;
                    state.jellyfin_status = Some(error);
                }
            }
        }
        while let Ok(error) = form.errors.try_recv() {
            state.jellyfin_status = Some(error);
            state.dirty = true;
        }
        while let Ok(server) = form.servers.try_recv() {
            found.push(server);
        }
    }
    for server in found {
        jellyfin_form_stop();
        let address = server.address.clone();
        if !state.jellyfin_servers.iter().any(|s| s.address == address) {
            state.jellyfin_servers.insert(0, server);
        }
        if state.settings_level == state::SettingsLevel::JellyfinLink {
            let effects = state.jellyfin_connect(address);
            for effect in effects {
                if let Effect::JellyfinConnect(address) = effect {
                    jellyfin_service().send(jellyfin::Command::Connect {
                        address,
                        device_id: jellyfin_device_id(state),
                    });
                }
            }
            state.focus = Target::JellyfinServer(0);
        }
    }
    while let Ok(response) = jellyfin_service().receiver.lock().unwrap().try_recv() {
        state.dirty = true;
        let linking = state.settings_level == state::SettingsLevel::JellyfinLink;
        match response {
            jellyfin::Response::Servers(list) => {
                for server in list {
                    if !state
                        .jellyfin_servers
                        .iter()
                        .any(|s| s.address == server.address)
                    {
                        state.jellyfin_servers.push(server);
                    }
                }
                if !linking || state.jellyfin_phone || state.jellyfin_selected.is_some() {
                    continue;
                }
                if let Some(first) = state.jellyfin_servers.first() {
                    let address = first.address.clone();
                    state.jellyfin_connect(address.clone());
                    jellyfin_service().send(jellyfin::Command::Connect {
                        address,
                        device_id: jellyfin_device_id(state),
                    });
                } else {
                    // Nothing answered: the phone form opens by itself.
                    state.jellyfin_phone = true;
                    state.jellyfin_status =
                        Some("No server found. Scan the QR and type its address".into());
                    jellyfin_form_start();
                }
            }
            jellyfin::Response::Code { code, address } => {
                if state.jellyfin_selected.as_deref() == Some(address.as_str()) {
                    state.jellyfin_code = Some(code);
                    state.jellyfin_error = false;
                    state.jellyfin_status = Some("Waiting for approval…".into());
                }
            }
            jellyfin::Response::Failed(message) => {
                debug_log!("Jellyfin failed: {message}");
                if state.launching {
                    state.launching = false;
                    state.notify(message.clone());
                } else if state.movie.as_ref().is_some_and(|m| m.detail.is_none()) {
                    state.notify(message.clone());
                }
                if linking {
                    state.jellyfin_error = true;
                    state.jellyfin_code = None;
                    state.jellyfin_status = Some(message);
                }
            }
            jellyfin::Response::Linked(link) => {
                state.preferences.jellyfin = link;
                state.jellyfin_reset();
                // Three rows to start with: the libraries themselves, then what
                // to watch next; more from Settings › Stacks.
                for collection in [jellyfin::LIBRARIES, "resume", "latest"] {
                    let mut stack = stacks::StackConfig {
                        source: "jellyfin".into(),
                        collection: collection.into(),
                        shape: if collection == jellyfin::LIBRARIES {
                            stacks::CardShape::Wide
                        } else {
                            stacks::CardShape::Poster
                        },
                        ..Default::default()
                    };
                    stack.name = stacks::row_title(&stack);
                    state.preferences.stacks.push(stack);
                    state.stack_items.push(vec![]);
                }
                let _ = storage::save(preferences_path, &state.preferences);
                if linking {
                    let i = state.preferences.stacks.len() - 3;
                    state.server_linked(i);
                }
                state.notify(format!(
                    "Jellyfin connected as {}",
                    state.preferences.jellyfin.user_name
                ));
                stacks_refresh(state);
                jellyfin_service()
                    .send(jellyfin::Command::Views(state.preferences.jellyfin.clone()));
            }
            jellyfin::Response::Items(index, items) => {
                debug_log!("Jellyfin row {index}: {} cards", items.len());
                if state.stack_items.len() <= index {
                    state.stack_items.resize(index + 1, vec![]);
                }
                state.stack_items[index] = items;
            }
            jellyfin::Response::Views(views) => state.jellyfin_views = views,
            jellyfin::Response::Libraries(libraries) => state.jellyfin_libraries = libraries,
            jellyfin::Response::Details(detail) => state.media_detail(*detail),
            jellyfin::Response::Album(album) => state.album_details(*album),
            jellyfin::Response::Play(mut request) => {
                state.launching = false;
                // Albums play in the background; the album card stays open.
                if let Some(music) = request.music.take() {
                    let id = request.item_id.unwrap_or(0);
                    let cover = state.album.as_ref().filter(|c| c.id == id).and_then(|c| {
                        c.album
                            .as_ref()
                            .and_then(|a| a.cover.clone())
                            .or_else(|| c.poster.clone())
                    });
                    state.music.start(id, music, cover);
                    state.dirty = true;
                    continue;
                }
                if let Some(card) = state.movie.as_ref() {
                    request.poster = card
                        .detail
                        .as_ref()
                        .and_then(|d| d.backdrop.clone())
                        .or_else(|| card.poster.clone());
                }
                match request.sanitize() {
                    Ok(request) => open_player(state, platform, request),
                    Err(error) => state.notify(error),
                }
            }
        }
    }
}

fn plex_service() -> &'static plex::Service {
    static SERVICE: std::sync::OnceLock<plex::Service> = std::sync::OnceLock::new();
    SERVICE.get_or_init(|| {
        let cache = if std::path::Path::new("/media/developer/com.archbtw.luma").exists() {
            std::path::PathBuf::from("/media/developer/com.archbtw.luma/posters")
        } else {
            std::env::temp_dir().join("luma-posters")
        };
        plex::Service::new(cache)
    })
}

/// Apply Plex replies; stacks refresh with the media ones (30 min).
fn plex_tick(state: &mut State, preferences_path: &std::path::Path, platform: &Platform) {
    while let Ok(response) = plex_service().receiver.lock().unwrap().try_recv() {
        state.dirty = true;
        let linking = state.settings_level == state::SettingsLevel::PlexLink;
        match response {
            plex::Response::Code { code, qr } => {
                state.plex_code = Some(code);
                state.plex_qr = phone_link::qr_matrix(&qr);
                state.plex_error = false;
                state.plex_status = Some("Waiting for sign-in…".into());
            }
            plex::Response::Failed(message) => {
                debug_log!("Plex failed: {message}");
                if state.launching {
                    state.launching = false;
                    state.notify(message.clone());
                } else if state.movie.as_ref().is_some_and(|m| m.detail.is_none()) {
                    state.notify(message.clone());
                }
                if linking {
                    state.plex_error = true;
                    state.plex_code = None;
                    state.plex_status = Some(message);
                }
            }
            plex::Response::Linked(link) => {
                state.preferences.plex = link;
                state.plex_code = None;
                state.plex_status = None;
                state.plex_qr.clear();
                for collection in ["ondeck", "recent"] {
                    let mut stack = stacks::StackConfig {
                        source: "plex".into(),
                        collection: collection.into(),
                        ..Default::default()
                    };
                    stack.name = stacks::row_title(&stack);
                    state.preferences.stacks.push(stack);
                    state.stack_items.push(vec![]);
                }
                let _ = storage::save(preferences_path, &state.preferences);
                if linking {
                    let i = state.preferences.stacks.len() - 2;
                    state.server_linked(i);
                }
                state.notify(format!(
                    "Plex connected: {}",
                    state.preferences.plex.server_name
                ));
                stacks_refresh(state);
                plex_service().send(plex::Command::Views(state.preferences.plex.clone()));
            }
            plex::Response::Items(index, items) => {
                if state.stack_items.len() <= index {
                    state.stack_items.resize(index + 1, vec![]);
                }
                state.stack_items[index] = items;
            }
            plex::Response::Views(views) => state.plex_views = views,
            plex::Response::Details(detail) => state.media_detail(*detail),
            plex::Response::Play(mut request) => {
                state.launching = false;
                if let Some(card) = state.movie.as_ref() {
                    request.poster = card
                        .detail
                        .as_ref()
                        .and_then(|d| d.backdrop.clone())
                        .or_else(|| card.poster.clone());
                }
                match request.sanitize() {
                    Ok(request) => open_player(state, platform, request),
                    Err(error) => state.notify(error),
                }
            }
        }
    }
}

fn jellyfin_device_id(state: &mut State) -> String {
    if state.preferences.jellyfin.device_id.is_empty() {
        state.preferences.jellyfin.device_id = phone_link::random_hex()[..32].to_owned();
    }
    state.preferences.jellyfin.device_id.clone()
}

fn steam_service() -> &'static steam::Service {
    static SERVICE: std::sync::OnceLock<steam::Service> = std::sync::OnceLock::new();
    SERVICE.get_or_init(|| {
        let cache = if std::path::Path::new("/media/developer/com.archbtw.luma").exists() {
            std::path::PathBuf::from("/media/developer/com.archbtw.luma/moonlight-cache")
        } else {
            std::env::temp_dir().join("luma-moonlight-cache")
        };
        steam::Service::new(cache)
    })
}
fn moonlight_service() -> &'static moonlight::Service {
    static SERVICE: std::sync::OnceLock<moonlight::Service> = std::sync::OnceLock::new();
    SERVICE.get_or_init(|| {
        let cache = if std::path::Path::new("/media/developer/com.archbtw.luma").exists() {
            std::path::PathBuf::from("/media/developer/com.archbtw.luma/moonlight-cache")
        } else {
            std::env::temp_dir().join("luma-moonlight-cache")
        };
        moonlight::Service::new(cache)
    })
}
fn moonlight_tick(state: &mut State, preferences_path: &std::path::Path, platform: &Platform) {
    if let Ok(rx) = steam_service().receiver.try_lock() {
        while let Ok((id, info)) = rx.try_recv() {
            state.game_info.insert(id, info);
            state.dirty = true;
        }
    }
    while let Ok(update) = moonlight_service().receiver.lock().unwrap().try_recv() {
        state.dirty = true;
        match update {
            moonlight::Update::Found(host) => {
                let validated_ip = host.address.clone();
                // mDNS may emit more than one IP for the same PC.
                // Never replace the pairing PIN status while the approval is pending.
                if let Some(existing) = state.moonlight_hosts.iter_mut().find(|h| {
                    h.address == host.address || (!host.uuid.is_empty() && h.uuid == host.uuid)
                }) {
                    *existing = host;
                } else {
                    state.moonlight_hosts.push(host);
                }
                if state.page == state::Page::Settings
                    && state.settings_level == state::SettingsLevel::MoonlightHosts
                {
                    state.moonlight_status = Some("Select a PC to pair".into());
                }
                // Keep the last validated LAN IP even before PIN succeeds.
                if !validated_ip.is_empty()
                    && !state.preferences.moonlight.paired
                    && state.preferences.moonlight.address != validated_ip
                {
                    state.preferences.moonlight.address = validated_ip;
                    let _ = storage::save(preferences_path, &state.preferences);
                }
            }
            moonlight::Update::DiscoveryDone => {
                if state.settings_level == state::SettingsLevel::MoonlightHosts
                    && state.moonlight_hosts.is_empty()
                {
                    state.moonlight_status = Some(
                        "No PCs found. Check Sunshine and network. Use Enter IP address.".into(),
                    );
                }
            }
            moonlight::Update::Identity(link) => {
                if !state.preferences.moonlight.paired {
                    state.preferences.moonlight = link;
                    let _ = storage::save(preferences_path, &state.preferences);
                }
            }
            moonlight::Update::Pin(pin) => state.moonlight_pin = Some(pin),
            moonlight::Update::PairProgress(message) => {
                if state.settings_level == state::SettingsLevel::MoonlightPin
                    && !state.moonlight_pair_error
                {
                    state.moonlight_status = Some(message);
                }
            }
            moonlight::Update::PairError(message) => {
                if state.settings_level == state::SettingsLevel::MoonlightPin {
                    state.moonlight_pair_error = true;
                    state.moonlight_status = Some(message);
                    state.focus = Target::MoonlightRetry;
                }
            }
            moonlight::Update::Status(message) => {
                if state.settings_level != state::SettingsLevel::MoonlightPin {
                    state.moonlight_status = Some(message);
                }
            }
            moonlight::Update::HostOnline(online) => {
                state.moonlight_online = online;
                if let Some(game) = state.game.as_mut() {
                    game.online = online;
                }
                if online && let Some(id) = state.moonlight_pending.take() {
                    if platform.webos {
                        state.launching = true;
                        state.moonlight_launched = true;
                        platform.send(Request::MoonlightLaunch(
                            state.preferences.moonlight.clone(),
                            id,
                        ));
                    } else {
                        state.notify("Desktop preview: would launch Moonlight");
                    }
                }
            }
            moonlight::Update::Paired(link) => {
                state.preferences.moonlight = link;
                let existing = state
                    .preferences
                    .stacks
                    .iter()
                    .position(|s| s.source == "moonlight");
                match existing {
                    Some(0) => {}
                    Some(i) => {
                        let stack = state.preferences.stacks.remove(i);
                        state.preferences.stacks.insert(0, stack);
                        if i < state.stack_items.len() {
                            let items = state.stack_items.remove(i);
                            state.stack_items.insert(0, items);
                        }
                    }
                    None => {
                        state.preferences.stacks.insert(
                            0,
                            stacks::StackConfig {
                                source: "moonlight".into(),
                                name: "Games".into(),
                                ..Default::default()
                            },
                        );
                        state.stack_items.insert(0, vec![]);
                    }
                }
                state.moonlight_status = Some(format!(
                    "Paired with {}",
                    state.preferences.moonlight.host_name
                ));
                let _ = storage::save(preferences_path, &state.preferences);
                // Show Games as the first Home row, not a Settings detail page.
                state.open(0);
                moonlight_service().send(moonlight::Command::Fetch(
                    state.preferences.moonlight.clone(),
                ));
            }
            moonlight::Update::Running(id) => {
                if id == 0 {
                    state.moonlight_launched = false;
                    if let Some(game) = state.game.as_mut() {
                        game.running = false;
                    }
                } else {
                    let item = state
                        .preferences
                        .stacks
                        .iter()
                        .position(|s| s.source == "moonlight")
                        .and_then(|row| state.stack_items.get(row).map(|items| (row, items)))
                        .and_then(|(row, items)| {
                            items
                                .iter()
                                .position(|i| i.id == id)
                                .map(|index| (row, index, items[index].clone()))
                        });
                    if let Some((row, index, item)) = item {
                        let hdr = state.moonlight_games.iter().any(|g| g.id == id && g.hdr);
                        let mut card = state::GameCard::new(
                            &item,
                            &state.preferences.moonlight.host_name,
                            true,
                            hdr,
                            Target::StackCard(row, index),
                        );
                        card.running = true;
                        if !state.game_info.contains_key(&id) {
                            steam_service().request(id, &item.title);
                        }
                        state.movie = None;
                        state.game = Some(card);
                        state.moonlight_online = true;
                        state.focus = Target::GamePlay;
                    }
                }
                state.dirty = true;
            }
            moonlight::Update::GameQuit(result) => {
                state.moonlight_status = None;
                match result {
                    Ok(()) => {
                        state.moonlight_launched = false;
                        state.close_game();
                        state.notify(format!(
                            "Game closed on {}",
                            state.preferences.moonlight.host_name
                        ));
                        platform.send(Request::CloseApp("com.limelight.webos".into()));
                    }
                    Err(error) => {
                        if let Some(game) = state.game.as_mut() {
                            game.running = true;
                        }
                        state.notify(error);
                    }
                }
                state.dirty = true;
            }
            moonlight::Update::Items(items, games) => {
                if state.preferences.games.note_seen(&games) {
                    let _ = storage::save(preferences_path, &state.preferences);
                }
                // Ask Steam for the whole row now, so a card opens complete.
                for game in &games {
                    if !state.game_info.contains_key(&game.id) {
                        steam_service().request(game.id, &game.title);
                    }
                }
                state.moonlight_items = items;
                state.moonlight_games = games;
                moonlight_refilter(state);
            }
        }
    }
}
/// The open Library page: apply its replies, ask for the next page and for
/// the posters around the screen.
fn library_tick(state: &mut State, preferences_path: &std::path::Path) {
    if state.library.is_none() {
        return;
    }
    for reply in library::poll() {
        if state.library_reply(reply) {
            let _ = storage::save(preferences_path, &state.preferences);
        }
    }
    for command in state.library_requests() {
        library::send(command);
    }
    state.library_posters();
}

/// Every Games stack shows its own category of the one Apollo list.
fn moonlight_refilter(state: &mut State) {
    let rows: Vec<(usize, String)> = state
        .preferences
        .stacks
        .iter()
        .enumerate()
        .filter(|(_, s)| s.source == "moonlight")
        .map(|(i, s)| (i, s.collection.clone()))
        .collect();
    for (row, collection) in rows {
        let picked = state.preferences.games.select(
            &collection,
            &state.moonlight_items,
            &state.moonlight_games,
        );
        if state.stack_items.len() <= row {
            state.stack_items.resize(row + 1, vec![]);
        }
        state.stack_items[row] = picked;
    }
    state.library_games_changed();
    state.dirty = true;
}
/// Plays a request: on the TV in the web player app (the TV's own web video
/// element, as media X uses), elsewhere in the built-in preview.
fn open_player(state: &mut State, platform: &Platform, request: player::OpenRequest) {
    if platform.webos {
        state.launching = true;
        state.web_player_item = request.item_id;
        platform.send(Request::WebPlayer(web_player_params(state, &request)));
        return;
    }
    state.playback = Some(player::Session::new(request.clone()));
    state.focus = Target::PlayerToggle;
    player_backend::send(player::Command::Open(Box::new(request)));
    state.dirty = true;
}

/// Launch parameters for com.archbtw.luma.player. Holds signed URLs and the token;
/// never logged.
fn web_player_params(state: &State, request: &player::OpenRequest) -> serde_json::Value {
    let (a, b) = state.accent_colors();
    let gradient = state.preferences.accent.kind == "gradient";
    let ink = view::contrast_text_for_accent(a, if gradient { b } else { a });
    let hex = |v: u32| format!("#{v:06x}");
    let ink = if ink[0] > 0.5 { "#ffffff" } else { "#121519" };
    let episode = |e: Option<(i64, i64)>| {
        e.map(|(season, number)| serde_json::json!({"season": season, "number": number}))
    };
    let tracks: Vec<_> = request
        .subtitles
        .iter()
        .map(|t| serde_json::json!({"name": t.name, "url": t.url}))
        .collect();
    let qualities: Vec<_> = request
        .qualities
        .iter()
        .map(|q| serde_json::json!({"name": q.name, "url": q.url}))
        .collect();
    let quality = request
        .qualities
        .iter()
        .position(|q| q.url.as_deref() == Some(request.url.as_str()))
        .unwrap_or(0);
    // Series titles carry " · S1E2"; the player shows the episode line instead.
    let title = request.title.split(" · S").next().unwrap_or(&request.title);
    serde_json::json!({
        "url": request.url,
        "title": title,
        "source": match request.source {
            player::Source::Series => "series",
            player::Source::Movie => "movie",
            player::Source::Youtube => "youtube",
            player::Source::Local => "local",
        },
        "episodeLine": request.episode_line,
        "resumeMs": request.resume_ms,
        "durationMs": request.duration_ms,
        "audio": request.audio.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
        "subtitles": tracks,
        "subtitle": request.selected_subtitle.map_or(-1, |i| i as i64),
        "qualities": qualities,
        "quality": quality,
        "id": request.item_id,
        "prev": episode(request.prev_episode),
        "next": episode(request.next_episode),
        "report": request.report.as_ref().map(|r| serde_json::json!({
            "service": r.service,
            "base": r.base,
            "token": r.token,
            "deviceId": r.device_id,
            "user": r.user,
            "version": env!("CARGO_PKG_VERSION"),
            "item": r.item,
            "source": r.source,
            "session": r.session,
            "episodes": r.episodes.iter().map(|e| serde_json::json!({
                "season": e.season,
                "number": e.number,
                "item": e.item,
                "url": e.url,
                "line": e.line,
            })).collect::<Vec<_>>(),
        })),
        "music": request.music.as_ref().map(|m| serde_json::json!({
            "album": m.album,
            "artist": m.artist,
            "year": m.year,
            "cover": m.cover,
            "start": m.start,
            "shuffle": m.shuffle,
            "songs": m.songs.iter().map(|s| serde_json::json!({
                "title": s.title,
                "artist": s.artist,
                "url": s.url,
                "item": s.item,
                "ms": s.ms,
            })).collect::<Vec<_>>(),
        })),
        "jellyfin": request.jellyfin,
        // While the debug log is on: where the player sends its lines.
        "debugLog": debug_log::player_sink(),
        // Settings › System: TrueHD and DTS asked of the TV, or set by hand.
        "audioSupport": {
            "truehd": state.preferences.truehd.key(),
            "dts": state.preferences.dts.key(),
        },
        "accentA": hex(a),
        "accentB": hex(if gradient { b } else { a }),
        "accentKind": if gradient { "gradient" } else { "solid" },
        "accentInk": ink,
    })
}

/// Signed in to Jellyfin with rows to show, and not one card in them.
fn jellyfin_rows_empty(state: &State) -> bool {
    state.preferences.jellyfin.linked()
        && state
            .preferences
            .stacks
            .iter()
            .enumerate()
            .filter(|(_, s)| s.show && s.source == "jellyfin")
            .all(|(i, _)| state.stack_items.get(i).is_none_or(Vec::is_empty))
        && state
            .preferences
            .stacks
            .iter()
            .any(|s| s.show && s.source == "jellyfin")
}

/// First retry of empty rows, and the longest wait between retries.
const STACKS_RETRY: Duration = Duration::from_secs(20);
const STACKS_RETRY_MAX: Duration = Duration::from_secs(5 * 60);

/// Auto UI resolution judges frames only once the first seconds after
/// start (fonts loading) are over.
const AUTO_SETTLE: Duration = Duration::from_secs(5);
/// Opening a film's page or starting a player or app draws a few slow frames
/// (artwork, a new surface) that say nothing about steady animation; Auto UI
/// resolution ignores frames for this long after one.
const AUTO_HEAVY_PAUSE: Duration = Duration::from_millis(2500);
static HEAVY_EFFECT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Home's media rows refresh this often while Luma is on screen.
const STACKS_EVERY: Duration = Duration::from_secs(30 * 60);
/// Coming back on screen refreshes them when the last load is this old.
const STACKS_AGAIN: Duration = Duration::from_secs(60);

fn stacks_refresh(state: &mut State) {
    let signed_in = |on: bool| if on { "signed in" } else { "not signed in" };
    debug_log!(
        "Home rows: loading {} (Jellyfin {}, Plex {}, Moonlight {})",
        state.preferences.stacks.len(),
        signed_in(state.preferences.jellyfin.linked()),
        signed_in(state.preferences.plex.linked()),
        if state.preferences.moonlight.paired {
            "paired"
        } else {
            "not paired"
        },
    );
    if state.preferences.plex.linked() {
        let wanted = state
            .preferences
            .stacks
            .iter()
            .enumerate()
            .filter(|(_, s)| s.source == "plex")
            .map(|(i, s)| (i, s.collection.clone()))
            .collect::<Vec<_>>();
        if !wanted.is_empty() {
            plex_service().send(plex::Command::Fetch(state.preferences.plex.clone(), wanted));
        }
    }
    if state.preferences.jellyfin.linked() {
        let wanted = state
            .preferences
            .stacks
            .iter()
            .enumerate()
            .filter(|(_, s)| s.source == "jellyfin")
            .map(|(i, s)| (i, s.collection.clone()))
            .collect::<Vec<_>>();
        if !wanted.is_empty() {
            jellyfin_service().send(jellyfin::Command::Fetch(
                state.preferences.jellyfin.clone(),
                wanted,
            ));
        }
    }
    if state.preferences.moonlight.paired {
        // One Apollo list feeds every Games stack.
        if state
            .preferences
            .stacks
            .iter()
            .any(|s| s.source == "moonlight")
        {
            moonlight_service().send(moonlight::Command::Fetch(
                state.preferences.moonlight.clone(),
            ));
        }
    }
}

fn weather_tick(
    state: &mut State,
    service: &weather_service::Service,
    visible: bool,
    preferences_path: &std::path::Path,
) {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let (command, changed) = state.weather.sync(
        &state.preferences,
        visible && state.page == state::Page::Home,
        now,
    );
    state.dirty |= changed;
    service.generations(state.weather.generations());
    if let Some(command) = command {
        service.send(command);
    }
    while let Ok(response) = service.receiver.try_recv() {
        match response {
            weather_service::Response::Search { id, result } => {
                state.dirty |= state.weather.accept_search(id, result)
            }
            weather_service::Response::Forecast { id, key, result } => {
                let (changed, command) = state.weather.accept_forecast(id, &key, result, now);
                state.dirty |= changed;
                if let Some(command) = command {
                    service.send(command);
                }
            }
        }
    }
    let weather = if state.preferences.show_weather {
        state
            .preferences
            .weather_location
            .as_ref()
            .and_then(|place| {
                state
                    .weather
                    .cache
                    .as_ref()
                    .filter(|cache| cache.key == place.key())
                    .map(|cache| (cache.reading, cache.updated_at))
            })
    } else {
        None
    };
    let local =
        time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let previous_phrase = state.rude_weather.current.clone();
    let saved = state.rude_weather.sync(
        &mut state.preferences,
        weather,
        now,
        local.date().to_julian_day(),
        local.hour(),
    );
    state.dirty |= previous_phrase != state.rude_weather.current;
    let mut saved = saved;
    if let Some(category) = state.rude_weather.wants_ai.take()
        && state.preferences.ai_headlines
        && state.preferences.greeting_style == weather_phrases::GreetingStyle::Rude
    {
        let seed = now as u64 ^ u64::from(local.nanosecond()).rotate_left(32);
        if let Some(note) = state.tech_writer.request(category, seed) {
            debug_log!("{note}");
        }
    }
    if let Some(outcome) = state.tech_writer.poll() {
        saved |= accept_ai_headlines(state, outcome, now);
    }
    if saved {
        let _ = storage::save(preferences_path, &state.preferences);
        state.dirty = true;
    }
}

/// Files the lines a model run wrote, then lets the Home headline use one.
fn accept_ai_headlines(state: &mut State, outcome: tech_writer::Outcome, now: i64) -> bool {
    debug_log!(
        "AI headlines: run took {:.1} s{}{}",
        outcome.elapsed.as_secs_f32(),
        if outcome.stats.is_empty() { "" } else { " · " },
        outcome.stats.join(" · ")
    );
    if let Some(error) = &outcome.error {
        debug_log!("AI headlines: {error}");
    }
    let label = outcome.category.label();
    let builtin: Vec<&str> = weather_phrases::subtitles(outcome.category).collect();
    let mut kept = 0;
    for (index, (word, raw)) in outcome.lines.iter().enumerate() {
        let Some((title, subtitle)) = tech_writer::accept(outcome.category, word, raw) else {
            debug_log!("AI headline dropped ({word}): {raw}");
            continue;
        };
        let lines = &mut state.preferences.weather_ai_lines;
        if lines.iter().any(|line| line.title == title) {
            continue;
        }
        let own = subtitle.is_some();
        let subtitle = subtitle.unwrap_or_else(|| {
            builtin
                .get((now as usize).wrapping_add(index * 7) % builtin.len().max(1))
                .map_or_else(String::new, |s| (*s).to_owned())
        });
        if subtitle.is_empty() {
            continue;
        }
        debug_log!(
            "AI headline kept ({word}): {title} | {subtitle}{}",
            if own { "" } else { " (built-in subtitle)" }
        );
        lines.push(tech_writer::Line {
            category: label.into(),
            title,
            subtitle,
        });
        kept += 1;
        let waiting = lines.iter().filter(|line| line.category == label).count();
        if waiting > tech_writer::KEEP
            && let Some(oldest) = lines.iter().position(|line| line.category == label)
        {
            lines.remove(oldest);
        }
    }
    if kept > 0 && state.rude_weather.offer_ai(&mut state.preferences, now) {
        state.dirty = true;
    }
    kept > 0
}

/// Scripted pointer drag steps: mdown:x:y, mmove:x:y, mup:x:y.
fn scripted_pointer(
    sdl: &sdl2::Sdl,
    window: &sdl2::video::Window,
    kind: &str,
    position: &str,
) -> Result<(), String> {
    let coordinates: Vec<f32> = position
        .split(':')
        .map(|value| value.parse::<f32>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let [x, y] = coordinates.as_slice() else {
        return Err("pointer: expects logical x:y coordinates".into());
    };
    let (w, h) = window.size();
    let scale = (w as f32 / 1920.).min(h as f32 / 1080.);
    let x = (x * scale + (w as f32 - 1920. * scale) / 2.).round() as i32;
    let y = (y * scale + (h as f32 - 1080. * scale) / 2.).round() as i32;
    let window_id = window.id();
    let event = match kind {
        "mdown" => Event::MouseButtonDown {
            timestamp: 0,
            window_id,
            which: SCRIPT_POINTER,
            mouse_btn: MouseButton::Left,
            clicks: 1,
            x,
            y,
        },
        "mup" => Event::MouseButtonUp {
            timestamp: 0,
            window_id,
            which: SCRIPT_POINTER,
            mouse_btn: MouseButton::Left,
            clicks: 1,
            x,
            y,
        },
        _ => Event::MouseMotion {
            timestamp: 0,
            window_id,
            which: SCRIPT_POINTER,
            mousestate: sdl2::mouse::MouseState::from_sdl_state(1),
            x,
            y,
            xrel: 0,
            yrel: 0,
        },
    };
    sdl.event()?.push_event(event)?;
    Ok(())
}

/// How far the page scrolls: to the lowest control, or to the real end of
/// the content when rows below the screen register no controls yet.
fn page_scroll_max(scene: &view::Scene, state: &state::State) -> f32 {
    let controls = scene
        .controls
        .iter()
        .filter(|c| c.scrollable)
        .map(|c| c.rect.y + c.rect.h + state.scroll - state.focus_bottom())
        .fold(0., f32::max);
    let content = if scene.page_bottom > 0. {
        scene.page_bottom + state.scroll - state.focus_bottom()
    } else {
        0.
    };
    controls.max(content)
}

fn scripted_click(
    sdl: &sdl2::Sdl,
    window: &sdl2::video::Window,
    position: &str,
) -> Result<(), String> {
    let coordinates: Vec<f32> = position
        .split(':')
        .map(|value| value.parse::<f32>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let [x, y] = coordinates.as_slice() else {
        return Err("click: expects logical x:y coordinates".into());
    };
    if !x.is_finite() || !y.is_finite() {
        return Err("Pointer coordinates must be finite".into());
    }
    let (w, h) = window.size();
    let scale = (w as f32 / 1920.).min(h as f32 / 1080.);
    let x = (x * scale + (w as f32 - 1920. * scale) / 2.).round() as i32;
    let y = (y * scale + (h as f32 - 1080. * scale) / 2.).round() as i32;
    let events = sdl.event()?;
    for event in [
        Event::MouseButtonDown {
            timestamp: 0,
            window_id: window.id(),
            which: SCRIPT_POINTER,
            mouse_btn: MouseButton::Left,
            clicks: 1,
            x,
            y,
        },
        Event::MouseButtonUp {
            timestamp: 0,
            window_id: window.id(),
            which: SCRIPT_POINTER,
            mouse_btn: MouseButton::Left,
            clicks: 1,
            x,
            y,
        },
    ] {
        events.push_event(event)?;
    }
    Ok(())
}

/// (visible, fullscreen camera, live cameras, hidden cameras) last sent to Smart Home.
type HaView = (bool, Option<String>, Vec<String>, Vec<String>);

/// Register with sam as Home, so the Home key relaunches the running Luma instead of
/// restarting it. Returns the SDL event type lifecycle events arrive as, and for a staged
/// Home a way to end the registration.
#[cfg(all(feature = "webos", target_os = "linux", target_arch = "arm"))]
#[allow(clippy::type_complexity)]
fn home_lifecycle(
    sdl: &sdl2::Sdl,
    as_home: bool,
) -> Result<(Option<u32>, Option<Box<dyn Fn()>>), String> {
    if !as_home {
        return Ok((None, None));
    }
    let subsystem = sdl.event()?;
    // SAFETY: registers one SDL user event type before any are pushed.
    let event_type = unsafe { subsystem.register_event()? };
    let pusher = move |sender: sdl2::event::EventSender| {
        move |lifecycle: platform::Lifecycle| {
            let code = match lifecycle {
                platform::Lifecycle::Relaunch => 1,
                platform::Lifecycle::Close => 2,
            };
            let _ = sender.push_event(Event::User {
                timestamp: 0,
                window_id: 0,
                type_: event_type,
                code,
                data1: std::ptr::null_mut(),
                data2: std::ptr::null_mut(),
            });
        }
    };
    // SAFETY: geteuid has no preconditions.
    let as_root = unsafe { libc::geteuid() } == 0;
    if platform::runs_as_lg_home() {
        // In place of LG's Home executable (webOS 5/6/24), LG's role is bound to that path,
        // so Luma registers through lifecycle v2 on its own connection.
        let push = pusher(subsystem.event_sender());
        luna::subscribe(
            Some(platform::home_id()),
            "luna://com.webos.applicationManager/registerApp",
            &serde_json::json!({"id": platform::home_id()}),
            // sam answers "registered" at once; without it sam would end Home later.
            Duration::from_secs(10),
            move |message| {
                eprintln!("Luma lifecycle: {message}");
                let refused = message
                    .get("returnValue")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false);
                match platform::Lifecycle::from_reply(&message) {
                    Some(lifecycle) => push(lifecycle),
                    // Registration lost (e.g. sam restarted): quit, so sam starts Home afresh.
                    None if refused => push(platform::Lifecycle::Close),
                    None => {}
                }
            },
        )?;
        return Ok((Some(event_type), None));
    }
    if !as_root {
        return Ok((None, None));
    }
    if std::path::Path::new(platform::HOME_LIFECYCLE_OFF).exists() {
        eprintln!(
            "Luma lifecycle: {} exists; Home stays unregistered",
            platform::HOME_LIFECYCLE_OFF
        );
        return Ok((None, None));
    }
    // A staged Home manifest (webOS 25/26) runs Luma's own executable as root under LG's
    // app-ID based Home role, which Luma's connection cannot claim. Hold the registration as
    // that app through `luna-send -a`; sam then expects lifecycle v1. Until sam answers, Home
    // is simply unregistered.
    let push = pusher(subsystem.event_sender());
    let close = pusher(subsystem.event_sender());
    match luna::subscribe_as_app(
        platform::home_id(),
        "luna://com.webos.applicationManager/registerNativeApp",
        &serde_json::json!({}),
        Duration::from_secs(10),
        move |message| {
            eprintln!("Luma lifecycle: {message}");
            if let Some(lifecycle) = platform::Lifecycle::from_reply(&message) {
                push(lifecycle);
            }
        },
        // A registration sam had accepted is gone: quit, so sam starts Home afresh.
        move |registered| {
            if registered {
                close(platform::Lifecycle::Close);
            }
        },
    ) {
        Ok(subscription) => Ok((
            Some(event_type),
            Some(Box::new(move || subscription.end()) as Box<dyn Fn()>),
        )),
        Err(error) => {
            eprintln!("Luma lifecycle: {error}; Home stays unregistered");
            Ok((None, None))
        }
    }
}

/// sam discards a system app's output. As Home, keep it in a small log.
#[cfg(all(feature = "webos", unix))]
fn log_home_output() {
    use std::os::unix::io::IntoRawFd;
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("/tmp/luma-home.log")
    {
        // SAFETY: duplicates a valid, owned descriptor onto stderr.
        unsafe { libc::dup2(file.into_raw_fd(), 2) };
    }
}

/// Turns the debug log on and says where it goes.
fn start_debug_log(preferences: &std::path::Path, offset: time::UtcOffset, why: &str) {
    match debug_log::start(&debug_log::directory(preferences), offset, why) {
        Ok(path) => eprintln!("Luma debug log: {}", path.display()),
        Err(error) => eprintln!("Luma debug log could not start: {error}"),
    }
}

/// "OpenUrl" for Effect::OpenUrl(…): the kind, never what it carries.
fn effect_name(effect: &Effect) -> String {
    let full = format!("{effect:?}");
    full.split(['(', '{', ' '])
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn run() -> Result<(), String> {
    let startup = Instant::now();
    // In place of LG's Home executable, before parsing, so option errors are kept too.
    #[cfg(all(feature = "webos", unix))]
    if platform::runs_as_lg_home() {
        log_home_output();
    }
    let mut options = Options::parse()?;
    // Running as LG's Home executable (webOS 5/6/24) is Home whatever the launch says.
    if cfg!(feature = "webos") && platform::runs_as_lg_home() {
        options.webos_home = true;
    }
    // A staged Home manifest (webOS 25/26) is only recognised from its launch parameters.
    #[cfg(all(feature = "webos", unix))]
    if options.webos_home && !platform::runs_as_lg_home() {
        log_home_output();
    }
    // As Home, keep using the normal app's settings (before any thread starts).
    #[cfg(all(feature = "webos", unix))]
    if options.webos_home {
        storage::adopt_app_owner();
    }
    let webos = cfg!(feature = "webos");
    let startup_stage = |stage: &str| {
        if webos {
            eprintln!(
                "Luma startup: {stage} after {} ms",
                startup.elapsed().as_millis()
            );
        }
    };
    if !webos && std::path::Path::new("/etc/starfish-release").exists() {
        return Err("This desktop build cannot run on a TV. Build with --features webos.".into());
    }
    if webos {
        if !options.webos_home && platform::redirect_to_home()? {
            return Ok(());
        }
        let surface_id = if options.webos_home {
            platform::home_id()
        } else {
            platform::APP_ID
        };
        // SDL-webOS reads APPID for its shell surface and getenv (not a hint)
        // for WMCLASS. Set these before video init and before starting workers.
        // SAFETY: startup is still single-threaded here.
        unsafe {
            std::env::set_var("APPID", surface_id);
            std::env::set_var("SDL_VIDEO_WAYLAND_WMCLASS", surface_id);
        }
        eprintln!("Luma webOS surface: {surface_id}");
        sdl2::hint::set("SDL_WEBOS_ACCESS_POLICY_KEYS_BACK", "true");
        // In place of LG's Home executable, SDL's own registration cannot reach the bus under
        // LG's Home role; Luma registers with sam itself below (lifecycle v2).
        if platform::runs_as_lg_home() {
            sdl2::hint::set("SDL_WEBOS_REGISTER_APP", "0");
        }
    }
    // Resolve the local offset before artwork, HTTP or platform workers start.
    let local_offset = OffsetDateTime::now_local()
        .map(|now| now.offset())
        .unwrap_or(time::UtcOffset::UTC);
    debug_log::set_offset(local_offset);
    // As LG's Home executable, assets are in the installed package, not beside /usr/bin.
    let root = match options.assets.clone() {
        Some(root) => root,
        None if webos => platform::install_dir()?.join("assets"),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".build/assets"),
    };
    let assets = view::Assets::load(root, webos)?;
    startup_stage("asset metadata");
    let default_preferences = options
        .preferences
        .clone()
        .map(Ok)
        .unwrap_or_else(|| storage::default_path(webos))?;
    let preferences_path = if options.test_extra_apps > 0 && options.preferences.is_none() {
        default_preferences.with_file_name("scroll-preview-preferences.json")
    } else {
        default_preferences.clone()
    };
    // The debug log starts before anything else can go wrong: by option, or
    // by a debug-log file a person put next to preferences.json.
    let log_flag = debug_log::flag(&preferences_path);
    if options.debug_log || log_flag.exists() {
        let why = if options.debug_log {
            "--debug-log".to_owned()
        } else {
            format!("{} exists", log_flag.display())
        };
        start_debug_log(&preferences_path, local_offset, &why);
    }
    // Uploaded video wallpapers join the catalog before preferences load, so
    // their saved selection survives normalization.
    custom_videos::init(&preferences_path);
    let _ = IMMICH.set(immich_frame::Service::new(immich_folder(&preferences_path)));
    // Start the preview with the user's current choices, but keep test favorites
    // and hidden placeholders out of their normal preferences file.
    let initial_preferences = if options.test_extra_apps > 0
        && options.preferences.is_none()
        && !preferences_path.exists()
    {
        storage::load(&default_preferences)?
    } else {
        storage::load(&preferences_path)?
    };
    if initial_preferences.debug_log {
        start_debug_log(&preferences_path, local_offset, "Settings › System");
    }
    let mut state = State::new(initial_preferences);
    state.debug_log_file = debug_log::current()
        .filter(|_| debug_log::enabled())
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let _ = moonlight_service();
    // Desktop game-card mockup and game Home row, using synthetic preview entries.
    #[cfg(debug_assertions)]
    if std::env::var_os("LUMA_GAME_PREVIEW").is_some() {
        let item = stacks::StackItem {
            title: std::env::var("LUMA_GAME_PREVIEW")
                .ok()
                .filter(|t| t.len() > 1)
                .unwrap_or_else(|| "Cyberpunk 2077".into()),
            id: 123,
            poster: None,
        };
        steam_service().request(item.id, &item.title);
        state.game = Some(state::GameCard::new(
            &item,
            "BEAST",
            true,
            true,
            Target::Nav(0),
        ));
        state.focus = Target::GamePlay;
        // Desktop QA of the entrance: Steam answered, frozen this many seconds in.
        if let Some(at) = std::env::var("LUMA_GAME_PREVIEW_AT")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            if let Ok((id, info)) = steam_service()
                .receiver
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(20))
            {
                state.game_info.insert(id, info);
            }
            if let Some(g) = state.game.as_mut() {
                let now = Instant::now();
                g.opened = now - std::time::Duration::from_secs(2);
                g.shown = Some(now - std::time::Duration::from_secs_f32(at));
                g.backdrop_at = g.shown;
            }
        }
    }
    #[cfg(debug_assertions)]
    if std::env::var_os("LUMA_STACK_PREVIEW").is_some() && state.preferences.stacks.is_empty() {
        state.preferences.stacks.push(stacks::StackConfig {
            name: "Games".into(),
            source: "moonlight".into(),
            ..Default::default()
        });
        state.stack_items.push(vec![
            stacks::StackItem {
                title: "Cyberpunk 2077".into(),
                id: 1,
                poster: None,
            },
            stacks::StackItem {
                title: "Hogwarts Legacy".into(),
                id: 2,
                poster: None,
            },
            stacks::StackItem {
                title: "Resident Evil".into(),
                id: 3,
                poster: None,
            },
        ]);
    }
    // Desktop QA of the Plex sign-in: LUMA_PLEX_PREVIEW=code|error.
    #[cfg(debug_assertions)]
    if let Ok(stage) = std::env::var("LUMA_PLEX_PREVIEW") {
        state.page = state::Page::Settings;
        state.section = state::STACKS_SECTION;
        state.settings_level = state::SettingsLevel::PlexLink;
        state.focus = Target::PlexCancel;
        state.plex_qr = phone_link::qr_matrix("https://app.plex.tv/auth#?clientID=0&code=QDS3");
        if stage == "error" {
            state.plex_error = true;
            state.plex_status =
                Some("Signed in, but no Plex Media Server answered. Is it on?".into());
        } else {
            state.plex_code = Some("QDS3".into());
            state.plex_status = Some("Waiting for sign-in…".into());
        }
    }
    // Desktop QA of the Home mini-player: LUMA_MINI_PREVIEW=<cover path>[:focused button 0-5].
    #[cfg(debug_assertions)]
    if let Ok(spec) = std::env::var("LUMA_MINI_PREVIEW") {
        let mut parts = spec.split(':');
        let cover = parts.next().filter(|p| !p.is_empty()).map(str::to_owned);
        let focus = parts.next().and_then(|v| v.parse::<usize>().ok());
        let songs: Vec<player::MusicSong> = [
            "Ambulance (Version With Rhodes Piano And Extended Outro)",
            "Homespin Rerun",
        ]
        .iter()
        .map(|t| player::MusicSong {
            title: (*t).to_owned(),
            artist: "Luke Vibert".into(),
            url: String::new(),
            item: String::new(),
            ms: 240_000,
        })
        .collect();
        state.music.now = Some(music::NowPlaying {
            album_id: 1,
            album: "Lover's Acid".into(),
            artist: "Luke Vibert".into(),
            cover,
            order: (0..songs.len()).collect(),
            songs,
            at: 0,
            shuffle: true,
            repeat: false,
            status: music::Status::Playing,
            position: 150.,
            duration: 240.,
            error: None,
        });
        if let Some(i) = focus {
            state.focus = state::mini::ROW[i.min(5)].clone();
            state.pointer = false;
        }
    }
    // Desktop QA of the album card: LUMA_ALBUM_PREVIEW=<cover image path>[:focused song].
    #[cfg(debug_assertions)]
    if let Ok(spec) = std::env::var("LUMA_ALBUM_PREVIEW") {
        let (cover, focus) = spec.rsplit_once(':').unwrap_or((spec.as_str(), ""));
        let item = stacks::StackItem {
            id: 7,
            title: "Claro".into(),
            poster: Some(cover.to_owned()),
        };
        state.open_album(item, Target::Nav(0));
        let mut album = state::album::Album {
            id: 7,
            title: "Claro".into(),
            artist: "Brothomstates".into(),
            year: 2001,
            songs: (1..=12)
                .map(|n| state::album::Song {
                    number: n,
                    title: if n == 5 {
                        "Brothomstates Ipxen (a much longer song title than usual)".into()
                    } else {
                        format!("Song number {n}")
                    },
                    artist: "Brothomstates".into(),
                    ms: 180_000 + n as u64 * 9_000,
                })
                .collect(),
            cover: Some(cover.to_owned()),
            backdrop: None,
        };
        if std::env::var("LUMA_ALBUM_NOBG").is_err() {
            album.backdrop = media::poster_backdrop(
                std::path::Path::new("/tmp"),
                "luma-album-preview-pb",
                cover,
            );
        }
        state.album_details(album);
        if let Ok(n) = focus.parse::<usize>() {
            state.focus = Target::AlbumSong(n);
            if let Some(card) = state.album.as_mut() {
                card.keep_visible(n);
            }
        }
    }
    // Desktop QA of the Jellyfin sign-in: LUMA_JELLYFIN_PREVIEW=code|search|error|phone.
    #[cfg(debug_assertions)]
    if let Ok(stage) = std::env::var("LUMA_JELLYFIN_PREVIEW") {
        state.page = state::Page::Settings;
        state.section = state::STACKS_SECTION;
        state.settings_level = state::SettingsLevel::JellyfinLink;
        state.focus = Target::JellyfinServer(0);
        if stage != "search" {
            state.jellyfin_servers.push(jellyfin::Server {
                name: "Living Room".into(),
                address: "http://192.168.1.50:8096".into(),
                version: "10.11.2".into(),
            });
            state.jellyfin_connect("http://192.168.1.50:8096".into());
        }
        match stage.as_str() {
            "search" => state.jellyfin_status = Some("Searching for servers…".into()),
            "error" => {
                state.jellyfin_error = true;
                state.jellyfin_status = Some(
                    "Quick Connect is off. Turn it on in Jellyfin: Dashboard › General".into(),
                );
            }
            "phone" => {
                state.jellyfin_phone = true;
                state.jellyfin_selected = None;
                state.jellyfin_qr =
                    phone_link::qr_matrix("http://192.168.1.206:47900/jellyfin?n=0");
                state.jellyfin_qr_caption = (
                    "Scan with your phone".into(),
                    "Type your Jellyfin server address there".into(),
                );
                state.jellyfin_status = Some("Scan the QR and type the server address".into());
                state.focus = Target::JellyfinServer(1);
            }
            _ => {
                state.jellyfin_code = Some("482917".into());
                state.jellyfin_status = Some("Waiting for approval…".into());
            }
        }
    }
    #[cfg(debug_assertions)]
    if let Ok(stage) = std::env::var("LUMA_MOONLIGHT_PREVIEW") {
        state.page = state::Page::Settings;
        state.section = state::STACKS_SECTION;
        let host = moonlight::Host {
            name: "BEAST".into(),
            address: "192.168.1.140".into(),
            uuid: "test-host".into(),
            online: true,
            https_port: 47984,
            ..Default::default()
        };
        state.moonlight_hosts.push(host.clone());
        state.moonlight_retry_host = Some(host);
        state.moonlight_status = Some("Waiting for the PIN…".into());
        if stage.starts_with("pin") {
            state.settings_level = state::SettingsLevel::MoonlightPin;
            state.moonlight_pin = Some("4829".into());
            state.focus = Target::MoonlightCancelButton;
            match stage.as_str() {
                "pin_pairing" => {
                    state.moonlight_status = Some("Step 3/5: verifying host identity…".into())
                }
                "pin_paired" => {
                    state.preferences.moonlight.paired = true;
                    state.moonlight_status = Some("Paired with BEAST".into());
                }
                "pin_error" => {
                    state.moonlight_pair_error = true;
                    state.moonlight_status = Some(
                        "Pairing step 2/5: Connection could not be completed. Try again.".into(),
                    );
                    state.focus = Target::MoonlightRetry;
                }
                _ => {}
            }
        } else {
            state.settings_level = state::SettingsLevel::MoonlightHosts;
            state.focus = Target::MoonlightHost(0);
        }
    }
    // Desktop QA of the Library page: LUMA_LIBRARY_PREVIEW=movies|games|albums[:focused card[:scroll]].
    #[cfg(debug_assertions)]
    if let Ok(spec) = std::env::var("LUMA_LIBRARY_PREVIEW") {
        let mut parts = spec.split(':');
        let kind = parts.next().unwrap_or_default();
        let games = kind == "games";
        let albums = kind == "albums";
        let focus: usize = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let scroll: f32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.);
        let images = [
            "forest.jpg",
            "dunes.jpg",
            "midnight.jpg",
            "prism.jpg",
            "northern-lights.jpg",
        ];
        let poster = |i: usize| {
            Some(
                assets
                    .root
                    .join("wallpapers")
                    .join(images[i % 5])
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        state.preferences.stacks = vec![crate::stacks::StackConfig {
            name: if games { "Games" } else { "Popular Movies" }.into(),
            source: if games { "moonlight" } else { "jellyfin" }.into(),
            collection: if games {
                "all"
            } else if albums {
                "albums"
            } else {
                "popular"
            }
            .into(),
            ..Default::default()
        }];
        state.stack_items = vec![
            (0..30)
                .map(|i| crate::stacks::StackItem {
                    title: format!("Title {i}"),
                    poster: poster(i),
                    id: i as i64 + 1,
                })
                .collect(),
        ];
        state.open_library(0, crate::stacks::HOME_ROW_MAX);
        if let Some(library) = state.library.as_mut() {
            library.items = (0..if games { 40 } else { 144 })
                .map(|i| library::LibItem {
                    id: i as i64 + 1,
                    title: format!("Title {i}"),
                    poster_url: String::new(),
                    poster: poster(i),
                })
                .collect();
            library.loaded = 3;
            library.pages = 3;
            library.total = if games { 40 } else { 4812 };
            library.opened = Instant::now() - std::time::Duration::from_secs(2);
            library.changed = library.opened;
            library.grid_focus = focus;
            library.reveal(focus);
            library.scroll_goal = library.scroll_goal.max(scroll);
            library.scroll = library.scroll_goal;
        }
        state.focus = match std::env::var("LUMA_LIBRARY_FOCUS").as_deref() {
            Ok("back") => Target::LibBack,
            Ok("search") => Target::LibSearch,
            Ok(tab) if tab.starts_with("tab") => Target::LibTab(tab[3..].parse().unwrap_or(0)),
            _ => Target::LibCard(focus),
        };
        state.dirty = true;
    }
    // Desktop QA of the Jellyfin Libraries row and its page: LUMA_HUB_PREVIEW=1.
    #[cfg(debug_assertions)]
    if std::env::var_os("LUMA_HUB_PREVIEW").is_some() {
        let libraries = [
            ("Movies", "412 movies", Some("forest.jpg")),
            ("TV Shows", "87 shows", Some("northern-lights.jpg")),
            ("Music", "1 album", Some("prism.jpg")),
            ("Home Videos", "", None),
        ];
        state.preferences.stacks = vec![crate::stacks::StackConfig {
            name: "Libraries".into(),
            source: "jellyfin".into(),
            collection: jellyfin::LIBRARIES.into(),
            shape: crate::stacks::CardShape::Wide,
            ..Default::default()
        }];
        state.jellyfin_libraries = libraries
            .iter()
            .enumerate()
            .map(|(i, (name, detail, _))| jellyfin::LibraryInfo {
                card: i as i64 + 1,
                collection: format!("{}:{i}", if *name == "Music" { "music" } else { "view" }),
                name: (*name).into(),
                detail: (*detail).into(),
            })
            .collect();
        state.stack_items = vec![
            libraries
                .iter()
                .enumerate()
                .map(|(i, (name, _, image))| crate::stacks::StackItem {
                    title: (*name).into(),
                    poster: image.map(|file| {
                        assets
                            .root
                            .join("wallpapers")
                            .join(file)
                            .to_string_lossy()
                            .into_owned()
                    }),
                    id: i as i64 + 1,
                })
                .collect(),
        ];
        state.dirty = true;
    }
    #[cfg(debug_assertions)]
    if std::env::var("LUMA_STACKS_PREVIEW").is_ok() {
        let images = [
            "forest.jpg",
            "dunes.jpg",
            "midnight.jpg",
            "prism.jpg",
            "northern-lights.jpg",
        ];
        state.preferences.stacks = vec![crate::stacks::StackConfig::default(); 3];
        state.stack_items = (0..3)
            .map(|row| {
                (0..12)
                    .map(|i| crate::stacks::StackItem {
                        title: format!("Film {row}.{i}"),
                        poster: Some(
                            assets
                                .root
                                .join("wallpapers")
                                .join(images[(i + row) % 5])
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        id: (row * 100 + i) as i64,
                    })
                    .collect()
            })
            .collect();
        state.dirty = true;
    }
    // Desktop design QA of the title card: LUMA_MOVIE_PREVIEW=movie|series[:panel].
    #[cfg(debug_assertions)]
    if let Ok(kind) = std::env::var("LUMA_MOVIE_PREVIEW") {
        state.movie_preview(&kind, &assets.root);
    }
    // Shuffle means a new order on every start, beginning with a random clip;
    // the saved seed alone replayed the same sequence each time.
    if !webos && let Ok(kind) = std::env::var("LUMA_PLAYER_PREVIEW") {
        let source = match kind.as_str() {
            "series" => player::Source::Series,
            "youtube" => player::Source::Youtube,
            "local" => player::Source::Local,
            _ => player::Source::Movie,
        };
        let mut session = player::Session::new(player::OpenRequest::preview(source));
        session.ready = std::env::var_os("LUMA_PLAYER_LOADING").is_none();
        session.paused = true;
        session.buffered_ms = session.position_ms + 600_000;
        state.playback = Some(session);
        state.focus = state::Target::PlayerToggle;
    }
    // TV diagnosis without the remote: a URL dropped into this file opens
    // the Luma player once at start (the file is consumed).
    if webos {
        let path = "/media/developer/com.archbtw.luma/cinema-test-url";
        if let Ok(url) = std::fs::read_to_string(path) {
            let _ = std::fs::remove_file(path);
            let mut request = player::OpenRequest::preview(player::Source::Movie);
            request.url = url.trim().to_owned();
            request.title = "Cinema test".into();
            request.resume_ms = 0;
            request.duration_ms = 0;
            request.audio.clear();
            request.subtitles.clear();
            request.qualities.clear();
            request.selected_subtitle = None;
            if let Ok(request) = request.sanitize() {
                state.playback = Some(player::Session::new(request.clone()));
                state.focus = state::Target::PlayerToggle;
                player_backend::send(player::Command::Open(Box::new(request)));
            }
        }
    }
    if webos && state.preferences.video_shuffle {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        state.preferences.video_shuffle_seed = seed;
        if let Some(&first) =
            video_catalog::playlist(&state.preferences.video_excluded, true, seed).first()
        {
            state.preferences.video_index = first;
        }
    }
    // App labels: preserve any names explicitly saved by the user.
    if webos {
        for (id, label) in [
            ("com.webos.app.hdmi1", "BEAST"),
            ("com.webos.app.hdmi2", "Apple TV"),
        ] {
            if !state.preferences.input_names.contains_key(id) {
                state.preferences.set_input_name(id, label);
            }
        }
    }
    if options.test_extra_apps > 0 {
        eprintln!(
            "Luma scroll preview: explicit {}-app expansion; preferences {}",
            options.test_extra_apps,
            preferences_path.display()
        );
    }
    if webos && options.weather_test_server.is_some() {
        return Err("Weather test servers are only available in desktop builds".into());
    }
    let weather_cache = weather_service::cache_path(&preferences_path);
    state.weather.cache = weather_service::load_cache(&weather_cache);
    let weather_service = weather_service::Service::new(
        weather_service::Client::new(options.weather_test_server.as_deref())?,
        weather_cache,
    );
    let home_service = smart_home::Service::new(
        smart_home::auth_path(&preferences_path),
        state.preferences.smart_home.server_url.clone(),
    );
    let platform = Platform::new(webos);
    effects(
        vec![Effect::Refresh, Effect::Root(None)],
        &mut state,
        &platform,
        &preferences_path,
        &weather_service,
        &home_service,
    );
    startup_stage("preferences and workers");

    if webos {
        // Ask EGL and Wayland compositor to preserve per-pixel alpha.
        sdl2::hint::set("SDL_VIDEO_EGL_ALLOW_TRANSPARENCY", "1");
    }
    let sdl = sdl2::init()?;
    let drivers: Vec<_> = sdl2::video::drivers().collect();
    if webos && !drivers.contains(&"wayland") {
        return Err(format!(
            "SDL has no Wayland backend (drivers: {}). Rebuild the package with the SDL-webOS runtime.",
            drivers.join(", ")
        ));
    }
    let ui_sound = ui_sound::UiSound::new(&sdl);
    let sounds_on = |state: &State| state.preferences.ui_sounds && ui_sound.is_some();
    let video = sdl.video().map_err(|error| {
        format!(
            "SDL video initialization: {error}; drivers: {}; XDG_RUNTIME_DIR={:?}; WAYLAND_DISPLAY={:?}",
            drivers.join(", "),
            std::env::var_os("XDG_RUNTIME_DIR"),
            std::env::var_os("WAYLAND_DISPLAY"),
        )
    })?;
    eprintln!("Luma SDL video driver: {}", video.current_video_driver());
    startup_stage("SDL video");
    let attributes = video.gl_attr();
    if webos {
        attributes.set_context_profile(GLProfile::GLES);
        attributes.set_context_version(2, 0);
    } else {
        attributes.set_context_profile(GLProfile::Core);
        attributes.set_context_version(3, 2);
    }
    attributes.set_double_buffer(true);
    attributes.set_alpha_size(8);
    attributes.set_depth_size(0);
    attributes.set_stencil_size(0);
    let requested = options.resolution.unwrap_or(if webos {
        state.preferences.resolved_ui_size()
    } else {
        Size::new(1280, 720)
    });
    let create_window = || {
        let mut builder = video.window("Luma", requested.width, requested.height);
        builder.opengl().allow_highdpi().position_centered();
        if options.fullscreen || (webos && options.resolution.is_none() && !requested.is_4k()) {
            builder.fullscreen_desktop();
        } else if webos {
            builder.borderless();
        } else {
            builder.resizable();
        }
        let window = builder.build().map_err(|e| e.to_string())?;
        let context = window.gl_create_context()?;
        Ok::<_, String>((window, context))
    };
    let mut opened = None;
    let mut failures = Vec::new();
    // Prefer true sample coverage of intact glyph outlines. Some TV EGL stacks
    // have fewer configurations, so retain a usable non-MSAA fallback.
    // A 4K 4x-MSAA color buffer is too expensive on C1/Mali-G51.
    // Glyphs still use native vector outlines and shader fringe AA at 1:1 resolution.
    let sample_options: &[u8] = if webos && requested.is_4k() {
        &[0]
    } else {
        &[4, 2, 0]
    };
    for &samples in sample_options {
        attributes.set_multisample_buffers(u8::from(samples > 0));
        attributes.set_multisample_samples(samples);
        match create_window() {
            Ok(window) => {
                opened = Some(window);
                break;
            }
            Err(error) => failures.push(format!("{samples}x MSAA: {error}")),
        }
    }
    let (mut window, _context) = opened.ok_or_else(|| failures.join("; "))?;
    gl::load_with(|name| video.gl_get_proc_address(name) as *const _);
    if !webos {
        unsafe { gl::Enable(gl::MULTISAMPLE) };
    }
    let mut samples = 0;
    let mut alpha_bits = 0;
    unsafe {
        gl::GetIntegerv(gl::SAMPLES, &mut samples);
        gl::GetIntegerv(0x0D55, &mut alpha_bits);
    }
    eprintln!("Luma GL: {samples} samples, {alpha_bits} alpha bits");
    if webos {
        let _ = std::fs::write(
            "/media/developer/com.archbtw.luma/luma-egl.txt",
            format!(
                "window={}x{} samples={} alpha_bits={}\n",
                requested.width, requested.height, samples, alpha_bits
            ),
        );
    }
    startup_stage("window and GL context");
    if let Err(error) = video.gl_set_swap_interval(1) {
        eprintln!("VSync: {error}");
    }
    // Animations run at the screen's own rate (VSync); say what that is.
    if let Some(mode) = window
        .display_index()
        .ok()
        .and_then(|index| video.current_display_mode(index).ok())
    {
        let (dw, dh) = window.drawable_size();
        let rate = if mode.refresh_rate > 0 {
            format!("{} Hz", mode.refresh_rate)
        } else {
            "an unreported rate".to_owned()
        };
        let setting = state.preferences.ui_resolution;
        eprintln!(
            "Luma display: {}x{} at {rate}, drawing {dw}x{dh} (UI resolution: {}{})",
            mode.w,
            mode.h,
            setting.label(),
            if setting == model::UiResolution::Auto {
                format!(
                    ", now {}",
                    state.preferences.resolved_ui_resolution().short()
                )
            } else {
                String::new()
            }
        );
    }
    let mut renderer = renderer::Renderer::new(&assets.root.join("font.ttf"), webos)?;
    renderer.set_fonts(
        state.preferences.heading_font,
        state.preferences.text_font,
        state.preferences.heading_size_pct,
        state.preferences.text_size_pct,
    );
    startup_stage("renderer");
    // SDL window resizing preserves the context and focus. The logical canvas
    // stays fixed at 1920x1080, regardless of EGL surface resolution.
    let mut applied_surface = requested;
    let mut last_working_choice = state.preferences.ui_resolution;
    let mut last_working_auto = state.preferences.auto_ui_cap;
    let mut auto_pacing = diagnostics::AutoPacing::default();
    let mut heavy_until = startup;
    // Debug log: each animation's frames, time and slowest frame.
    let mut burst: Option<(Instant, u32, f64)> = None;
    // The weather sky draws at the screen's rate unless this TV cannot keep
    // up; then every 33 ms (30 fps), as before.
    let mut sky_every = Duration::ZERO;
    let mut sky_window: Option<(Instant, u32)> = None;
    let mut sky_logged = false;
    let mut prior_present: Option<Instant> = None;
    let mut auto_downshifts = 0usize;
    let mut resolution_change_count = 0usize;
    let mut observed_resolutions: Vec<(Size, f64)> = Vec::new();
    let mut profile = diagnostics::Profile::default();
    let mut presented = 0usize;
    // LUMA_PROF_SLOW=<ms>: log every presented frame slower than that.
    let prof_slow: Option<f64> = std::env::var("LUMA_PROF_SLOW")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut slow_at: Option<Instant> = None;
    // Tabs whose text and icons were built ahead of the first switch to them,
    // for the fonts and drawable size in `warm_key`.
    let mut warmed: Vec<state::Page> = Vec::new();
    let mut warm_key = String::new();
    let mut captured = None;
    let mut last_drawable = None;
    let mut events = sdl.event_pump()?;
    // Lifecycle as Home: sam sends "relaunch" (bring the running Luma to the front) instead of
    // killing and restarting it, and "close" when it wants Luma to exit. `end_registration`
    // drops a staged Home's registration (webOS 25/26) if a relaunch did not reach the front.
    type EndRegistration = Box<dyn Fn()>;
    #[cfg(not(all(feature = "webos", target_os = "linux", target_arch = "arm")))]
    let (lifecycle_event, end_registration): (Option<u32>, Option<EndRegistration>) = (None, None);
    #[cfg(all(feature = "webos", target_os = "linux", target_arch = "arm"))]
    let (lifecycle_event, end_registration): (Option<u32>, Option<EndRegistration>) =
        home_lifecycle(&sdl, webos && options.webos_home)?;
    // Whether a staged Home's relaunch reached the front; see platform::ForegroundCheck.
    let mut foreground = platform::ForegroundCheck::default();
    let mut press: Option<Press> = None;
    // A Magic Remote press on nothing at all on Home: its release clears
    // Home away to the wallpaper.
    let mut empty_press = false;
    // When Up/Down started being held, to speed up long Settings lists.
    let mut vertical_held: Option<std::time::Instant> = None;
    let mut drag: Option<Drag> = None;
    let mut cinema_scrub = false;
    // Last cursor position (1920x1080 space) for edge auto-scroll of rows.
    let mut hover: Option<(f32, f32)> = None;
    let mut edge_lane_active: Option<Lane> = None;
    let mut edge_armed = true;
    let mut edge_at = Instant::now();
    // HDMI pictures are polled while Home shows, so Auto inputs follow a
    // device being switched on or put to sleep.
    let mut signals_at = Instant::now();
    let mut retry_at = Instant::now();
    let mut signals_pending = false;
    let initial_nav = view::navigation_in(
        state.preferences.layout,
        &assets,
        state.preferences.language,
        state.show_smart_tab(),
    );
    let (mut pill, mut pill_width) =
        view::nav_pill_target(state.preferences.layout, initial_nav.items[0]);
    let mut last_frame = Instant::now();
    let mut last_sky_frame = Instant::now();
    let mut last_minute = None;
    let mut frame = 0u64;
    let mut visible = true;
    // Home's media rows (Jellyfin, Plex, Moonlight): loaded at start, again
    // when Luma comes back on screen, and every STACKS_EVERY while shown.
    let mut stacks_at: Option<Instant> = None;
    // Rows that came back empty (the network not up yet after a cold boot)
    // are asked for again soon: 20 s, then twice as long, up to 5 minutes.
    let mut stacks_retry: Option<(Instant, Duration)> = None;
    // Cameras without a picture, logged once until they have one again.
    let mut blank_cameras = std::collections::BTreeSet::new();
    let mut last_ha_view: Option<HaView> = None;
    let mut last_ha_connect = false;
    // Starfish requires a fully displayed/foreground native Wayland window.
    // Starting its Load() before our first GL frame can stall the lifecycle.
    let video_init_after = Instant::now() + Duration::from_secs(2);
    let mut video_diag_count = 0usize;
    let mut still_preload_key = None;
    #[cfg(all(feature = "webos", target_os = "linux", target_arch = "arm"))]
    let mut aerial_player = if webos {
        match hw_wallpaper::Player::new() {
            Ok(player) => Some(player),
            Err(error) => {
                eprintln!("Hardware wallpaper unavailable: {error}");
                None
            }
        }
    } else {
        None
    };
    #[cfg(not(all(feature = "webos", target_os = "linux", target_arch = "arm")))]
    let mut aerial_player = webos.then(aerial_video::Player::new);
    let mut running = true;
    let mut queued_event = None;
    let mut text_active = false;
    let mut initial_focus = options.script.is_empty();
    let scripted_input = !options.script.is_empty();
    let mut scene = compose(
        &mut state,
        &assets,
        pill,
        pill_width,
        options.time.unwrap_or_else(OffsetDateTime::now_utc),
    );
    while running {
        let on_ha_connect = state.page == state::Page::Settings
            && state.subpage == Some(state::Subpage::SmartConnect);
        if last_ha_connect && !on_ha_connect {
            home_service.send(smart_home::Command::EndPairing);
        }
        last_ha_connect = on_ha_connect;
        let ha_view = (
            visible
                && (state.page == state::Page::SmartHome
                    || state.ha_camera.is_some()
                    // Smart Home widgets show live states and snapshots on Home.
                    || (state.page == state::Page::Home
                        && state.preferences.widgets.uses(widgets::Kind::Smart))),
            state.ha_camera.clone(),
            state
                .ha_tab(1)
                .iter()
                .map(|e| e.id.clone())
                .collect::<Vec<_>>(),
            state.preferences.smart_home.hidden_cameras.clone(),
        );
        if last_ha_view.as_ref() != Some(&ha_view) {
            home_service.send(smart_home::Command::View {
                visible: ha_view.0,
                fullscreen: ha_view.1.clone(),
                cameras: ha_view.2.clone(),
                hidden: ha_view.3.clone(),
            });
            last_ha_view = Some(ha_view);
        }
        while let Some(update) = home_service.try_recv() {
            match &update {
                smart_home::Update::Offline => debug_log!("Home Assistant offline"),
                smart_home::Update::Connected => debug_log!("Home Assistant connected"),
                smart_home::Update::Error(error) => debug_log!("Home Assistant error: {error}"),
                smart_home::Update::PairingError(error) => {
                    debug_log!("Home Assistant pairing error: {error}")
                }
                smart_home::Update::SnapshotFailed { id } => {
                    if blank_cameras.insert(id.clone()) {
                        debug_log!("Home Assistant camera {id}: no picture (retrying quietly)")
                    }
                }
                smart_home::Update::Snapshot { id, .. } => {
                    blank_cameras.remove(id);
                }
                smart_home::Update::SignedOut => debug_log!("Home Assistant signed out"),
                _ => {}
            }
            let should_save = matches!(&update, smart_home::Update::Authorized { .. });
            state.accept_ha(update);
            if should_save {
                let _ = storage::save(&preferences_path, &state.preferences);
            }
        }
        let now = options
            .time
            .unwrap_or_else(|| OffsetDateTime::now_utc().to_offset(local_offset));
        // Redraw once per minute, or once per second while seconds are shown.
        let clock_slot = if state.preferences.show_seconds {
            now.unix_timestamp()
        } else {
            now.unix_timestamp() / 60
        };
        if last_minute != Some(clock_slot) {
            last_minute = Some(clock_slot);
            state.dirty = true;
        }
        #[cfg(all(feature = "webos", target_os = "linux", target_arch = "arm"))]
        if let Some(start) = state.video_transition_started
            && start.elapsed() >= state::VIDEO_HANDOFF
        {
            hw_wallpaper::log_transition_event(state.preferences.video_index, "crossfade_end");
        }
        state.tick();
        // One call per frame: it marks the id as fetched, so a second call
        // for another service would never see it.
        if let Some(id) = state.movie_prefetch() {
            if plex::owns(id) {
                plex_service().send(plex::Command::Details(state.preferences.plex.clone(), id));
            } else if jellyfin::owns(id) {
                jellyfin_service().send(jellyfin::Command::Details(
                    state.preferences.jellyfin.clone(),
                    id,
                ));
            }
        }
        if webos && video_diag_count < 16 && frame.is_multiple_of(30) {
            let line = format!(
                "frame={} visible={} mode={} player={} playlist={}\n",
                frame,
                visible,
                state.preferences.background,
                aerial_player.as_ref().is_some_and(|player| player.active()),
                aerial_player.as_ref().map_or(0, |player| player.len())
            );
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/media/developer/com.archbtw.luma/luma-video-state.txt")
            {
                let _ = f.write_all(line.as_bytes());
            }
            video_diag_count += 1;
        }
        if aerial_player.is_none() {
            player_backend::set_video_plane_free(state.playback.is_some());
        }
        // A pending clip swap is timed in tens of milliseconds; wake for it.
        #[cfg_attr(
            not(all(feature = "webos", target_os = "linux", target_arch = "arm")),
            allow(unused_mut)
        )]
        let mut video_swap_pending = false;
        if let Some(player) = aerial_player.as_mut() {
            let enabled = visible
                && presented > 0
                && Instant::now() >= video_init_after
                && state.preferences.background == "video"
                && state.playback.is_none();
            let requested = enabled.then_some(state.preferences.video_index);
            if !enabled {
                state.video_frame_ready = false;
                state.video_transition_started = None;
            }
            #[cfg(all(feature = "webos", target_os = "linux", target_arch = "arm"))]
            player.set_eager_next(
                state.page == state::Page::Home && state.focus == state::Target::VideoTitle,
            );
            if let Some(next) = player.tick(
                requested,
                &state.preferences.video_excluded,
                state.preferences.video_shuffle,
                state.preferences.video_shuffle_seed,
            ) {
                if let Some((chosen, next_seed)) = video_catalog::next_choice(
                    state.preferences.video_index,
                    &state.preferences.video_excluded,
                    state.preferences.video_shuffle,
                    state.preferences.video_shuffle_seed,
                ) && chosen == next
                {
                    state.preferences.video_shuffle_seed = next_seed;
                }
                state.preferences.video_index = next;
                let _ = storage::save(&preferences_path, &state.preferences);
                state.dirty = true;
            }
            #[cfg(all(feature = "webos", target_os = "linux", target_arch = "arm"))]
            {
                // tick(None) above closes the NDL session synchronously; only then
                // may uMediaServer claim the main scaler for the movie.
                player_backend::set_video_plane_free(state.playback.is_some() && !player.active());
                // A clip swap first paints the next still over the plane, then
                // closes the old session, so the hand-off never shows black.
                let active = player.showing();
                video_swap_pending = player.swap_pending();
                if state.video_frame_ready != active {
                    state.video_frame_ready = active;
                    state.video_transition_started = active.then_some(Instant::now());
                    hw_wallpaper::log_transition_event(
                        state.preferences.video_index,
                        if active { "active" } else { "inactive" },
                    );
                    if active {
                        hw_wallpaper::log_transition_event(
                            state.preferences.video_index,
                            "crossfade_start",
                        );
                    }
                    state.dirty = true;
                }
            }
            #[cfg(not(all(feature = "webos", target_os = "linux", target_arch = "arm")))]
            if let Some(frame) = player.take_frame() {
                renderer.upload_aerial(&frame);
                if !state.video_frame_ready {
                    state.video_transition_started = Some(Instant::now());
                }
                state.video_frame_ready = true;
                // Each new video frame is a legitimate repaint, not an animation timer.
                state.dirty = true;
                if player.frame_count() <= 3 || player.frame_count() % 300 == 0 {
                    eprintln!("Luma video GPU frame #{} uploaded", player.frame_count());
                    let path = "/media/developer/com.archbtw.luma/luma-gpu-frames.txt";
                    use std::io::Write;
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        let _ = writeln!(
                            file,
                            "video_index={} frame={} bytes={} rgb_start={:?} rgb_middle={:?}",
                            state.preferences.video_index,
                            player.frame_count(),
                            frame.len(),
                            &frame[0..3],
                            &frame[frame.len() / 2..frame.len() / 2 + 3]
                        );
                    }
                }
            }
            if state.video_active != player.active() {
                state.video_active = player.active();
                state.dirty = true;
            }
            let title = if state.preferences.background == "video" {
                Some(player.name(state.preferences.video_index))
            } else {
                None
            };
            if state.video_title.as_deref() != title {
                state.video_title = title.map(str::to_owned);
                state.dirty = true;
            }
        }
        // Decode the stills a swap can show next ahead of time, off the GL
        // thread, so Next Video and the automatic swap never stall a frame.
        if state.preferences.background == "video" {
            let prefs = &state.preferences;
            let key = (
                prefs.video_index,
                prefs.video_shuffle,
                prefs.video_shuffle_seed,
                prefs.video_excluded.len(),
            );
            if still_preload_key != Some(key) {
                still_preload_key = Some(key);
                let current = prefs.video_index;
                let excluded = &prefs.video_excluded;
                let (shuffle, seed) = (prefs.video_shuffle, prefs.video_shuffle_seed);
                let mut indices = vec![current];
                indices.extend(video_catalog::next_index(current, excluded, shuffle, seed));
                indices.extend(video_catalog::prev_index(current, excluded, shuffle, seed));
                indices.dedup();
                let paths: Vec<String> = indices
                    .into_iter()
                    .map(|index| view::video_still_path(&assets, index))
                    .collect();
                renderer.preload_stills(&paths);
            }
        }
        state.dirty |= assets.artwork.poll();
        // Decode every app icon ahead of time: cards that enter before their
        // icon arrive grey and turn colored a moment later.
        for app in &state.inventory.apps {
            if crate::branding::for_key(&app.brand).is_none()
                && let Some(source) = app.icon.as_ref()
            {
                assets.artwork.get(source);
            }
        }
        while let Ok(response) = platform.receiver.try_recv() {
            match response {
                Response::Inventory(Ok((mut inventory, warning))) => {
                    if std::mem::take(&mut state.reload_artwork) {
                        assets.artwork.reset();
                    } else {
                        assets.artwork.refresh();
                    }
                    scroll_preview::append(
                        &mut inventory,
                        options.test_extra_apps,
                        state.preferences.language,
                    );
                    if state.set_inventory(inventory, warning) {
                        let _ = storage::save(&preferences_path, &state.preferences);
                    }
                    // Desktop QA of the HDMI buttons: LUMA_SIGNALS_PREVIEW=1:false,2:true
                    // (port number: has a picture). The desktop has no HDMI.
                    #[cfg(debug_assertions)]
                    if let Ok(spec) = std::env::var("LUMA_SIGNALS_PREVIEW") {
                        let signals: Vec<(u8, bool)> = spec
                            .split(',')
                            .filter_map(|pair| pair.split_once(':'))
                            .filter_map(|(port, on)| Some((port.parse().ok()?, on == "true")))
                            .collect();
                        model::apply_signals(&mut state.inventory.inputs, &signals);
                    }
                    if initial_focus
                        && state.focus == Target::Nav(0)
                        && state.page == state::Page::Home
                    {
                        state.focus = state.first_content();
                    }
                    initial_focus = false;
                }
                Response::Inventory(Err(error)) => {
                    state.loading = false;
                    state.error = Some(error);
                    state.dirty = true;
                    if initial_focus {
                        state.focus = state.first_content();
                        initial_focus = false;
                    }
                }
                Response::Signals(signals) => {
                    signals_pending = false;
                    if model::apply_signals(&mut state.inventory.inputs, &signals) {
                        state.dirty = true;
                    }
                }
                Response::Root(result) => {
                    match result {
                        Ok(status) => state.root = status,
                        Err(error) => {
                            eprintln!("Luma root features: {error}");
                            state.notify("Root features could not be changed");
                        }
                    }
                    state.dirty = true;
                }
                Response::Launched(result) => {
                    state.launching = false;
                    match result {
                        Err(error) => {
                            eprintln!("{error}");
                            if error == platform::INSTALL_PLAYER_MESSAGE
                                || error == platform::INSTALL_AERIALS_MESSAGE
                            {
                                state.notify(error);
                            } else {
                                state.notify(state.preferences.language.t("appOpenError"));
                            }
                        }
                        Ok(()) => {
                            state.notice = None;
                            state.dirty = true;
                        }
                    }
                }
            }
        }
        if webos
            && visible
            && state.page == state::Page::Home
            && !signals_pending
            && signals_at.elapsed() >= Duration::from_secs(4)
        {
            signals_at = Instant::now();
            signals_pending = true;
            platform.send(Request::Signals);
        }
        // A failed app list retries by itself: there is no error card to press.
        if state.error.is_some() && !state.loading && retry_at.elapsed() >= Duration::from_secs(10)
        {
            retry_at = Instant::now();
            state.loading = true;
            platform.send(Request::Refresh);
        }
        weather_tick(&mut state, &weather_service, visible, &preferences_path);
        state.dirty |= widgets::tick(
            &mut state.widget_data,
            &state.preferences.widgets,
            state.preferences.weather_location.as_ref(),
            visible && state.page == state::Page::Home,
            state.widget_detail.is_some(),
        );
        for (id, images) in media::thumbs_poll() {
            state.movie_thumbs(id, images);
        }
        jellyfin_tick(&mut state, &preferences_path, &platform);
        video_upload_tick(&mut state, &preferences_path);
        immich_tick(&mut state, &preferences_path, visible);
        plex_tick(&mut state, &preferences_path, &platform);
        // Poll a native worker rather than doing any media I/O in the render loop.
        while let Some(update) = player_backend::poll() {
            if let Some(session) = state.playback.as_mut() {
                let previous = (session.ready, session.position_ms, session.duration_ms);
                session.ready = update.first_frame_ms.is_some() || update.status == 2;
                session.first_frame_ms = update.first_frame_ms;
                if update.position_ms > 0 {
                    session.position_ms = update.position_ms;
                }
                if update.duration_ms > 0 {
                    session.duration_ms = update.duration_ms;
                }
                session.paused = update.status == 3;
                if let Some(cues) = update.cues {
                    session.cues = cues;
                    state.dirty = true;
                }
                if update.status == -1 {
                    session.error = update
                        .error
                        .or_else(|| Some("Video playback failed".into()));
                }
                state.dirty |=
                    previous != (session.ready, session.position_ms, session.duration_ms);
            }
        }

        moonlight_tick(&mut state, &preferences_path, &platform);
        // A cursor resting near the left/right edge of a Home row scrolls it,
        // faster the closer it is to the screen edge (like the stock LG home).
        let edge_dt = edge_at.elapsed().as_secs_f32().min(0.05);
        edge_at = Instant::now();
        let edge = hover
            .filter(|_| drag.is_none())
            .and_then(|pos| edge_lane(&state, &scene.controls, pos));
        if edge.is_none() {
            edge_armed = true;
        }
        let edge = edge.filter(|_| edge_armed);
        let mut edge_moving = false;
        if let Some((lane, push)) = edge {
            let step = push.signum() * push.abs().powf(1.5) * 1500. * edge_dt;
            match lane {
                Lane::Favorites => {
                    let max = state.home_scroll_max();
                    let next = (state.home_scroll + step).clamp(0., max);
                    if next != state.home_scroll {
                        state.home_scroll = next;
                        state.home_scroll_goal = next;
                        state.dirty = true;
                        edge_moving = true;
                    }
                }
                Lane::Stack(row) => edge_moving = state.nudge_stack_row(row, step),
                Lane::Similar => {}
            }
        }
        if edge_lane_active != edge.map(|e| e.0) {
            match edge_lane_active {
                Some(Lane::Stack(row)) => state.release_stack_row(row, 0.),
                // Favorites settle on a whole card too, like after a drag.
                Some(Lane::Favorites) => state.release_favorites(),
                _ => {}
            }
            edge_lane_active = edge.map(|e| e.0);
        }
        // Rebuild hit regions before input whenever inventory/layout changed.
        if state.dirty {
            scene = compose(&mut state, &assets, pill, pill_width, now);
        }
        for event in queued_event.take().into_iter().chain(events.poll_iter()) {
            // tvOS interruption contract: the next real input snaps the previous
            // view transition to its destination BEFORE hit-testing. This also
            // keeps the Magic Remote Back button clickable during entry motion.
            let new_navigation = matches!(
                &event,
                Event::MouseButtonDown { .. } | Event::KeyDown { repeat: false, .. }
            );
            if new_navigation
                && (state.panel_started.is_some()
                    || state.previous_panel_layer.is_some()
                    || state.previous_sidebar_layer.is_some())
            {
                state.panel_started = None;
                state.previous_panel_layer = None;
                state.previous_sidebar_layer = None;
                state.focus_tweens.clear();
                state.dirty = true;
                scene = compose(&mut state, &assets, pill, pill_width, now);
            }
            // Window-entry pointer events can arrive after a slow first upload.
            // Bounded scripted captures own input until their final frame; only
            // their explicitly injected SDL clicks may alter pointer focus.
            let script_owns_input = scripted_input
                && (options.frames.is_some()
                    || options.profile_frames.is_some()
                    || !options.script.is_empty());
            let external_input = match &event {
                Event::MouseButtonDown { which, .. } | Event::MouseButtonUp { which, .. } => {
                    *which != SCRIPT_POINTER
                }
                Event::MouseMotion { which, .. } => *which != SCRIPT_POINTER,
                Event::MouseWheel { .. }
                | Event::KeyDown { .. }
                | Event::KeyUp { .. }
                | Event::TextEditing { .. }
                | Event::TextInput { .. } => true,
                _ => false,
            };
            if script_owns_input && external_input {
                continue;
            }
            if external_input {
                state.last_input = std::time::Instant::now();
            }
            // Clean wallpaper mode: the first key, click or wheel only brings
            // the interface back. Its release is swallowed with it, since the
            // press that would pair with it never reached the handlers.
            if state.ambient_on() {
                let wake = match &event {
                    // Left and Right reach direction(), which flips the clip.
                    Event::KeyDown {
                        keycode: Some(Keycode::Left | Keycode::Right),
                        ..
                    } => false,
                    Event::KeyDown { .. }
                    | Event::MouseButtonDown { .. }
                    | Event::MouseWheel { .. }
                    | Event::ControllerButtonDown { .. } => true,
                    Event::MouseMotion { xrel, yrel, .. } => xrel.abs() + yrel.abs() > 8,
                    _ => false,
                };
                if wake {
                    state.leave_ambient();
                    continue;
                }
            }
            let mut pending = vec![];
            match event {
                Event::MouseMotion { x, y, .. } if state.playback.is_some() => {
                    if let Some((px, py)) = pointer_position(&window, x, y) {
                        if let Some(playback) = state.playback.as_mut() {
                            playback.reveal();
                            playback.hover_seek_ms =
                                if (816. ..=886.).contains(&py) && playback.panel.is_none() {
                                    Some(
                                        ((px - 88.) / 1744.)
                                            .clamp(0., 1.)
                                            .mul_add(playback.duration_ms as f32, 0.)
                                            as u64,
                                    )
                                } else {
                                    None
                                };
                            if cinema_scrub {
                                playback.position_ms = (((px - 88.) / 1744.).clamp(0., 1.)
                                    * (playback.duration_ms as f32))
                                    as u64;
                                playback.hover_seek_ms = Some(playback.position_ms);
                            }
                        }
                        state.dirty = true;
                        if !cinema_scrub
                            && let Some(target) = hit(&scene.controls, &window, x, y, &state)
                        {
                            state.pointer_focus(target);
                        }
                    }
                    continue;
                }
                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } if state.playback.is_some() => {
                    press = None;
                    drag = None;
                    if let Some((px, py)) = pointer_position(&window, x, y) {
                        if (816. ..=886.).contains(&py)
                            && state.playback.as_ref().is_some_and(|p| p.panel.is_none())
                        {
                            cinema_scrub = true;
                            if let Some(p) = state.playback.as_mut() {
                                p.position_ms = (((px - 88.) / 1744.).clamp(0., 1.)
                                    * (p.duration_ms as f32))
                                    as u64;
                                p.hover_seek_ms = Some(p.position_ms);
                                p.reveal();
                            }
                            state.dirty = true;
                        } else if let Some(target) = hit(&scene.controls, &window, x, y, &state) {
                            state.pointer_focus(target);
                        }
                    }
                    continue;
                }
                Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } if state.playback.is_some() => {
                    if cinema_scrub {
                        cinema_scrub = false;
                        if let Some(session) = state.playback.as_mut() {
                            session.hover_seek_ms = None;
                            pending
                                .push(Effect::Player(player::Command::Seek(session.position_ms)));
                        }
                        state.dirty = true;
                    } else if let Some((px, py)) = pointer_position(&window, x, y) {
                        if state.playback.as_ref().and_then(|p| p.panel).is_some()
                            && scene.modal.is_some_and(|r| !r.contains(px, py))
                        {
                            pending = state.back();
                        } else if let Some(target) = hit(&scene.controls, &window, x, y, &state) {
                            state.pointer_focus(target);
                            pending = state.activate();
                        }
                    }
                }
                Event::MouseWheel { x, y, .. } if state.playback.is_some() => {
                    let d = if y > 0 {
                        state::Direction::Up
                    } else {
                        state::Direction::Down
                    };
                    if state.playback.as_ref().is_some_and(|p| p.panel.is_some()) {
                        pending = state.direction(d, &scene.controls);
                    } else if let Some(p) = state.playback.as_mut() {
                        let delta = if y > 0 { 10_000 } else { -10_000 };
                        pending.push(Effect::Player(p.seek_by(delta)));
                    }
                    state.dirty = true;
                    let _ = x;
                }
                Event::Quit { .. } => running = false,
                Event::User { type_, code, .. } if Some(type_) == lifecycle_event => {
                    if code == 1 {
                        eprintln!("Luma lifecycle: relaunch, back to Home");
                        press = None;
                        pending = state.return_home();
                        // A staged Home must come forward itself. If it does not, its
                        // registration would leave the Home key dead: checked below.
                        if end_registration.is_some() {
                            foreground.relaunched(Instant::now(), visible);
                        }
                        // SAFETY: the window outlives the event loop.
                        unsafe {
                            sdl2::sys::SDL_RestoreWindow(window.raw());
                            sdl2::sys::SDL_ShowWindow(window.raw());
                            sdl2::sys::SDL_RaiseWindow(window.raw());
                        }
                        visible = true;
                        state.dirty = true;
                        pending.push(Effect::Refresh);
                        if stacks_at.is_some_and(|at| at.elapsed() >= STACKS_AGAIN) {
                            stacks_at = None;
                        }
                    } else if code == 2 {
                        running = false;
                    }
                }
                Event::Window {
                    window_id,
                    win_event,
                    ..
                } if window_id == window.id() => match win_event {
                    WindowEvent::Hidden | WindowEvent::Minimized => {
                        visible = false;
                        press = None;
                        drag = None;
                        if matches!(state.overlay, Some(Overlay::SystemKeyboard)) {
                            pending = state.finish_text_input();
                        }
                        profile.pause();
                    }
                    WindowEvent::FocusLost => press = None,
                    WindowEvent::Shown | WindowEvent::Restored | WindowEvent::FocusGained => {
                        visible = true;
                        // Back from an app: Continue Watching may have moved on.
                        if stacks_at.is_some_and(|at| at.elapsed() >= STACKS_AGAIN) {
                            stacks_at = None;
                        }
                        // Shown and Restored can come from SDL itself; only focus confirms.
                        if matches!(win_event, WindowEvent::FocusGained) {
                            foreground.confirmed();
                        }
                        state.dirty = true;
                        // Double Start+Select on a TV gamepad (pad_watch): close the game.
                        let quit = std::fs::remove_file(pad_watch::FLAG).is_ok();
                        if quit && state.preferences.moonlight.paired {
                            pending.push(Effect::MoonlightQuit);
                        } else if state.moonlight_launched && state.preferences.moonlight.paired {
                            // Back from a stream (Home on the remote): offer Resume / Close game.
                            moonlight_service().send(moonlight::Command::CheckRunning(
                                state.preferences.moonlight.clone(),
                            ));
                        }
                        if webos {
                            pending.push(Effect::Refresh);
                        }
                        // Back from the web player: media has the new watch time.
                        if let Some(id) = state.web_player_item.take()
                            && state.movie.as_ref().is_some_and(|m| m.id == id)
                        {
                            pending.push(Effect::MediaDetails(id));
                        }
                    }
                    WindowEvent::Resized(..)
                    | WindowEvent::SizeChanged(..)
                    | WindowEvent::Exposed => state.dirty = true,
                    _ => {}
                },
                Event::TextInput { text, .. } => state.text(&text),
                Event::TextEditing {
                    text,
                    start,
                    length,
                    ..
                } if text.is_empty() && matches!(state.overlay, Some(Overlay::SystemKeyboard)) => {
                    state.delete_surrounding_text(start, length);
                }
                Event::KeyDown {
                    keycode: Some(key),
                    repeat,
                    ..
                } => {
                    let focus_before = state.focus.clone();
                    let page_before = state.page;
                    hover = None;
                    // Holding Left/Right keeps moving along a row; the top
                    // navigation still needs one press per page.
                    let sideways = !repeat || !matches!(state.focus, state::Target::Nav(_));
                    // Holding Up/Down runs through long lists (Settings, Smart
                    // Home): one row per repeat, then two, then four.
                    let listing =
                        matches!(state.page, state::Page::Settings | state::Page::SmartHome)
                            && !matches!(state.focus, state::Target::Nav(_));
                    let vertical = !repeat || listing;
                    if matches!(key, Keycode::Up | Keycode::Down) && !repeat {
                        vertical_held = Some(std::time::Instant::now());
                    }
                    let steps = match vertical_held.map(|t| t.elapsed().as_secs_f32()) {
                        Some(s) if repeat && s > 2.5 => 4,
                        Some(s) if repeat && s > 1.2 => 2,
                        _ => 1,
                    };
                    match key {
                        Keycode::Up | Keycode::Down if vertical => {
                            let direction = if key == Keycode::Up {
                                Direction::Up
                            } else {
                                Direction::Down
                            };
                            for step in 0..steps {
                                if step > 0 {
                                    scene = compose(&mut state, &assets, pill, pill_width, now);
                                }
                                pending.extend(state.direction(direction, &scene.controls));
                            }
                        }
                        Keycode::Left if sideways => {
                            pending = state.direction(Direction::Left, &scene.controls)
                        }
                        Keycode::Right if sideways => {
                            pending = state.direction(Direction::Right, &scene.controls)
                        }
                        Keycode::Return | Keycode::KpEnter
                            if matches!(state.overlay, Some(Overlay::SystemKeyboard)) =>
                        {
                            press = None;
                            pending = state.finish_text_input();
                        }
                        Keycode::Return | Keycode::KpEnter if !repeat => {
                            press = Some(Press {
                                target: state.focus.clone(),
                                start: Instant::now(),
                                held: false,
                                pointer: false,
                            });
                        }
                        Keycode::Escape | Keycode::AcBack if !repeat => {
                            press = None;
                            pending = state.back();
                        }
                        Keycode::Backspace if state.text_input_active() => state.delete(),
                        Keycode::Backspace if !repeat => {
                            press = None;
                            pending = state.back();
                        }
                        Keycode::Clear
                            if matches!(state.overlay, Some(Overlay::SystemKeyboard)) =>
                        {
                            let length = state.editing_text().len() as i32;
                            state.delete_surrounding_text(-length, length);
                        }
                        Keycode::M | Keycode::Menu if !repeat && state.overlay.is_none() => {
                            press = None;
                            state.options();
                        }
                        Keycode::F5 if !repeat => pending.push(Effect::Reload),
                        key if !repeat && is_back_keycode(i32::from(key), webos) => {
                            press = None;
                            pending = state.back();
                        }
                        _ => {}
                    }
                    if sounds_on(&state)
                        && (state.focus != focus_before || state.page != page_before)
                        && let Some(sound) = &ui_sound
                    {
                        sound.tick();
                    }
                }
                Event::KeyUp {
                    keycode: Some(Keycode::Return | Keycode::KpEnter),
                    ..
                } => {
                    if let Some(held) = press.take()
                        && !held.pointer
                        && !held.held
                        && held.target == state.focus
                    {
                        pending = state.activate();
                        if sounds_on(&state)
                            && let Some(sound) = &ui_sound
                        {
                            sound.select();
                        }
                    }
                }
                Event::MouseMotion { x, y, .. }
                    if state.moving.is_none()
                        && !matches!(state.overlay, Some(Overlay::SystemKeyboard)) =>
                {
                    let position = pointer_position(&window, x, y);
                    hover = position;
                    // Edit mode: a drag that starts on a widget carries it.
                    if state.widget_edit.is_some()
                        && let (Some(d), Some((px, py))) = (drag.as_mut(), position)
                    {
                        let distance =
                            ((px - d.started.0).powi(2) + (py - d.started.1).powi(2)).sqrt();
                        if !state.widget_grabbed()
                            && distance > 12.
                            && let Some((index, rect)) = scene.controls.iter().find_map(|c| match c
                                .target
                            {
                                Target::Widget(i)
                                    if (c.rect.x..c.rect.x + c.rect.w).contains(&d.started.0)
                                        && (c.rect.y..c.rect.y + c.rect.h)
                                            .contains(&d.started.1) =>
                                {
                                    Some((i, c.rect))
                                }
                                _ => None,
                            })
                        {
                            state.widget_grab(index, rect, d.started.0, d.started.1);
                        }
                        if state.widget_grabbed() {
                            d.moved = true;
                            state.widget_drag_to(px, py, &scene.controls);
                            press = None;
                            continue;
                        }
                    }
                    if let (Some(d), Some((px, py))) = (drag.as_mut(), position) {
                        let distance =
                            ((px - d.started.0).powi(2) + (py - d.started.1).powi(2)).sqrt();
                        if distance > 12. && !d.moved {
                            d.moved = true;
                            d.horizontal = d.lane.is_some()
                                && (px - d.started.0).abs() > (py - d.started.1).abs();
                        }
                        if d.moved && state.overlay.is_none() {
                            let movement = if d.horizontal {
                                d.previous.0 - px
                            } else {
                                d.previous.1 - py
                            };
                            if state.library.is_some()
                                && state.movie.is_none()
                                && state.game.is_none()
                            {
                                state.library_drag(d.previous.1 - py);
                                let seconds = d.previous_at.elapsed().as_secs_f32().max(0.006);
                                d.velocity = ((d.previous.1 - py) / seconds).clamp(-3500., 3500.);
                                d.previous = (px, py);
                                d.previous_at = Instant::now();
                                press = None;
                                continue;
                            }
                            if state.movie.is_some() {
                                state.movie_drag(movie_scroll(&state, d.horizontal), movement);
                                let seconds = d.previous_at.elapsed().as_secs_f32().max(0.006);
                                d.velocity = (movement / seconds).clamp(-3500., 3500.);
                                d.previous = (px, py);
                                d.previous_at = Instant::now();
                                press = None;
                                continue;
                            }
                            let max = if d.horizontal {
                                state.home_scroll_max()
                            } else {
                                page_scroll_max(&scene, &state).max(0.)
                            };
                            match (d.horizontal, d.lane) {
                                (true, Some(Lane::Stack(row))) => {
                                    state.drag_stack_row(row, movement)
                                }
                                (true, _) => {
                                    state.home_scroll =
                                        state.rubber(state.home_scroll, movement, max);
                                    state.home_scroll_goal = state.home_scroll;
                                }
                                _ => {
                                    state.scroll = state.rubber(state.scroll, movement, max);
                                    state.scroll_goal = state.scroll;
                                }
                            }
                            let seconds = d.previous_at.elapsed().as_secs_f32().max(0.006);
                            d.velocity = (movement / seconds).clamp(-3500., 3500.);
                            d.previous = (px, py);
                            d.previous_at = Instant::now();
                            state.dirty = true;
                            press = None;
                            continue;
                        }
                    }
                    let hovered = hit(&scene.controls, &window, x, y, &state);
                    if let Some(target) = hovered.clone() {
                        state.pointer_focus(target);
                    }
                    if press
                        .as_ref()
                        .is_some_and(|p| p.pointer && hovered.as_ref() != Some(&p.target))
                    {
                        press = None;
                    }
                }
                Event::MouseButtonDown {
                    mouse_btn, x, y, ..
                } => {
                    if mouse_btn == MouseButton::Left
                        && state.overlay.is_none()
                        && let Some(pos) = pointer_position(&window, x, y)
                    {
                        // The row under the pointer decides what a sideways drag moves;
                        // the direction of the first movement decides the axis.
                        let lane = if state.movie.is_some() {
                            matches!(
                                hit(&scene.controls, &window, x, y, &state),
                                Some(Target::MovieSimilar(_))
                            )
                            .then_some(Lane::Similar)
                        } else if state.page == state::Page::Home {
                            match hit(&scene.controls, &window, x, y, &state) {
                                Some(Target::App(_)) => Some(Lane::Favorites),
                                Some(Target::StackCard(row, _)) => Some(Lane::Stack(row)),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        state.pointer = true;
                        drag = Some(Drag {
                            previous: pos,
                            started: pos,
                            previous_at: Instant::now(),
                            velocity: 0.,
                            moved: false,
                            horizontal: false,
                            lane,
                        });
                        state.stop_kinetic();
                    }
                    empty_press = mouse_btn == MouseButton::Left
                        && scene.modal.is_none()
                        && hit(&scene.controls, &window, x, y, &state).is_none()
                        && state.home_can_clear();
                    if let Some(target) = hit(&scene.controls, &window, x, y, &state)
                        && state.moving.is_none()
                    {
                        if matches!(state.overlay, Some(Overlay::SystemKeyboard)) {
                            pending = state.finish_text_input();
                        }
                        state.pointer_focus(target);
                        if mouse_btn == MouseButton::Right {
                            state.options();
                            press = None;
                        } else if mouse_btn == MouseButton::Left {
                            press = Some(Press {
                                target: state.focus.clone(),
                                start: Instant::now(),
                                held: false,
                                pointer: true,
                            });
                        }
                    } else if mouse_btn == MouseButton::Left {
                        press = None;
                    }
                }
                Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } => {
                    if state.widget_grabbed() {
                        drag = None;
                        press = None;
                        let pending = state.widget_drop();
                        running &= effects(
                            pending,
                            &mut state,
                            &platform,
                            &preferences_path,
                            &weather_service,
                            &home_service,
                        );
                    } else if state.movie.is_some()
                        && let Some(d) = drag.take_if(|d| d.moved)
                    {
                        state.movie_release(movie_scroll(&state, d.horizontal), d.velocity);
                        press = None;
                    } else if state.library.is_some()
                        && state.game.is_none()
                        && let Some(d) = drag.take_if(|d| d.moved)
                    {
                        state.library_release(d.velocity);
                        press = None;
                    } else if let Some(d) = drag.take()
                        && d.moved
                    {
                        let max = if d.horizontal {
                            state.home_scroll_max()
                        } else {
                            page_scroll_max(&scene, &state).max(0.)
                        };
                        // The edge auto-scroll waits until the pointer has left the edge.
                        edge_armed = false;
                        if let (true, Some(Lane::Stack(row))) = (d.horizontal, d.lane) {
                            state.release_stack_row(row, d.velocity);
                        } else if d.horizontal && !(0. ..=max).contains(&state.home_scroll) {
                            // Pulled past an end: spring back instead of coasting.
                            state.home_scroll_goal = state.home_scroll.clamp(0., max);
                        } else if !d.horizontal && !(0. ..=max).contains(&state.scroll) {
                            state.scroll_goal = state.scroll.clamp(0., max);
                        } else if !state.preferences.reduced_motion {
                            state.start_kinetic(d.velocity, d.horizontal, max);
                        }
                        state.dirty = true;
                        press = None;
                    } else if std::mem::take(&mut empty_press)
                        && hit(&scene.controls, &window, x, y, &state).is_none()
                        && state.home_can_clear()
                    {
                        // Clicked beside everything: Home clears to the wallpaper.
                        press = None;
                        state.clear_home();
                    } else {
                        match press.take() {
                            Some(held)
                                if held.pointer
                                    && !held.held
                                    && hit(&scene.controls, &window, x, y, &state).as_ref()
                                        == Some(&held.target) =>
                            {
                                // The video caption has no OK action: its halves are ‹ and ›.
                                if held.target == state::Target::VideoTitle {
                                    let rect = scene
                                        .controls
                                        .iter()
                                        .find(|c| c.target == state::Target::VideoTitle)
                                        .map(|c| c.rect);
                                    if let (Some(rect), Some((px, _))) =
                                        (rect, pointer_position(&window, x, y))
                                    {
                                        let direction = if px < rect.x + rect.w / 2. {
                                            state::Direction::Left
                                        } else {
                                            state::Direction::Right
                                        };
                                        pending = state.direction(direction, &scene.controls);
                                    }
                                } else {
                                    pending = state.activate()
                                }
                            }
                            None if scene.modal.is_some_and(|rect| {
                                pointer_position(&window, x, y)
                                    .is_some_and(|(x, y)| !rect.contains(x, y))
                            }) =>
                            {
                                pending = state.back()
                            }
                            _ => {}
                        }
                    }
                }
                Event::MouseWheel {
                    x,
                    y,
                    mouse_x,
                    mouse_y,
                    ..
                } if state.overlay.is_none() && state.movie.is_some() => {
                    // Title card: the row under the pointer slides sideways,
                    // the side panel list or the page moves up and down.
                    let over_similar =
                        pointer_position(&window, mouse_x, mouse_y).is_some_and(|(px, py)| {
                            scene.controls.iter().any(|control| {
                                matches!(control.target, Target::MovieSimilar(_))
                                    && control.rect.contains(px, py)
                            })
                        });
                    let steps = (if x != 0 { x } else { -y }) as f32;
                    state.movie_wheel(movie_scroll(&state, over_similar || x != 0), steps);
                    press = None;
                    state.pointer = true;
                }
                Event::MouseWheel { y, .. }
                    if state.overlay.is_none()
                        && state.movie.is_none()
                        && state.game.is_none()
                        && state.library.is_some() =>
                {
                    // Library: the wheel moves the grid; focus stays put.
                    state.library_wheel(-y as f32 * 150.);
                    press = None;
                    state.pointer = true;
                }
                Event::MouseWheel {
                    x,
                    y,
                    mouse_x,
                    mouse_y,
                    ..
                } if state.overlay.is_none() && state.movie.is_none() => {
                    let over_stack = if state.page == state::Page::Home {
                        pointer_position(&window, mouse_x, mouse_y).and_then(|(px, py)| {
                            scene
                                .controls
                                .iter()
                                .find_map(|control| match control.target {
                                    Target::StackCard(row, _) if control.rect.contains(px, py) => {
                                        Some(row)
                                    }
                                    _ => None,
                                })
                        })
                    } else {
                        None
                    };
                    // The vertical wheel always moves the whole page: a row
                    // under the pointer must not swallow it. Only a sideways wheel
                    // slides the row under the pointer.
                    if let Some(row) = over_stack.filter(|_| x != 0) {
                        state.scroll_stack_row(row, x as f32);
                        press = None;
                    } else if state.page == state::Page::Home && x != 0 {
                        state.scroll_favorites(x as f32 * state::HOME_TILE_PITCH);
                        press = None;
                    } else {
                        let max = page_scroll_max(&scene, &state);
                        let wanted = state.scroll - y as f32 * 90.;
                        let max = max.max(0.);
                        if wanted < 0. && state.scroll <= 0.5 {
                            state.bounce(state::BounceAxis::Page, -60.);
                        } else if wanted > max && state.scroll >= max - 0.5 {
                            state.bounce(state::BounceAxis::Page, 60.);
                        }
                        state.scroll = wanted.clamp(0., max);
                        state.scroll_goal = state.scroll;
                    }
                    state.pointer = true;
                    state.dirty = true;
                }
                _ => {}
            }
            running &= effects(
                pending,
                &mut state,
                &platform,
                &preferences_path,
                &weather_service,
                &home_service,
            );
            if press
                .as_ref()
                .is_some_and(|p| !p.held && p.target != state.focus)
            {
                press = None;
            }
            let pressed = press.as_ref().filter(|p| !p.held).map(|p| p.target.clone());
            if state.pressed != pressed {
                state.pressed = pressed;
                state.dirty = true;
            }
            // SDL may deliver several navigation events in a single poll.
            if state.dirty {
                scene = compose(&mut state, &assets, pill, pill_width, now);
            }
        }
        if let Some(held) = &mut press
            && !held.held
            && held.start.elapsed() >= Duration::from_millis(600)
        {
            held.held = true;
            state.pressed = None;
            let pending = state.hold();
            running &= effects(
                pending,
                &mut state,
                &platform,
                &preferences_path,
                &weather_service,
                &home_service,
            );
        }
        let script_waiting = options.script.front().is_some_and(|action| {
            (action == "await-search" && state.weather.searching)
                || (action == "await-weather" && state.weather.fetching())
        });
        // Scripted async waits should consume wall-clock time, not thousands
        // of unthrottled render frames before a mock HTTP reply can arrive.
        if script_waiting {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !state.loading
            && frame.is_multiple_of(3)
            && !script_waiting
            && let Some(action) = options.script.pop_front()
        {
            if let Some(position) = action.strip_prefix("click:") {
                scripted_click(&sdl, &window, position)?;
            } else if let Some((kind, position)) = action
                .split_once(':')
                .filter(|(k, _)| matches!(*k, "mdown" | "mmove" | "mup"))
            {
                scripted_pointer(&sdl, &window, kind, position)?;
            } else if action == "dump" {
                eprintln!(
                    "DUMP scroll={:.1} goal={:.1} home={:.1}/{:.1} focus={:?} pointer={}",
                    state.scroll,
                    state.scroll_goal,
                    state.home_scroll,
                    state.home_scroll_goal,
                    state.focus,
                    state.pointer
                );
            } else {
                let pending = script(&action, &mut state, &scene.controls)?;
                running &= effects(
                    pending,
                    &mut state,
                    &platform,
                    &preferences_path,
                    &weather_service,
                    &home_service,
                );
            }
        }
        weather_tick(&mut state, &weather_service, visible, &preferences_path);
        state.dirty |= widgets::tick(
            &mut state.widget_data,
            &state.preferences.widgets,
            state.preferences.weather_location.as_ref(),
            visible && state.page == state::Page::Home,
            state.widget_detail.is_some(),
        );
        for (id, images) in media::thumbs_poll() {
            state.movie_thumbs(id, images);
        }
        jellyfin_tick(&mut state, &preferences_path, &platform);
        video_upload_tick(&mut state, &preferences_path);
        immich_tick(&mut state, &preferences_path, visible);
        plex_tick(&mut state, &preferences_path, &platform);
        moonlight_tick(&mut state, &preferences_path, &platform);
        library_tick(&mut state, &preferences_path);
        if state.music.tick() {
            state.music_changed();
        }
        // The pill's title scrolls while it has focus.
        if state.music.now.is_some() && state::mini::ROW.contains(&state.focus) {
            state.dirty = true;
        }
        // SDL-webOS disables text input when its own panel is dismissed, even
        // if it did not forward a Back/Return key to the application.
        if text_active
            && matches!(state.overlay, Some(Overlay::SystemKeyboard))
            && !video.text_input().is_active()
        {
            press = None;
            let pending = state.finish_text_input();
            running &= effects(
                pending,
                &mut state,
                &platform,
                &preferences_path,
                &weather_service,
                &home_service,
            );
        }
        let keyboard = state.text_input_active();
        if keyboard != text_active {
            if keyboard {
                video.text_input().start();
            } else {
                video.text_input().stop();
            }
            text_active = keyboard;
        }
        // NOPE sky animates at the display rate, only on a visible Home; on a
        // TV that cannot keep that up it drops to 30 fps (sky_every), as
        // video decoding and remote navigation come before decorative weather.
        let sky_running = visible
            && state.page == state::Page::Home
            && state.preferences.show_weather
            && !state.preferences.reduced_motion;
        // Settings illustrations redraw at display rate (VSync paces them).
        // They never count as transient animation, so they cannot trigger
        // the Auto resolution downgrade.
        let art_running = visible
            && state.page == state::Page::Settings
            && state.settings_art_live
            && !state.preferences.reduced_motion;
        if art_running {
            state.dirty = true;
        }
        if sky_running && last_sky_frame.elapsed() >= sky_every {
            last_sky_frame = Instant::now();
            state.dirty = true;
        }
        // Apply explicit Settings › Appearance › UI resolution immediately.
        // Resizing the same borderless SDL window avoids tearing down the GL context.
        // Desktop diagnostic --resolution owns its surface, so it cannot be overridden.
        if webos && options.resolution.is_none() {
            let target = state.preferences.resolved_ui_size();
            if target != applied_surface {
                // Do not request a second resize while the previous SDL event is queued.
                auto_pacing.clear();
                prior_present = None;
                match window.set_size(target.width, target.height) {
                    Ok(()) => {
                        eprintln!(
                            "Luma resolution change {}x{} -> {}x{} (same EGL context)",
                            applied_surface.width,
                            applied_surface.height,
                            target.width,
                            target.height
                        );
                        applied_surface = target;
                        last_working_choice = state.preferences.ui_resolution;
                        last_working_auto = state.preferences.auto_ui_cap;
                        resolution_change_count += 1;
                        state.dirty = true;
                    }
                    Err(error) => {
                        eprintln!(
                            "Luma live resolution change rejected: {error}; restoring previous preference"
                        );
                        state.preferences.ui_resolution = last_working_choice;
                        state.preferences.auto_ui_cap = last_working_auto;
                        let _ = storage::save(&preferences_path, &state.preferences);
                        state.dirty = true;
                    }
                }
            }
        }
        let dt = last_frame.elapsed().as_secs_f32().min(0.1);
        last_frame = Instant::now();
        assets.active_heading.set(state.preferences.heading_font);
        assets.active_text.set(state.preferences.text_font);
        assets.heading_pct.set(state.preferences.heading_size_pct);
        assets.text_pct.set(state.preferences.text_size_pct);
        let navigation = view::navigation_in(
            state.preferences.layout,
            &assets,
            state.preferences.language,
            state.show_smart_tab(),
        );
        // Along the bar: across for the top bars and the dock, down the rail.
        let (target, target_width) = view::nav_pill_target(
            state.preferences.layout,
            navigation.items[state.page_index()],
        );
        if state.preferences.reduced_motion
            || ((pill - target).abs() < 0.2 && (pill_width - target_width).abs() < 0.2)
        {
            pill = target;
            pill_width = target_width;
        } else {
            pill += (target - pill) * (1. - (-20. * dt).exp());
            pill_width += (target_width - pill_width) * (1. - (-20. * dt).exp());
            state.dirty = true;
        }
        if renderer.take_fresh() {
            state.dirty = true;
        }
        // While a tab sits still, build the other tabs' text runs and icons one
        // per idle frame: otherwise the first switch to a tab tessellates and
        // decodes all of it inside the transition and the animation stutters.
        if visible
            && !state.animating()
            && state.overlay.is_none()
            && state.page_started.elapsed() > Duration::from_millis(1500)
        {
            let key = format!(
                "{:?} {:?} {} {} {:?}",
                state.preferences.heading_font,
                state.preferences.text_font,
                state.preferences.heading_size_pct,
                state.preferences.text_size_pct,
                window.drawable_size(),
            );
            if key != warm_key {
                warmed.clear();
                warm_key = key;
            }
            let next = [
                state::Page::Home,
                state::Page::Apps,
                state::Page::SmartHome,
                state::Page::Settings,
            ]
            .into_iter()
            .find(|page| {
                *page != state.page
                    && !warmed.contains(page)
                    && (*page != state::Page::SmartHome || state.show_smart_tab())
            });
            if let Some(page) = next {
                warmed.push(page);
                let shown = (state.page, state.scroll);
                state.page = page;
                state.scroll = 0.;
                let ahead = view::scene_with_pill(&state, &assets, pill, pill_width, now);
                (state.page, state.scroll) = shown;
                renderer.prepare(&ahead.draw);
                if prof_slow.is_some() {
                    eprintln!("LUMA_WARM {page:?}");
                }
            }
        }
        let final_frame = options.frames.is_some_and(|limit| frame + 1 >= limit)
            || (visible
                && options
                    .profile_frames
                    .is_some_and(|limit| presented + 1 >= limit));
        if ((state.dirty || options.profile_frames.is_some()) && visible) || final_frame {
            renderer.set_fonts(
                state.preferences.heading_font,
                state.preferences.text_font,
                state.preferences.heading_size_pct,
                state.preferences.text_size_pct,
            );
            assets.active_heading.set(state.preferences.heading_font);
            assets.active_text.set(state.preferences.text_font);
            assets.heading_pct.set(state.preferences.heading_size_pct);
            assets.text_pct.set(state.preferences.text_size_pct);
            scene = compose(&mut state, &assets, pill, pill_width, now);
            let (width, height) = window.drawable_size();
            if width > 0 && height > 0 {
                if final_frame && let Some(path) = &options.screenshot {
                    captured = Some(renderer.capture(
                        &scene.draw,
                        path,
                        options.capture_size.unwrap_or(Size::HD),
                    )?);
                }
                renderer.render(
                    &scene.draw,
                    width,
                    height,
                    webos
                        && (state.playback.is_some()
                            || (state.preferences.background == "video"
                                && state.video_frame_ready)),
                );
                window.gl_swap_window();
                if renderer.decoding() {
                    state.dirty = true;
                }
                // Auto profile is a moving three-second window of actual display
                // presentation intervals, not CPU-only submission time or idle waits.
                // Only animated frames count. Never increase resolution automatically:
                // repeated oscillation creates a worse experience than a lower tier.
                let present_at = Instant::now();
                let animated = state.animating()
                    || (pill - target).abs() > 0.2
                    || (pill_width - target_width).abs() > 0.2;
                if webos
                    && options.resolution.is_none()
                    && state.preferences.ui_resolution == model::UiResolution::Auto
                    && state.preferences.resolved_ui_resolution() != model::UiResolution::Hd
                    && options.profile_frames.is_none()
                {
                    // Animations and the weather sky count; the first
                    // seconds after start (fonts loading) do not.
                    if HEAVY_EFFECT.swap(false, std::sync::atomic::Ordering::Relaxed) {
                        auto_pacing.clear();
                        heavy_until = present_at + AUTO_HEAVY_PAUSE;
                    }
                    let measured = (animated || (sky_running && sky_every.is_zero()))
                        && startup.elapsed() > AUTO_SETTLE
                        && present_at >= heavy_until;
                    if let Some((p95, mean, slow)) = auto_pacing.frame(present_at, measured) {
                        observed_resolutions.push((applied_surface, p95));
                        debug_log!(
                            "Auto UI resolution: {} p95 {p95:.1} ms, mean {mean:.1} ms over {} frames",
                            state.preferences.resolved_ui_resolution().short(),
                            diagnostics::AutoPacing::SAMPLES
                        );
                        if slow {
                            let new_cap = state.preferences.resolved_ui_resolution().lower();
                            eprintln!(
                                "LUMA_AUTO_RESOLUTION_DOWNGRADE {} -> {} p95_ms={p95:.2} mean_ms={mean:.2} samples={}",
                                state.preferences.resolved_ui_resolution().short(),
                                new_cap.short(),
                                diagnostics::AutoPacing::SAMPLES
                            );
                            state.preferences.auto_ui_cap = new_cap;
                            auto_downshifts += 1;
                            state.dirty = true;
                            let _ = storage::save(&preferences_path, &state.preferences);
                            auto_pacing.clear();
                        }
                    }
                }
                // Debug log: how fast each animation actually ran.
                if animated {
                    let gap = prior_present
                        .map_or(0., |p| present_at.duration_since(p).as_secs_f64() * 1000.);
                    burst = Some(match burst {
                        Some((start, frames, slowest)) => (start, frames + 1, slowest.max(gap)),
                        None => (present_at, 1, 0.),
                    });
                } else if let Some((start, frames, slowest)) = burst.take()
                    && frames >= 8
                {
                    let seconds = present_at.duration_since(start).as_secs_f64();
                    debug_log!(
                        "animation: {frames} frames in {:.0} ms = {:.0} fps, slowest frame {slowest:.1} ms",
                        seconds * 1000.,
                        f64::from(frames) / seconds.max(0.001)
                    );
                }
                // The sky alone on screen: if this TV draws it under 45 fps,
                // it goes back to 30 fps and leaves the GPU to the rest.
                if sky_running && !animated && sky_every.is_zero() {
                    let (start, frames) = sky_window.unwrap_or((present_at, 0));
                    let elapsed = present_at.duration_since(start).as_secs_f64();
                    if elapsed >= 2. {
                        let fps = f64::from(frames) / elapsed;
                        if fps < 45. {
                            sky_every = Duration::from_millis(33);
                            eprintln!("Luma weather sky: {fps:.0} fps on this TV, drawn at 30 fps");
                        } else if !sky_logged {
                            debug_log!("weather sky: {fps:.0} fps");
                        }
                        sky_logged = true;
                        sky_window = None;
                    } else {
                        sky_window = Some((start, frames + 1));
                    }
                } else {
                    sky_window = None;
                }
                prior_present = Some(present_at);
                if presented == 0 {
                    startup_stage("first frame");
                    if webos {
                        let stats = renderer.stats();
                        eprintln!(
                            "Luma vector text: {} glyphs, {} text runs, {} KiB glyph geometry, {} KiB GPU text",
                            stats.cached_glyphs,
                            stats.cached_text_runs,
                            stats.glyph_geometry_bytes / 1024,
                            stats.text_geometry_bytes / 1024,
                        );
                    }
                }
                presented += 1;
                if let Some(limit) = prof_slow {
                    let now = Instant::now();
                    if let Some(previous) = slow_at {
                        let delta = now.duration_since(previous).as_secs_f64() * 1000.;
                        if delta > limit {
                            let stats = renderer.stats();
                            eprintln!(
                                "LUMA_SLOW frame={presented} dt={delta:.1} cpu={:.1} build={:.1} tess={} page={:?} focus={:?}",
                                stats.cpu_submission_ms,
                                stats.build_ms,
                                stats.glyph_tessellations,
                                state.page,
                                state.focus,
                            );
                        }
                    }
                    slow_at = Some(now);
                }
                if options.profile_frames.is_some() {
                    profile.record(
                        Instant::now(),
                        renderer.stats().cpu_submission_ms,
                        final_frame && options.screenshot.is_some(),
                    );
                }
                let drawable = Size::new(width, height);
                if last_drawable != Some(drawable) {
                    if options.diagnostics.is_some() || options.resolution.is_some() {
                        eprintln!(
                            "Luma drawable: {}x{}, density {:.3}x, 4K buffer: {}",
                            width,
                            height,
                            drawable.scale(),
                            drawable.is_4k()
                        );
                    }
                    last_drawable = Some(drawable);
                }
            }
            state.dirty = false;
        }
        frame += 1;
        if final_frame {
            running = false;
        }
        if foreground.failed(Instant::now())
            && let Some(end_registration) = &end_registration
        {
            // Fall back to the restart sam does for an unregistered Home: remember that and
            // end the registration, which quits Luma so sam starts it afresh.
            eprintln!(
                "Luma lifecycle: a relaunch did not bring Luma to the front; creating {}",
                platform::HOME_LIFECYCLE_OFF
            );
            if let Err(error) =
                storage::as_root(|| std::fs::write(platform::HOME_LIFECYCLE_OFF, b""))
            {
                eprintln!("Luma lifecycle: {error}");
            }
            end_registration();
        }
        // Block until input instead of polling at frame rate on idle screens.
        // VSync paces animation; holds, scripted checks and icons still being
        // decoded need short timed wakes.
        if visible && stacks_at.is_none_or(|at| at.elapsed() >= STACKS_EVERY) {
            stacks_at = Some(Instant::now());
            stacks_refresh(&mut state);
            stacks_retry = Some((Instant::now() + STACKS_RETRY, STACKS_RETRY));
        }
        if let Some((due, gap)) = stacks_retry
            && Instant::now() >= due
        {
            stacks_retry = None;
            if visible && jellyfin_rows_empty(&state) {
                debug_log!("Home rows: Jellyfin rows still empty, asking again");
                stacks_refresh(&mut state);
                let gap = (gap * 2).min(STACKS_RETRY_MAX);
                stacks_retry = Some((Instant::now() + gap, gap));
            }
        }
        let timeout = if !visible {
            250
        } else if pill != target
            || pill_width != target_width
            || state.animating()
            || edge_moving
            || options.profile_frames.is_some()
            // Settings illustrations draw at the display rate.
            || art_running
        {
            0
        } else if sky_running {
            sky_every.as_millis() as u32
        } else if press.is_some()
            || options.frames.is_some()
            || assets.artwork.pending()
            || renderer.decoding()
            || video_swap_pending
        {
            8
        } else {
            100
        };
        queued_event = events.wait_event_timeout(timeout);
    }
    if text_active {
        video.text_input().stop();
    }
    if !options.script.is_empty() {
        return Err("Frame budget ended before the input script completed".into());
    }
    if let Some(path) = options.dump {
        let state = serde_json::json!({ "page": state.page, "focus": state.focus, "query": state.query,
            "filter": state.filter, "preferences": state.preferences, "apps": state.apps().iter().map(|a| &a.id).collect::<Vec<_>>(),
            "error": state.error, "scroll": state.scroll, "home_scroll": state.home_scroll, "subpage": state.subpage, "modal": state.overlay.is_some(), "weather": state.weather,
            "inventory": state.inventory.apps,
            "ui_text": scene.draw.iter().filter_map(|draw| match draw { renderer::Draw::Text(value, ..) => Some(value), _ => None }).collect::<Vec<_>>() });
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(path) = options.diagnostics {
        let (width, height) = window.size();
        let (dw, dh) = window.drawable_size();
        let display_mode = window.display_index().ok().and_then(|index| video.current_display_mode(index).ok())
            .map(|mode| serde_json::json!({ "width": mode.w, "height": mode.h, "refresh_hz": mode.refresh_rate }));
        let report = serde_json::json!({
            "logical_layout": Size::HD, "requested_window": requested,
            "window": Size::new(width, height), "drawable": Size::new(dw, dh),
            "display_mode": display_mode,
            "drawable_is_4k": Size::new(dw, dh).is_4k(), "presented_frames": presented,
            "graphics": renderer.info, "last_presented": renderer.stats(), "capture": captured,
            "profile": options.profile_frames.map(|_| profile.summary()),
            "ui_resolution": state.preferences.ui_resolution,
            "ui_auto_cap":state.preferences.auto_ui_cap,
            "ui_applied_surface":applied_surface,
            "ui_resolution_changes":resolution_change_count,
            "ui_auto_downshifts":auto_downshifts,
            "ui_animation_p95_windows":observed_resolutions,
        });
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn pointer_position(window: &sdl2::video::Window, x: i32, y: i32) -> Option<(f32, f32)> {
    let (width, height) = window.size();
    let scale = (width as f32 / 1920.).min(height as f32 / 1080.);
    if scale <= 0. {
        return None;
    }
    Some((
        (x as f32 - (width as f32 - 1920. * scale) / 2.) / scale,
        (y as f32 - (height as f32 - 1080. * scale) / 2.) / scale,
    ))
}

/// Cursor resting near the left/right screen edge over a Home row: which row,
/// and a signed push (−1…1) that grows towards the edge.
fn edge_lane(
    state: &State,
    controls: &[state::Control],
    (px, py): (f32, f32),
) -> Option<(Lane, f32)> {
    const ZONE: f32 = 140.;
    if state.page != state::Page::Home
        || !state.pointer
        || state.overlay.is_some()
        || state.moving.is_some()
    {
        return None;
    }
    let push = if px > 1920. - ZONE {
        (px - (1920. - ZONE)) / ZONE
    } else if px < ZONE {
        -(ZONE - px) / ZONE
    } else {
        return None;
    };
    controls.iter().find_map(|c| {
        let inside = py >= c.rect.y && py <= c.rect.y + c.rect.h;
        match &c.target {
            state::Target::App(_) if inside => Some((Lane::Favorites, push)),
            state::Target::StackCard(row, _) if inside => Some((Lane::Stack(*row), push)),
            _ => None,
        }
    })
}

fn hit(
    controls: &[state::Control],
    window: &sdl2::video::Window,
    x: i32,
    y: i32,
    state: &State,
) -> Option<Target> {
    let (x, y) = pointer_position(window, x, y)?;
    controls
        .iter()
        .rev()
        .find(|control| {
            control.rect.contains(x, y)
                && (!control.scrollable
                    || ((state::HOME_VIEWPORT_LEFT..state::HOME_VIEWPORT_RIGHT).contains(&x)
                        && y >= 108.8 + state.preferences.layout.body_offset().1
                        && y < state.preferences.layout.pointer_bottom(state.main_bottom())))
        })
        .map(|control| control.target.clone())
}
