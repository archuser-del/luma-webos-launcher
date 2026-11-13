//! Jellyfin for Home stacks. Luma signs in with Quick Connect: the TV shows a
//! code, the user approves it on the phone. Nothing is typed with the remote
//! and no password is ever seen. The access token is never logged.
use crate::media::{Detail, Episode, PlayRequest};
use crate::stacks::StackItem;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// Saved sign-in. The server address is what the user picked or entered.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct JellyfinLink {
    pub server: String,
    pub server_name: String,
    pub user_id: String,
    pub user_name: String,
    pub token: String,
    /// Random per-install id; Jellyfin lists Luma as its own device.
    pub device_id: String,
}

impl JellyfinLink {
    pub fn linked(&self) -> bool {
        !self.token.is_empty() && !self.server.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Server {
    pub name: String,
    pub address: String,
    pub version: String,
}

/// Row contents: (id, label). Libraries follow as "view:<id>", music
/// libraries as "music:<id>" (square album cards).
/// The row of library cards: every library on the server as a wide banner.
pub const LIBRARIES: &str = "libraries";

pub const COLLECTIONS: [(&str, &str); 5] = [
    (LIBRARIES, "Libraries"),
    ("resume", "Continue watching"),
    ("nextup", "Next up"),
    ("latest", "Recently added"),
    ("albums", "Recently added albums"),
];

/// Rows of albums: shown with square cards.
pub fn music_collection(collection: &str) -> bool {
    collection == "albums" || collection.starts_with("music:")
}

pub enum Command {
    /// Look for servers on the local network; a saved address is checked too.
    Discover(Option<String>),
    /// Start Quick Connect on this server and wait until the phone approves.
    Connect {
        address: String,
        device_id: String,
    },
    /// Stop waiting for a Quick Connect approval.
    Cancel,
    /// Cards for (stack index, collection id).
    Fetch(JellyfinLink, Vec<(usize, String)>),
    /// The user's libraries, offered as row contents.
    Views(JellyfinLink),
    Details(JellyfinLink, i64),
    Play(JellyfinLink, PlayRequest),
    /// Songs and cover of an album card.
    Album(JellyfinLink, i64),
    /// Play an album card from this song, in order or shuffled.
    PlayAlbum(JellyfinLink, i64, usize, bool),
}

/// One server library on the Libraries row.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryInfo {
    /// Id of its Home card.
    pub card: i64,
    /// "view:<id>" or "music:<id>", what the Library page loads.
    pub collection: String,
    pub name: String,
    /// "412 movies", or empty when the server did not say.
    pub detail: String,
}

pub enum Response {
    Servers(Vec<Server>),
    /// The server's libraries, for the Libraries row and its Library page.
    Libraries(Vec<LibraryInfo>),
    Code {
        code: String,
        address: String,
    },
    Linked(JellyfinLink),
    Failed(String),
    Items(usize, Vec<StackItem>),
    Views(Vec<(String, String)>),
    Details(Box<Detail>),
    Play(Box<crate::player::OpenRequest>),
    Album(Box<crate::state::album::Album>),
}

pub struct Service {
    sender: Mutex<mpsc::Sender<Command>>,
    pub receiver: Mutex<mpsc::Receiver<Response>>,
}

/// Bumped by every Connect and Cancel; a Quick Connect wait stops when it moves.
static ATTEMPT: AtomicU64 = AtomicU64::new(0);

impl Service {
    pub fn new(cache: PathBuf) -> Self {
        let (sender, commands) = mpsc::channel::<Command>();
        let (reply, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = std::fs::create_dir_all(&cache);
            for command in commands {
                match command {
                    Command::Discover(saved) => discover(&reply, saved),
                    Command::Connect { address, device_id } => {
                        let attempt = ATTEMPT.fetch_add(1, Ordering::SeqCst) + 1;
                        let reply = reply.clone();
                        thread::spawn(move || connect(&reply, address, device_id, attempt));
                    }
                    Command::Cancel => {
                        ATTEMPT.fetch_add(1, Ordering::SeqCst);
                    }
                    Command::Fetch(link, stacks) => fetch(&reply, &cache, &link, stacks),
                    Command::Views(link) => {
                        let _ = reply.send(Response::Views(views(&agent(), &link)));
                    }
                    Command::Details(link, id) => details(&reply, &cache, &link, id),
                    Command::Play(link, request) => play(&reply, &link, request),
                    Command::Album(link, id) => album(&reply, &cache, &link, id),
                    Command::PlayAlbum(link, id, start, shuffle) => {
                        play_album(&reply, &link, id, start, shuffle)
                    }
                }
            }
        });
        Self {
            sender: Mutex::new(sender),
            receiver: Mutex::new(receiver),
        }
    }
    pub fn send(&self, command: Command) {
        let _ = self.sender.lock().unwrap().send(command);
    }
}

/// Jellyfin ids are GUID strings; the title card and the player key items by
/// i64. Card ids carry this bit, far above any media source id, and map back here.
const CARD_TAG: i64 = 1 << 52;
static CARD_IDS: Mutex<BTreeMap<i64, String>> = Mutex::new(BTreeMap::new());
/// Cards that are albums: OK opens the album card, not the title card.
static ALBUM_IDS: Mutex<std::collections::BTreeSet<i64>> =
    Mutex::new(std::collections::BTreeSet::new());

pub fn is_album(id: i64) -> bool {
    ALBUM_IDS.lock().unwrap().contains(&id)
}

pub fn card_id(item: &str) -> i64 {
    // FNV-1a: stable across runs, so cached posters keep their file names.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in item.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let id = (hash as i64 & (CARD_TAG - 1)) | CARD_TAG;
    CARD_IDS.lock().unwrap().insert(id, item.to_owned());
    id
}

/// Whether a title card or Play request belongs to Jellyfin.
pub fn owns(id: i64) -> bool {
    id & CARD_TAG != 0 && CARD_IDS.lock().unwrap().contains_key(&id)
}

fn item_of(id: i64) -> Option<String> {
    CARD_IDS.lock().unwrap().get(&id).cloned()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn auth(device_id: &str, token: &str) -> String {
    let mut header = format!(
        "MediaBrowser Client=\"Luma\", Device=\"LG TV\", DeviceId=\"{device_id}\", Version=\"{}\"",
        env!("CARGO_PKG_VERSION")
    );
    if !token.is_empty() {
        header.push_str(&format!(", Token=\"{token}\""));
    }
    header
}

fn read(mut response: ureq::http::Response<ureq::Body>) -> Option<Value> {
    if !(200..300).contains(&response.status().as_u16()) {
        return None;
    }
    let text = response
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_string()
        .ok()?;
    serde_json::from_str(&text).ok()
}

fn get(agent: &ureq::Agent, link: &JellyfinLink, path: &str) -> Option<Value> {
    let response = agent
        .get(&format!("{}{path}", link.server))
        .header("Authorization", &auth(&link.device_id, &link.token))
        .call();
    let value = match response {
        Ok(response) => read(response),
        Err(error) => {
            crate::debug_log!(
                "Jellyfin GET {}{} failed: {error}",
                link.server,
                crate::debug_log::path_only(path)
            );
            return None;
        }
    };
    if value.is_none() {
        crate::debug_log!(
            "Jellyfin GET {}{}: unreadable answer",
            link.server,
            crate::debug_log::path_only(path)
        );
    }
    value
}

/// "192.168.1.5" → "http://192.168.1.5:8096"; keeps a scheme, port and path given.
pub fn normalize(address: &str) -> String {
    let address = address.trim().trim_end_matches('/');
    if address.is_empty() {
        return String::new();
    }
    if address.contains("://") {
        return address.to_owned();
    }
    let host = address.split('/').next().unwrap_or(address);
    if host.contains(':') {
        format!("http://{address}")
    } else {
        format!("http://{host}:8096{}", &address[host.len()..])
    }
}

fn public_info(agent: &ureq::Agent, address: &str) -> Option<Server> {
    let info = read(
        agent
            .get(&format!("{address}/System/Info/Public"))
            .call()
            .ok()?,
    )?;
    info.get("Id").and_then(Value::as_str)?;
    Some(Server {
        name: text(&info, "ServerName"),
        address: address.to_owned(),
        version: text(&info, "Version"),
    })
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// Jellyfin answers "who is JellyfinServer?" on UDP 7359 with its address.
/// A server in a container (Umbrel, Docker) names its internal address, so
/// the host that answered is tried with the same port. When nothing answers,
/// the local /24 is checked for port 8096.
fn discover(reply: &mpsc::Sender<Response>, saved: Option<String>) {
    let agent = agent();
    let mut found: Vec<Server> = vec![];
    if let Some(server) = saved.as_deref().and_then(|a| public_info(&agent, a)) {
        found.push(server);
        let _ = reply.send(Response::Servers(found.clone()));
    }
    let local = local_ipv4();
    let mut answers: Vec<(String, std::net::IpAddr)> = vec![];
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        let _ = socket.set_broadcast(true);
        let _ = socket.set_read_timeout(Some(Duration::from_millis(400)));
        let mut targets = vec!["255.255.255.255:7359".to_owned()];
        if let Some(ip) = local {
            let o = ip.octets();
            targets.push(format!("{}.{}.{}.255:7359", o[0], o[1], o[2]));
        }
        let deadline = Instant::now() + Duration::from_millis(2500);
        let mut sent = 0;
        let mut buffer = [0u8; 2048];
        while Instant::now() < deadline {
            if sent < 3 {
                for target in &targets {
                    let _ = socket.send_to(b"who is JellyfinServer?", target.as_str());
                }
                sent += 1;
            }
            if let Ok((n, from)) = socket.recv_from(&mut buffer)
                && let Ok(answer) = serde_json::from_slice::<Value>(&buffer[..n])
            {
                let address = text(&answer, "Address");
                if !address.is_empty() && !answers.iter().any(|(a, _)| *a == address) {
                    answers.push((address, from.ip()));
                }
            }
        }
    }
    for (address, from) in answers {
        let address = normalize(&address);
        for candidate in [address.clone(), with_host(&address, &from.to_string())] {
            if found.iter().any(|s| s.address == candidate) {
                break;
            }
            if reachable(&candidate)
                && let Some(server) = public_info(&agent, &candidate)
            {
                found.push(server);
                break;
            }
        }
    }
    if found.is_empty()
        && let Some(ip) = local
    {
        for address in scan(ip) {
            if let Some(server) = public_info(&agent, &address) {
                found.push(server);
            }
        }
    }
    let _ = reply.send(Response::Servers(found));
}

/// The TV's own LAN address (no packet is sent by a UDP connect).
fn local_ipv4() -> Option<std::net::Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// Same scheme and port, another host: "http://10.21.0.15:8096" → "http://192.168.1.68:8096".
fn with_host(address: &str, host: &str) -> String {
    let (scheme, rest) = address.split_once("://").unwrap_or(("http", address));
    let authority = rest.split('/').next().unwrap_or(rest);
    let port = authority.rsplit_once(':').map(|(_, p)| p).unwrap_or("8096");
    format!("{scheme}://{host}:{port}")
}

fn socket_addr(address: &str) -> Option<std::net::SocketAddr> {
    use std::net::ToSocketAddrs;
    let (scheme, rest) = address.split_once("://")?;
    let authority = rest.split('/').next()?;
    let target = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:{}", if scheme == "https" { 443 } else { 80 })
    };
    target.to_socket_addrs().ok()?.next()
}

/// Quick TCP check so an unreachable container address costs 600 ms, not a timeout.
fn reachable(address: &str) -> bool {
    socket_addr(address).is_some_and(|a| {
        std::net::TcpStream::connect_timeout(&a, Duration::from_millis(600)).is_ok()
    })
}

/// Hosts on the local /24 with port 8096 open.
fn scan(local: std::net::Ipv4Addr) -> Vec<String> {
    let o = local.octets();
    let (tx, rx) = mpsc::channel();
    for chunk in (1u8..=254).collect::<Vec<_>>().chunks(64) {
        let handles: Vec<_> = chunk
            .iter()
            .filter(|&&last| last != o[3])
            .map(|&last| {
                let tx = tx.clone();
                let addr = std::net::SocketAddr::from(([o[0], o[1], o[2], last], 8096));
                thread::spawn(move || {
                    if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(400))
                        .is_ok()
                    {
                        let _ = tx.send(format!("http://{}:8096", addr.ip()));
                    }
                })
            })
            .collect();
        for handle in handles {
            let _ = handle.join();
        }
    }
    drop(tx);
    rx.into_iter().collect()
}

/// Phone form check: the address answers as a Jellyfin server.
pub fn check(address: &str) -> Result<Server, String> {
    let address = normalize(address);
    if address.is_empty() {
        return Err("Type the server address".into());
    }
    public_info(&agent(), &address)
        .ok_or_else(|| format!("No Jellyfin server answered at {address}"))
}

/// Quick Connect: show the code, poll every 3 s for 10 minutes, then trade
/// the approved secret for an access token.
fn connect(reply: &mpsc::Sender<Response>, address: String, device_id: String, attempt: u64) {
    let agent = agent();
    let header = auth(&device_id, "");
    let initiate = agent
        .post(&format!("{address}/QuickConnect/Initiate"))
        .header("Authorization", &header)
        .send_empty()
        .ok()
        .and_then(read);
    let Some(initiate) = initiate else {
        let _ = reply.send(Response::Failed(
            "Quick Connect is off. Turn it on in Jellyfin: Dashboard › General".into(),
        ));
        return;
    };
    let secret = text(&initiate, "Secret");
    let code = text(&initiate, "Code");
    if secret.is_empty() || code.is_empty() {
        let _ = reply.send(Response::Failed("The server gave no code".into()));
        return;
    }
    let _ = reply.send(Response::Code {
        code,
        address: address.clone(),
    });
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        thread::sleep(Duration::from_secs(3));
        if ATTEMPT.load(Ordering::SeqCst) != attempt {
            return;
        }
        if Instant::now() > deadline {
            let _ = reply.send(Response::Failed("The code expired. Get a new one".into()));
            return;
        }
        let state = agent
            .get(&format!("{address}/QuickConnect/Connect?secret={secret}"))
            .header("Authorization", &header)
            .call()
            .ok()
            .and_then(read);
        if state
            .as_ref()
            .and_then(|s| s.get("Authenticated"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            break;
        }
    }
    let session = agent
        .post(&format!("{address}/Users/AuthenticateWithQuickConnect"))
        .header("Authorization", &header)
        .header("Content-Type", "application/json")
        .send(json!({ "Secret": secret }).to_string())
        .ok()
        .and_then(read);
    let Some(session) = session else {
        let _ = reply.send(Response::Failed(
            "Sign-in was approved but failed. Try again".into(),
        ));
        return;
    };
    let user = session.get("User").cloned().unwrap_or_default();
    let server_name = public_info(&agent, &address)
        .map(|s| s.name)
        .unwrap_or_default();
    if ATTEMPT.load(Ordering::SeqCst) != attempt {
        return;
    }
    let _ = reply.send(Response::Linked(JellyfinLink {
        server: address,
        server_name,
        user_id: text(&user, "Id"),
        user_name: text(&user, "Name"),
        token: text(&session, "AccessToken"),
        device_id,
    }));
}

const FIELDS: &str = "Fields=PrimaryImageAspectRatio&EnableImageTypes=Primary,Backdrop,Thumb";

fn list(value: Option<Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items,
        Some(v) => v
            .get("Items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        None => vec![],
    }
}

/// Up to `limit` items of one row. Episodes stand for their series.
fn collection_items(
    agent: &ureq::Agent,
    link: &JellyfinLink,
    collection: &str,
    limit: usize,
) -> Vec<Value> {
    let user = &link.user_id;
    let path = match collection {
        "resume" => {
            format!("/UserItems/Resume?userId={user}&MediaTypes=Video&Limit={limit}&{FIELDS}")
        }
        "nextup" => format!("/Shows/NextUp?userId={user}&Limit={limit}&{FIELDS}"),
        "latest" => format!(
            "/Items/Latest?userId={user}&IncludeItemTypes=Movie,Series&Limit={limit}&{FIELDS}"
        ),
        "albums" => format!(
            "/Items/Latest?userId={user}&IncludeItemTypes=MusicAlbum&Limit={limit}&{FIELDS}"
        ),
        view if view.starts_with("music:") => format!(
            "/Items?userId={user}&ParentId={}&Recursive=true&IncludeItemTypes=MusicAlbum\
             &SortBy=DateCreated&SortOrder=Descending&Limit={limit}&{FIELDS}",
            &view[6..]
        ),
        view => {
            let Some(parent) = view.strip_prefix("view:") else {
                return vec![];
            };
            format!(
                "/Items?userId={user}&ParentId={parent}&Recursive=true&IncludeItemTypes=Movie,Series\
                 &SortBy=DateCreated&SortOrder=Descending&Limit={limit}&{FIELDS}"
            )
        }
    };
    list(get(agent, link, &path))
}

/// (Jellyfin id, title) the card shows: an episode opens its series.
fn card_target(item: &Value) -> (String, String) {
    if text(item, "Type") == "MusicAlbum" {
        let id = text(item, "Id");
        ALBUM_IDS.lock().unwrap().insert(card_id(&id));
        return (id, text(item, "Name"));
    }
    if text(item, "Type") == "Episode" {
        let series = text(item, "SeriesId");
        if !series.is_empty() {
            return (series, text(item, "SeriesName"));
        }
    }
    (text(item, "Id"), text(item, "Name"))
}

/// Image endpoints need no token.
fn image_url(link: &JellyfinLink, item: &str, kind: &str, size: &str) -> String {
    format!(
        "{}/Items/{item}/Images/{kind}?{size}&quality=90",
        link.server
    )
}

/// Poster with fallbacks: some servers list a Primary tag whose file is
/// gone (404), then the Thumb or the first Backdrop is cropped instead.
fn poster(
    agent: &ureq::Agent,
    cache: &Path,
    link: &JellyfinLink,
    item: &str,
    name: &str,
) -> Option<String> {
    // The renderer crops any shape to the card, so a wide still is never
    // stretched.
    [
        ("Primary", "fillHeight=405"),
        ("Thumb", "fillHeight=405"),
        ("Backdrop/0", "fillHeight=405"),
    ]
    .iter()
    .find_map(|(kind, size)| {
        crate::media::download_as(agent, cache, name, &image_url(link, item, kind, size))
    })
}

fn fetch(
    reply: &mpsc::Sender<Response>,
    cache: &Path,
    link: &JellyfinLink,
    stacks: Vec<(usize, String)>,
) {
    let agent = agent();
    for (index, collection) in stacks {
        if collection == LIBRARIES {
            fetch_libraries(reply, cache, link, &agent, index);
            continue;
        }
        let mut seen = vec![];
        let mut targets = vec![];
        // Episodes fold into their series: ask for more, keep 20 cards.
        let max = crate::stacks::HOME_ROW_MAX;
        for item in collection_items(&agent, link, &collection, max * 2) {
            let (item, title) = card_target(&item);
            if item.is_empty() || seen.contains(&item) || targets.len() >= max {
                continue;
            }
            seen.push(item.clone());
            targets.push((item, title));
        }
        // Cached posters show at once; the rest arrive in one more update.
        let mut cards: Vec<StackItem> = targets
            .iter()
            .map(|(item, title)| {
                let id = card_id(item);
                let cached = cache.join(format!("jf-{id}.jpg"));
                StackItem {
                    title: title.clone(),
                    poster: cached
                        .exists()
                        .then(|| cached.to_string_lossy().into_owned()),
                    id,
                }
            })
            .collect();
        let _ = reply.send(Response::Items(index, cards.clone()));
        if cards.iter().all(|c| c.poster.is_some()) {
            continue;
        }
        for ((item, _), card) in targets.iter().zip(cards.iter_mut()) {
            if card.poster.is_none() {
                let name = format!("jf-{}", card.id);
                card.poster = if is_album(card.id) {
                    album_cover(&agent, link, item, "fillHeight=405")
                        .and_then(|url| crate::media::download_as(&agent, cache, &name, &url))
                } else {
                    poster(&agent, cache, link, item, &name)
                };
            }
        }
        let _ = reply.send(Response::Items(index, cards));
    }
}

/// Libraries the hub can open: (collection prefix, id, name, Jellyfin's type).
fn view_list(
    agent: &ureq::Agent,
    link: &JellyfinLink,
) -> Vec<(&'static str, String, String, String)> {
    list(get(
        agent,
        link,
        &format!("/UserViews?userId={}", link.user_id),
    ))
    .iter()
    .filter(|v| text(v, "Type") == "CollectionFolder")
    .filter_map(|v| {
        let kind = v.get("CollectionType").and_then(Value::as_str);
        let prefix = match kind {
            Some("movies" | "tvshows" | "homevideos" | "mixed") | None => "view",
            Some("music") => "music",
            _ => return None,
        };
        Some((
            prefix,
            text(v, "Id"),
            text(v, "Name"),
            kind.unwrap_or("mixed").to_owned(),
        ))
    })
    .collect()
}

/// "412 movies": how much a library holds, in its own words.
fn library_detail(count: u64, kind: &str) -> String {
    let (one, many) = match kind {
        "movies" => ("movie", "movies"),
        "tvshows" => ("show", "shows"),
        "music" => ("album", "albums"),
        "homevideos" => ("video", "videos"),
        _ => ("title", "titles"),
    };
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The Libraries row: one wide card per library, with the library's own
/// picture and how much is in it.
fn fetch_libraries(
    reply: &mpsc::Sender<Response>,
    cache: &Path,
    link: &JellyfinLink,
    agent: &ureq::Agent,
    index: usize,
) {
    let views = view_list(agent, link);
    let mut infos = vec![];
    let mut cards = vec![];
    for (prefix, id, name, kind) in &views {
        let card = card_id(id);
        let types = if *prefix == "music" {
            "MusicAlbum"
        } else {
            "Movie,Series"
        };
        let detail = get(
            agent,
            link,
            &format!(
                "/Items?userId={}&ParentId={id}&Recursive=true&IncludeItemTypes={types}&Limit=0",
                link.user_id
            ),
        )
        .and_then(|body| body.get("TotalRecordCount").and_then(Value::as_u64))
        .map(|count| library_detail(count, kind))
        .unwrap_or_default();
        infos.push(LibraryInfo {
            card,
            collection: format!("{prefix}:{id}"),
            name: name.clone(),
            detail,
        });
        let cached = cache.join(format!("jf-lib-{card}.jpg"));
        cards.push(StackItem {
            title: name.clone(),
            poster: cached
                .exists()
                .then(|| cached.to_string_lossy().into_owned()),
            id: card,
        });
    }
    let _ = reply.send(Response::Libraries(infos));
    let _ = reply.send(Response::Items(index, cards.clone()));
    if cards.iter().all(|c| c.poster.is_some()) {
        return;
    }
    for ((_, id, _, _), card) in views.iter().zip(cards.iter_mut()) {
        if card.poster.is_none() {
            // Jellyfin draws every library a 16:9 picture of its own.
            card.poster = crate::media::download_as(
                agent,
                cache,
                &format!("jf-lib-{}", card.id),
                &image_url(link, id, "Primary", "fillWidth=850&fillHeight=478"),
            );
        }
    }
    let _ = reply.send(Response::Items(index, cards));
}

/// Cards per Library page: eight rows of six.
const LIBRARY_PAGE: usize = 48;

/// One page of a row's whole list for the Library (View Library), newest
/// first; a search looks through every library.
pub fn library_page(
    link: &JellyfinLink,
    collection: &str,
    query: &str,
    page: u32,
) -> Option<crate::media::LibraryPage> {
    let agent = agent();
    let user = &link.user_id;
    let start = (page.max(1) as usize - 1) * LIBRARY_PAGE;
    let paged = format!("StartIndex={start}&Limit={LIBRARY_PAGE}&{FIELDS}");
    let search = if query.is_empty() {
        String::new()
    } else {
        let q: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        format!("&searchTerm={q}")
    };
    let path = match collection {
        "resume" if query.is_empty() => {
            format!("/UserItems/Resume?userId={user}&MediaTypes=Video&{paged}")
        }
        "nextup" if query.is_empty() => format!("/Shows/NextUp?userId={user}&{paged}"),
        c => {
            let (kind, parent) = if music_collection(c) {
                ("MusicAlbum", c.strip_prefix("music:"))
            } else {
                ("Movie,Series", c.strip_prefix("view:"))
            };
            let parent = parent.map(|p| format!("&ParentId={p}")).unwrap_or_default();
            format!(
                "/Items?userId={user}{parent}&Recursive=true&IncludeItemTypes={kind}\
                 &SortBy=DateCreated,SortName&SortOrder=Descending{search}&{paged}"
            )
        }
    };
    let body = get(&agent, link, &path)?;
    let total = body
        .get("TotalRecordCount")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let mut items: Vec<(i64, String, String)> = vec![];
    for item in list(Some(body)) {
        let (item, title) = card_target(&item);
        let id = card_id(&item);
        if item.is_empty() || items.iter().any(|i| i.0 == id) {
            continue;
        }
        items.push((
            id,
            title,
            image_url(link, &item, "Primary", "fillHeight=405"),
        ));
    }
    Some(crate::media::LibraryPage {
        items,
        pages: total.div_ceil(LIBRARY_PAGE) as u32,
        total,
    })
}

/// Movie and show libraries ("view:<id>", name), music ones ("music:<id>", name).
fn views(agent: &ureq::Agent, link: &JellyfinLink) -> Vec<(String, String)> {
    view_list(agent, link)
        .into_iter()
        .map(|(prefix, id, name, _)| (format!("{prefix}:{id}"), name))
        .collect()
}

/// Songs of an album in disc and track order.
fn songs(agent: &ureq::Agent, link: &JellyfinLink, album: &str) -> Vec<Value> {
    list(get(
        agent,
        link,
        &format!(
            "/Items?userId={}&ParentId={album}&IncludeItemTypes=Audio&Recursive=true\
             &SortBy=ParentIndexNumber,IndexNumber,SortName",
            link.user_id
        ),
    ))
}

/// Album art: the album's own image, else the first song's (embedded in the
/// file). Many libraries list an album image whose file is gone.
fn album_cover(
    agent: &ureq::Agent,
    link: &JellyfinLink,
    album: &str,
    size: &str,
) -> Option<String> {
    let works = |url: &str| agent.get(url).call().is_ok_and(|r| r.status().is_success());
    let own = image_url(link, album, "Primary", size);
    if works(&own) {
        return Some(own);
    }
    let first = songs(agent, link, album)
        .into_iter()
        .find(|s| s.get("ImageTags").and_then(|t| t.get("Primary")).is_some())?;
    let url = image_url(link, &text(&first, "Id"), "Primary", size);
    works(&url).then_some(url)
}

fn album_artist(item: &Value) -> String {
    let artist = text(item, "AlbumArtist");
    if artist.is_empty() {
        names(item, "Artists", 2)
    } else {
        artist
    }
}

fn album(reply: &mpsc::Sender<Response>, cache: &Path, link: &JellyfinLink, id: i64) {
    let agent = agent();
    let Some(guid) = item_of(id) else {
        return;
    };
    let Some(item) = get(
        &agent,
        link,
        &format!("/Items/{guid}?userId={}", link.user_id),
    ) else {
        let _ = reply.send(Response::Failed(
            "This album is not on the server any more".into(),
        ));
        return;
    };
    let mut detail = crate::state::album::Album {
        id,
        title: text(&item, "Name"),
        artist: album_artist(&item),
        year: item
            .get("ProductionYear")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        songs: songs(&agent, link, &guid)
            .iter()
            .map(|s| crate::state::album::Song {
                number: s.get("IndexNumber").and_then(Value::as_i64).unwrap_or(0),
                title: text(s, "Name"),
                artist: names(s, "Artists", 2),
                ms: ticks_ms(s.get("RunTimeTicks")),
            })
            .collect(),
        cover: None,
        backdrop: None,
    };
    let _ = reply.send(Response::Album(Box::new(detail.clone())));
    let name = format!("jf-{id}-cover");
    detail.cover = album_cover(&agent, link, &guid, "fillHeight=880")
        .and_then(|url| crate::media::download_as(&agent, cache, &name, &url));
    detail.backdrop = detail
        .cover
        .as_deref()
        .and_then(|c| crate::media::poster_backdrop(cache, &format!("{name}-pb"), c));
    let _ = reply.send(Response::Album(Box::new(detail)));
}

/// Audio as the TV plays it: MP3, AAC and FLAC as they are, anything else
/// converted to MP3 by the server. The URL carries the token: never logged.
fn audio_url(link: &JellyfinLink, item: &str, session: &str) -> String {
    format!(
        "{}/Audio/{item}/universal?UserId={}&DeviceId={}&ApiKey={}&PlaySessionId={session}\
         &Container=mp3,aac,m4a|aac,m4b|aac,flac,wav&TranscodingContainer=mp3\
         &TranscodingProtocol=http&AudioCodec=mp3&MaxStreamingBitrate=320000",
        link.server, link.user_id, link.device_id, link.token
    )
}

fn play_album(
    reply: &mpsc::Sender<Response>,
    link: &JellyfinLink,
    id: i64,
    start: usize,
    shuffle: bool,
) {
    let agent = agent();
    let Some(guid) = item_of(id) else {
        return;
    };
    let item = get(
        &agent,
        link,
        &format!("/Items/{guid}?userId={}", link.user_id),
    );
    let list = songs(&agent, link, &guid);
    if list.is_empty() {
        let _ = reply.send(Response::Failed("This album has no songs".into()));
        return;
    }
    let session = crate::phone_link::random_hex();
    let artist = item.as_ref().map(album_artist).unwrap_or_default();
    let songs: Vec<crate::player::MusicSong> = list
        .iter()
        .map(|s| {
            let item = text(s, "Id");
            crate::player::MusicSong {
                title: text(s, "Name"),
                artist: names(s, "Artists", 2),
                url: audio_url(link, &item, &session),
                item,
                ms: ticks_ms(s.get("RunTimeTicks")),
            }
        })
        .collect();
    let start = start.min(songs.len() - 1);
    let first = songs[start].clone();
    let title = item.as_ref().map(|i| text(i, "Name")).unwrap_or_default();
    let report = crate::player::Report {
        service: "jellyfin".into(),
        base: link.server.clone(),
        token: link.token.clone(),
        device_id: link.device_id.clone(),
        user: link.user_id.clone(),
        item: first.item.clone(),
        source: first.item.clone(),
        session,
        episodes: vec![],
    };
    let _ = reply.send(Response::Play(Box::new(crate::player::OpenRequest {
        url: first.url.clone(),
        title: title.clone(),
        source: crate::player::Source::Local,
        resume_ms: 0,
        duration_ms: first.ms,
        poster: None,
        audio: vec![],
        subtitles: vec![],
        qualities: vec![],
        chapters: vec![],
        item_id: Some(id),
        episode_line: None,
        prev_episode: None,
        next_episode: None,
        selected_subtitle: None,
        report: Some(report),
        jellyfin: None,
        music: Some(crate::player::Music {
            album: title,
            artist,
            year: item
                .as_ref()
                .and_then(|i| i.get("ProductionYear"))
                .and_then(Value::as_i64)
                .unwrap_or(0),
            cover: album_cover(&agent, link, &guid, "fillHeight=880").unwrap_or_default(),
            songs,
            start,
            shuffle,
        }),
    })));
}

fn ticks_ms(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0) / 10_000
}

fn names(item: &Value, key: &str, max: usize) -> String {
    item.get(key)
        .and_then(Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(Value::as_str)
                .take(max)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn people(item: &Value, kind: &str, max: usize) -> String {
    item.get("People")
        .and_then(Value::as_array)
        .map(|v| {
            v.iter()
                .filter(|p| p.get("Type").and_then(Value::as_str) == Some(kind))
                .filter_map(|p| p.get("Name").and_then(Value::as_str))
                .take(max)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn streams<'a>(item: &'a Value, kind: &str) -> Vec<&'a Value> {
    item.get("MediaStreams")
        .and_then(Value::as_array)
        .map(|v| {
            v.iter()
                .filter(|s| s.get("Type").and_then(Value::as_str) == Some(kind))
                .collect()
        })
        .unwrap_or_default()
}

fn stream_label(stream: &Value) -> String {
    let label = text(stream, "DisplayTitle");
    if label.is_empty() {
        text(stream, "Language")
    } else {
        label
    }
}

fn tags(item: &Value) -> Vec<String> {
    let mut tags = vec![];
    if let Some(video) = streams(item, "Video").first() {
        if video.get("Height").and_then(Value::as_u64).unwrap_or(0) >= 2000
            || video.get("Width").and_then(Value::as_u64).unwrap_or(0) >= 3800
        {
            tags.push("4K".to_owned());
        }
        let range = text(video, "VideoRange");
        if !range.is_empty() && range != "SDR" {
            tags.push(text(video, "VideoRangeType").replace("DOVI", "Dolby Vision"));
        }
    }
    if streams(item, "Audio")
        .iter()
        .any(|a| a.get("Channels").and_then(Value::as_u64).unwrap_or(0) >= 6)
    {
        tags.push("5.1".to_owned());
    }
    tags.retain(|t| !t.is_empty());
    tags
}

fn episodes(agent: &ureq::Agent, link: &JellyfinLink, series: &str) -> Vec<Value> {
    list(get(
        agent,
        link,
        &format!(
            "/Shows/{series}/Episodes?userId={}&Fields=Overview&EnableUserData=true",
            link.user_id
        ),
    ))
    .into_iter()
    .filter(|e| e.get("IndexNumber").is_some())
    .collect()
}

fn season_number(e: &Value) -> (i64, i64) {
    (
        e.get("ParentIndexNumber")
            .and_then(Value::as_i64)
            .unwrap_or(1),
        e.get("IndexNumber").and_then(Value::as_i64).unwrap_or(0),
    )
}

fn played(e: &Value) -> (bool, f32) {
    let data = e.get("UserData");
    let watched = data
        .and_then(|d| d.get("Played"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let share = data
        .and_then(|d| d.get("PlayedPercentage"))
        .and_then(Value::as_f64)
        .unwrap_or(0.) as f32
        / 100.;
    (watched, if watched { 1. } else { share })
}

fn details(reply: &mpsc::Sender<Response>, cache: &Path, link: &JellyfinLink, id: i64) {
    let agent = agent();
    let Some(guid) = item_of(id) else {
        return;
    };
    let Some(item) = get(
        &agent,
        link,
        &format!(
            "/Items/{guid}?userId={}&Fields=Overview,Genres,People,ProductionLocations,MediaStreams,MediaSources",
            link.user_id
        ),
    ) else {
        let _ = reply.send(Response::Failed(
            "This title is not on the server any more".into(),
        ));
        return;
    };
    let serial = text(&item, "Type") == "Series";
    let mut detail = Detail {
        id,
        serial,
        title: text(&item, "Name"),
        year: item
            .get("ProductionYear")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        minutes: (ticks_ms(item.get("RunTimeTicks")) / 60_000) as i64,
        genres: names(&item, "Genres", 3),
        countries: names(&item, "ProductionLocations", 2),
        plot: text(&item, "Overview"),
        director: people(&item, "Director", 2),
        cast: people(&item, "Actor", 5),
        tags: tags(&item),
        audio: streams(&item, "Audio")
            .iter()
            .map(|s| stream_label(s))
            .collect(),
        ..Default::default()
    };
    // Versions, qualities and every subtitle (image ones say they burn in).
    let sources = item
        .get("MediaSources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let first = sources.first().unwrap_or(&item);
    let hdr = source_hdr(first);
    detail.subtitles = streams(first, "Subtitle")
        .iter()
        .map(|s| (subtitle_label(s, hdr), String::new()))
        .collect();
    if sources.len() > 1 {
        detail.versions = sources
            .iter()
            .map(|s| (version_label(s), text(s, "Id")))
            .collect();
    }
    detail.default_quality = Some(if hdr { "Original · HDR" } else { "Original" }.into());
    // A series has no file of its own: its episodes get the whole ladder,
    // and the player still never re-encodes an HDR episode.
    detail.qualities = if serial {
        QUALITY_LADDER.iter().map(|q| q.0.to_owned()).collect()
    } else {
        quality_ladder(first)
    };
    if serial {
        let list = episodes(&agent, link, &guid);
        for e in &list {
            let (season, number) = season_number(e);
            let (watched, progress) = played(e);
            if !detail.seasons.contains(&season) {
                detail.seasons.push(season);
            }
            detail.episodes.push(Episode {
                season,
                number,
                title: text(e, "Name"),
                minutes: (ticks_ms(e.get("RunTimeTicks")) / 60_000) as i64,
                progress,
                watched,
                thumb_url: image_url(link, &text(e, "Id"), "Primary", "fillWidth=480"),
                thumb: None,
            });
        }
        detail.next = detail.episodes.iter().position(|e| !e.watched);
    }
    let _ = reply.send(Response::Details(Box::new(detail.clone())));
    // Pictures second, so the text shows without waiting for them.
    let name = format!("jf-{id}");
    detail.poster = poster(&agent, cache, link, &guid, &name);
    let has_backdrop = item
        .get("BackdropImageTags")
        .and_then(Value::as_array)
        .is_some_and(|v| !v.is_empty());
    detail.backdrop = if has_backdrop {
        crate::media::download_as(
            &agent,
            cache,
            &format!("{name}-b"),
            &image_url(link, &guid, "Backdrop/0", "fillWidth=1920"),
        )
    } else {
        None
    }
    .or_else(|| {
        detail
            .poster
            .as_deref()
            .and_then(|p| crate::media::poster_backdrop(cache, &format!("{name}-pb"), p))
    });
    crate::media::season_stills(&agent, cache, &mut detail);
    let _ = reply.send(Response::Details(Box::new(detail)));
}

/// HLS that Jellyfin remuxes (or transcodes only when it must) for the TV's
/// web video element. The URL carries the token: never logged.
/// Lower qualities offered for SDR files, re-encoded to H.264. The player's
/// streams.js has the same ladder; the names travel between the two.
pub const QUALITY_LADDER: [(&str, u64, u64); 5] = [
    ("1080p · 20 Mbps", 20_000_000, 1080),
    ("1080p · 10 Mbps", 10_000_000, 1080),
    ("720p · 6 Mbps", 6_000_000, 720),
    ("720p · 3 Mbps", 3_000_000, 720),
    ("480p · 1.5 Mbps", 1_500_000, 480),
];
/// Subtitle formats the player draws itself; anything else is an image
/// format (PGS, VobSub, DVB) that has to be burned into the picture.
const TEXT_SUBTITLES: [&str; 11] = [
    "srt", "subrip", "ass", "ssa", "vtt", "webvtt", "mov_text", "ttml", "text", "smi", "sami",
];

fn source_video(source: &Value) -> Option<&Value> {
    streams(source, "Video").first().copied()
}
/// HDR10, HLG or Dolby Vision. Jellyfin turns HDR into SDR whenever it
/// re-encodes, so these files are only ever played as they are.
pub fn source_hdr(source: &Value) -> bool {
    source_video(source).is_some_and(|v| {
        let range = text(v, "VideoRangeType");
        text(v, "VideoRange").eq_ignore_ascii_case("hdr")
            || (!range.is_empty() && range != "SDR" && range != "Unknown")
    })
}
/// Lower qualities worth offering for this file (none for HDR).
pub fn quality_ladder(source: &Value) -> Vec<String> {
    if source_hdr(source) {
        return vec![];
    }
    let height = source_video(source)
        .and_then(|v| v.get("Height"))
        .and_then(Value::as_u64)
        .unwrap_or(1080)
        .max(480);
    let bitrate = source
        .get("Bitrate")
        .and_then(Value::as_u64)
        .filter(|b| *b > 0)
        .unwrap_or(u64::MAX);
    QUALITY_LADDER
        .iter()
        .filter(|(_, b, h)| *h <= height && *b < bitrate)
        .map(|(name, _, _)| (*name).to_owned())
        .collect()
}
/// (bitrate, height) of a quality picked on the title card.
pub fn quality_spec(label: &str) -> Option<(u64, u64)> {
    QUALITY_LADDER
        .iter()
        .find(|(name, _, _)| *name == label)
        .map(|(_, b, h)| (*b, *h))
}
fn image_subtitle(stream: &Value) -> bool {
    let codec = text(stream, "Codec").to_lowercase();
    stream.get("IsTextSubtitleStream").and_then(Value::as_bool) != Some(true)
        && !TEXT_SUBTITLES.contains(&codec.as_str())
}
/// A subtitle as the pickers show it; image subtitles say what they cost.
pub fn subtitle_label(stream: &Value, hdr: bool) -> String {
    let mut name = text(stream, "DisplayTitle");
    if name.is_empty() {
        name = text(stream, "Title");
    }
    if name.is_empty() {
        name = text(stream, "Language");
    }
    if name.is_empty() {
        name = "Subtitles".into();
    }
    match (image_subtitle(stream), hdr) {
        (false, _) => name,
        (true, false) => format!("{name} · burned in"),
        (true, true) => format!("{name} · burned in, SDR"),
    }
}
/// A version as the Version picker shows it: the server's name for it,
/// else what it is ("4K · HDR10 · HEVC").
fn version_label(source: &Value) -> String {
    let name = text(source, "Name");
    if !name.is_empty() {
        return name;
    }
    let Some(video) = source_video(source) else {
        return "Version".into();
    };
    let mut parts = vec![];
    match video.get("Height").and_then(Value::as_u64).unwrap_or(0) {
        h if h >= 2000 => parts.push("4K".to_owned()),
        h if h > 0 => parts.push(format!("{h}p")),
        _ => {}
    }
    if source_hdr(source) {
        parts.push(text(video, "VideoRangeType").replace("DOVI", "Dolby Vision"));
    }
    parts.push(codec_name(&text(video, "Codec")));
    parts.join(" · ")
}
fn codec_name(codec: &str) -> String {
    match codec.to_lowercase().as_str() {
        "hevc" | "h265" => "HEVC".into(),
        "h264" | "avc" => "H.264".into(),
        "av1" => "AV1".into(),
        "vp9" => "VP9".into(),
        other => other.to_uppercase(),
    }
}
/// The fields of a MediaSource the web player needs, and nothing else:
/// launch parameters stay small.
pub fn trim_source(source: &Value) -> Value {
    const KEEP: [&str; 19] = [
        "Index",
        "Type",
        "Codec",
        "Language",
        "DisplayTitle",
        "Title",
        "IsDefault",
        "IsForced",
        "IsExternal",
        "IsTextSubtitleStream",
        "Channels",
        "Height",
        "Width",
        "BitRate",
        "BitDepth",
        "VideoRange",
        "VideoRangeType",
        "Profile",
        "ChannelLayout",
    ];
    let streams: Vec<Value> = source
        .get("MediaStreams")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|s| matches!(text(s, "Type").as_str(), "Video" | "Audio" | "Subtitle"))
                .take(64)
                .map(|s| {
                    let mut kept = serde_json::Map::new();
                    for key in KEEP {
                        if let Some(v) = s.get(key).filter(|v| !v.is_null()) {
                            kept.insert(key.into(), v.clone());
                        }
                    }
                    Value::Object(kept)
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "Id": text(source, "Id"),
        "Name": text(source, "Name"),
        "Container": text(source, "Container"),
        "Bitrate": source.get("Bitrate").cloned().unwrap_or(Value::Null),
        "MediaStreams": streams,
    })
}
/// The original file, as the TV would fetch it for Direct Play.
fn static_url(
    link: &JellyfinLink,
    item: &str,
    source: &str,
    container: &str,
    session: &str,
) -> String {
    let ext = container
        .split(',')
        .next()
        .filter(|c| !c.is_empty())
        .unwrap_or("mkv");
    format!(
        "{}/Videos/{item}/stream.{ext}?static=true&MediaSourceId={source}&PlaySessionId={session}\
         &DeviceId={}&ApiKey={}",
        link.server, link.device_id, link.token
    )
}

fn play(reply: &mpsc::Sender<Response>, link: &JellyfinLink, request: PlayRequest) {
    let agent = agent();
    let Some(guid) = item_of(request.id) else {
        return;
    };
    let fail = |why: &str| {
        let _ = reply.send(Response::Failed(why.to_owned()));
    };
    let Some(item) = get(
        &agent,
        link,
        &format!("/Items/{guid}?userId={}", link.user_id),
    ) else {
        return fail("This title is not on the server any more");
    };
    let series = text(&item, "Type") == "Series";
    let list = if series {
        episodes(&agent, link, &guid)
    } else {
        vec![]
    };
    let position = if series {
        let wanted = request.episode.or_else(|| {
            // Next up: the episode in progress, else the first one not watched.
            list.iter()
                .find(|e| {
                    let (watched, share) = played(e);
                    !watched && share > 0.
                })
                .or_else(|| list.iter().find(|e| !played(e).0))
                .or(list.first())
                .map(season_number)
        });
        let Some(i) = wanted.and_then(|w| list.iter().position(|e| season_number(e) == w)) else {
            return fail("No episode to play");
        };
        Some(i)
    } else {
        None
    };
    let target = position.map_or(&item, |i| &list[i]);
    let target_id = text(target, "Id");
    // Every version of the file, with its streams. Which one plays and how
    // (Direct Play, Direct Stream or a transcode) the web player decides on
    // the TV, which knows what it can decode.
    let info = agent
        .post(&format!(
            "{}/Items/{target_id}/PlaybackInfo?userId={}",
            link.server, link.user_id
        ))
        .header("Authorization", &auth(&link.device_id, &link.token))
        .header("Content-Type", "application/json")
        .send(json!({ "UserId": link.user_id }).to_string())
        .ok()
        .and_then(read);
    let sources = info
        .as_ref()
        .and_then(|i| i.get("MediaSources"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // A version chosen on the title card applies to that title, not to the
    // other episodes of a series.
    let chosen = request
        .version
        .as_ref()
        .filter(|_| !series)
        .and_then(|id| sources.iter().find(|s| &text(s, "Id") == id));
    let Some(source) = chosen.or(sources.first()).cloned() else {
        return fail("The server has no playable file for this title");
    };
    let source_id = text(&source, "Id");
    let session = info
        .as_ref()
        .map(|i| text(i, "PlaySessionId"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(crate::phone_link::random_hex);
    let hdr = source_hdr(&source);
    let container = text(&source, "Container");
    let url = static_url(link, &target_id, &source_id, &container, &session);
    // Subtitle picked on the card, by its label, in this file's own list.
    let subtitle_index = request.subtitle.as_ref().and_then(|want| {
        streams(&source, "Subtitle")
            .iter()
            .find(|s| &subtitle_label(s, hdr) == want)
            .and_then(|s| s.get("Index").and_then(Value::as_i64))
    });
    // Lower quality never applies to HDR: it would be re-encoded to SDR.
    let quality = request
        .quality
        .as_deref()
        .and_then(quality_spec)
        .filter(|_| !hdr);
    let subtitles: Vec<crate::player::Track> = streams(&source, "Subtitle")
        .iter()
        .filter(|s| !image_subtitle(s))
        .map(|s| crate::player::Track {
            name: subtitle_label(s, hdr),
            url: Some(format!(
                "{}/Videos/{target_id}/{source_id}/Subtitles/{}/0/Stream.vtt?ApiKey={}",
                link.server,
                s.get("Index").and_then(Value::as_i64).unwrap_or(0),
                link.token
            )),
        })
        .collect();
    let selected_subtitle = request
        .subtitle
        .as_ref()
        .and_then(|want| subtitles.iter().position(|t| &t.name == want));
    let plan = json!({
        "base": link.server,
        "token": link.token,
        "deviceId": link.device_id,
        "item": target_id,
        "session": session,
        "source": trim_source(&source),
        "audio": Value::Null,
        "subtitle": subtitle_index,
        "quality": quality.map(|(bitrate, height)| json!({ "bitrate": bitrate, "height": height })),
    });
    let neighbour = |i: Option<usize>| i.and_then(|i| list.get(i)).map(season_number);
    let title = text(&item, "Name");
    let (title, episode_line) = match position {
        Some(i) => {
            let (season, number) = season_number(&list[i]);
            let name = text(&list[i], "Name");
            (
                format!("{title} · S{season}E{number}"),
                Some(if name.is_empty() {
                    format!("Season {season} · Episode {number}")
                } else {
                    format!("Season {season} · Episode {number} · {name}")
                }),
            )
        }
        None => (title, None),
    };
    let report = crate::player::Report {
        service: "jellyfin".into(),
        base: link.server.clone(),
        token: link.token.clone(),
        device_id: link.device_id.clone(),
        user: link.user_id.clone(),
        item: target_id.clone(),
        source: source_id.clone(),
        session: session.clone(),
        episodes: list
            .iter()
            .map(|e| {
                let (season, number) = season_number(e);
                let id = text(e, "Id");
                let name = text(e, "Name");
                crate::player::ReportEpisode {
                    season,
                    number,
                    // Fallback only: the web player asks Jellyfin for each
                    // episode's file and chooses how to play it.
                    url: static_url(link, &id, &id, "mkv", &session),
                    line: if name.is_empty() {
                        format!("Season {season} · Episode {number}")
                    } else {
                        format!("Season {season} · Episode {number} · {name}")
                    },
                    item: id,
                }
            })
            .collect(),
    };
    let _ = reply.send(Response::Play(Box::new(crate::player::OpenRequest {
        url,
        title,
        source: if series {
            crate::player::Source::Series
        } else {
            crate::player::Source::Movie
        },
        resume_ms: target
            .get("UserData")
            .map_or(0, |d| ticks_ms(d.get("PlaybackPositionTicks"))),
        duration_ms: ticks_ms(target.get("RunTimeTicks")),
        poster: None,
        audio: streams(&source, "Audio")
            .iter()
            .map(|s| crate::player::Track {
                name: stream_label(s),
                url: None,
            })
            .collect(),
        subtitles,
        qualities: vec![],
        chapters: vec![],
        item_id: Some(request.id),
        episode_line,
        prev_episode: neighbour(position.and_then(|i| i.checked_sub(1))),
        next_episode: neighbour(position.map(|i| i + 1)),
        selected_subtitle,
        report: Some(report),
        music: None,
        jellyfin: Some(plan),
    })));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real LAN: `cargo test discover_lan -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn discover_lan() {
        let (tx, rx) = mpsc::channel();
        discover(&tx, None);
        let Some(Response::Servers(list)) = rx.try_iter().last() else {
            panic!("no reply");
        };
        println!("{list:?}");
        assert!(!list.is_empty());
    }

    /// Real server: LUMA_JF_LINK=<file with the saved "jellyfin" JSON>
    /// `cargo test _live -- --ignored --nocapture`. Prints no token.
    #[test]
    #[ignore]
    fn library_live() {
        let path = std::env::var("LUMA_JF_LINK").expect("LUMA_JF_LINK");
        let link: JellyfinLink =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut lists = vec![
            "albums".to_owned(),
            "latest".to_owned(),
            "nextup".to_owned(),
        ];
        lists.extend(views(&agent(), &link).into_iter().map(|v| v.0));
        for list in lists {
            let first = library_page(&link, &list, "", 1).expect("page 1");
            let second = library_page(&link, &list, "", 2).expect("page 2");
            println!(
                "{list}: {} total, {} pages, page 1 {}, page 2 {}",
                first.total,
                first.pages,
                first.items.len(),
                second.items.len()
            );
        }
        let found = library_page(&link, "albums", "Claro", 1).expect("search");
        println!(
            "search Claro: {:?}",
            found.items.iter().map(|i| &i.1).collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore]
    fn album_live() {
        let path = std::env::var("LUMA_JF_LINK").expect("LUMA_JF_LINK");
        let link: JellyfinLink =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let agent = agent();
        let cards = collection_items(&agent, &link, "albums", 3);
        assert!(!cards.is_empty(), "no albums");
        let (guid, title) = card_target(&cards[0]);
        let id = card_id(&guid);
        assert!(is_album(id));
        let cache = std::env::temp_dir().join("luma-album-live");
        std::fs::create_dir_all(&cache).unwrap();
        let (tx, rx) = mpsc::channel();
        album(&tx, &cache, &link, id);
        let last = rx.try_iter().last();
        let Some(Response::Album(a)) = last else {
            panic!("no album")
        };
        println!(
            "{title}: {} · {} · cover {:?}",
            a.artist,
            a.meta(),
            a.cover.is_some()
        );
        assert!(!a.songs.is_empty() && a.cover.is_some() && a.backdrop.is_some());
        play_album(&tx, &link, id, 1, false);
        let Some(Response::Play(request)) = rx.try_iter().last() else {
            panic!("no play")
        };
        let music = request.music.unwrap();
        assert_eq!(music.start, 1);
        let response = agent
            .get(&music.songs[1].url)
            .header("Range", "bytes=0-1023")
            .call()
            .unwrap();
        let kind = response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("").to_owned());
        // A server that lost its media folder answers 404/500 here: shown, not failed.
        println!("song 2: {} {:?}", response.status(), kind);
        assert!(
            views(&agent, &link)
                .iter()
                .any(|(k, _)| k.starts_with("music:"))
        );
    }

    #[test]
    fn container_address_takes_sender_host() {
        assert_eq!(
            with_host("http://10.21.0.15:8096", "192.168.1.68"),
            "http://192.168.1.68:8096"
        );
        assert_eq!(
            with_host("https://10.0.0.2:8920/jf", "192.168.1.5"),
            "https://192.168.1.5:8920"
        );
        assert_eq!(
            with_host("http://10.0.0.2", "192.168.1.5"),
            "http://192.168.1.5:8096"
        );
    }

    #[test]
    fn addresses_get_scheme_and_port() {
        assert_eq!(normalize("192.168.1.5"), "http://192.168.1.5:8096");
        assert_eq!(normalize("192.168.1.5:8920/"), "http://192.168.1.5:8920");
        assert_eq!(
            normalize("nas.local/jellyfin"),
            "http://nas.local:8096/jellyfin"
        );
        assert_eq!(
            normalize("https://demo.jellyfin.org/stable"),
            "https://demo.jellyfin.org/stable"
        );
        assert_eq!(normalize("  "), "");
    }

    #[test]
    fn the_libraries_row_is_built_from_the_server() {
        use std::io::{Read, Write};
        // A stand-in Jellyfin: two libraries the hub shows, one it skips.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&image::RgbImage::from_pixel(32, 18, image::Rgb([9, 9, 9])))
            .unwrap();
        let server = thread::spawn(move || {
            let mut seen = vec![];
            // UserViews, two counts, two pictures.
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 2048];
                let n = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                let (kind, body): (&str, Vec<u8>) = if path.starts_with("/UserViews") {
                    (
                        "application/json",
                        serde_json::json!({"Items": [
                            {"Id": "aaa", "Name": "Movies", "Type": "CollectionFolder", "CollectionType": "movies"},
                            {"Id": "bbb", "Name": "Music", "Type": "CollectionFolder", "CollectionType": "music"},
                            {"Id": "ccc", "Name": "Books", "Type": "CollectionFolder", "CollectionType": "books"}
                        ]})
                        .to_string()
                        .into_bytes(),
                    )
                } else if path.contains("/Images/Primary") {
                    ("image/jpeg", jpeg.clone())
                } else if path.contains("ParentId=aaa") {
                    (
                        "application/json",
                        br#"{"Items":[],"TotalRecordCount":412}"#.to_vec(),
                    )
                } else {
                    (
                        "application/json",
                        br#"{"Items":[],"TotalRecordCount":1}"#.to_vec(),
                    )
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
                seen.push(path);
            }
            seen
        });
        let cache = std::env::temp_dir().join(format!("luma-jf-hub-{port}"));
        std::fs::create_dir_all(&cache).unwrap();
        let link = JellyfinLink {
            server: format!("http://127.0.0.1:{port}"),
            user_id: "u1".into(),
            token: "t".into(),
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        fetch(&tx, &cache, &link, vec![(0, LIBRARIES.to_owned())]);
        let seen = server.join().unwrap();
        assert!(
            seen.iter()
                .any(|p| p.contains("ParentId=bbb") && p.contains("MusicAlbum"))
        );
        let mut libraries = vec![];
        let mut last_cards = vec![];
        for response in rx.try_iter() {
            match response {
                Response::Libraries(l) => libraries = l,
                Response::Items(0, cards) => last_cards = cards,
                _ => {}
            }
        }
        assert_eq!(libraries.len(), 2);
        assert_eq!(
            (
                libraries[0].collection.as_str(),
                libraries[0].detail.as_str()
            ),
            ("view:aaa", "412 movies")
        );
        assert_eq!(
            (
                libraries[1].collection.as_str(),
                libraries[1].detail.as_str()
            ),
            ("music:bbb", "1 album")
        );
        assert_eq!(last_cards.len(), 2);
        assert_eq!(last_cards[0].title, "Movies");
        assert_eq!(last_cards[0].id, libraries[0].card);
        // The library pictures were downloaded for the cards.
        assert!(last_cards.iter().all(|c| {
            c.poster
                .as_ref()
                .is_some_and(|p| std::path::Path::new(p).is_file())
        }));
        let _ = std::fs::remove_dir_all(&cache);
    }
    #[test]
    fn versions_qualities_and_subtitles_follow_the_file() {
        let hdr = json!({
            "Id": "v4k", "Name": "4K HDR", "Container": "mkv", "Bitrate": 60000000,
            "SupportsDirectPlay": false, "Path": "/secret/path.mkv",
            "MediaStreams": [
                {"Index": 0, "Type": "Video", "Codec": "hevc", "VideoRange": "HDR", "VideoRangeType": "HDR10", "Height": 2160},
                {"Index": 1, "Type": "Audio", "Codec": "truehd", "DisplayTitle": "English TrueHD 7.1", "Channels": 8},
                {"Index": 2, "Type": "Subtitle", "Codec": "PGSSUB", "DisplayTitle": "English PGS"},
                {"Index": 3, "Type": "Subtitle", "Codec": "subrip", "DisplayTitle": "English SRT", "IsTextSubtitleStream": true},
                {"Index": 4, "Type": "Attachment", "Codec": "ttf"}
            ]
        });
        let sdr = json!({
            "Id": "v1080", "Container": "mkv", "Bitrate": 12000000,
            "MediaStreams": [
                {"Index": 0, "Type": "Video", "Codec": "h264", "VideoRange": "SDR", "VideoRangeType": "SDR", "Height": 1080},
                {"Index": 1, "Type": "Subtitle", "Codec": "dvd_subtitle", "DisplayTitle": "French"}
            ]
        });
        assert!(source_hdr(&hdr));
        assert!(!source_hdr(&sdr));
        // HDR: nothing lower to offer. SDR 12 Mbps: only what is below it.
        assert!(quality_ladder(&hdr).is_empty());
        assert_eq!(
            quality_ladder(&sdr),
            vec![
                "1080p · 10 Mbps",
                "720p · 6 Mbps",
                "720p · 3 Mbps",
                "480p · 1.5 Mbps"
            ]
        );
        assert_eq!(quality_spec("720p · 6 Mbps"), Some((6_000_000, 720)));
        let subs = streams(&hdr, "Subtitle");
        assert_eq!(
            subtitle_label(subs[0], true),
            "English PGS · burned in, SDR"
        );
        assert_eq!(subtitle_label(subs[1], true), "English SRT");
        assert_eq!(
            subtitle_label(streams(&sdr, "Subtitle")[0], false),
            "French · burned in"
        );
        assert_eq!(version_label(&hdr), "4K HDR");
        assert_eq!(version_label(&sdr), "1080p · H.264");
        // The player gets streams, never paths or attachments.
        let trimmed = trim_source(&hdr);
        assert!(trimmed.get("Path").is_none());
        assert_eq!(trimmed["MediaStreams"].as_array().unwrap().len(), 4);
        assert_eq!(trimmed["MediaStreams"][0]["VideoRangeType"], "HDR10");
    }
    #[test]
    fn play_hands_the_chosen_version_to_the_player() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let mut seen = vec![];
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let n = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                let body = if path.contains("/PlaybackInfo") {
                    json!({"PlaySessionId": "sess9", "MediaSources": [
                        {"Id": "v4k", "Name": "4K HDR", "Container": "mkv", "MediaStreams": [
                            {"Index": 0, "Type": "Video", "Codec": "hevc", "VideoRange": "HDR", "VideoRangeType": "DOVIWithHDR10"},
                            {"Index": 1, "Type": "Subtitle", "Codec": "PGSSUB", "DisplayTitle": "English"}]},
                        {"Id": "v1080", "Name": "1080p", "Container": "mkv", "MediaStreams": [
                            {"Index": 0, "Type": "Video", "Codec": "h264", "VideoRangeType": "SDR"}]}
                    ]})
                } else {
                    json!({"Id": "movie1", "Name": "Film", "Type": "Movie", "RunTimeTicks": 72000000000u64})
                };
                let body = body.to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                seen.push(path);
            }
            seen
        });
        let link = JellyfinLink {
            server: format!("http://127.0.0.1:{port}"),
            user_id: "u1".into(),
            token: "tok".into(),
            device_id: "dev".into(),
            ..Default::default()
        };
        let id = card_id("movie1");
        let (tx, rx) = mpsc::channel();
        play(
            &tx,
            &link,
            PlayRequest {
                id,
                version: Some("v4k".into()),
                // Ignored: the chosen version is HDR.
                quality: Some("720p · 6 Mbps".into()),
                subtitle: Some("English · burned in, SDR".into()),
                ..Default::default()
            },
        );
        server.join().unwrap();
        let Some(Response::Play(request)) = rx.try_iter().last() else {
            panic!("no play reply");
        };
        let plan = request.jellyfin.as_ref().expect("jellyfin plan");
        assert_eq!(plan["source"]["Id"], "v4k");
        assert_eq!(plan["session"], "sess9");
        assert_eq!(plan["item"], "movie1");
        assert_eq!(plan["subtitle"], 1);
        assert!(plan["quality"].is_null(), "no lower quality for HDR");
        // The fallback address is the original file, never a transcode.
        assert!(
            request
                .url
                .contains("/Videos/movie1/stream.mkv?static=true&MediaSourceId=v4k")
        );
        assert_eq!(request.report.as_ref().unwrap().source, "v4k");
        assert_eq!(request.report.as_ref().unwrap().user, "u1");
    }
    #[test]
    fn libraries_say_what_they_hold() {
        assert_eq!(library_detail(412, "movies"), "412 movies");
        assert_eq!(library_detail(1, "tvshows"), "1 show");
        assert_eq!(library_detail(0, "music"), "0 albums");
        assert_eq!(library_detail(7, "mixed"), "7 titles");
        assert_eq!(COLLECTIONS[0], (LIBRARIES, "Libraries"));
    }
    #[test]
    fn card_ids_are_stable_and_distinct() {
        let a = card_id("f0b3381645f04afb9a0e392e74b6a1b0");
        assert_eq!(a, card_id("f0b3381645f04afb9a0e392e74b6a1b0"));
        assert!(a > 0 && owns(a));
        assert_eq!(
            item_of(a).as_deref(),
            Some("f0b3381645f04afb9a0e392e74b6a1b0")
        );
        assert!(!owns(123456));
    }

    #[test]
    fn episodes_stand_for_their_series() {
        let episode = json!({"Type": "Episode", "Id": "e1", "Name": "Pilot", "SeriesId": "s1", "SeriesName": "Show"});
        assert_eq!(card_target(&episode), ("s1".into(), "Show".into()));
        let movie = json!({"Type": "Movie", "Id": "m1", "Name": "Film"});
        assert_eq!(card_target(&movie), ("m1".into(), "Film".into()));
    }

    /// Whole flow against the public demo server; the demo user approves the
    /// code as the phone would. `cargo test jellyfin_demo -- --ignored`
    #[test]
    #[ignore]
    fn jellyfin_demo() {
        let server = "https://demo.jellyfin.org/stable".to_owned();
        let (reply, responses) = mpsc::channel();
        let device = "luma-test-device".to_owned();
        let worker = {
            let (reply, server, device) = (reply.clone(), server.clone(), device.clone());
            thread::spawn(move || connect(&reply, server, device, ATTEMPT.load(Ordering::SeqCst)))
        };
        let Ok(Response::Code { code, .. }) = responses.recv_timeout(Duration::from_secs(30))
        else {
            panic!("no code");
        };
        let agent = agent();
        let demo = agent
            .post(&format!("{server}/Users/AuthenticateByName"))
            .header("Authorization", &auth("luma-test-phone", ""))
            .header("Content-Type", "application/json")
            .send(r#"{"Username":"demo","Pw":""}"#)
            .ok()
            .and_then(read)
            .expect("demo login");
        let phone = auth("luma-test-phone", &text(&demo, "AccessToken"));
        let approved = agent
            .post(&format!("{server}/QuickConnect/Authorize?code={code}"))
            .header("Authorization", &phone)
            .send_empty()
            .expect("authorize");
        assert!(approved.status().is_success());
        let link = match responses.recv_timeout(Duration::from_secs(30)) {
            Ok(Response::Linked(link)) => link,
            _ => panic!("not linked"),
        };
        worker.join().unwrap();
        assert!(link.linked() && !link.user_id.is_empty());

        let cache = std::env::temp_dir().join("luma-jellyfin-test");
        let _ = std::fs::create_dir_all(&cache);
        fetch(
            &reply,
            &cache,
            &link,
            vec![(0, "resume".into()), (1, "latest".into())],
        );
        let mut rows = BTreeMap::new();
        while let Ok(Response::Items(i, items)) = responses.recv_timeout(Duration::from_secs(1)) {
            rows.insert(i, items);
        }
        let resume = rows.get(&0).expect("resume row");
        assert!(!resume.is_empty());
        assert!(rows.get(&1).is_some_and(|r| !r.is_empty()));
        assert!(
            resume.iter().all(|c| c.poster.is_some()),
            "posters downloaded"
        );
        assert!(!views(&agent, &link).is_empty());

        // Title card and Play for every card of the resume row.
        for card in resume {
            details(&reply, &cache, &link, card.id);
            let mut last = None;
            while let Ok(Response::Details(d)) = responses.recv_timeout(Duration::from_secs(1)) {
                last = Some(d);
            }
            let detail = last.expect("details");
            assert_eq!(detail.title, card.title);
            assert!(detail.poster.is_some() && detail.backdrop.is_some());
            if detail.serial {
                assert!(!detail.episodes.is_empty());
            }
            play(
                &reply,
                &link,
                PlayRequest {
                    id: card.id,
                    ..Default::default()
                },
            );
            let Ok(Response::Play(open)) = responses.recv_timeout(Duration::from_secs(20)) else {
                panic!("no play for {}", card.title);
            };
            let status = agent
                .get(&open.url)
                .header("Range", "bytes=0-100")
                .call()
                .unwrap()
                .status();
            assert!(status.is_success(), "{} stream {status}", card.title);
            assert!(open.report.is_some());
        }
    }
}
