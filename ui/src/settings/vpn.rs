//! The VPN page. Plaitway's part: OpenVPN and WireGuard profiles up side by side, with the helper
//! `plaitwayd` deciding which one holds each route and the DNS. Everything goes through the
//! `plaitway` command, read as the JSON it prints; the credentials a profile asks for are
//! kept in the keyring items the Plaitway app keeps, so that the two, and the command, share
//! them.

use super::*;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};

const CLI: &str = "plaitway";
const SERVICE: &str = "plaitwayd";
/// What the keyring items are filed under, as the Plaitway app files them.
const SECRET_SCHEMA: &str = "io.github.koukeneko.plaitway.credentials";
const USER_PASSWORD: &str = "user-password";
const KEY_PASSPHRASE: &str = "key-passphrase";
/// What `plaitway show` prints in place of a secret.
const HIDDEN: &str = "[hidden]";
/// How far back the traffic graph goes, in seconds.
const TRAFFIC_SPAN: f64 = 120.0;
/// How many lines of a log are kept.
const LOG_LINES: usize = 5000;

pub(crate) fn installed() -> bool {
    glib::find_program_in_path(CLI).is_some()
}

// ---- what the command prints ----

fn nullable<'de, D: serde::Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A 64-bit count, which protobuf's JSON writes as a string.
fn count<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Count {
        Number(u64),
        Text(String),
    }
    match Option::<Count>::deserialize(d)? {
        Some(Count::Number(n)) => Ok(n),
        Some(Count::Text(text)) => text.parse().map_err(serde::de::Error::custom),
        None => Ok(0),
    }
}

#[derive(Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
struct Profile {
    id: String,
    name: String,
    kind: String,
    desired_enabled: bool,
    state: String,
    last_error: String,
    #[serde(deserialize_with = "nullable")]
    settings: ProfileSettings,
    #[serde(deserialize_with = "nullable")]
    summary: Summary,
    #[serde(deserialize_with = "nullable")]
    status: Status,
    #[serde(deserialize_with = "nullable")]
    credential_request: CredentialRequest,
}

#[derive(Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
struct ProfileSettings {
    auto_connect: bool,
    tunnel_mode: String,
    priority: i32,
    exclude_private_ips: bool,
    #[serde(deserialize_with = "nullable")]
    on_demand: OnDemand,
}

#[derive(Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default)]
struct OnDemand {
    ethernet: bool,
    wifi: bool,
}

#[derive(Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
struct Summary {
    endpoints: Vec<Endpoint>,
    routes: Vec<String>,
    dns_servers: Vec<String>,
    addresses: Vec<String>,
    public_key: String,
}

#[derive(Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default)]
struct Endpoint {
    host: String,
    port: u32,
    protocol: String,
}

#[derive(Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
struct Status {
    interface_name: String,
    addresses: Vec<String>,
    remote: String,
    connected_since: Option<String>,
    #[serde(deserialize_with = "count")]
    rx_bytes: u64,
    #[serde(deserialize_with = "count")]
    tx_bytes: u64,
    routes: Vec<RouteStatus>,
    dns: Vec<DnsStatus>,
    warnings: Vec<String>,
}

#[derive(Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
struct RouteStatus {
    prefix: String,
    state: String,
    detail: String,
    shadowed_by: String,
}

#[derive(Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
struct DnsStatus {
    servers: Vec<String>,
    match_domains: Vec<String>,
    state: String,
    detail: String,
}

#[derive(Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default)]
struct CredentialRequest {
    kind: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Event {
    snapshot: Option<Snapshot>,
    changed: Option<Profile>,
    removed: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Snapshot {
    profiles: Vec<Profile>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Diagnostics {
    #[serde(deserialize_with = "nullable")]
    daemon: DaemonInfo,
    #[serde(deserialize_with = "nullable")]
    network: NetworkInfo,
    owned_routes: Vec<OwnedRoute>,
    stale_routes: Vec<StaleRoute>,
    resolver_entries: Vec<String>,
    recent_journal: Vec<JournalEntry>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DaemonInfo {
    version: String,
    privileged: bool,
    engines: Vec<EngineInfo>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct EngineInfo {
    kind: String,
    available: bool,
    version: String,
    detail: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct NetworkInfo {
    default_gateway_v4: String,
    default_interface_v4: String,
    default_gateway_v6: String,
    default_interface_v6: String,
    interfaces: Vec<String>,
    last_change: Option<String>,
    last_change_reason: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OwnedRoute {
    prefix: String,
    owner: String,
    kind: String,
    via: String,
    state: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct StaleRoute {
    key: String,
    prefix: String,
    gateway: String,
    interface: String,
    reason: String,
    owned: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JournalEntry {
    time: Option<String>,
    owner: String,
    kind: String,
    key: String,
    state: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LogLine {
    time: Option<String>,
    level: String,
    text: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ImportResponse {
    #[serde(deserialize_with = "nullable")]
    profile: Profile,
    warnings: Vec<ImportWarning>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ImportWarning {
    line: i32,
    directive: String,
    message: String,
}

impl ImportWarning {
    /// `第 4 行：up – removed, a profile cannot run programs`.
    fn summary(&self) -> String {
        let place = if self.line > 0 { format!("第 {} 行：", self.line) } else { String::new() };
        let parts: Vec<&str> = [self.directive.as_str(), self.message.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        place + &parts.join(" – ")
    }
}

// ---- how states are said ----

impl Profile {
    fn state(&self) -> &str {
        self.state.trim_start_matches("PROFILE_STATE_")
    }

    fn connected(&self) -> bool {
        self.state() == "CONNECTED"
    }

    /// Disconnected or failed: what is read as a connection starts can change now.
    fn idle(&self) -> bool {
        matches!(self.state(), "DISCONNECTED" | "FAILED")
    }

    fn wireguard(&self) -> bool {
        self.kind == "PROFILE_KIND_WIREGUARD"
    }

    fn secret_kind(&self) -> &'static str {
        if self.credential_request.kind == "CREDENTIAL_KIND_KEY_PASSPHRASE" {
            KEY_PASSPHRASE
        } else {
            USER_PASSWORD
        }
    }

    fn connected_since(&self) -> Option<i64> {
        let since = self.status.connected_since.as_deref().filter(|_| self.connected())?;
        glib::DateTime::from_iso8601(since, None).ok().map(|time| time.to_unix())
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "PROFILE_KIND_OPENVPN" => "OpenVPN",
        "PROFILE_KIND_WIREGUARD" => "WireGuard",
        _ => "未知",
    }
}

fn state_label(profile: &Profile) -> &'static str {
    match profile.state() {
        "CONNECTED" => "已連線",
        "CONNECTING" => "正在連線…",
        "RECONNECTING" => "正在重新連線…",
        "DISCONNECTING" => "正在中斷連線…",
        "AWAITING_CREDENTIALS" => "等待登入資訊",
        "FAILED" => "連線失敗",
        "DISCONNECTED" => "未連線",
        _ => "未知",
    }
}

/// Every state has a glyph of its own and a word as well as a colour, the Plaitway app's.
fn state_glyph(profile: &Profile) -> &'static str {
    match profile.state() {
        "CONNECTED" => "plaitway-state-connected-symbolic",
        "CONNECTING" => "plaitway-state-connecting-symbolic",
        "RECONNECTING" => "plaitway-state-reconnecting-symbolic",
        "DISCONNECTING" => "plaitway-state-disconnecting-symbolic",
        "AWAITING_CREDENTIALS" => "plaitway-state-credentials-symbolic",
        "FAILED" => "plaitway-state-attention-symbolic",
        _ => "plaitway-state-idle-symbolic",
    }
}

fn state_tone(profile: &Profile) -> &'static str {
    match profile.state() {
        "CONNECTED" => "success",
        "CONNECTING" | "RECONNECTING" | "DISCONNECTING" | "AWAITING_CREDENTIALS" => "warning",
        "FAILED" => "error",
        _ => "dim-label",
    }
}

fn route_state(state: &str) -> &str {
    state.trim_start_matches("ROUTE_STATE_")
}

fn route_state_label(state: &str, shadowed_by: Option<&str>) -> String {
    match route_state(state) {
        "INSTALLED" => "已安裝".into(),
        "PENDING" => "等待中".into(),
        "SHADOWED" => shadowed_by.map_or("被遮蔽".into(), |name| format!("被 {name} 遮蔽")),
        "BLOCKED" => "被本機網路阻擋".into(),
        "FAILED" => "失敗".into(),
        _ => "未知".into(),
    }
}

/// A shadowed prefix is a profile standing by behind a higher priority one, as it is meant
/// to: neutral. A blocked one is a conflict worth knowing about.
fn route_tone(state: &str) -> &'static str {
    match route_state(state) {
        "INSTALLED" => "success",
        "PENDING" | "BLOCKED" => "warning",
        "FAILED" => "error",
        _ => "dim-label",
    }
}

fn route_kind_label(kind: &str) -> &'static str {
    match kind {
        "ROUTE_KIND_TUNNEL" => "通道",
        "ROUTE_KIND_BYPASS" => "旁路",
        "ROUTE_KIND_DEFAULT" => "預設路由",
        _ => "未知",
    }
}

const TUNNEL_MODES: [(&str, &str, &str); 3] = [
    ("TUNNEL_MODE_AUTO", "auto", "自動"),
    ("TUNNEL_MODE_FULL", "full", "完整通道"),
    ("TUNNEL_MODE_SPLIT", "split", "分割通道"),
];

/// Decimal units, as the file manager counts them.
fn format_bytes(count: u64) -> String {
    const UNITS: [&str; 7] = ["B", "kB", "MB", "GB", "TB", "PB", "EB"];
    if count < 1000 {
        return format!("{count} B");
    }
    let (mut value, mut unit) = (count as f64, 0);
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn format_uptime(seconds: i64) -> String {
    let total = seconds.max(0);
    format!("{}:{:02}:{:02}", total / 3600, total % 3600 / 60, total % 60)
}

fn format_endpoint(endpoint: &Endpoint) -> String {
    let text = if endpoint.port == 0 {
        endpoint.host.clone()
    } else {
        format!("{}:{}", endpoint.host, endpoint.port)
    };
    if endpoint.protocol.is_empty() {
        text
    } else {
        format!("{text} ({})", endpoint.protocol)
    }
}

/// A time the command printed, in local time, as `format` writes it.
fn local_time(time: Option<&str>, format: &str) -> String {
    time.and_then(|time| glib::DateTime::from_iso8601(time, None).ok())
        .and_then(|time| time.to_local().ok())
        .and_then(|time| time.format(format).ok())
        .map(String::from)
        .unwrap_or_default()
}

// ---- who holds which route ----

#[derive(Clone, Copy, PartialEq, Debug)]
enum Standing {
    Holds,
    /// A profile with a higher priority holds the prefix.
    Waits,
    Blocked,
    Failed,
    Pending,
}

/// The daemon puts in a default route as two halves, so that it outranks the one it replaces.
const DEFAULT_HALVES: [(&str, &str, &str); 2] =
    [("0.0.0.0/1", "128.0.0.0/1", "0.0.0.0/0"), ("::/1", "8000::/1", "::/0")];

/// What a connected profile asks for, with how it stands; nothing while it is not connected.
fn requests_of(profile: &Profile) -> Vec<(String, Standing)> {
    let mut requests: Vec<(String, Standing)> = profile
        .status
        .routes
        .iter()
        .map(|route| {
            let standing = match route_state(&route.state) {
                "INSTALLED" => Standing::Holds,
                "SHADOWED" => Standing::Waits,
                "BLOCKED" => Standing::Blocked,
                "FAILED" => Standing::Failed,
                _ => Standing::Pending,
            };
            (route.prefix.clone(), standing)
        })
        .collect();
    for (low, high, whole) in DEFAULT_HALVES {
        let low_at = requests.iter().position(|(prefix, _)| prefix == low);
        let high_at = requests.iter().position(|(prefix, _)| prefix == high);
        if let (Some(low_at), Some(high_at)) = (low_at, high_at) {
            if requests[low_at].1 == requests[high_at].1 {
                requests[low_at].0 = whole.to_string();
                requests.remove(high_at);
            }
        }
    }
    requests
}

/// One prefix and every connected profile asking for it, by index in priority order.
#[derive(Debug, PartialEq)]
struct Route {
    prefix: String,
    holder: Option<usize>,
    requests: Vec<(usize, Standing)>,
}

/// The default routes first, then the rest in the order the profiles name them.
fn routes_of(profiles: &[Profile]) -> Vec<Route> {
    let mut routes: Vec<Route> = Vec::new();
    for (index, profile) in profiles.iter().enumerate() {
        for (prefix, standing) in requests_of(profile) {
            let at = match routes.iter().position(|route| route.prefix == prefix) {
                Some(at) => at,
                None => {
                    routes.push(Route { prefix, holder: None, requests: Vec::new() });
                    routes.len() - 1
                }
            };
            routes[at].requests.push((index, standing));
            if standing == Standing::Holds {
                routes[at].holder = Some(index);
            }
        }
    }
    routes.sort_by_key(|route| !matches!(route.prefix.as_str(), "0.0.0.0/0" | "::/0"));
    routes
}

/// The prefixes a profile holds, and how many it asks for.
fn holds(profile: &Profile) -> (usize, usize) {
    let requests = requests_of(profile);
    let held = requests.iter().filter(|(_, standing)| *standing == Standing::Holds).count();
    (held, profile.summary.routes.len().max(requests.len()))
}

fn route_name(prefix: &str) -> String {
    match prefix {
        "0.0.0.0/0" => "預設路由".into(),
        "::/0" => "預設路由 (IPv6)".into(),
        _ => prefix.into(),
    }
}

/// Who holds a route and who waits behind it.
fn why(route: &Route, profiles: &[Profile]) -> String {
    let name = route_name(&route.prefix);
    let Some(holder) = route.holder else {
        return format!("{name}：沒有通道持有它");
    };
    let waiting: Vec<&str> = route
        .requests
        .iter()
        .filter(|(index, standing)| *index != holder && *standing == Standing::Waits)
        .map(|(index, _)| profiles[*index].name.as_str())
        .collect();
    if waiting.is_empty() {
        format!("{name}：由 {} 持有", profiles[holder].name)
    } else {
        format!("{name}：由 {} 持有，{} 在後面等待", profiles[holder].name, waiting.join(" · "))
    }
}

/// A tunnel's priority, counted from 1, and how it stands.
fn state_line(profile: &Profile, rank: usize) -> String {
    match profile.state() {
        "CONNECTED" => match holds(profile) {
            (_, 0) => format!("優先順序 {rank} · 已連線"),
            (held, asked) => format!("優先順序 {rank} · 持有 {held}/{asked} 條路由"),
        },
        "CONNECTING" | "RECONNECTING" => format!("優先順序 {rank} · 連線中"),
        "DISCONNECTING" => format!("優先順序 {rank} · 中斷連線中"),
        "AWAITING_CREDENTIALS" => format!("優先順序 {rank} · 需要登入資訊"),
        "FAILED" if !profile.last_error.is_empty() => profile.last_error.clone(),
        "FAILED" => format!("優先順序 {rank} · 失敗"),
        _ => format!("優先順序 {rank} · 未連線"),
    }
}

/// Under a profile's name: its kind, and how it stands, as the network page says it.
fn caption(profile: &Profile) -> String {
    let kind = kind_label(&profile.kind);
    match profile.state() {
        "CONNECTED" => match holds(profile) {
            (_, 0) => format!("{kind} · 已連線"),
            (held, asked) => format!("{kind} · 持有 {held}/{asked} 條路由"),
        },
        "DISCONNECTED" => kind.to_string(),
        _ => format!("{kind} · {}", state_label(profile)),
    }
}

// ---- the profiles, as the helper says they are ----

#[derive(Clone, Copy)]
struct Sample {
    at: f64,
    rx: u64,
    tx: u64,
}

#[derive(Default)]
struct Model {
    /// Highest priority first.
    profiles: RefCell<Vec<Profile>>,
    /// Why the helper cannot be used, from the watch that ended last; None while it answers.
    failure: RefCell<Option<String>>,
    loaded: Cell<bool>,
    listeners: RefCell<Vec<Box<dyn Fn() -> bool>>>,
    traffic: RefCell<HashMap<String, VecDeque<Sample>>>,
    /// The profiles whose request for credentials is being answered or asked about.
    asking: RefCell<HashSet<String>>,
}

impl Model {
    /// `update` runs at every change for as long as it returns true.
    fn listen(&self, update: impl Fn() -> bool + 'static) {
        self.listeners.borrow_mut().push(Box::new(update));
    }

    fn changed(&self) {
        // A listener may add another as it runs, as a page it opens does.
        let listeners = self.listeners.take();
        let mut kept: Vec<_> = listeners.into_iter().filter(|update| update()).collect();
        kept.append(&mut self.listeners.borrow_mut());
        self.listeners.replace(kept);
    }

    fn profile(&self, id: &str) -> Option<Profile> {
        self.profiles.borrow().iter().find(|profile| profile.id == id).cloned()
    }

    fn name_of(&self, id: &str) -> String {
        self.profile(id).map_or_else(|| id.to_string(), |profile| profile.name)
    }

    fn apply(&self, event: Event) {
        let mut profiles = self.profiles.borrow_mut();
        if let Some(snapshot) = event.snapshot {
            *profiles = snapshot.profiles;
        }
        if let Some(changed) = event.changed {
            match profiles.iter_mut().find(|profile| profile.id == changed.id) {
                Some(profile) => *profile = changed,
                None => profiles.push(changed),
            }
        }
        if let Some(removed) = event.removed {
            profiles.retain(|profile| profile.id != removed);
        }
        profiles.sort_by_key(|profile| profile.settings.priority);

        let now = glib::monotonic_time() as f64 / 1e6;
        let mut traffic = self.traffic.borrow_mut();
        traffic.retain(|id, _| profiles.iter().any(|profile| &profile.id == id && profile.connected()));
        for profile in profiles.iter().filter(|profile| profile.connected()) {
            let samples = traffic.entry(profile.id.clone()).or_default();
            let (rx, tx) = (profile.status.rx_bytes, profile.status.tx_bytes);
            if samples.back().is_some_and(|last| last.rx == rx && last.tx == tx && now - last.at < 1.0) {
                continue;
            }
            samples.push_back(Sample { at: now, rx, tx });
            while samples.front().is_some_and(|first| now - first.at > TRAFFIC_SPAN) {
                samples.pop_front();
            }
        }
    }

    /// Bytes a second in and out, between the last two samples.
    fn rate(&self, id: &str) -> Option<(f64, f64)> {
        let traffic = self.traffic.borrow();
        let samples = traffic.get(id)?;
        let count = samples.len();
        let (before, last) = (samples.get(count.checked_sub(2)?)?, samples.get(count - 1)?);
        let span = last.at - before.at;
        (span > 0.0).then(|| {
            (last.rx.saturating_sub(before.rx) as f64 / span, last.tx.saturating_sub(before.tx) as f64 / span)
        })
    }
}

// ---- running the command ----

/// Runs `plaitway args` with `env` and `input` on its standard input, never a terminal, so
/// that it asks nothing; `then` gets what it printed and what it said, or why it failed.
fn plaitway_with(
    args: &[&str],
    env: &[(&str, &str)],
    input: Option<String>,
    then: impl FnOnce(Result<(String, String), String>) + 'static,
) {
    let launcher = gio::SubprocessLauncher::new(
        gio::SubprocessFlags::STDIN_PIPE
            | gio::SubprocessFlags::STDOUT_PIPE
            | gio::SubprocessFlags::STDERR_PIPE,
    );
    for (name, value) in env {
        launcher.setenv(name, value, true);
    }
    let argv: Vec<&std::ffi::OsStr> =
        std::iter::once(CLI).chain(args.iter().copied()).map(std::ffi::OsStr::new).collect();
    let child = match launcher.spawn(&argv) {
        Ok(child) => child,
        Err(err) => return then(Err(err.to_string())),
    };
    let command = args.first().copied().unwrap_or_default().to_string();
    let waited = child.clone();
    child.communicate_utf8_async(Some(input.unwrap_or_default()), gio::Cancellable::NONE, move |result| {
        match result {
            Ok((out, err)) => {
                let (out, err) =
                    (out.map(String::from).unwrap_or_default(), err.map(String::from).unwrap_or_default());
                if waited.is_successful() {
                    then(Ok((out, err)))
                } else {
                    // Not a password, which goes through standard input.
                    eprintln!("plaitway {command}: {}", err.trim());
                    then(Err(said(&err)))
                }
            }
            Err(err) => then(Err(err.to_string())),
        }
    });
}

fn plaitway(args: &[&str], then: impl FnOnce(Result<String, String>) + 'static) {
    plaitway_with(args, &[], None, move |result| then(result.map(|(out, _)| out)));
}

/// What the command said went wrong, without its name before it.
fn said(err: &str) -> String {
    let lines: Vec<&str> = err
        .lines()
        .map(|line| line.trim().trim_start_matches("plaitway: "))
        .filter(|line| !line.is_empty() && !line.starts_with("Run \"plaitway help\""))
        .collect();
    lines.join("\n")
}

/// Runs a command that changes something; what went wrong is told under `title`.
fn change_profile(parent: &impl IsA<gtk::Widget>, title: String, args: &[&str]) {
    let parent = parent.as_ref().clone();
    plaitway(args, move |result| {
        if let Err(err) = result {
            alert::notice(&parent, &title, Some(&err));
        }
    });
}

/// Moves a profile to `place`, a place from 1 or up or down. The helper tells its watchers
/// nothing of the new order, so the profiles are read again once it is taken.
fn move_profile(parent: &impl IsA<gtk::Widget>, model: &Rc<Model>, id: &str, place: &str) {
    let (parent, model) = (parent.as_ref().clone(), model.clone());
    plaitway(&["move", id, place], move |result| {
        if let Err(err) = result {
            return alert::notice(&parent, "無法重新排序", Some(&err));
        }
        // The menu bar's VPN item reads the new order too.
        output("nekoland", &["msg", "vpn"]);
        plaitway(&["list", "-json"], move |listed| {
            match listed.map(|text| serde_json::from_str::<Snapshot>(&text)) {
                Ok(Ok(snapshot)) => {
                    model.apply(Event { snapshot: Some(snapshot), changed: None, removed: None });
                    model.changed();
                }
                Ok(Err(err)) => eprintln!("plaitway list: {err}"),
                Err(err) => eprintln!("plaitway list: {err}"),
            }
        });
    });
}

fn set_enabled(parent: &impl IsA<gtk::Widget>, profile: &Profile, on: bool) {
    if on {
        change_profile(
            parent,
            format!("無法連線到「{}」", profile.name),
            &["connect", "-no-wait", &profile.id],
        );
    } else {
        change_profile(parent, format!("無法中斷「{}」", profile.name), &["disconnect", &profile.id]);
    }
}

/// Follows the helper with `plaitway watch` while the page is there; a helper that is gone
/// or restarting ends the watch, which starts again.
fn follow(model: Rc<Model>, page: &adw::PreferencesPage) {
    let argv = [CLI, "watch", "-json"].map(std::ffi::OsStr::new);
    let child = match gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE,
    ) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("plaitway watch: {err}");
            return;
        }
    };
    let Some(events) = child.stdout_pipe() else { return };
    let stopped = child.clone();
    let destroyed = page.connect_destroy(move |_| stopped.force_exit());
    let lines = gio::DataInputStream::new(&events);
    let weak = page.downgrade();
    glib::spawn_future_local(async move {
        let mut last_said = String::new();
        while let Ok(Some(line)) = lines.read_line_utf8_future(glib::Priority::DEFAULT).await {
            let Some(page) = weak.upgrade() else { break };
            match serde_json::from_str::<Event>(&line) {
                Ok(event) => {
                    model.failure.take();
                    model.loaded.set(true);
                    model.apply(event);
                    model.changed();
                    review_credentials(&model, page.upcast_ref());
                }
                Err(_) => last_said = said(&line),
            }
        }
        child.force_exit();
        let Some(page) = weak.upgrade() else { return };
        page.disconnect(destroyed);
        if last_said.is_empty() {
            last_said = "the daemon closed the stream".into();
        }
        let told = model.failure.borrow().as_deref() == Some(last_said.as_str());
        if !told {
            model.profiles.borrow_mut().clear();
            model.failure.replace(Some(last_said));
            model.loaded.set(true);
            model.changed();
        }
        glib::timeout_add_seconds_local_once(2, move || follow(model, &page));
    });
}

// ---- credentials ----

/// The keyring items of the Plaitway app, under its libsecret schema.
mod keyring {
    use super::*;
    use crate::secrets;

    fn item(id: &str, kind: &str) -> secrets::Attributes {
        secrets::Attributes::from([
            ("xdg:schema".to_string(), SECRET_SCHEMA.to_string()),
            ("profile_id".to_string(), id.to_string()),
            ("kind".to_string(), kind.to_string()),
        ])
    }

    /// Whether anything is kept for the profile, of either kind.
    pub(super) async fn has(id: &str, kinds: &[&str]) -> bool {
        for kind in kinds {
            match secrets::has(&item(id, kind)).await {
                Ok(true) => return true,
                Ok(false) => {}
                Err(err) => eprintln!("keyring: {err}"),
            }
        }
        false
    }

    /// What is kept for the profile: its user name and password. A locked keyring asks for
    /// its own password, which `plaitway` does not do.
    pub(super) async fn load(id: &str, kind: &str) -> Option<(String, String)> {
        #[derive(Deserialize)]
        struct Kept {
            #[serde(default)]
            username: String,
            password: String,
        }
        match secrets::lookup(&item(id, kind)).await {
            Ok(Some(value)) => match serde_json::from_str::<Kept>(&value) {
                Ok(kept) => Some((kept.username, kept.password)),
                Err(err) => {
                    eprintln!("keyring: {id} ({kind}): {err}");
                    None
                }
            },
            Ok(None) => None,
            Err(err) => {
                if !err.matches(gio::IOErrorEnum::FailedHandled) {
                    eprintln!("keyring: {err}");
                }
                None
            }
        }
    }

    /// Keeps what was typed for the profile, as the Plaitway app keeps it.
    pub(super) async fn save(
        id: &str,
        kind: &str,
        username: &str,
        password: &str,
    ) -> Result<(), glib::Error> {
        let value = serde_json::json!({ "username": username, "password": password }).to_string();
        match secrets::store(&item(id, kind), &format!("Plaitway: {id} ({kind})"), &value).await {
            // The keyring's own prompt was turned down: nothing is kept, as was chosen.
            Err(err) if err.matches(gio::IOErrorEnum::FailedHandled) => Ok(()),
            stored => stored,
        }
    }

    /// Removes what is kept for the profile, of every kind.
    pub(super) async fn forget(id: &str, kinds: &[&str]) {
        for kind in kinds {
            if let Err(err) = secrets::clear(&item(id, kind)).await {
                eprintln!("keyring: {err}");
            }
        }
    }
}

/// Answers each profile that asks for credentials: with what the keyring keeps for it, else
/// with what is typed. `plaitway connect` stops connecting a profile whose answer the server
/// refuses, and is asked again with what it said.
fn review_credentials(model: &Rc<Model>, parent: &gtk::Widget) {
    let waiting: Vec<Profile> = model
        .profiles
        .borrow()
        .iter()
        .filter(|profile| profile.state() == "AWAITING_CREDENTIALS")
        .cloned()
        .collect();
    for profile in waiting {
        if !model.asking.borrow_mut().insert(profile.id.clone()) {
            continue;
        }
        let (model, parent) = (model.clone(), parent.clone());
        glib::spawn_future_local(async move {
            let kind = profile.secret_kind();
            let kept =
                if profile.last_error.is_empty() { keyring::load(&profile.id, kind).await } else { None };
            let Some((user, secret)) = kept else {
                let refused = Some(profile.last_error.clone()).filter(|err| !err.is_empty());
                return ask_credentials(model, parent, profile, refused);
            };
            let id = profile.id.clone();
            connect_as(&id, &user, &secret, move |result| match result {
                Ok(_) => {
                    model.asking.borrow_mut().remove(&profile.id);
                }
                Err(err) => {
                    glib::spawn_future_local(async move {
                        // A server that could not be reached did not refuse what is kept.
                        if !err.contains(FAILED_TO_CONNECT) {
                            keyring::forget(&profile.id, &[kind]).await;
                        }
                        answer_failed(model, parent, profile, err);
                    });
                }
            });
        });
    }
}

/// What `plaitway connect` says when the connection failed, not the credentials.
const FAILED_TO_CONNECT: &str = ": failed to connect";

/// After `plaitway connect` failed: a connection that failed is told, refused credentials
/// are asked for again.
fn answer_failed(model: Rc<Model>, parent: gtk::Widget, profile: Profile, err: String) {
    if err.contains(FAILED_TO_CONNECT) {
        model.asking.borrow_mut().remove(&profile.id);
        alert::notice(&parent, &format!("無法連線到「{}」", profile.name), Some(&err));
    } else {
        ask_credentials(model, parent, profile, Some(err));
    }
}

/// `plaitway connect` with the credentials given: the password on standard input, never in
/// the arguments, where other processes can read it.
fn connect_as(id: &str, username: &str, secret: &str, then: impl FnOnce(Result<String, String>) + 'static) {
    let mut args = vec!["connect"];
    if !username.is_empty() {
        args.extend(["-username", username]);
    }
    args.push(id);
    plaitway_with(&args, &[], Some(format!("{secret}\n")), move |result| then(result.map(|(out, _)| out)));
}

fn ask_credentials(model: Rc<Model>, parent: gtk::Widget, profile: Profile, refused: Option<String>) {
    let kind = profile.secret_kind();
    let asks_username = kind == USER_PASSWORD;
    let title = if asks_username {
        format!("輸入「{}」的密碼", profile.name)
    } else {
        format!("輸入「{}」的金鑰通行密語", profile.name)
    };
    let refused = refused.map(|err| match (err.contains("authentication failed"), asks_username) {
        (true, true) => "使用者名稱或密碼不正確".to_string(),
        (true, false) => "通行密語不正確".to_string(),
        (false, _) => err,
    });
    let dialog = adw::AlertDialog::new(Some(&title), refused.as_deref());
    let username = gtk::Entry::builder()
        .placeholder_text("使用者名稱")
        .width_request(alert::FIELD_WIDTH)
        .activates_default(true)
        .build();
    let password = gtk::PasswordEntry::builder()
        .placeholder_text(if asks_username { "密碼" } else { "通行密語" })
        .show_peek_icon(true)
        .width_request(alert::FIELD_WIDTH)
        .activates_default(true)
        .build();
    let fields = gtk::Box::new(gtk::Orientation::Vertical, 12);
    if asks_username {
        fields.append(&username);
    }
    fields.append(&password);
    dialog.set_extra_child(Some(&fields));
    dialog.add_responses(&[("cancel", "取消"), ("connect", "連線")]);
    dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("connect"));
    dialog.set_close_response("cancel");
    dialog.set_response_enabled("connect", false);
    let filled = glib::clone!(
        #[weak]
        dialog,
        #[weak]
        username,
        #[weak]
        password,
        move || {
            let ready = !password.text().is_empty() && (!asks_username || !username.text().trim().is_empty());
            dialog.set_response_enabled("connect", ready);
        }
    );
    let again = filled.clone();
    username.connect_changed(move |_| again());
    password.connect_changed(move |_| filled());
    let first: gtk::Widget =
        if asks_username { username.clone().upcast() } else { password.clone().upcast() };
    first.connect_map(|field| {
        let field = field.clone();
        glib::idle_add_local_once(move || {
            field.grab_focus();
        });
    });

    // Answered elsewhere, as by the Plaitway app, it is not asked here any more.
    let answered = Rc::new(Cell::new(false));
    let (shown, id, watched) = (dialog.downgrade(), profile.id.clone(), answered.clone());
    let still_asks = model.clone();
    model.listen(move || {
        let Some(dialog) = shown.upgrade() else {
            return false;
        };
        if watched.get() {
            return false;
        }
        if still_asks.profile(&id).is_none_or(|profile| profile.state() != "AWAITING_CREDENTIALS") {
            watched.set(true);
            still_asks.asking.borrow_mut().remove(&id);
            dialog.force_close();
            return false;
        }
        true
    });

    dialog.choose(&parent.clone(), gio::Cancellable::NONE, move |response| {
        if answered.replace(true) {
            return;
        }
        if response != "connect" {
            model.asking.borrow_mut().remove(&profile.id);
            return set_enabled(&parent, &profile, false);
        }
        let (user, secret) = (username.text().trim().to_string(), password.text().to_string());
        // The fields go with the dialog; they do not keep the secret.
        password.set_text("");
        let (id, name, typed) = (profile.id.clone(), user.clone(), secret.clone());
        connect_as(&id, &name, &typed, move |result| match result {
            Ok(_) => {
                model.asking.borrow_mut().remove(&profile.id);
                glib::spawn_future_local(async move {
                    if let Err(err) = keyring::save(&profile.id, kind, &user, &secret).await {
                        alert::notice(&parent, "無法儲存登入資訊", Some(err.message()));
                    }
                });
            }
            Err(err) => answer_failed(model, parent, profile, err),
        });
    });
}

// ---- the page ----

/// The VPN page: Plaitway's profiles, where Plaitway is installed, and NetworkManager's VPNs.
pub(crate) fn vpn_page() -> adw::PreferencesPage {
    let plaitway = installed();
    let page = if plaitway { plaitway_page() } else { adw::PreferencesPage::new() };
    network::follow_vpns(&page, !plaitway);
    page
}

fn plaitway_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let model = Rc::new(Model::default());

    let helper = group("");
    helper.set_visible(false);
    let helper_row: Rc<RefCell<Option<adw::ActionRow>>> = Rc::default();
    let profiles = group("設定檔");
    let disconnect_all = gtk::Button::builder()
        .label("全部中斷連線")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .visible(false)
        .build();
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("從檔案加入 VPN")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    add.connect_clicked(import_profiles);
    let shown = model.clone();
    disconnect_all.connect_clicked(move |button| {
        for profile in shown.profiles.borrow().iter().filter(|profile| profile.desired_enabled) {
            set_enabled(button, profile, false);
        }
    });
    profiles.set_header_suffix(Some(&line(0, &[disconnect_all.upcast_ref(), add.upcast_ref()])));
    let more = group("");
    more.add(&subpage_row("路由", String::new, routing_page(&model)));
    more.add(&subpage_row("診斷", String::new, diagnostics_page(&model)));
    for group in [&helper, &profiles, &more] {
        page.add(group);
    }

    let rows: Rc<RefCell<Vec<(String, ProfileRow)>>> = Rc::default();
    let empty = adw::ActionRow::builder().title("沒有設定檔").visible(false).build();
    profiles.add(&empty);
    let told: Rc<RefCell<Option<String>>> = Rc::default();
    let weak = page.downgrade();
    let shown = model.clone();
    let update = move || {
        if weak.upgrade().is_none() {
            return false;
        }
        let failure = shown.failure.borrow().clone();
        if *told.borrow() != failure {
            if let Some(row) = helper_row.take() {
                helper.remove(&row);
            }
            helper.set_visible(failure.is_some());
            if let Some(failure) = failure.as_deref() {
                let row = helper_status(failure);
                helper.add(&row);
                helper_row.replace(Some(row));
            }
            told.replace(failure.clone());
        }
        profiles.set_visible(failure.is_none() && shown.loaded.get());
        more.set_visible(failure.is_none() && shown.loaded.get());
        let list = shown.profiles.borrow().clone();
        disconnect_all.set_visible(list.iter().any(|profile| profile.desired_enabled));
        empty.set_visible(list.is_empty());
        let ids: Vec<&str> = list.iter().map(|profile| profile.id.as_str()).collect();
        let same = rows.borrow().iter().map(|(id, _)| id.as_str()).eq(ids.iter().copied());
        if !same {
            for (_, row) in rows.take() {
                profiles.remove(&row.row);
            }
            let made: Vec<(String, ProfileRow)> =
                list.iter().map(|profile| (profile.id.clone(), ProfileRow::new(&shown, profile))).collect();
            for (_, row) in &made {
                profiles.add(&row.row);
            }
            rows.replace(made);
        }
        for ((_, row), profile) in rows.borrow().iter().zip(&list) {
            row.show(profile);
        }
        true
    };
    update();
    model.listen(update);
    follow(model, &page);
    page
}

/// What stands in the way of the helper, and what can be done about it.
fn helper_status(failure: &str) -> adw::ActionRow {
    let state =
        output("systemctl", &["show", SERVICE, "--property=LoadState,ActiveState"]).unwrap_or_default();
    let value = |key: &str| field(&state, key, '=');
    let (title, body, action) = if failure.starts_with("refusing to use") {
        ("輔助程式不受信任", failure.to_string(), None)
    } else if failure.starts_with("permission denied") {
        ("權限不足", "只有主控台使用者與管理員可以使用輔助程式。".to_string(), None)
    } else if value("LoadState").as_deref() == Some("not-found") {
        ("未安裝輔助程式", "請安裝 plaitway 套件。".to_string(), None)
    } else if value("ActiveState").as_deref() != Some("active") {
        ("輔助程式未執行", failure.to_string(), Some(("啟動輔助程式", "start")))
    } else if failure.starts_with("the daemon is not running") {
        ("輔助程式無法使用", failure.to_string(), Some(("重新啟動輔助程式", "restart")))
    } else {
        ("執行中，但沒有回應。", failure.to_string(), Some(("重新啟動輔助程式", "restart")))
    };
    let row = adw::ActionRow::builder().title(title).subtitle(&body).subtitle_selectable(true).build();
    let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    icon.add_css_class("warning");
    row.add_prefix(&icon);
    if let Some((label, verb)) = action {
        let button = gtk::Button::builder().label(label).valign(gtk::Align::Center).build();
        button.connect_clicked(move |button| control_helper(button, verb));
        row.add_suffix(&button);
    }
    row
}

/// Starts or restarts `plaitwayd`, as root; restarting drops every tunnel, so it asks first.
fn control_helper(parent: &gtk::Button, verb: &'static str) {
    let run = {
        let parent = parent.clone();
        move || {
            let (reason, title) = if verb == "start" {
                ("啟動 Plaitway 輔助程式", "無法啟動輔助程式")
            } else {
                ("重新啟動 Plaitway 輔助程式", "無法重新啟動輔助程式")
            };
            as_root(reason, &["systemctl", verb, SERVICE], None, move |worked| {
                if !worked {
                    alert::notice(&parent, title, None);
                }
            });
        }
    };
    if verb == "start" {
        run();
    } else {
        alert::confirm(parent, "重新啟動輔助程式？", Some("已連線的設定檔將中斷連線。"), "重新啟動", run);
    }
}

/// A profile in the list: its state, its switch, and its page one level down.
struct ProfileRow {
    row: adw::ActionRow,
    glyph: gtk::Image,
    switch: gtk::Switch,
    updating: Rc<Cell<bool>>,
}

impl ProfileRow {
    fn new(model: &Rc<Model>, profile: &Profile) -> Self {
        let row = adw::ActionRow::builder().activatable(true).build();
        // A prefix goes before those added earlier: the handle, added last, is leftmost.
        let glyph = gtk::Image::new();
        row.add_prefix(&glyph);
        row.add_prefix(&gtk::Image::from_icon_name("list-drag-handle-symbolic"));
        // Dragged onto another, a profile takes its place in the order, as the input sources do.
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        let dragged = profile.id.clone();
        let grabbed = Rc::new(Cell::new((0, 0)));
        let held = grabbed.clone();
        drag.connect_prepare(move |_, x, y| {
            held.set((x as i32, y as i32));
            Some(gdk::ContentProvider::for_value(&dragged.to_value()))
        });
        drag.connect_drag_begin(glib::clone!(
            #[weak]
            row,
            move |_, drag| {
                let copy = adw::ActionRow::builder()
                    .title(row.title())
                    .subtitle(row.subtitle().unwrap_or_default())
                    .build();
                copy.add_prefix(&gtk::Image::from_icon_name("list-drag-handle-symbolic"));
                let card = gtk::ListBox::builder().css_classes(["boxed-list"]).build();
                card.set_size_request(row.width(), row.height());
                card.append(&copy);
                gtk::DragIcon::for_drag(drag).set_child(Some(&card));
                let (x, y) = grabbed.get();
                drag.set_hotspot(x, y);
            }
        ));
        row.add_controller(drag);
        let drop = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
        let (target, ordered) = (profile.id.clone(), model.clone());
        drop.connect_drop(move |drop, value, _, _| {
            let Ok(dragged) = value.get::<String>() else { return false };
            let place = {
                let profiles = ordered.profiles.borrow();
                // Text dragged from another program is not a profile.
                if !profiles.iter().any(|profile| profile.id == dragged) {
                    return false;
                }
                profiles.iter().position(|profile| profile.id == target)
            };
            let Some(place) = place else { return false };
            if dragged == target {
                return false;
            }
            let Some(row) = drop.widget() else { return false };
            move_profile(&row, &ordered, &dragged, &(place + 1).to_string());
            true
        });
        row.add_controller(drop);
        let switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
        row.add_suffix(&switch);
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let updating = Rc::new(Cell::new(false));
        let (id, shown, quiet) = (profile.id.clone(), model.clone(), updating.clone());
        switch.connect_active_notify(move |switch| {
            if let Some(profile) = shown.profile(&id).filter(|_| !quiet.get()) {
                set_enabled(switch, &profile, switch.is_active());
            }
        });
        let (id, shown) = (profile.id.clone(), model.clone());
        row.connect_activated(move |row| open_profile(row, &shown, &id));
        Self { row, glyph, switch, updating }
    }

    fn show(&self, profile: &Profile) {
        self.row.set_title(&glib::markup_escape_text(&profile.name));
        self.row.set_subtitle(&glib::markup_escape_text(&caption(profile)));
        show_glyph(&self.glyph, profile);
        self.updating.set(true);
        self.switch.set_active(profile.desired_enabled);
        self.updating.set(false);
        self.switch.set_sensitive(profile.state() != "DISCONNECTING");
        self.switch.update_property(&[gtk::accessible::Property::Label(&profile.name)]);
    }
}

fn show_glyph(glyph: &gtk::Image, profile: &Profile) {
    glyph.set_icon_name(Some(state_glyph(profile)));
    for tone in ["success", "warning", "error", "dim-label"] {
        glyph.remove_css_class(tone);
    }
    glyph.add_css_class(state_tone(profile));
    glyph.set_tooltip_text(Some(state_label(profile)));
}

/// Shows `content` one level down from `from`, under `title`, with `bar` under the header.
fn push(
    from: &impl IsA<gtk::Widget>,
    title: &str,
    bar: Option<&gtk::Widget>,
    content: &impl IsA<gtk::Widget>,
) -> Option<adw::NavigationPage> {
    let navigation =
        from.ancestor(adw::NavigationView::static_type()).and_downcast::<adw::NavigationView>()?;
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    if let Some(bar) = bar {
        view.add_top_bar(bar);
    }
    view.set_content(Some(content));
    let page = adw::NavigationPage::builder().title(title).child(&view).build();
    navigation.push(&page);
    Some(page)
}

/// The strip under a page's header with what only that page does.
fn page_bar(children: &[&gtk::Widget]) -> gtk::Box {
    let bar = line(6, children);
    bar.set_margin_start(12);
    bar.set_margin_end(12);
    bar.set_margin_top(6);
    bar.set_margin_bottom(6);
    bar
}

fn import_profiles(button: &gtk::Button) {
    let profiles = gtk::FileFilter::new();
    profiles.set_name(Some("VPN 設定檔"));
    profiles.add_suffix("conf");
    profiles.add_suffix("ovpn");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&profiles);
    let dialog = gtk::FileDialog::builder().title("選擇 VPN 設定檔").filters(&filters).build();
    let window = button.root().and_downcast::<gtk::Window>();
    let button = button.clone();
    dialog.open_multiple(window.as_ref(), gio::Cancellable::NONE, move |chosen| {
        let files = match chosen {
            Ok(files) => files,
            Err(err) if err.matches(gtk::DialogError::Dismissed) => return,
            Err(err) => return alert::notice(&button, "無法加入 VPN", Some(err.message())),
        };
        let paths: Vec<PathBuf> = (0..files.n_items())
            .filter_map(|at| files.item(at).and_downcast::<gio::File>())
            .filter_map(|file| file.path())
            .collect();
        if paths.is_empty() {
            return alert::notice(&button, "無法加入 VPN", Some("請選擇這部電腦上的檔案。"));
        }
        import_next(button, paths.into(), Vec::new());
    });
}

/// Imports the files one after another, then tells what went wrong or what was left out.
fn import_next(button: gtk::Button, mut paths: VecDeque<PathBuf>, mut report: Vec<(bool, String)>) {
    let Some(path) = paths.pop_front() else {
        if report.is_empty() {
            return;
        }
        let failed = report.iter().any(|(failed, _)| *failed);
        let body: Vec<String> = report.into_iter().map(|(_, line)| line).collect();
        let title = if failed { "無法加入 VPN" } else { "已略過部分內容" };
        return alert::notice(&button, title, Some(&body.join("\n")));
    };
    let file = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let credentials = auth_user_pass(&path);
    let shown = path.to_string_lossy().into_owned();
    plaitway(&["import", "-json", &shown], move |result| {
        match result.map(|out| serde_json::from_str::<ImportResponse>(&out)) {
            Ok(Ok(imported)) => {
                for warning in &imported.warnings {
                    report.push((false, format!("{file}：{}", warning.summary())));
                }
                // What the profile's auth-user-pass file holds is kept as typed credentials are.
                if let Some((username, password)) = credentials {
                    let (id, parent) = (imported.profile.id, button.clone());
                    glib::spawn_future_local(async move {
                        if let Err(err) = keyring::save(&id, USER_PASSWORD, &username, &password).await {
                            alert::notice(&parent, "無法儲存登入資訊", Some(err.message()));
                        }
                    });
                }
            }
            Ok(Err(err)) => report.push((true, format!("{file}：{err}"))),
            Err(err) => report.push((true, format!("{file}：{err}"))),
        }
        import_next(button, paths, report);
    });
}

/// The user name and password of an OpenVPN profile's `auth-user-pass file`, read only from
/// the profile's own folder: a profile is untrusted, and must not have another file sent.
fn auth_user_pass(path: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let named = text.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next()?.eq_ignore_ascii_case("auth-user-pass")).then(|| words.next()).flatten()
    })?;
    let folder = path.parent()?.canonicalize().ok()?;
    let file = folder.join(named.trim_matches(['"', '\''])).canonicalize().ok()?;
    if !file.starts_with(&folder) {
        return None;
    }
    let held = std::fs::read_to_string(file).ok()?;
    let mut lines = held.lines();
    let (username, password) = (lines.next()?.trim().to_string(), lines.next()?.trim_end().to_string());
    (!username.is_empty() && !password.is_empty()).then_some((username, password))
}

// ---- a profile's page ----

fn property_row(title: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle_selectable(true)
        .subtitle_lines(0)
        .css_classes(["property"])
        .build()
}

fn open_profile(from: &adw::ActionRow, model: &Rc<Model>, id: &str) {
    let Some(profile) = model.profile(id) else { return };
    let page = adw::PreferencesPage::new();
    let Some(shown_page) = push(from, &profile.name, None, &page) else {
        return;
    };

    // The state, why it is so, how long it has been up, and the switch.
    let hero = group("");
    let state = adw::ActionRow::builder().title_lines(0).subtitle_lines(0).subtitle_selectable(true).build();
    let glyph = gtk::Image::builder().pixel_size(24).build();
    state.add_prefix(&glyph);
    let uptime = gtk::Label::builder().css_classes(["numeric", "dim-label"]).build();
    let retry = gtk::Button::builder().label("重試").valign(gtk::Align::Center).build();
    let switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    for suffix in [uptime.upcast_ref::<gtk::Widget>(), retry.upcast_ref(), switch.upcast_ref()] {
        state.add_suffix(suffix);
    }
    hero.add(&state);
    page.add(&hero);

    let attention = group("需要注意");
    page.add(&attention);

    let traffic = group("");
    let received = property_row("已接收");
    let sent = property_row("已傳送");
    let (received_rate, sent_rate) = (rate_label(), rate_label());
    received.add_prefix(&gtk::Image::from_icon_name("plaitway-arrow-down-symbolic"));
    sent.add_prefix(&gtk::Image::from_icon_name("plaitway-arrow-up-symbolic"));
    received.add_suffix(&received_rate);
    sent.add_suffix(&sent_rate);
    traffic.add(&received);
    traffic.add(&sent);
    let graph = traffic_graph(model, id);
    traffic.add(&graph);
    page.add(&traffic);

    let details = group("");
    let remote = property_row("遠端");
    let interface = property_row("介面");
    let addresses = property_row("位址");
    let dns = property_row("DNS");
    let public_key = property_row("公開金鑰");
    let copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text("拷貝")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    public_key.add_suffix(&copy);
    for row in [&remote, &interface, &addresses, &dns, &public_key] {
        details.add(row);
    }
    page.add(&details);

    let more = group("");
    let routes_row = navigation_row("路由與 DNS");
    let logs_row = navigation_row("記錄");
    let configuration_row = navigation_row("組態");
    for row in [&routes_row, &logs_row, &configuration_row] {
        more.add(row);
    }
    page.add(&more);

    let behaviour = group("設定");
    let name = adw::EntryRow::builder().title("名稱").show_apply_button(true).build();
    let auto_connect = adw::SwitchRow::builder().title("自動連線").build();
    let modes: Vec<&str> = TUNNEL_MODES.iter().map(|(_, _, label)| *label).collect();
    let tunnel_mode = adw::ComboRow::builder().title("通道模式").model(&gtk::StringList::new(&modes)).build();
    let exclude_private = adw::SwitchRow::builder().title("排除私有 IP").build();
    let priority = adw::ActionRow::builder().title("優先順序").build();
    let position = gtk::Label::builder().css_classes(["numeric"]).build();
    let up = icon_button("go-up-symbolic", "上移");
    let down = icon_button("go-down-symbolic", "下移");
    for suffix in [position.upcast_ref::<gtk::Widget>(), up.upcast_ref(), down.upcast_ref()] {
        priority.add_suffix(suffix);
    }
    behaviour.add(&name);
    behaviour.add(&auto_connect);
    behaviour.add(&tunnel_mode);
    behaviour.add(&exclude_private);
    behaviour.add(&priority);
    page.add(&behaviour);

    let on_demand = group("隨選連線");
    let ethernet = adw::SwitchRow::builder().title("乙太網路").build();
    let wifi = adw::SwitchRow::builder().title("Wi-Fi").build();
    on_demand.add(&ethernet);
    on_demand.add(&wifi);
    page.add(&on_demand);

    let actions = group("");
    let forget =
        adw::ActionRow::builder().title("移除已儲存的登入資訊").activatable(true).visible(false).build();
    let remove =
        adw::ActionRow::builder().title("移除設定檔").activatable(true).css_classes(["error"]).build();
    actions.add(&forget);
    actions.add(&remove);
    page.add(&actions);

    let updating = Rc::new(Cell::new(false));
    let since: Rc<Cell<Option<i64>>> = Rc::default();
    let id = id.to_string();

    // What is typed or switched goes to the helper; its watch puts the page right after.
    let changed_setting = {
        let (model, id, updating) = (model.clone(), id.clone(), updating.clone());
        move |widget: &gtk::Widget, flag: String| {
            if updating.get() {
                return;
            }
            if let Some(profile) = model.profile(&id) {
                change_profile(widget, "無法儲存".into(), &["set", &flag, &profile.id]);
            }
        }
    };
    let (setting, ids) = (changed_setting.clone(), id.clone());
    let named = model.clone();
    let commit_name = move |row: &adw::EntryRow| {
        let typed = row.text().trim().to_string();
        let Some(profile) = named.profile(&ids) else {
            return;
        };
        if typed.is_empty() {
            return row.set_text(&profile.name);
        }
        if typed != profile.name {
            setting(row.upcast_ref(), format!("-name={typed}"));
        }
    };
    name.connect_apply(commit_name);
    let setting = changed_setting.clone();
    auto_connect.connect_active_notify(move |row| {
        setting(row.upcast_ref(), format!("-auto-connect={}", row.is_active()))
    });
    let setting = changed_setting.clone();
    tunnel_mode.connect_selected_notify(move |row| {
        if let Some((_, flag, _)) = TUNNEL_MODES.get(row.selected() as usize) {
            setting(row.upcast_ref(), format!("-tunnel-mode={flag}"));
        }
    });
    let setting = changed_setting.clone();
    exclude_private.connect_active_notify(move |row| {
        setting(row.upcast_ref(), format!("-exclude-private-ips={}", row.is_active()))
    });
    for row in [&ethernet, &wifi] {
        let (setting, ethernet, wifi) = (changed_setting.clone(), ethernet.downgrade(), wifi.downgrade());
        row.connect_active_notify(move |row| {
            let (Some(ethernet), Some(wifi)) = (ethernet.upgrade(), wifi.upgrade()) else {
                return;
            };
            let networks: Vec<&str> = [(ethernet.is_active(), "ethernet"), (wifi.is_active(), "wifi")]
                .into_iter()
                .filter_map(|(on, network)| on.then_some(network))
                .collect();
            let networks = if networks.is_empty() { "off".to_string() } else { networks.join(",") };
            setting(row.upcast_ref(), format!("-on-demand={networks}"));
        });
    }
    for (button, place) in [(&up, "up"), (&down, "down")] {
        let (ids, moved) = (id.clone(), model.clone());
        button.connect_clicked(move |button| move_profile(button, &moved, &ids, place));
    }
    let (toggled, ids, quiet) = (model.clone(), id.clone(), updating.clone());
    switch.connect_active_notify(move |switch| {
        if let Some(profile) = toggled.profile(&ids).filter(|_| !quiet.get()) {
            set_enabled(switch, &profile, switch.is_active());
        }
    });
    let (retried, ids) = (model.clone(), id.clone());
    retry.connect_clicked(move |button| {
        if let Some(profile) = retried.profile(&ids) {
            set_enabled(button, &profile, true);
        }
    });
    let (copied, ids) = (model.clone(), id.clone());
    copy.connect_clicked(move |button| {
        if let Some(profile) = copied.profile(&ids) {
            button.clipboard().set_text(&profile.summary.public_key);
        }
    });
    let (opened, ids) = (model.clone(), id.clone());
    routes_row.connect_activated(move |row| profile_routes(row, &opened, &ids));
    let (opened, ids) = (model.clone(), id.clone());
    logs_row
        .connect_activated(move |row| logs_page(row, &format!("「{}」的記錄", opened.name_of(&ids)), &ids));
    let (opened, ids) = (model.clone(), id.clone());
    configuration_row.connect_activated(move |row| configuration_page(row, &opened, &ids));
    let (ids, shown_forget) = (id.clone(), forget.downgrade());
    forget.connect_activated(move |_| {
        let (ids, shown_forget) = (ids.clone(), shown_forget.clone());
        glib::spawn_future_local(async move {
            keyring::forget(&ids, &[USER_PASSWORD, KEY_PASSPHRASE]).await;
            if let Some(row) = shown_forget.upgrade() {
                row.set_visible(false);
            }
        });
    });
    let (removed, ids) = (model.clone(), id.clone());
    remove.connect_activated(move |row| {
        let Some(profile) = removed.profile(&ids) else {
            return;
        };
        let (parent, id) = (row.clone(), profile.id.clone());
        alert::confirm(
            row,
            &format!("移除「{}」？", profile.name),
            Some("設定檔及其已儲存的登入資訊將被移除。"),
            "移除",
            move || {
                let target = id.clone();
                plaitway(&["remove", &target], move |result| match result {
                    Ok(_) => {
                        glib::spawn_future_local(async move {
                            keyring::forget(&id, &[USER_PASSWORD, KEY_PASSPHRASE]).await;
                        });
                    }
                    Err(err) => alert::notice(&parent, "無法移除設定檔", Some(&err)),
                });
            },
        );
    });
    let (ids, shown_forget) = (id.clone(), forget.downgrade());
    page.connect_map(move |_| {
        let (ids, shown_forget) = (ids.clone(), shown_forget.clone());
        glib::spawn_future_local(async move {
            let kept = keyring::has(&ids, &[USER_PASSWORD, KEY_PASSPHRASE]).await;
            if let Some(row) = shown_forget.upgrade() {
                row.set_visible(kept);
            }
        });
    });

    let ticking = since.clone();
    let shown_uptime = uptime.downgrade();
    glib::timeout_add_seconds_local(1, move || {
        let Some(uptime) = shown_uptime.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if let Some(since) = ticking.get() {
            let now = glib::DateTime::now_utc().map(|now| now.to_unix()).unwrap_or(since);
            uptime.set_label(&format_uptime(now - since));
        }
        glib::ControlFlow::Continue
    });

    let attention_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    let shown = model.clone();
    let (weak, shown_page) = (page.downgrade(), shown_page.downgrade());
    let update = move || {
        let Some(page) = weak.upgrade() else { return false };
        let Some(profile) = shown.profile(&id) else {
            // Removed: what showed it goes back to the list.
            if let Some(navigation) =
                page.ancestor(adw::NavigationView::static_type()).and_downcast::<adw::NavigationView>()
            {
                navigation.pop_to_tag(SHOWN_PAGE);
            }
            return false;
        };
        if let Some(shown_page) = shown_page.upgrade() {
            shown_page.set_title(&profile.name);
        }
        show_glyph(&glyph, &profile);
        state.set_title(state_label(&profile));
        state.set_subtitle(&glib::markup_escape_text(&profile.last_error));
        retry.set_visible(profile.state() == "FAILED");
        since.set(profile.connected_since());
        uptime.set_visible(since.get().is_some());
        if let Some(started) = since.get() {
            let now = glib::DateTime::now_utc().map(|now| now.to_unix()).unwrap_or(started);
            uptime.set_label(&format_uptime(now - started));
        }
        updating.set(true);
        switch.set_active(profile.desired_enabled);
        switch.set_sensitive(profile.state() != "DISCONNECTING");
        switch.update_property(&[gtk::accessible::Property::Label(&profile.name)]);

        for row in attention_rows.take() {
            attention.remove(&row);
        }
        let lost = profile
            .status
            .routes
            .iter()
            .filter(|route| matches!(route_state(&route.state), "BLOCKED" | "FAILED"))
            .count();
        let mut rows = Vec::new();
        if lost > 0 {
            let row = navigation_row("未生效的路由");
            row.add_prefix(&warning_icon());
            row.add_suffix(
                &gtk::Label::builder().label(lost.to_string()).css_classes(["numeric", "dim-label"]).build(),
            );
            let (opened, ids) = (shown.clone(), profile.id.clone());
            row.connect_activated(move |row| profile_routes(row, &opened, &ids));
            rows.push(row);
        }
        for warning in &profile.status.warnings {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(warning))
                .title_lines(0)
                .title_selectable(true)
                .build();
            row.add_prefix(&warning_icon());
            rows.push(row);
        }
        for row in &rows {
            attention.add(row);
        }
        attention.set_visible(!rows.is_empty());
        attention_rows.replace(rows);

        traffic.set_visible(profile.connected());
        let rate = shown.rate(&profile.id);
        let per_second = |bytes: f64| format!("{}/秒", format_bytes(bytes.round() as u64));
        received_rate.set_label(&rate.map_or("–".into(), |(rx, _)| per_second(rx)));
        sent_rate.set_label(&rate.map_or("–".into(), |(_, tx)| per_second(tx)));
        received.set_subtitle(&format_bytes(profile.status.rx_bytes));
        sent.set_subtitle(&format_bytes(profile.status.tx_bytes));
        graph.queue_draw();

        // The endpoint in use, or the ones the profile names while it is not connected.
        let named_remotes: Vec<String> = profile.summary.endpoints.iter().map(format_endpoint).collect();
        let remote_text = if profile.status.remote.is_empty() {
            named_remotes.join("\n")
        } else {
            profile.status.remote.clone()
        };
        let in_use = if profile.status.addresses.is_empty() {
            &profile.summary.addresses
        } else {
            &profile.status.addresses
        };
        for (row, value) in [
            (&remote, remote_text),
            (&interface, profile.status.interface_name.clone()),
            (&addresses, in_use.join(", ")),
            (&dns, profile.summary.dns_servers.join(", ")),
            (&public_key, profile.summary.public_key.clone()),
        ] {
            row.set_visible(!value.is_empty());
            row.set_subtitle(&glib::markup_escape_text(&value));
        }
        details.set_visible(
            [&remote, &interface, &addresses, &dns, &public_key].iter().any(|row| row.is_visible()),
        );

        // What is being typed stays until it is applied.
        if !name.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN) {
            name.set_text(&profile.name);
        }
        auto_connect.set_active(profile.settings.auto_connect);
        let mode =
            TUNNEL_MODES.iter().position(|(value, _, _)| *value == profile.settings.tunnel_mode).unwrap_or(0);
        tunnel_mode.set_selected(mode as u32);
        // The tunnel mode and the address ranges are read as a connection starts.
        let later = if profile.idle() { "" } else { "下次連線時生效" };
        tunnel_mode.set_subtitle(later);
        exclude_private.set_visible(profile.wireguard());
        exclude_private.set_active(profile.settings.exclude_private_ips);
        exclude_private.set_subtitle(later);
        ethernet.set_active(profile.settings.on_demand.ethernet);
        wifi.set_active(profile.settings.on_demand.wifi);
        let profiles = shown.profiles.borrow();
        let rank = profiles.iter().position(|other| other.id == profile.id).unwrap_or(0);
        position.set_label(&(rank + 1).to_string());
        up.set_sensitive(rank > 0);
        down.set_sensitive(rank + 1 < profiles.len());
        updating.set(false);
        true
    };
    update();
    model.listen(update);
}

fn navigation_row(title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).activatable(true).build();
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}

fn rate_label() -> gtk::Label {
    gtk::Label::builder().css_classes(["numeric"]).build()
}

fn warning_icon() -> gtk::Image {
    let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    icon.add_css_class("warning");
    icon
}

/// The rates in and out over the last two minutes: in solid, out dashed.
fn traffic_graph(model: &Rc<Model>, id: &str) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_height(64).margin_top(12).margin_bottom(6).build();
    let (model, id) = (Rc::downgrade(model), id.to_string());
    area.set_draw_func(move |area, cr, width, height| {
        let Some(model) = model.upgrade() else { return };
        let traffic = model.traffic.borrow();
        let Some(samples) = traffic.get(&id) else { return };
        let samples: Vec<Sample> = samples.iter().copied().collect();
        let rates: Vec<(f64, f64, f64)> = samples
            .windows(2)
            .filter(|pair| pair[1].at > pair[0].at)
            .map(|pair| {
                let span = pair[1].at - pair[0].at;
                (
                    pair[1].at,
                    pair[1].rx.saturating_sub(pair[0].rx) as f64 / span,
                    pair[1].tx.saturating_sub(pair[0].tx) as f64 / span,
                )
            })
            .collect();
        let Some(&(newest, ..)) = rates.last() else { return };
        let top = rates.iter().map(|(_, rx, tx)| rx.max(*tx)).fold(1.0, f64::max);
        let (width, height) = (f64::from(width), f64::from(height));
        let ink = area.color();
        let x = |at: f64| width - (newest - at) / TRAFFIC_SPAN * width;
        let y = |rate: f64| height - 1.5 - rate / top * (height - 3.0);
        for (column, alpha, dashed) in [(0usize, 0.9, false), (1, 0.5, true)] {
            cr.new_path();
            for (at, rx, tx) in &rates {
                cr.line_to(x(*at), y(if column == 0 { *rx } else { *tx }));
            }
            cr.set_source_rgba(f64::from(ink.red()), f64::from(ink.green()), f64::from(ink.blue()), alpha);
            cr.set_line_width(1.5);
            cr.set_dash(if dashed { &[4.0, 3.0] } else { &[] }, 0.0);
            if let Err(err) = cr.stroke() {
                eprintln!("traffic graph: {err}");
            }
        }
    });
    area
}

/// Every route and DNS entry the profile asked for, whether it is in, and why not.
fn profile_routes(from: &adw::ActionRow, model: &Rc<Model>, id: &str) {
    let page = adw::PreferencesPage::new();
    if push(from, "路由與 DNS", None, &page).is_none() {
        return;
    }
    let groups: Rc<RefCell<Vec<adw::PreferencesGroup>>> = Rc::default();
    let (shown, id, weak) = (model.clone(), id.to_string(), page.downgrade());
    let last: RefCell<Option<Profile>> = RefCell::default();
    let update = move || {
        let Some(page) = weak.upgrade() else { return false };
        let Some(profile) = shown.profile(&id) else {
            return false;
        };
        let same = last.borrow().as_ref().is_some_and(|before| {
            before.status.routes == profile.status.routes
                && before.status.dns == profile.status.dns
                && before.summary.routes == profile.summary.routes
        });
        if same {
            return true;
        }
        last.replace(Some(profile.clone()));
        for group in groups.take() {
            page.remove(&group);
        }
        let routes = group("路由");
        if !profile.status.routes.is_empty() {
            for route in &profile.status.routes {
                let shadowed_by = (!route.shadowed_by.is_empty()).then(|| shown.name_of(&route.shadowed_by));
                let row = adw::ActionRow::builder()
                    .title(format!("<tt>{}</tt>", glib::markup_escape_text(&route.prefix)))
                    .subtitle(glib::markup_escape_text(&route.detail))
                    .subtitle_lines(0)
                    .title_selectable(true)
                    .build();
                row.add_suffix(&status_label(
                    &route.state,
                    &route_state_label(&route.state, shadowed_by.as_deref()),
                ));
                routes.add(&row);
            }
        } else {
            // Not connected: the prefixes the profile names itself.
            for prefix in &profile.summary.routes {
                let row = adw::ActionRow::builder()
                    .title(format!("<tt>{}</tt>", glib::markup_escape_text(prefix)))
                    .title_selectable(true)
                    .build();
                routes.add(&row);
            }
        }
        if profile.status.routes.is_empty() && profile.summary.routes.is_empty() {
            routes.add(&adw::ActionRow::builder().title("沒有路由").build());
        }
        let mut made = vec![routes];
        if !profile.status.dns.is_empty() {
            let dns = group("DNS");
            for entry in &profile.status.dns {
                // "." is the helper's way of saying every domain.
                let domains: Vec<&str> = entry
                    .match_domains
                    .iter()
                    .map(|domain| if domain == "." { "所有網域" } else { domain.as_str() })
                    .collect();
                let subtitle = [domains.join(", "), entry.detail.clone()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&entry.servers.join(", ")))
                    .subtitle(glib::markup_escape_text(&subtitle))
                    .subtitle_lines(0)
                    .build();
                row.add_suffix(&status_label(&entry.state, &route_state_label(&entry.state, None)));
                dns.add(&row);
            }
            made.push(dns);
        }
        for group in &made {
            page.add(group);
        }
        groups.replace(made);
        true
    };
    update();
    model.listen(update);
}

/// A route's state as a word with its glyph, coloured by its tone.
fn status_label(state: &str, label: &str) -> gtk::Box {
    let glyph = match route_state(state) {
        "INSTALLED" => "object-select-symbolic",
        "PENDING" => "content-loading-symbolic",
        "SHADOWED" => "list-remove-symbolic",
        "BLOCKED" => "dialog-warning-symbolic",
        "FAILED" => "dialog-error-symbolic",
        _ => "dialog-question-symbolic",
    };
    let icon = gtk::Image::from_icon_name(glyph);
    let words = gtk::Label::new(Some(label));
    let shown = line(6, &[icon.upcast_ref(), words.upcast_ref()]);
    shown.set_valign(gtk::Align::Center);
    shown.add_css_class(route_tone(state));
    shown
}

// ---- routing ----

/// Every prefix a connected profile asks for, who holds it and who waits behind, and the
/// tunnels in priority order.
fn routing_page(model: &Rc<Model>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let groups: Rc<RefCell<Vec<adw::PreferencesGroup>>> = Rc::default();
    let signature: RefCell<String> = RefCell::default();
    let (shown, weak) = (model.clone(), page.downgrade());
    let update = move || {
        let Some(page) = weak.upgrade() else { return false };
        let profiles = shown.profiles.borrow().clone();
        let now = profiles
            .iter()
            .map(|profile| {
                let routes: Vec<String> = profile
                    .status
                    .routes
                    .iter()
                    .map(|route| format!("{}{}", route.prefix, route.state))
                    .collect();
                format!(
                    "{}{}{}{}{}",
                    profile.id,
                    profile.name,
                    profile.desired_enabled,
                    profile.state,
                    routes.join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(";");
        if *signature.borrow() == now {
            return true;
        }
        signature.replace(now);
        for group in groups.take() {
            page.remove(&group);
        }
        let routes = group("路由");
        let found = routes_of(&profiles);
        for route in &found {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&route_name(&route.prefix)))
                .subtitle(glib::markup_escape_text(&why(route, &profiles)))
                .subtitle_lines(0)
                .build();
            let icon =
                if route.holder.is_some() { "object-select-symbolic" } else { "dialog-warning-symbolic" };
            let icon = gtk::Image::from_icon_name(icon);
            icon.add_css_class(if route.holder.is_some() { "success" } else { "warning" });
            row.add_prefix(&icon);
            routes.add(&row);
        }
        if found.is_empty() {
            routes.add(&adw::ActionRow::builder().title("還沒有路由被要求").build());
        }
        let tunnels = group("通道");
        for (rank, profile) in profiles.iter().enumerate() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&profile.name))
                .subtitle(glib::markup_escape_text(&state_line(profile, rank + 1)))
                .build();
            let glyph = gtk::Image::new();
            show_glyph(&glyph, profile);
            row.add_prefix(&glyph);
            let button = gtk::Button::builder()
                .label(if profile.desired_enabled { "中斷連線" } else { "連線" })
                .valign(gtk::Align::Center)
                .sensitive(profile.state() != "DISCONNECTING")
                .build();
            let (on, profile) = (!profile.desired_enabled, profile.clone());
            button.connect_clicked(move |button| set_enabled(button, &profile, on));
            row.add_suffix(&button);
            tunnels.add(&row);
        }
        if profiles.is_empty() {
            tunnels.add(&adw::ActionRow::builder().title("沒有設定檔").build());
        }
        for group in [&routes, &tunnels] {
            page.add(group);
        }
        groups.replace(vec![routes, tunnels]);
        true
    };
    update();
    model.listen(update);
    page
}

// ---- diagnostics ----

/// The helper, the network, the routes the helper owns, what is stale, and the helper's own
/// log; read again every two seconds while it is shown.
fn diagnostics_page(model: &Rc<Model>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let actions = group("");
    let copy = gtk::Button::builder().label("拷貝報告").valign(gtk::Align::Center).sensitive(false).build();
    let resync = gtk::Button::builder().label("重新同步").valign(gtk::Align::Center).build();
    actions.set_header_suffix(Some(&line(6, &[copy.upcast_ref(), resync.upcast_ref()])));
    let helper_log = navigation_row("輔助程式記錄");
    helper_log.connect_activated(|row| logs_page(row, "輔助程式記錄", ""));
    let restart = adw::ActionRow::builder().title("重新啟動輔助程式").activatable(true).build();
    restart.connect_activated(|row| {
        let parent = row.clone();
        alert::confirm(
            row,
            "重新啟動輔助程式？",
            Some("已連線的設定檔將中斷連線。"),
            "重新啟動",
            move || {
                as_root(
                    "重新啟動 Plaitway 輔助程式",
                    &["systemctl", "restart", SERVICE],
                    None,
                    move |worked| {
                        if !worked {
                            alert::notice(&parent, "無法重新啟動輔助程式", None);
                        }
                    },
                );
            },
        );
    });
    actions.add(&helper_log);
    actions.add(&restart);
    page.add(&actions);

    let shown: Rc<RefCell<(String, Vec<adw::PreferencesGroup>)>> = Rc::default();
    let report: Rc<RefCell<String>> = Rc::default();
    // It reads itself again, once a stale route is removed.
    let refresh: Rc<RefCell<Rc<dyn Fn()>>> = Rc::new(RefCell::new(Rc::new(|| {})));
    *refresh.borrow_mut() = {
        let (model, weak, shown, report, copy, again) = (
            model.clone(),
            page.downgrade(),
            shown.clone(),
            report.clone(),
            copy.downgrade(),
            Rc::downgrade(&refresh),
        );
        Rc::new(move || {
            let (model, weak, shown, report, copy, again) =
                (model.clone(), weak.clone(), shown.clone(), report.clone(), copy.clone(), again.clone());
            plaitway(&["diagnostics", "-json"], move |result| {
                let Some(page) = weak.upgrade() else { return };
                let text = result.unwrap_or_else(|err| err);
                if shown.borrow().0 == text {
                    return;
                }
                for group in std::mem::take(&mut shown.borrow_mut().1) {
                    page.remove(&group);
                }
                let groups = match serde_json::from_str::<Diagnostics>(&text) {
                    Ok(diagnostics) => {
                        report.replace(report_text(&diagnostics, &model));
                        let refresh: Rc<dyn Fn()> = Rc::new(move || {
                            if let Some(refresh) = again.upgrade() {
                                let refresh = refresh.borrow().clone();
                                refresh();
                            }
                        });
                        diagnostics_groups(&diagnostics, &model, refresh)
                    }
                    Err(_) => {
                        report.take();
                        let failed = group("");
                        failed.add(
                            &adw::ActionRow::builder()
                                .title("輔助程式無法使用")
                                .subtitle(glib::markup_escape_text(&text))
                                .build(),
                        );
                        vec![failed]
                    }
                };
                if let Some(copy) = copy.upgrade() {
                    copy.set_sensitive(!report.borrow().is_empty());
                }
                for group in &groups {
                    page.add(group);
                }
                *shown.borrow_mut() = (text, groups);
            });
        })
    };
    copy.connect_clicked(move |button| button.clipboard().set_text(&report.borrow()));
    let refresh = refresh.borrow().clone();
    let again = refresh.clone();
    resync.connect_clicked(move |button| {
        button.set_sensitive(false);
        let (button, again) = (button.clone(), again.clone());
        plaitway(&["resync"], move |result| {
            button.set_sensitive(true);
            if let Err(err) = result {
                alert::notice(&button, "無法重新同步", Some(&err));
            }
            again();
        });
    });
    let ticker: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let (started, again) = (ticker.clone(), refresh.clone());
    page.connect_map(move |_| {
        again();
        let again = again.clone();
        let source = glib::timeout_add_seconds_local(2, move || {
            again();
            glib::ControlFlow::Continue
        });
        if let Some(old) = started.replace(Some(source)) {
            old.remove();
        }
    });
    page.connect_unmap(move |_| {
        if let Some(source) = ticker.take() {
            source.remove();
        }
    });
    page
}

fn diagnostics_groups(
    diagnostics: &Diagnostics,
    model: &Rc<Model>,
    refresh: Rc<dyn Fn()>,
) -> Vec<adw::PreferencesGroup> {
    let named = |owner: &str| model.name_of(owner);
    let mut groups = Vec::new();

    let daemon = &diagnostics.daemon;
    let helper = group("輔助程式");
    helper.add(&fact_row("輔助程式版本", &daemon.version));
    helper.add(&fact_row("權限", if daemon.privileged { "完整" } else { "受限" }));
    for engine in &daemon.engines {
        let value = if engine.available { engine.version.as_str() } else { engine.detail.as_str() };
        helper.add(&fact_row(kind_label(&engine.kind), value));
    }
    groups.push(helper);

    if !diagnostics.stale_routes.is_empty() {
        let stale = group("過時路由");
        for route in &diagnostics.stale_routes {
            let via = [route.gateway.as_str(), route.interface.as_str()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            let details: Vec<&str> =
                [via.as_str(), route.reason.as_str(), if route.owned { "由 Plaitway 安裝" } else { "" }]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect();
            let row = adw::ActionRow::builder()
                .title(format!("<tt>{}</tt>", glib::markup_escape_text(&route.prefix)))
                .subtitle(glib::markup_escape_text(&details.join("\n")))
                .subtitle_lines(0)
                .build();
            row.add_prefix(&warning_icon());
            let remove = gtk::Button::builder().label("移除").valign(gtk::Align::Center).build();
            let (route, refresh) = (route.clone(), refresh.clone());
            remove.connect_clicked(move |button| {
                let (key, parent, refresh) = (route.key.clone(), button.clone(), refresh.clone());
                alert::confirm(
                    button,
                    &format!("移除路由 {}？", route.prefix),
                    None,
                    "移除",
                    move || {
                        plaitway(&["remove-route", &key], move |result| {
                            // Already gone, as a resync can make it, is what was asked.
                            if let Err(err) = &result {
                                if !err.starts_with("not found") {
                                    alert::notice(&parent, "無法移除路由", Some(err));
                                }
                            }
                            refresh();
                        });
                    },
                );
            });
            row.add_suffix(&remove);
            stale.add(&row);
        }
        groups.push(stale);
    }

    let network = &diagnostics.network;
    let net = group("網路");
    let join = |parts: &[&str]| {
        parts.iter().filter(|part| !part.is_empty()).copied().collect::<Vec<_>>().join(" · ")
    };
    let last_change =
        join(&[&local_time(network.last_change.as_deref(), "%H:%M:%S"), &network.last_change_reason]);
    for (title, value) in [
        ("預設閘道", join(&[&network.default_gateway_v4, &network.default_interface_v4])),
        ("IPv6 預設閘道", join(&[&network.default_gateway_v6, &network.default_interface_v6])),
        ("網路介面", network.interfaces.join(", ")),
        ("上次變更", last_change),
    ] {
        if !value.is_empty() {
            let row = property_row(title);
            row.set_subtitle(&glib::markup_escape_text(&value));
            net.add(&row);
        }
    }
    groups.push(net);

    let owned = group("路由");
    for route in &diagnostics.owned_routes {
        let row = adw::ActionRow::builder()
            .title(format!("<tt>{}</tt>", glib::markup_escape_text(&route.prefix)))
            .subtitle(glib::markup_escape_text(&format!(
                "{} · {} · {}",
                named(&route.owner),
                route_kind_label(&route.kind),
                route.via
            )))
            .title_selectable(true)
            .build();
        row.add_suffix(&status_label(&route.state, &route_state_label(&route.state, None)));
        owned.add(&row);
    }
    if diagnostics.owned_routes.is_empty() {
        owned.add(&adw::ActionRow::builder().title("無").build());
    }
    groups.push(owned);

    let lists = group("");
    let resolver = adw::ExpanderRow::builder().title("解析器項目").build();
    for entry in &diagnostics.resolver_entries {
        resolver.add_row(
            &adw::ActionRow::builder()
                .title(format!("<tt>{}</tt>", glib::markup_escape_text(entry)))
                .title_selectable(true)
                .build(),
        );
    }
    let recent = adw::ExpanderRow::builder().title("最近變更").build();
    for entry in diagnostics.recent_journal.iter().rev().take(50) {
        let owner = if entry.owner.is_empty() { String::new() } else { named(&entry.owner) };
        let caption = join(&[&local_time(entry.time.as_deref(), "%H:%M:%S"), &owner]);
        let row = adw::ActionRow::builder()
            .title(format!("<tt>{}</tt>", glib::markup_escape_text(&format!("{} {}", entry.kind, entry.key))))
            .subtitle(glib::markup_escape_text(&caption))
            .build();
        row.add_suffix(&gtk::Label::builder().label(&entry.state).css_classes(["dim-label"]).build());
        recent.add_row(&row);
    }
    for (expander, empty) in [
        (&resolver, diagnostics.resolver_entries.is_empty()),
        (&recent, diagnostics.recent_journal.is_empty()),
    ] {
        if empty {
            expander.add_row(&adw::ActionRow::builder().title("無").build());
        }
        lists.add(expander);
    }
    groups.push(lists);
    groups
}

/// Everything the diagnostics know, as plain text for a bug report, in the words the
/// Plaitway app writes it; no log lines, which are the person's to look at first.
fn report_text(diagnostics: &Diagnostics, model: &Model) -> String {
    let named = |owner: &str| model.name_of(owner);
    let daemon = &diagnostics.daemon;
    let mut lines = vec![format!("Helper {} (privileged: {})", daemon.version, daemon.privileged)];
    for engine in &daemon.engines {
        let value = if engine.available { &engine.version } else { &engine.detail };
        lines.push(format!("  {}: {value}", kind_label(&engine.kind)));
    }
    let network = &diagnostics.network;
    lines.extend(["".into(), "Network".into()]);
    lines.push(format!("  gateway IPv4: {} {}", network.default_gateway_v4, network.default_interface_v4));
    lines.push(format!("  gateway IPv6: {} {}", network.default_gateway_v6, network.default_interface_v6));
    lines.push(format!("  interfaces: {}", network.interfaces.join(", ")));
    if network.last_change.is_some() {
        let when = local_time(network.last_change.as_deref(), "%Y-%m-%d %H:%M:%S");
        lines.push(format!("  last change: {when} {}", network.last_change_reason));
    }
    lines.extend(["".into(), "Owned routes".into()]);
    for route in &diagnostics.owned_routes {
        lines.push(format!(
            "  {} {} {} via {} ({})",
            route.prefix,
            route_state(&route.state),
            route.kind.trim_start_matches("ROUTE_KIND_"),
            route.via,
            named(&route.owner)
        ));
    }
    lines.extend(["".into(), "Stale routes".into()]);
    for route in &diagnostics.stale_routes {
        let owned = if route.owned { " (installed by Plaitway)" } else { "" };
        lines.push(format!(
            "  {} via {} {}: {}{owned}",
            route.prefix, route.gateway, route.interface, route.reason
        ));
    }
    lines.extend(["".into(), "Resolver entries".into()]);
    lines.extend(diagnostics.resolver_entries.iter().map(|entry| format!("  {entry}")));
    lines.extend(["".into(), "Recent changes".into()]);
    let from = diagnostics.recent_journal.len().saturating_sub(20);
    for entry in &diagnostics.recent_journal[from..] {
        let when = Some(local_time(entry.time.as_deref(), "%Y-%m-%d %H:%M:%S"))
            .filter(|when| !when.is_empty())
            .unwrap_or("-".into());
        let owner = if entry.owner.is_empty() { String::new() } else { named(&entry.owner) };
        lines.push(format!("  {when} {} {} {} {owner}", entry.kind, entry.key, entry.state));
    }
    lines.join("\n")
}

// ---- logs ----

const LOG_LEVELS: [&str; 4] = ["所有層級", "資訊以上", "警告與錯誤", "僅錯誤"];

fn level_rank(level: &str) -> u32 {
    match level {
        "LOG_LEVEL_DEBUG" => 0,
        "LOG_LEVEL_WARN" => 2,
        "LOG_LEVEL_ERROR" => 3,
        _ => 1,
    }
}

/// A profile's log, or the helper's own for an empty `id`, following the newest line while it
/// is shown; debug lines are left out unless asked for.
fn logs_page(from: &adw::ActionRow, title: &str, id: &str) {
    let search = gtk::SearchEntry::builder().placeholder_text("搜尋記錄").hexpand(true).build();
    let level = gtk::DropDown::from_strings(&LOG_LEVELS);
    level.set_selected(1);
    let bar = page_bar(&[search.upcast_ref(), level.upcast_ref()]);
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(12)
        .right_margin(12)
        .top_margin(12)
        .bottom_margin(12)
        .build();
    let buffer = view.buffer();
    for tag in ["WARN", "ERROR", "DEBUG"] {
        buffer.tag_table().add(&gtk::TextTag::builder().name(tag).build());
    }
    let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
    let empty = adw::StatusPage::builder().title("沒有記錄").icon_name("text-x-generic-symbolic").build();
    let stack = gtk::Stack::new();
    stack.add_named(&scroller, Some("lines"));
    stack.add_named(&empty, Some("empty"));
    stack.set_visible_child_name("empty");
    if push(from, title, Some(bar.upcast_ref()), &stack).is_none() {
        return;
    }
    let colour_tags = {
        let view = view.downgrade();
        move || {
            let Some(view) = view.upgrade() else { return };
            let ink = view.color();
            let table = view.buffer().tag_table();
            for (name, colour) in [
                ("WARN", gdk::RGBA::new(0.80, 0.55, 0.0, 1.0)),
                ("ERROR", gdk::RGBA::new(0.88, 0.11, 0.14, 1.0)),
                ("DEBUG", gdk::RGBA::new(ink.red(), ink.green(), ink.blue(), 0.55)),
            ] {
                if let Some(tag) = table.lookup(name) {
                    tag.set_foreground_rgba(Some(&colour));
                }
            }
        }
    };
    colour_tags();

    let lines: Rc<RefCell<Vec<(u32, String)>>> = Rc::default();
    let shows = {
        let (search, level) = (search.downgrade(), level.downgrade());
        move |rank: u32, text: &str| {
            let (Some(search), Some(level)) = (search.upgrade(), level.upgrade()) else {
                return false;
            };
            let wanted = search.text().to_lowercase();
            // The levels are offered from all of them up to errors only, as the ranks go.
            rank >= level.selected() && (wanted.is_empty() || text.to_lowercase().contains(&wanted))
        }
    };
    let write = {
        let (buffer, stack) = (buffer.clone(), stack.downgrade());
        move |rank: u32, text: &str| {
            let mut end = buffer.end_iter();
            let tag = ["DEBUG", "", "WARN", "ERROR"][rank as usize];
            if tag.is_empty() {
                buffer.insert(&mut end, text);
            } else {
                buffer.insert_with_tags_by_name(&mut end, text, &[tag]);
            }
            if let Some(stack) = stack.upgrade() {
                stack.set_visible_child_name("lines");
            }
        }
    };
    let refill = {
        let (lines, buffer, shows, write, stack) =
            (lines.clone(), buffer.clone(), shows.clone(), write.clone(), stack.downgrade());
        move || {
            buffer.set_text("");
            if let Some(stack) = stack.upgrade() {
                stack.set_visible_child_name("empty");
            }
            for (rank, text) in lines.borrow().iter().filter(|(rank, text)| shows(*rank, text)) {
                write(*rank, text);
            }
        }
    };
    let again = refill.clone();
    search.connect_search_changed(move |_| again());
    let again = refill.clone();
    level.connect_selected_notify(move |_| again());

    let mut args =
        vec![CLI.to_string(), "logs".into(), "-f".into(), "-json".into(), "-n".into(), "500".into()];
    if !id.is_empty() {
        args.push(id.to_string());
    }
    let argv: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    let child = match gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE,
    ) {
        Ok(child) => child,
        Err(err) => {
            empty.set_description(Some(&err.to_string()));
            return;
        }
    };
    let stopped = child.clone();
    stack.connect_destroy(move |_| stopped.force_exit());
    let stopped = child.clone();
    stack.connect_unrealize(move |_| stopped.force_exit());
    let Some(said_out) = child.stdout_pipe() else {
        return;
    };
    let input = gio::DataInputStream::new(&said_out);
    let (weak, adjustment) = (stack.downgrade(), scroller.vadjustment());
    glib::spawn_future_local(async move {
        while let Ok(Some(text)) = input.read_line_utf8_future(glib::Priority::DEFAULT).await {
            if weak.upgrade().is_none() {
                break;
            }
            let (rank, shown) = match serde_json::from_str::<LogLine>(&text) {
                Ok(line) => {
                    let rank = level_rank(&line.level);
                    let tag = ["DEBUG", "INFO", "WARN", "ERROR"][rank as usize];
                    let when = local_time(line.time.as_deref(), "%H:%M:%S");
                    (rank, format!("{when} {tag:<5} {}\n", line.text))
                }
                // What the command said, as that the helper cannot be reached.
                _ => (3, format!("{}\n", said(&text))),
            };
            let at_end = adjustment.value() + adjustment.page_size() >= adjustment.upper() - 4.0;
            {
                let mut kept = lines.borrow_mut();
                kept.push((rank, shown.clone()));
                if kept.len() > LOG_LINES {
                    let extra = kept.len() - LOG_LINES;
                    kept.drain(..extra);
                }
            }
            if shows(rank, &shown) {
                write(rank, &shown);
            }
            if at_end {
                let adjustment = adjustment.clone();
                glib::idle_add_local_once(move || adjustment.set_value(adjustment.upper()));
            }
        }
        child.force_exit();
    });
}

// ---- a profile's text ----

fn placeholder(number: usize) -> String {
    format!("‹secret {number}›")
}

/// The text of a profile with each secret in a numbered placeholder, and the secrets: from
/// the text as `plaitway show` hides them and the whole text, so that what is secret is what
/// Plaitway says is. None when the two do not line up.
fn mask(hidden: &str, full: &str) -> Option<(String, Vec<String>)> {
    let hidden: Vec<&str> = hidden.split_inclusive('\n').collect();
    let full: Vec<&str> = full.split_inclusive('\n').collect();
    let (mut out, mut secrets) = (String::new(), Vec::new());
    let (mut i, mut j) = (0, 0);
    while i < hidden.len() {
        let line = hidden[i];
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        if body == HIDDEN {
            // A block's body: the lines up to the one the hidden text goes on with.
            let next = hidden.get(i + 1).copied();
            let start = j;
            while j < full.len() && Some(full[j]) != next {
                j += 1;
            }
            secrets.push(full[start..j].concat());
            out.push_str(&placeholder(secrets.len()));
            out.push_str(if ending.is_empty() { "\n" } else { ending });
            i += 1;
            continue;
        }
        let original = *full.get(j)?;
        if line == original {
            out.push_str(line);
        } else {
            // A secret value: what `show` wrote as `key = [hidden]`.
            let prefix = body.strip_suffix(HIDDEN)?.strip_suffix(' ')?;
            let value = original.trim_end_matches(['\r', '\n']).strip_prefix(prefix)?;
            let spaces = &value[..value.len() - value.trim_start().len()];
            secrets.push(value.trim_start().to_string());
            out.push_str(&format!("{prefix}{spaces}{}{ending}", placeholder(secrets.len())));
        }
        i += 1;
        j += 1;
    }
    (j == full.len()).then_some((out, secrets))
}

/// The text with its placeholders put back as the secrets they stand for.
fn unmask(text: &str, secrets: &[String]) -> Result<String, String> {
    let mut text = text.to_string();
    for (at, secret) in secrets.iter().enumerate() {
        let token = placeholder(at + 1);
        match text.matches(&token).count() {
            0 => {}
            1 => {
                // A block's lines take the place of the placeholder's line, its ending kept.
                let secret =
                    secret.strip_suffix('\n').map(|block| block.trim_end_matches('\r')).unwrap_or(secret);
                text = text.replacen(&token, secret, 1);
            }
            _ => return Err(format!("機密 {} 出現了不只一次", at + 1)),
        }
    }
    Ok(text)
}

/// A profile's text, read and saved through `plaitway show` and `plaitway edit`.
#[derive(Default)]
struct Configuration {
    /// The whole text as the helper has it.
    full: String,
    /// The text with placeholders, and the secrets they stand for; None when the secrets
    /// could not be told apart.
    masked: Option<(String, Vec<String>)>,
}

fn configuration_page(from: &adw::ActionRow, model: &Rc<Model>, id: &str) {
    let reveal = gtk::ToggleButton::builder().label("顯示機密").build();
    let revert = gtk::Button::builder().label("還原").sensitive(false).build();
    let save = gtk::Button::builder().label("儲存").sensitive(false).build();
    let save_reconnect = gtk::Button::builder()
        .label("儲存並重新連線")
        .sensitive(false)
        .css_classes(["suggested-action"])
        .build();
    let spacer = gtk::Box::builder().hexpand(true).build();
    let bar = page_bar(&[
        reveal.upcast_ref(),
        spacer.upcast_ref(),
        revert.upcast_ref(),
        save.upcast_ref(),
        save_reconnect.upcast_ref(),
    ]);
    let banner = adw::Banner::builder().use_markup(false).build();
    let view = gtk::TextView::builder()
        .monospace(true)
        .wrap_mode(gtk::WrapMode::None)
        .left_margin(8)
        .right_margin(12)
        .top_margin(12)
        .bottom_margin(12)
        .build();
    let numbers = gtk::Label::builder()
        .css_classes(["monospace", "dim-label"])
        .xalign(1.0)
        .yalign(0.0)
        .margin_start(12)
        .margin_end(4)
        .margin_top(12)
        .build();
    view.set_gutter(gtk::TextWindowType::Left, Some(&numbers));
    let buffer = view.buffer();
    for name in ["directive", "comment", "rejected"] {
        buffer.tag_table().add(&gtk::TextTag::builder().name(name).build());
    }
    let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&scroller);
    let failure = adw::StatusPage::builder().icon_name("dialog-warning-symbolic").build();
    let stack = gtk::Stack::new();
    stack.add_named(&content, Some("text"));
    stack.add_named(&failure, Some("failure"));
    stack.add_named(
        &gtk::Spinner::builder().spinning(true).halign(gtk::Align::Center).valign(gtk::Align::Center).build(),
        Some("waiting"),
    );
    stack.set_visible_child_name("waiting");
    if push(from, &format!("「{}」的組態", model.name_of(id)), Some(bar.upcast_ref()), &stack).is_none()
    {
        return;
    }

    let loaded: Rc<RefCell<Configuration>> = Rc::default();
    // What the view showed as it was loaded, to tell whether it has been changed.
    let start: Rc<RefCell<String>> = Rc::default();
    let colour = {
        let view = view.downgrade();
        move || {
            let Some(view) = view.upgrade() else { return };
            let ink = view.color();
            let table = view.buffer().tag_table();
            if let Some(tag) = table.lookup("directive") {
                tag.set_weight(700);
            }
            if let Some(tag) = table.lookup("comment") {
                tag.set_foreground_rgba(Some(&gdk::RGBA::new(ink.red(), ink.green(), ink.blue(), 0.55)));
                tag.set_style(gtk::pango::Style::Italic);
            }
            if let Some(tag) = table.lookup("rejected") {
                tag.set_paragraph_background_rgba(Some(&gdk::RGBA::new(0.88, 0.11, 0.14, 0.18)));
            }
        }
    };
    colour();
    let highlight = {
        let (numbers, start, revert, save, save_reconnect, reveal, loaded) = (
            numbers.downgrade(),
            start.clone(),
            revert.downgrade(),
            save.downgrade(),
            save_reconnect.downgrade(),
            reveal.downgrade(),
            loaded.clone(),
        );
        move |buffer: &gtk::TextBuffer| {
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
            buffer.remove_tag_by_name("directive", &buffer.start_iter(), &buffer.end_iter());
            buffer.remove_tag_by_name("comment", &buffer.start_iter(), &buffer.end_iter());
            for (number, line) in text.split('\n').enumerate() {
                let Some(begin) = buffer.iter_at_line(number as i32) else {
                    continue;
                };
                let indent = line.len() - line.trim_start().len();
                let trimmed = line.trim_start();
                let (tag, length) = if trimmed.starts_with(['#', ';']) {
                    ("comment", trimmed.len())
                } else {
                    ("directive", trimmed.find([' ', '\t', '=']).unwrap_or(trimmed.len()))
                };
                let chars = |bytes: &str| bytes.chars().count() as i32;
                let mut from = begin;
                from.forward_chars(chars(&line[..indent]));
                let mut to = from;
                to.forward_chars(chars(&trimmed[..length]));
                buffer.apply_tag_by_name(tag, &from, &to);
            }
            if let Some(numbers) = numbers.upgrade() {
                let count = text.split('\n').count();
                numbers.set_label(&(1..=count).map(|n| n.to_string()).collect::<Vec<_>>().join("\n"));
            }
            let edited = text != *start.borrow();
            for button in [revert.upgrade(), save.upgrade(), save_reconnect.upgrade()].into_iter().flatten() {
                button.set_sensitive(edited);
            }
            // Hidden again only as it was loaded: an edit with the secrets shown is saved first.
            if let Some(reveal) = reveal.upgrade() {
                let hideable = !reveal.is_active() || text == loaded.borrow().full;
                reveal.set_sensitive(loaded.borrow().masked.is_some() && hideable);
            }
        }
    };
    let again = highlight.clone();
    buffer.connect_changed(move |buffer| again(buffer));

    // Shows the text as loaded, its secrets hidden unless they are asked for.
    let show = {
        let (buffer, loaded, start, reveal, banner) =
            (buffer.clone(), loaded.clone(), start.clone(), reveal.downgrade(), banner.downgrade());
        move || {
            let revealed = reveal.upgrade().is_some_and(|reveal| reveal.is_active());
            let text = match (&loaded.borrow().masked, revealed) {
                (Some((masked, _)), false) => masked.clone(),
                _ => loaded.borrow().full.clone(),
            };
            start.replace(text.clone());
            buffer.set_text(&text);
            if let Some(banner) = banner.upgrade() {
                banner.set_revealed(false);
            }
        }
    };
    let load: Rc<dyn Fn()> = {
        let (id, loaded, show, stack, failure, reveal) = (
            id.to_string(),
            loaded.clone(),
            show.clone(),
            stack.downgrade(),
            failure.downgrade(),
            reveal.downgrade(),
        );
        Rc::new(move || {
            let (id, loaded, show, stack, failure, reveal) =
                (id.clone(), loaded.clone(), show.clone(), stack.clone(), failure.clone(), reveal.clone());
            let target = id.clone();
            plaitway(&["show", &target], move |hidden| {
                let again = id.clone();
                plaitway(&["show", "-secrets", &again], move |full| {
                    let (Some(stack), Some(failure)) = (stack.upgrade(), failure.upgrade()) else {
                        return;
                    };
                    match (hidden, full) {
                        (Ok(hidden), Ok(full)) => {
                            let masked = mask(&hidden, &full);
                            if let Some(reveal) = reveal.upgrade() {
                                // Without placeholders, the only way to edit is with the secrets.
                                if masked.is_none() {
                                    reveal.set_active(true);
                                }
                                reveal.set_label(if reveal.is_active() {
                                    "隱藏機密"
                                } else {
                                    "顯示機密"
                                });
                            }
                            loaded.replace(Configuration { full, masked });
                            show();
                            stack.set_visible_child_name("text");
                        }
                        (Err(err), _) | (_, Err(err)) => {
                            let title = if err.starts_with("permission denied") {
                                "需要管理員權限"
                            } else {
                                "無法讀取組態"
                            };
                            failure.set_title(title);
                            failure.set_description(Some(&err));
                            stack.set_visible_child_name("failure");
                        }
                    }
                });
            });
        })
    };
    load();

    let (shown_buffer, shown_loaded, again) = (buffer.clone(), loaded.clone(), show.clone());
    let shown_banner = banner.downgrade();
    reveal.connect_toggled(move |reveal| {
        reveal.set_label(if reveal.is_active() { "隱藏機密" } else { "顯示機密" });
        let text = shown_buffer.text(&shown_buffer.start_iter(), &shown_buffer.end_iter(), false).to_string();
        let secrets = shown_loaded.borrow().masked.as_ref().map(|(_, secrets)| secrets.clone());
        match (reveal.is_active(), secrets) {
            // The edits so far stay, with the secrets in them.
            (true, Some(secrets)) => match unmask(&text, &secrets) {
                Ok(full) => shown_buffer.set_text(&full),
                Err(err) => {
                    reveal.set_active(false);
                    if let Some(banner) = shown_banner.upgrade() {
                        banner.set_title(&err);
                        banner.set_revealed(true);
                    }
                }
            },
            _ => again(),
        }
    });
    let again = show.clone();
    revert.connect_clicked(move |_| again());

    for (button, reconnect) in [(&save, false), (&save_reconnect, true)] {
        let (id, buffer, loaded, load, reveal, banner) = (
            id.to_string(),
            buffer.clone(),
            loaded.clone(),
            load.clone(),
            reveal.downgrade(),
            banner.downgrade(),
        );
        button.connect_clicked(move |button| {
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
            let revealed = reveal.upgrade().is_some_and(|reveal| reveal.is_active());
            let whole = match (&loaded.borrow().masked, revealed) {
                (Some((_, secrets)), false) => unmask(&text, secrets),
                _ => Ok(text),
            };
            let tell = {
                let (banner, buffer) = (banner.clone(), buffer.clone());
                move |err: &str| {
                    let Some(banner) = banner.upgrade() else { return };
                    banner.set_title(err);
                    banner.set_revealed(true);
                    mark_rejected_line(&buffer, err);
                }
            };
            let whole = match whole {
                Ok(whole) => whole,
                Err(err) => return tell(&err),
            };
            let (id, button, loaded, load) = (id.clone(), button.clone(), loaded.clone(), load.clone());
            // Saved meanwhile elsewhere, as by the Plaitway app, it is asked before it is replaced.
            let again = id.clone();
            plaitway(&["show", "-secrets", &again], move |current| {
                let parent = button.clone();
                let replace = move || save_text(&button, &id, whole, reconnect, tell, load);
                match current {
                    Ok(current) if current != loaded.borrow().full => {
                        alert::confirm(
                            &parent,
                            "設定檔已在別處變更",
                            Some("以這裡的內容取代？"),
                            "取代",
                            replace,
                        );
                    }
                    _ => replace(),
                }
            });
        });
    }
}

/// Marks the line a refusal names, as `line 4: …`.
fn mark_rejected_line(buffer: &gtk::TextBuffer, err: &str) {
    buffer.remove_tag_by_name("rejected", &buffer.start_iter(), &buffer.end_iter());
    let lower = err.to_lowercase();
    let Some(at) = lower.find("line ") else { return };
    let digits: String = lower[at + 5..].chars().take_while(char::is_ascii_digit).collect();
    let Some(number) = digits.parse::<i32>().ok().filter(|number| *number > 0) else {
        return;
    };
    let Some(from) = buffer.iter_at_line(number - 1) else {
        return;
    };
    let mut to = from;
    to.forward_to_line_end();
    buffer.apply_tag_by_name("rejected", &from, &to);
}

/// Saves `text` as the profile's through `plaitway edit`, whose editor copies it over the
/// file it opens: the command has no other way to take a whole text. The text, secrets and
/// all, is in a file of this user's alone only while it runs.
fn save_text(
    button: &gtk::Button,
    id: &str,
    text: String,
    reconnect: bool,
    tell: impl Fn(&str) + 'static,
    load: Rc<dyn Fn()>,
) {
    let dir = glib::user_runtime_dir().join(format!("nekoland-plaitway-{}", std::process::id()));
    let file = dir.join(format!("profile-{}", glib::monotonic_time()));
    let written = {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .and_then(|_| std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file))
            .and_then(|mut opened| opened.write_all(text.as_bytes()))
    };
    if let Err(err) = written {
        let _ = std::fs::remove_file(&file);
        return alert::notice(button, "無法儲存", Some(&err.to_string()));
    }
    let editor = format!("cp -- {}", file.display());
    let mut args = vec!["edit"];
    if reconnect {
        args.push("-reconnect");
    }
    args.push(id);
    let button = button.clone();
    plaitway_with(&args, &[("VISUAL", &editor)], None, move |result| {
        if let Err(err) = std::fs::remove_file(&file) {
            eprintln!("{}: {err}", file.display());
        }
        match result {
            Ok((_, said_too)) => {
                let warnings: Vec<&str> =
                    said_too.lines().filter_map(|line| line.strip_prefix("warning: ")).collect();
                if !warnings.is_empty() {
                    alert::notice(&button, "已略過部分內容", Some(&warnings.join("\n")));
                }
                load();
            }
            Err(err) => {
                let refused = err.lines().find_map(|line| {
                    line.strip_prefix("rejected: ").or_else(|| line.strip_prefix("not saved: "))
                });
                match refused {
                    Some(reason) => tell(reason),
                    None => alert::notice(&button, "無法儲存", Some(&err)),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected(id: &str, routes: &[(&str, &str)]) -> Profile {
        Profile {
            id: id.into(),
            name: id.into(),
            state: "PROFILE_STATE_CONNECTED".into(),
            status: Status {
                routes: routes
                    .iter()
                    .map(|(prefix, state)| RouteStatus {
                        prefix: prefix.to_string(),
                        state: format!("ROUTE_STATE_{state}"),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn a_default_route_in_halves_is_one_route_held_by_the_first_profile() {
        let profiles = [
            connected(
                "office",
                &[("0.0.0.0/1", "INSTALLED"), ("128.0.0.0/1", "INSTALLED"), ("10.0.0.0/8", "INSTALLED")],
            ),
            connected(
                "home",
                &[("192.168.1.0/24", "INSTALLED"), ("0.0.0.0/1", "SHADOWED"), ("128.0.0.0/1", "SHADOWED")],
            ),
        ];
        let routes = routes_of(&profiles);
        assert_eq!(routes[0].prefix, "0.0.0.0/0");
        assert_eq!(routes[0].holder, Some(0));
        assert_eq!(routes[0].requests, vec![(0, Standing::Holds), (1, Standing::Waits)]);
        assert_eq!(routes.len(), 3);
        assert_eq!(why(&routes[0], &profiles), "預設路由：由 office 持有，home 在後面等待");
        assert_eq!(holds(&profiles[1]), (1, 2));
    }

    #[test]
    fn secrets_go_into_placeholders_and_come_back() {
        let full = "client\n<key>\nKEY LINE 1\nKEY LINE 2\n</key>\nremote a 1194\n";
        let hidden = "client\n<key>\n[hidden]\n</key>\nremote a 1194\n";
        let (masked, secrets) = mask(hidden, full).unwrap();
        assert_eq!(masked, "client\n<key>\n‹secret 1›\n</key>\nremote a 1194\n");
        assert_eq!(unmask(&masked, &secrets).unwrap(), full);

        let full = "[Interface]\nPrivateKey=abc=\nAddress = 10.0.0.2/32\n# PresharedKey = xyz\n";
        let hidden = "[Interface]\nPrivateKey= [hidden]\nAddress = 10.0.0.2/32\n# PresharedKey = [hidden]\n";
        let (masked, secrets) = mask(hidden, full).unwrap();
        assert_eq!(
            masked,
            "[Interface]\nPrivateKey=‹secret 1›\nAddress = 10.0.0.2/32\n# PresharedKey = ‹secret 2›\n"
        );
        let edited = masked.replace("10.0.0.2", "10.0.0.3");
        assert_eq!(unmask(&edited, &secrets).unwrap(), full.replace("10.0.0.2", "10.0.0.3"));
        assert!(unmask(&format!("{masked}‹secret 1›\n"), &secrets).is_err());
    }

    #[test]
    fn a_block_never_closed_hides_the_rest() {
        let full = "client\n<key>\nKEY\nMORE\n";
        let hidden = "client\n<key>\n[hidden]\n";
        let (masked, secrets) = mask(hidden, full).unwrap();
        assert_eq!(masked, "client\n<key>\n‹secret 1›\n");
        assert_eq!(unmask(&masked, &secrets).unwrap(), full);
    }

    #[test]
    fn the_watch_reads_as_the_command_prints_it() {
        let line = r#"{"changed":{"id":"A","name":"wg","kind":"PROFILE_KIND_WIREGUARD","desiredEnabled":true,"state":"PROFILE_STATE_CONNECTED","settings":{"priority":2,"onDemand":null},"summary":null,"status":{"rxBytes":"1234","txBytes":"99","connectedSince":"2026-11-22T16:45:17Z","routes":[]},"credentialRequest":null}}"#;
        let event: Event = serde_json::from_str(line).unwrap();
        let profile = event.changed.unwrap();
        assert_eq!(profile.status.rx_bytes, 1234);
        assert_eq!(profile.settings.priority, 2);
        assert!(profile.connected());
        assert_eq!(caption(&profile), "WireGuard · 已連線");
    }

    #[test]
    fn an_auth_user_pass_file_is_read_only_from_the_profile_folder() {
        let dir = std::env::temp_dir().join(format!("plaitway-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("profile")).unwrap();
        std::fs::write(dir.join("profile/login.txt"), "me\nsecret\n").unwrap();
        std::fs::write(dir.join("outside.txt"), "them\nother\n").unwrap();
        std::fs::write(dir.join("profile/a.ovpn"), "client\nauth-user-pass login.txt\n").unwrap();
        std::fs::write(dir.join("profile/b.ovpn"), "client\nauth-user-pass ../outside.txt\n").unwrap();
        assert_eq!(auth_user_pass(&dir.join("profile/a.ovpn")), Some(("me".into(), "secret".into())));
        assert_eq!(auth_user_pass(&dir.join("profile/b.ovpn")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
