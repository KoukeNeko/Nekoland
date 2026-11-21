//! Where software comes from, as Market reads it: AppStream for Ubuntu's archive
//! and Flathub, snapd's socket for the Snap Store, `brew` for Homebrew, and the Packages
//! lists apt keeps for each other repository. Each is asked through its own program or
//! files, with no library of its own; what they say is turned into offers, and offers of
//! the same program from different places are put together as one app.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    Apt,
    Flatpak,
    Snap,
    Brew,
}

/// One place an app can be installed from, and what it is there.
#[derive(Clone, Debug, Default)]
pub struct Offer {
    pub kind: Option<Kind>,
    /// The package, Flatpak application ID, snap or formula name.
    pub id: String,
    /// The place, as the source table names it: Ubuntu, Flathub, Snap Store, Homebrew, or
    /// a repository's own name.
    pub origin: String,
    /// Under the place: apt, flatpak, snap, a tap, or apt · 自家套件庫.
    pub via: String,
    pub version: String,
    /// The version installed from here, if it is.
    pub installed: Option<String>,
    pub size: String,
    pub publisher: String,
    pub verified: bool,
    /// Installing from here takes the administrator's password.
    pub admin: bool,
    /// A snap that asks for classic confinement.
    pub classic: bool,
    pub cask: bool,
    /// The repository that must be added before installing from it.
    pub repo: Option<&'static Vendor>,
}

impl Offer {
    pub fn updates(&self) -> &'static str {
        match self.kind {
            Some(Kind::Snap) => "自動",
            Some(Kind::Apt) => "隨系統",
            _ => "手動",
        }
    }

    pub fn sandbox(&self) -> &'static str {
        match self.kind {
            Some(Kind::Flatpak) => "按權限",
            Some(Kind::Snap) if !self.classic => "受限",
            _ => "無",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct App {
    /// What offers of the same program share: an AppStream ID, or a package name.
    pub key: String,
    pub name: String,
    pub summary: String,
    pub description: String,
    pub developer: String,
    pub homepage: String,
    pub license: String,
    pub icon: Option<PathBuf>,
    /// An icon theme's name for it, when AppStream gives a name rather than a file.
    pub icon_name: String,
    /// Its icon on the web, for one known before any source's data is on this computer.
    pub icon_url: String,
    pub screenshot: String,
    /// A command-line program, without a window: shown by its command.
    pub cli: bool,
    pub offers: Vec<Offer>,
}

impl App {
    pub fn installed(&self) -> Option<&Offer> {
        self.offers.iter().find(|offer| offer.installed.is_some())
    }
}

/// The key apps are put together by: an AppStream ID without its `.desktop`, lowercased.
pub fn key_of(id: &str) -> String {
    id.trim_end_matches(".desktop").to_lowercase()
}

/// Puts apps with the same key together, their offers in the order the store prefers
/// them, Flathub before the Snap Store before apt before Homebrew.
pub fn merge(apps: impl IntoIterator<Item = App>) -> Vec<App> {
    let mut order = Vec::new();
    let mut merged: HashMap<String, App> = HashMap::new();
    for app in apps {
        match merged.get_mut(&app.key) {
            Some(into) => {
                for (field, value) in [
                    (&mut into.name, app.name),
                    (&mut into.summary, app.summary),
                    (&mut into.description, app.description),
                    (&mut into.developer, app.developer),
                    (&mut into.homepage, app.homepage),
                    (&mut into.license, app.license),
                    (&mut into.screenshot, app.screenshot),
                    (&mut into.icon_url, app.icon_url),
                ] {
                    if field.is_empty() {
                        *field = value;
                    }
                }
                into.icon = into.icon.take().or(app.icon);
                if into.icon_name.is_empty() {
                    into.icon_name = app.icon_name;
                }
                into.cli &= app.cli;
                for offer in app.offers {
                    if !into
                        .offers
                        .iter()
                        .any(|o| o.kind == offer.kind && o.id == offer.id && o.origin == offer.origin)
                    {
                        into.offers.push(offer);
                    }
                }
            }
            None => {
                order.push(app.key.clone());
                merged.insert(app.key.clone(), app);
            }
        }
    }
    order
        .into_iter()
        .filter_map(|key| merged.remove(&key))
        .map(|mut app| {
            app.offers.sort_by_key(|offer| rank(offer));
            app
        })
        .collect()
}

fn rank(offer: &Offer) -> u8 {
    match offer.kind {
        Some(Kind::Flatpak) => 0,
        Some(Kind::Snap) => 1,
        Some(Kind::Apt) if offer.repo.is_none() && offer.origin == UBUNTU => 2,
        Some(Kind::Apt) => 3,
        _ => 4,
    }
}

pub const UBUNTU: &str = "Ubuntu";

fn output(program: &str, args: &[&str]) -> Option<String> {
    match Command::new(program).args(args).output() {
        Ok(out) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => {
            eprintln!(
                "{program} {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
            None
        }
        // A source whose program is not installed has nothing to offer.
        Err(_) => None,
    }
}

// AppStream: Ubuntu's archive and Flathub.

/// The blocks `appstreamcli search --details` and `list-categories --details` print, each
/// as its fields; a field over several lines is joined, a list kept one item a line.
pub fn parse_appstream(text: &str) -> Vec<HashMap<String, String>> {
    let mut blocks = Vec::new();
    let mut block = HashMap::new();
    let mut field: Option<String> = None;
    for line in text.lines() {
        if line == "---" {
            blocks.push(std::mem::take(&mut block));
            field = None;
        } else if let Some(rest) = line.strip_prefix("  ") {
            if let Some(field) = &field {
                let value: &mut String = block.entry(field.clone()).or_default();
                let rest = rest.trim();
                // An empty line between a description's paragraphs.
                if rest.is_empty() {
                    if !value.is_empty() && !value.ends_with('\n') {
                        value.push_str("\n\n");
                    }
                    continue;
                }
                if !value.is_empty() && !value.ends_with('\n') {
                    value.push(if rest.starts_with("- ") { '\n' } else { ' ' });
                }
                value.push_str(rest.trim_start_matches("- "));
            }
        } else if let Some((name, value)) = line.split_once(':') {
            // Fields past the description's, such as what a program provides, are not read.
            if name.starts_with(' ')
                || name.contains(' ') && !matches!(name, "Internal ID" | "Default Screenshot URL")
            {
                field = None;
                continue;
            }
            field = Some(name.to_string());
            block.insert(name.to_string(), value.trim().to_string());
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }
    blocks
}

fn appstream_app(block: &HashMap<String, String>) -> Option<App> {
    let identifier = block.get("Identifier")?;
    let (id, kind) = identifier.split_once(' ').unwrap_or((identifier, ""));
    let cli = kind.contains("console-application");
    if !cli && !kind.contains("desktop-application") {
        return None;
    }
    let field = |name: &str| block.get(name).cloned().unwrap_or_default();
    let (offer, icon) = if let Some(package) = block.get("Package") {
        let offer = Offer {
            kind: Some(Kind::Apt),
            id: package.clone(),
            origin: UBUNTU.into(),
            via: "apt".into(),
            publisher: UBUNTU.into(),
            admin: true,
            ..Default::default()
        };
        (offer, apt_icon(&field("Icon")))
    } else if let Some(bundle) = block.get("Bundle").and_then(|b| b.strip_prefix("flatpak:app/")) {
        let flatpak_id = bundle.split('/').next()?.to_string();
        let offer = Offer {
            kind: Some(Kind::Flatpak),
            id: flatpak_id.clone(),
            origin: "Flathub".into(),
            via: "flatpak".into(),
            publisher: field("Developer"),
            verified: !field("Developer").is_empty(),
            ..Default::default()
        };
        (offer, flatpak_icon(&field("Icon")))
    } else {
        return None;
    };
    Some(App {
        key: key_of(id),
        name: field("Name"),
        summary: field("Summary"),
        description: field("Description"),
        developer: field("Developer"),
        homepage: field("Homepage"),
        license: field("License"),
        screenshot: field("Default Screenshot URL"),
        icon_name: Some(field("Icon"))
            .filter(|name| !name.contains('.') || name.starts_with("org.") && !name.ends_with(".png"))
            .unwrap_or_default(),
        icon,
        cli,
        offers: vec![offer],
        ..Default::default()
    })
}

fn apt_icon(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let catalogs = std::fs::read_dir("/var/lib/swcatalog/icons").ok()?;
    catalogs
        .flatten()
        .flat_map(|origin| ["64x64", "128x128", "48x48"].map(|size| origin.path().join(size).join(name)))
        .find(|path| path.exists())
}

fn flatpak_icon(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let user = glib_home().join(".local/share/flatpak/appstream");
    ["/var/lib/flatpak/appstream".into(), user]
        .iter()
        .flat_map(|root| std::fs::read_dir(root).into_iter().flatten().flatten())
        .flat_map(|remote| std::fs::read_dir(remote.path()).into_iter().flatten().flatten())
        .flat_map(|arch| {
            ["64x64", "128x128"].map(|size| arch.path().join("active/icons").join(size).join(name))
        })
        .find(|path| path.exists())
}

fn glib_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

pub fn search_appstream(query: &str) -> Vec<App> {
    output("appstreamcli", &["search", "--details", query])
        .map(|text| parse_appstream(&text).iter().filter_map(appstream_app).collect())
        .unwrap_or_default()
}

/// Apps in a freedesktop category, the well-known ones first.
pub fn category(name: &str, known: &[&str]) -> Vec<App> {
    let mut apps: Vec<App> = output("appstreamcli", &["list-categories", "--details", name])
        .map(|text| parse_appstream(&text).iter().filter_map(appstream_app).collect())
        .unwrap_or_default();
    apps.retain(|app| !app.cli);
    let mut apps = merge(apps);
    apps.sort_by_key(|app| {
        let at = known
            .iter()
            .position(|id| key_of(id) == app.key)
            .unwrap_or(known.len());
        (
            at,
            app.icon.is_none() && app.icon_name.is_empty(),
            app.offers.len() == 1,
        )
    });
    apps.truncate(24);
    apps
}

// apt: versions, what is installed, other repositories and packages without AppStream.

/// `apt-cache policy` for several packages: each one's installed and candidate versions,
/// and where the candidate comes from.
pub fn parse_policy(text: &str) -> HashMap<String, (Option<String>, String, String)> {
    let mut found = HashMap::new();
    let mut name = String::new();
    let (mut installed, mut candidate, mut from) = (None, String::new(), String::new());
    let mut in_candidate = false;
    let mut flush =
        |name: &str, installed: &mut Option<String>, candidate: &mut String, from: &mut String| {
            if !name.is_empty() && !candidate.is_empty() && candidate != "(none)" {
                found.insert(
                    name.to_string(),
                    (installed.take(), std::mem::take(candidate), std::mem::take(from)),
                );
            }
            *installed = None;
            candidate.clear();
            from.clear();
        };
    for line in text.lines() {
        if !line.starts_with(' ') && line.ends_with(':') {
            flush(&name, &mut installed, &mut candidate, &mut from);
            name = line.trim_end_matches(':').to_string();
        } else if let Some(v) = line.trim().strip_prefix("Installed: ") {
            installed = (v != "(none)").then(|| v.to_string());
        } else if let Some(v) = line.trim().strip_prefix("Candidate: ") {
            candidate = v.to_string();
        } else if line.starts_with(" *** ") || line.starts_with("     ") && !line.starts_with("        ") {
            // A version line of the table: the next source line is where it comes from.
            in_candidate =
                line.trim().trim_start_matches("*** ").split(' ').next() == Some(candidate.as_str());
        } else if in_candidate && from.is_empty() && line.starts_with("        ") {
            let source = line.trim().split(' ').nth(1).unwrap_or_default();
            if source.contains("://") {
                from = source.to_string();
            }
        }
    }
    flush(&name, &mut installed, &mut candidate, &mut from);
    found
}

/// Fills in apt offers' versions, and gives each the repository it comes from.
pub fn fill_apt(apps: &mut [App]) {
    let names: Vec<String> = apps
        .iter()
        .flat_map(|app| &app.offers)
        .filter(|offer| offer.kind == Some(Kind::Apt))
        .map(|offer| offer.id.clone())
        .collect();
    if names.is_empty() {
        return;
    }
    let args: Vec<&str> = std::iter::once("policy")
        .chain(names.iter().map(String::as_str))
        .collect();
    let policy = output("apt-cache", &args)
        .map(|text| parse_policy(&text))
        .unwrap_or_default();
    let repos = repositories();
    for app in apps.iter_mut() {
        app.offers.retain_mut(|offer| {
            if offer.kind != Some(Kind::Apt) {
                return true;
            }
            let Some((installed, candidate, from)) = policy.get(&offer.id) else {
                return offer.repo.is_some();
            };
            offer.version = candidate.clone();
            offer.installed = installed.clone();
            if let Some(repo) = repos
                .iter()
                .find(|repo| from.starts_with(repo.uri.trim_end_matches('/')))
            {
                offer.origin = repo.name.clone();
                offer.via = "apt · 自家套件庫".into();
                offer.publisher = repo.name.clone();
                offer.repo = None;
            }
            true
        });
    }
}

/// Sizes of apt offers, from `apt-cache show`, and versions and sizes of Flatpak ones from
/// their remote, which AppStream does not say.
pub fn fill_sizes(app: &mut App) {
    for offer in &mut app.offers {
        match offer.kind {
            Some(Kind::Apt) if offer.size.is_empty() && offer.repo.is_none() => {
                let text = output("apt-cache", &["show", "--no-all-versions", &offer.id]).unwrap_or_default();
                if let Some(kib) = text
                    .lines()
                    .find_map(|l| l.strip_prefix("Installed-Size: "))
                    .and_then(|v| v.trim().parse::<u64>().ok())
                {
                    offer.size = human_size(kib * 1024);
                }
            }
            Some(Kind::Flatpak) if offer.version.is_empty() || offer.size.is_empty() => {
                let text = output("flatpak", &["remote-info", "flathub", &offer.id])
                    .or_else(|| output("flatpak", &["remote-info", "--user", "flathub", &offer.id]))
                    .unwrap_or_default();
                for line in text.lines().map(str::trim) {
                    if let Some(v) = line.strip_prefix("Version: ") {
                        offer.version = v.to_string();
                    } else if let Some(v) = line.strip_prefix("Installed Size: ") {
                        offer.size = v.replace('\u{a0}', " ");
                    }
                }
            }
            _ => {}
        }
    }
}

/// Packages by name that AppStream does not know, mostly command-line programs.
pub fn search_apt_names(query: &str) -> Vec<App> {
    let Some(text) = output("apt-cache", &["search", "--names-only", query]) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| line.split_once(" - "))
        .filter(|(name, _)| !name.ends_with("-dev") && !name.ends_with("-doc") && !name.starts_with("lib"))
        .take(12)
        .map(|(name, summary)| App {
            key: name.to_string(),
            name: name.to_string(),
            summary: summary.to_string(),
            cli: true,
            offers: vec![Offer {
                kind: Some(Kind::Apt),
                id: name.to_string(),
                origin: UBUNTU.into(),
                via: "apt".into(),
                publisher: UBUNTU.into(),
                admin: true,
                ..Default::default()
            }],
            ..Default::default()
        })
        .collect()
}

/// A repository other than Ubuntu's own, from /etc/apt/sources.list.d.
#[derive(Clone, Debug, PartialEq)]
pub struct Repository {
    pub name: String,
    pub uri: String,
    pub suites: Vec<String>,
    pub file: PathBuf,
}

const UBUNTU_HOSTS: [&str; 4] = [
    "archive.ubuntu.com",
    "security.ubuntu.com",
    "ports.ubuntu.com",
    "esm.ubuntu.com",
];

/// The repositories a sources file names other than Ubuntu's own: deb822 `.sources`, or
/// one-line `.list`. Ubuntu's archive is known by its file or its key as well as by its
/// hosts, since a mirror may have any address.
pub fn parse_sources(file: &Path, text: &str) -> Vec<Repository> {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    if stem == "ubuntu" || stem.starts_with("ubuntu-esm") || text.contains("ubuntu-archive-keyring") {
        return Vec::new();
    }
    let mut repos = every_source(file, text);
    repos.retain(|repo| !UBUNTU_HOSTS.iter().any(|host| repo.uri.contains(host)));
    repos
}

/// Whether removing `repo`, which deletes its file, takes no other repository with it.
pub fn removable(repo: &Repository) -> bool {
    std::fs::read_to_string(&repo.file).is_ok_and(|text| every_source(&repo.file, &text).len() == 1)
}

/// Every repository a sources file names.
fn every_source(file: &Path, text: &str) -> Vec<Repository> {
    let mut repos = Vec::new();
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let fallback = |uri: &str| -> String {
        if let Some(ppa) = uri.split("ppa.launchpadcontent.net/").nth(1) {
            return format!(
                "PPA {}",
                ppa.trim_end_matches("/ubuntu/")
                    .trim_end_matches("/ubuntu")
                    .trim_end_matches('/')
            );
        }
        stem.split(['-', '_'])
            .map(capitalize)
            .collect::<Vec<_>>()
            .join(" ")
    };
    if file.extension().is_some_and(|ext| ext == "sources") {
        for stanza in text.split("\n\n") {
            let field = |name: &str| {
                stanza
                    .lines()
                    .find_map(|line| line.strip_prefix(name).and_then(|rest| rest.strip_prefix(':')))
                    .map(|value| value.trim().to_string())
            };
            if field("Enabled").as_deref() == Some("no")
                || !field("Types").unwrap_or_default().split(' ').any(|t| t == "deb")
            {
                continue;
            }
            let Some(uri) = field("URIs").and_then(|uris| uris.split(' ').next().map(str::to_string)) else {
                continue;
            };
            repos.push(Repository {
                name: field("X-Repolib-Name").unwrap_or_else(|| fallback(&uri)),
                suites: field("Suites")
                    .unwrap_or_default()
                    .split(' ')
                    .map(str::to_string)
                    .collect(),
                uri,
                file: file.to_path_buf(),
            });
        }
    } else {
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("deb "))
        {
            // `deb [options] uri suite components`, the options optional.
            let rest = line[4..].trim();
            let rest = match rest.strip_prefix('[') {
                Some(after) => after
                    .split_once(']')
                    .map(|(_, rest)| rest.trim())
                    .unwrap_or_default(),
                None => rest,
            };
            let mut words = rest.split_whitespace();
            let (Some(uri), Some(suite)) = (words.next(), words.next()) else {
                continue;
            };
            if repos.iter().any(|repo: &Repository| repo.uri == uri) {
                continue;
            }
            repos.push(Repository {
                name: fallback(uri),
                uri: uri.to_string(),
                suites: vec![suite.to_string()],
                file: file.to_path_buf(),
            });
        }
    }
    repos
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

pub fn repositories() -> Vec<Repository> {
    let mut files: Vec<PathBuf> = std::fs::read_dir("/etc/apt/sources.list.d")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "sources" || ext == "list")
        })
        .collect();
    files.sort();
    let mut repos: Vec<Repository> = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for repo in parse_sources(&file, &text) {
            // Steam's two files name one place under two names; the first is kept.
            if !repos.iter().any(|known| known.uri == repo.uri) {
                repos.push(repo);
            }
        }
    }
    repos
}

/// The packages a repository offers, from the Packages lists apt fetched for it.
pub fn repository_packages(repo: &Repository) -> Vec<App> {
    let mangled = repo
        .uri
        .split_once("://")
        .map_or(repo.uri.as_str(), |(_, rest)| rest)
        .trim_end_matches('/')
        .replace('/', "_");
    let mut apps = Vec::new();
    for entry in std::fs::read_dir("/var/lib/apt/lists")
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&mangled) || !name.ends_with("_Packages") {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
        for stanza in text.split("\n\n") {
            let field = |key: &str| {
                stanza
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix(key)
                            .and_then(|rest| rest.strip_prefix(": "))
                            .map(str::to_string)
                    })
                    .unwrap_or_default()
            };
            let package = field("Package");
            if package.is_empty() || apps.iter().any(|app: &App| app.key == package) {
                continue;
            }
            let size = field("Installed-Size")
                .parse::<u64>()
                .map(|kib| human_size(kib * 1024))
                .unwrap_or_default();
            apps.push(App {
                key: package.clone(),
                name: package.clone(),
                summary: field("Description"),
                cli: true,
                offers: vec![Offer {
                    kind: Some(Kind::Apt),
                    id: package,
                    origin: repo.name.clone(),
                    via: "apt · 自家套件庫".into(),
                    version: field("Version"),
                    size,
                    publisher: repo.name.clone(),
                    admin: true,
                    ..Default::default()
                }],
                ..Default::default()
            });
        }
    }
    let installed = installed_versions(apps.iter().map(|app| app.key.as_str()));
    for app in &mut apps {
        app.offers[0].installed = installed.get(&app.key).cloned();
    }
    apps
}

pub fn installed_versions<'a>(names: impl Iterator<Item = &'a str>) -> HashMap<String, String> {
    let names: Vec<&str> = names.collect();
    if names.is_empty() {
        return HashMap::new();
    }
    let mut args = vec!["-W", "-f", "${Package}\t${db:Status-Abbrev}\t${Version}\n"];
    args.extend(&names);
    // dpkg-query fails for packages it has never heard of, but still prints the others.
    let out = Command::new("dpkg-query")
        .args(&args)
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let (name, status, version) = (parts.next()?, parts.next()?, parts.next()?);
            status
                .starts_with("ii")
                .then(|| (name.to_string(), version.to_string()))
        })
        .collect()
}

pub fn human_size(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else if bytes >= 1e6 {
        format!("{:.0} MB", bytes / 1e6)
    } else {
        format!("{:.0} KB", (bytes / 1e3).max(1.0))
    }
}

// Repositories the store knows how to add, for programs whose makers publish them only
// there. Each is added as its makers say: their signing key in /etc/apt/keyrings, and a
// sources file that trusts it for their repository alone.

#[derive(Debug, PartialEq)]
pub struct Vendor {
    pub name: &'static str,
    pub file: &'static str,
    pub uri: &'static str,
    pub suite: &'static str,
    pub components: &'static str,
    pub key: &'static str,
    /// Package, name, summary, and whether it is a command-line program.
    pub packages: &'static [(&'static str, &'static str, &'static str, bool)],
}

pub const VENDORS: [Vendor; 3] = [
    Vendor {
        name: "Ookla",
        file: "ookla-speedtest",
        uri: "https://packagecloud.io/ookla/speedtest-cli/ubuntu/",
        suite: "jammy",
        components: "main",
        key: "https://packagecloud.io/ookla/speedtest-cli/gpgkey",
        packages: &[(
            "speedtest",
            "speedtest",
            "Speedtest by Ookla，在終端機測量網路速度",
            true,
        )],
    },
    Vendor {
        name: "Microsoft",
        file: "vscode",
        uri: "https://packages.microsoft.com/repos/code",
        suite: "stable",
        components: "main",
        key: "https://packages.microsoft.com/keys/microsoft.asc",
        packages: &[("code", "Visual Studio Code", "Microsoft 的程式碼編輯器", false)],
    },
    Vendor {
        name: "Google Chrome",
        file: "google-chrome",
        uri: "https://dl.google.com/linux/chrome/deb/",
        suite: "stable",
        components: "main",
        key: "https://dl.google.com/linux/linux_signing_key.pub",
        packages: &[(
            "google-chrome-stable",
            "Google Chrome",
            "Google 的網頁瀏覽器",
            false,
        )],
    },
];

/// Programs from repositories the store could add, for a search; one already added is
/// found through apt instead.
pub fn search_vendors(query: &str, repos: &[Repository]) -> Vec<App> {
    let query = query.to_lowercase();
    VENDORS
        .iter()
        .filter(|vendor| !vendor_added(vendor, repos))
        .flat_map(|vendor| vendor.packages.iter().map(move |package| (vendor, package)))
        .filter(|(_, (package, name, _, _))| package.contains(&query) || name.to_lowercase().contains(&query))
        .map(|(vendor, &(package, name, summary, cli))| App {
            key: package.to_string(),
            name: name.to_string(),
            summary: summary.to_string(),
            developer: vendor.name.to_string(),
            cli,
            offers: vec![vendor_offer(vendor, package, true)],
            ..Default::default()
        })
        .collect()
}

/// A package from a vendor's repository; `adding` when the repository is not added yet.
pub fn vendor_offer(vendor: &'static Vendor, package: &str, adding: bool) -> Offer {
    Offer {
        kind: Some(Kind::Apt),
        id: package.to_string(),
        origin: vendor.name.to_string(),
        via: "apt · 自家套件庫".into(),
        publisher: vendor.name.to_string(),
        verified: true,
        admin: true,
        repo: adding.then_some(vendor),
        ..Default::default()
    }
}

/// Whether a vendor's repository is among the ones apt already uses.
pub fn vendor_added(vendor: &Vendor, repos: &[Repository]) -> bool {
    repos.iter().any(|repo| same_place(&repo.uri, vendor.uri))
}

fn same_place(a: &str, b: &str) -> bool {
    let bare = |uri: &str| {
        uri.split_once("://")
            .map_or(uri, |(_, rest)| rest)
            .trim_end_matches('/')
            .to_string()
    };
    bare(a) == bare(b)
}

// Flatpak.

pub fn flatpak_installed() -> Vec<App> {
    let Some(text) = output(
        "flatpak",
        &["list", "--app", "--columns=application,name,version,size,origin"],
    ) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let (id, name, version, size, remote) = (
                parts.next()?,
                parts.next()?,
                parts.next()?,
                parts.next()?,
                parts.next()?,
            );
            Some(App {
                key: key_of(id),
                name: name.to_string(),
                icon: flatpak_icon(&format!("{id}.png")).or_else(|| installed_flatpak_icon(id)),
                offers: vec![Offer {
                    kind: Some(Kind::Flatpak),
                    id: id.to_string(),
                    origin: if remote == "flathub" {
                        "Flathub".into()
                    } else {
                        remote.to_string()
                    },
                    via: "flatpak".into(),
                    version: version.to_string(),
                    installed: Some(version.to_string()),
                    size: size.replace('\u{a0}', " "),
                    ..Default::default()
                }],
                ..Default::default()
            })
        })
        .collect()
}

fn installed_flatpak_icon(id: &str) -> Option<PathBuf> {
    let user = glib_home().join(".local/share/flatpak/exports/share/icons/hicolor");
    [
        PathBuf::from("/var/lib/flatpak/exports/share/icons/hicolor"),
        user,
    ]
    .iter()
    .flat_map(|root| ["128x128/apps", "64x64/apps", "scalable/apps"].map(|dir| root.join(dir)))
    .flat_map(|dir| ["png", "svg"].map(|ext| dir.join(format!("{id}.{ext}"))))
    .find(|path| path.exists())
}

/// Flatpak applications that have a newer version: (ID, remote).
pub fn flatpak_updates() -> Vec<(String, String)> {
    output(
        "flatpak",
        &["remote-ls", "--updates", "--app", "--columns=application,origin"],
    )
    .map(|text| {
        text.lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(id, remote)| (id.to_string(), remote.to_string()))
            .collect()
    })
    .unwrap_or_default()
}

// The Snap Store, through snapd's socket.

pub const SNAPD_SOCKET: &str = "/run/snapd.socket";

fn snapd(path: &str) -> Option<serde_json::Value> {
    let mut stream = UnixStream::connect(SNAPD_SOCKET).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(8))).ok()?;
    write!(stream, "GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let body = response.split_once("\r\n\r\n")?.1;
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if value["type"] == "error" {
        eprintln!("snapd {path}: {}", value["result"]["message"]);
        return None;
    }
    Some(value["result"].clone())
}

fn snap_app(snap: &serde_json::Value, installed: bool) -> Option<App> {
    let text = |key: &str| snap[key].as_str().unwrap_or_default().to_string();
    let name = text("name");
    if name.is_empty() || !matches!(snap["type"].as_str(), Some("app") | None) {
        return None;
    }
    let key = snap["common-ids"]
        .as_array()
        .and_then(|ids| ids.first())
        .and_then(|id| id.as_str())
        .map(key_of)
        // Without one, a snap of the same name may be someone else's program.
        .unwrap_or_else(|| format!("snap:{name}"));
    if installed && snap["apps"].as_array().is_none_or(|apps| apps.is_empty()) {
        return None;
    }
    let publisher = &snap["publisher"];
    let size = snap["download-size"]
        .as_u64()
        .or(snap["installed-size"].as_u64())
        .map(human_size)
        .unwrap_or_default();
    let icon = installed
        .then(|| PathBuf::from(format!("/snap/{name}/current/meta/gui/icon.png")))
        .filter(|path| path.exists());
    let title = text("title");
    Some(App {
        key,
        name: if title.is_empty() { name.clone() } else { title },
        summary: text("summary"),
        description: text("description"),
        developer: publisher["display-name"].as_str().unwrap_or_default().to_string(),
        homepage: text("website"),
        license: text("license"),
        icon,
        offers: vec![Offer {
            kind: Some(Kind::Snap),
            id: name,
            origin: "Snap Store".into(),
            via: "snap".into(),
            version: text("version"),
            installed: installed.then(|| text("version")),
            size,
            publisher: publisher["display-name"].as_str().unwrap_or_default().to_string(),
            verified: matches!(publisher["validation"].as_str(), Some("verified" | "starred")),
            admin: true,
            classic: snap["confinement"] == "classic",
            ..Default::default()
        }],
        ..Default::default()
    })
}

pub fn search_snaps(query: &str) -> Vec<App> {
    let query: String = query
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();
    snapd(&format!("/v2/find?q={query}"))
        .and_then(|result| result.as_array().cloned())
        .map(|snaps| {
            snaps
                .iter()
                .take(10)
                .filter_map(|snap| snap_app(snap, false))
                .collect()
        })
        .unwrap_or_default()
}

pub fn snaps_installed() -> Vec<App> {
    snapd("/v2/snaps")
        .and_then(|result| result.as_array().cloned())
        .map(|snaps| snaps.iter().filter_map(|snap| snap_app(snap, true)).collect())
        .unwrap_or_default()
}

/// Snaps with a newer revision, and when snapd next refreshes them.
pub fn snap_updates() -> (Vec<String>, String) {
    let names = snapd("/v2/find?select=refresh")
        .and_then(|result| result.as_array().cloned())
        .map(|snaps| {
            snaps
                .iter()
                .filter_map(|snap| snap["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let next = output("snap", &["refresh", "--time"])
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("next: ").map(str::to_string))
        })
        .unwrap_or_default();
    (names, next)
}

// Homebrew.

fn brew_app(formula: &serde_json::Value, cask: bool) -> Option<App> {
    let text = |value: &serde_json::Value| value.as_str().unwrap_or_default().to_string();
    let name = if cask {
        text(&formula["token"])
    } else {
        text(&formula["name"])
    };
    if name.is_empty() {
        return None;
    }
    let version = if cask {
        text(&formula["version"])
    } else {
        text(&formula["versions"]["stable"])
    };
    let installed = if cask {
        formula["installed"].as_str().map(str::to_string)
    } else {
        formula["installed"]
            .as_array()
            .and_then(|list| list.last())
            .map(|i| text(&i["version"]))
    };
    let tap = text(&formula["tap"]);
    let display = if cask {
        formula["name"][0].as_str().unwrap_or(&name).to_string()
    } else {
        name.clone()
    };
    Some(App {
        key: name.clone(),
        name: display,
        summary: text(&formula["desc"]),
        homepage: text(&formula["homepage"]),
        license: text(&formula["license"]),
        cli: !cask,
        offers: vec![Offer {
            kind: Some(Kind::Brew),
            id: if tap == "homebrew/core" || tap == "homebrew/cask" || tap.is_empty() {
                name.clone()
            } else {
                format!("{tap}/{name}")
            },
            origin: "Homebrew".into(),
            via: if tap == "homebrew/core" || tap.is_empty() {
                "brew".into()
            } else {
                tap
            },
            version,
            installed,
            publisher: "Homebrew".into(),
            cask,
            ..Default::default()
        }],
        ..Default::default()
    })
}

/// Formulae and casks out of `brew info --json=v2`.
pub fn parse_brew_info(text: &str) -> Vec<App> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let formulae = value["formulae"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| brew_app(f, false));
    let casks = value["casks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| brew_app(c, true));
    formulae.chain(casks).collect()
}

pub fn search_brew(query: &str) -> Vec<App> {
    let Some(names) = output("brew", &["search", "--formula", query]) else {
        return Vec::new();
    };
    let names: Vec<&str> = names
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with("==>"))
        .take(10)
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    let mut args = vec!["info", "--json=v2"];
    args.extend(&names);
    output("brew", &args)
        .map(|text| parse_brew_info(&text))
        .unwrap_or_default()
}

pub fn brew_installed() -> Vec<App> {
    output("brew", &["info", "--json=v2", "--installed"])
        .map(|text| {
            // Only what was asked for, not what came along as a dependency.
            let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            let asked: HashSet<String> = value["formulae"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|f| {
                    f["installed"][0]["installed_on_request"]
                        .as_bool()
                        .unwrap_or(true)
                })
                .filter_map(|f| f["name"].as_str().map(str::to_string))
                .collect();
            parse_brew_info(&text)
                .into_iter()
                .filter(|app| app.offers[0].cask || asked.contains(&app.key))
                .collect()
        })
        .unwrap_or_default()
}

pub fn brew_updates() -> Vec<String> {
    output("brew", &["outdated", "--json=v2"])
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .map(|value| {
            ["formulae", "casks"]
                .iter()
                .flat_map(|kind| value[kind].as_array().cloned().unwrap_or_default())
                .filter_map(|item| item["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// apt, installed and upgradable.

pub fn apt_installed() -> Vec<App> {
    let Some(manual) = output("apt-mark", &["showmanual"]) else {
        return Vec::new();
    };
    // What a person installed, not the system's own packages: the ones with a window.
    let desktop: HashSet<String> = output("dpkg-query", &["-S", "/usr/share/applications/*.desktop"])
        .map(|text| {
            text.lines()
                .filter_map(|line| line.split_once(": "))
                .flat_map(|(packages, _)| {
                    packages
                        .split(", ")
                        .map(|p| p.split(':').next().unwrap_or(p).to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<&str> = manual.lines().filter(|name| desktop.contains(*name)).collect();
    let mut args = vec![
        "-W",
        "-f",
        "${Package}\t${Version}\t${Installed-Size}\t${binary:Summary}\n",
    ];
    args.extend(&names);
    let repos = repositories();
    let mut apps: Vec<App> = output("dpkg-query", &args)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let (name, version, size, summary) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            Some(App {
                key: name.to_string(),
                name: name.to_string(),
                summary: summary.to_string(),
                offers: vec![Offer {
                    kind: Some(Kind::Apt),
                    id: name.to_string(),
                    origin: UBUNTU.into(),
                    via: "apt".into(),
                    version: version.to_string(),
                    installed: Some(version.to_string()),
                    size: size
                        .parse::<u64>()
                        .map(|kib| human_size(kib * 1024))
                        .unwrap_or_default(),
                    publisher: UBUNTU.into(),
                    admin: true,
                    ..Default::default()
                }],
                ..Default::default()
            })
        })
        .collect();
    // Which repository each came from, by the version apt would install now.
    if !repos.is_empty() {
        let mut args = vec!["policy"];
        args.extend(apps.iter().map(|app| app.key.as_str()));
        let policy = output("apt-cache", &args)
            .map(|text| parse_policy(&text))
            .unwrap_or_default();
        for app in &mut apps {
            if let Some(repo) = policy.get(&app.key).and_then(|(_, _, from)| {
                repos
                    .iter()
                    .find(|r| from.starts_with(r.uri.trim_end_matches('/')))
            }) {
                app.offers[0].origin = repo.name.clone();
                app.offers[0].via = "apt · 自家套件庫".into();
                app.offers[0].publisher = repo.name.clone();
            }
        }
    }
    apps
}

/// `apt list --upgradable`: each package and the version it would go to.
pub fn parse_upgradable(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .filter(|line| line.contains("[upgradable from: "))
        .filter_map(|line| {
            let (name, rest) = line.split_once('/')?;
            let new = rest.split(' ').nth(1)?;
            let old = line.split("[upgradable from: ").nth(1)?.trim_end_matches(']');
            Some((name.to_string(), old.to_string(), new.to_string()))
        })
        .collect()
}

pub fn apt_updates() -> Vec<(String, String, String)> {
    output("apt", &["list", "--upgradable"])
        .map(|text| parse_upgradable(&text))
        .unwrap_or_default()
}

// Installing, removing and updating.

/// What to run for a change: commands that take the administrator's password all go in
/// one `sudo`, so that one password covers them.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Plan {
    pub user: Vec<Vec<String>>,
    pub admin: Vec<Vec<String>>,
    /// The person's own commands that wait for the administrator's, as removing what an
    /// install replaces does.
    pub after: Vec<Vec<String>>,
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| w.to_string()).collect()
}

/// apt-get as Market runs it, with nothing to answer: a configuration file changed
/// both here and in the new version keeps the one here, as dpkg would otherwise ask on a
/// closed input and leave the package half set up.
/// Its status lines go to what it prints, where `run_one` reads them for the progress.
pub const APT_GET: [&str; 12] = [
    "env",
    "DEBIAN_FRONTEND=noninteractive",
    "apt-get",
    "-y",
    "-o",
    "Dpkg::Options::=--force-confdef",
    "-o",
    "Dpkg::Options::=--force-confold",
    "-o",
    APT_LOCK_WAIT,
    "-o",
    "APT::Status-Fd=1",
];

/// apt fails at once while another apt holds its lock, as a second install started before
/// the first ends would; this has it wait for that one instead, for up to ten minutes.
const APT_LOCK_WAIT: &str = "DPkg::Lock::Timeout=600";

fn apt_update() -> Vec<String> {
    words(&["apt-get", "-o", APT_LOCK_WAIT, "update"])
}

pub fn install_plan(offer: &Offer) -> Plan {
    let mut plan = Plan::default();
    match offer.kind {
        Some(Kind::Apt) => {
            if let Some(vendor) = offer.repo {
                plan.admin.extend(add_repository(vendor));
            }
            plan.admin.push(
                [&APT_GET[..], &["install", &offer.id]]
                    .concat()
                    .iter()
                    .map(|w| w.to_string())
                    .collect(),
            );
        }
        Some(Kind::Flatpak) => {
            // Into the person's own installation, which takes no password.
            plan.user.push(words(&[
                "flatpak",
                "remote-add",
                "--user",
                "--if-not-exists",
                "flathub",
                "https://dl.flathub.org/repo/flathub.flatpakrepo",
            ]));
            plan.user.push(words(&[
                "flatpak",
                "install",
                "--user",
                "-y",
                "--noninteractive",
                "flathub",
                &offer.id,
            ]));
        }
        Some(Kind::Snap) => {
            let mut command = words(&["snap", "install", &offer.id]);
            if offer.classic {
                command.push("--classic".into());
            }
            plan.admin.push(command);
        }
        Some(Kind::Brew) => {
            let mut command = words(&["brew", "install"]);
            if offer.cask {
                command.push("--cask".into());
            }
            command.push(offer.id.clone());
            plan.user.push(command);
        }
        None => {}
    }
    plan
}

pub fn remove_plan(offer: &Offer) -> Plan {
    let mut plan = Plan::default();
    match offer.kind {
        Some(Kind::Apt) => plan.admin.push(
            [&APT_GET[..], &["remove", &offer.id]]
                .concat()
                .iter()
                .map(|w| w.to_string())
                .collect(),
        ),
        Some(Kind::Flatpak) => {
            // Installed for the person or for everyone; flatpak finds which, though the
            // second asks for the password through polkit, which sudo stands in for.
            let user = output("flatpak", &["list", "--user", "--app", "--columns=application"])
                .is_some_and(|t| t.lines().any(|l| l == offer.id));
            if user {
                plan.user.push(words(&[
                    "flatpak",
                    "uninstall",
                    "--user",
                    "-y",
                    "--noninteractive",
                    &offer.id,
                ]));
            } else {
                plan.admin.push(words(&[
                    "flatpak",
                    "uninstall",
                    "--system",
                    "-y",
                    "--noninteractive",
                    &offer.id,
                ]));
            }
        }
        Some(Kind::Snap) => plan.admin.push(words(&["snap", "remove", &offer.id])),
        Some(Kind::Brew) => plan.user.push(words(&["brew", "uninstall", &offer.id])),
        None => {}
    }
    plan
}

/// The commands that add a vendor's repository: its key, then its sources file.
pub fn add_repository(vendor: &Vendor) -> Vec<Vec<String>> {
    let keyring = format!("/etc/apt/keyrings/{}.gpg", vendor.file);
    let sources = format!(
        "Types: deb\nURIs: {}\nSuites: {}\nComponents: {}\nSigned-By: {keyring}\nX-Repolib-Name: {}\n",
        vendor.uri, vendor.suite, vendor.components, vendor.name
    );
    vec![
        words(&["install", "-d", "-m", "0755", "/etc/apt/keyrings"]),
        words(&[
            "sh",
            "-c",
            &format!(
                "curl -fsSL {} | gpg --dearmor --yes -o {keyring}",
                shell_quote(vendor.key)
            ),
        ]),
        words(&[
            "sh",
            "-c",
            &format!(
                "printf %s {} > /etc/apt/sources.list.d/{}.sources",
                shell_quote(&sources),
                vendor.file
            ),
        ]),
        apt_update(),
    ]
}

pub fn remove_repository(repo: &Repository) -> Plan {
    Plan {
        user: Vec::new(),
        after: Vec::new(),
        admin: vec![
            words(&["rm", "-f", &repo.file.to_string_lossy()]),
            apt_update(),
        ],
    }
}

pub fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@,+".contains(c))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// Runs a plan: the person's own commands first, then the administrator's under one sudo,
/// whose password the askpass program asks for, then the person's that wait for those. The
/// first failure stops it, and what the failing command said is returned, or nothing if the
/// password was not given.
pub fn run(plan: &Plan, reason: &str) -> Result<(), String> {
    run_watched(plan, reason, &Watch::default())
}

/// As `run`, saying in `watch` how far it has got where apt or snapd tell.
pub fn run_watched(plan: &Plan, reason: &str, watch: &Watch) -> Result<(), String> {
    for command in &plan.user {
        run_one(Command::new(&command[0]).args(&command[1..]), command, watch)?;
    }
    if !plan.admin.is_empty() {
        run_admin(&plan.admin, reason, watch)?;
    }
    for command in &plan.after {
        run_one(Command::new(&command[0]).args(&command[1..]), command, watch)?;
    }
    Ok(())
}

/// How far a running change has got: downloading or installing, and how much of that.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub downloading: bool,
    pub fraction: f64,
}

/// What a change has said of its progress so far, None until it says anything.
pub type Watch = Arc<Mutex<Option<Progress>>>;

fn report(watch: &Watch, progress: Progress) {
    if let Ok(mut held) = watch.lock() {
        *held = Some(progress);
    }
}

/// apt's own status line, written to APT::Status-Fd: `dlstatus:` while it downloads and
/// `pmstatus:` while dpkg installs, each with the percentage done as its third field.
fn apt_progress(line: &str) -> Option<Progress> {
    let mut fields = line.splitn(4, ':');
    let downloading = match fields.next()? {
        "dlstatus" => true,
        "pmstatus" => false,
        _ => return None,
    };
    let percent: f64 = fields.nth(1)?.parse().ok()?;
    Some(Progress {
        downloading,
        fraction: (percent / 100.0).clamp(0.0, 1.0),
    })
}

/// How often snapd is asked about a snap being installed.
const SNAP_POLL_INTERVAL: Duration = Duration::from_millis(500);
const SNAP_DOWNLOAD_TASK: &str = "download-snap";

/// The progress of the snap change in progress, from snapd: its download by bytes while
/// that runs, then its remaining tasks by count.
fn snap_progress() -> Option<Progress> {
    let changes = snapd("/v2/changes?select=in-progress")?;
    let change = changes
        .as_array()?
        .iter()
        .find(|change| change["kind"].as_str().is_some_and(|kind| kind.ends_with("-snap")))?;
    let tasks = change["tasks"].as_array()?;
    let downloading = tasks.iter().find(|task| {
        task["kind"] == SNAP_DOWNLOAD_TASK && task["status"] == "Doing"
    });
    if let Some(task) = downloading {
        let done = task["progress"]["done"].as_f64()?;
        let total = task["progress"]["total"].as_f64().filter(|total| *total > 0.0)?;
        return Some(Progress {
            downloading: true,
            fraction: (done / total).clamp(0.0, 1.0),
        });
    }
    let finished = tasks.iter().filter(|task| task["status"] == "Done").count();
    Some(Progress {
        downloading: false,
        fraction: finished as f64 / tasks.len().max(1) as f64,
    })
}

/// Asks snapd about the change in progress until `running` is cleared.
fn watch_snap(watch: &Watch, running: &AtomicBool) {
    while running.load(Ordering::Relaxed) {
        if let Some(progress) = snap_progress() {
            report(watch, progress);
        }
        std::thread::sleep(SNAP_POLL_INTERVAL);
    }
}

fn run_admin(admin: &[Vec<String>], reason: &str, watch: &Watch) -> Result<(), String> {
    let script = admin
        .iter()
        .map(|command| {
            command
                .iter()
                .map(|word| shell_quote(word))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" && ");
    let askpass = crate::program::path("nekoland-askpass");
    let mut sudo = Command::new("sudo");
    sudo.env("SUDO_ASKPASS", askpass)
        .env("NEKOLAND_ASKPASS_REASON", reason)
        .args(["-A", "--", "sh", "-c", &script]);
    let shown = words(&["sudo", "sh", "-c", &script]);
    let installs_snap = admin.iter().any(|command| command.starts_with(&words(&["snap", "install"])));
    if !installs_snap {
        return run_one(&mut sudo, &shown, watch);
    }
    // snap says nothing of its progress unless on a terminal; snapd tells it instead.
    let running = AtomicBool::new(true);
    std::thread::scope(|scope| {
        scope.spawn(|| watch_snap(watch, &running));
        let result = run_one(&mut sudo, &shown, watch);
        running.store(false, Ordering::Relaxed);
        result
    })
}

/// Runs a command, reading apt's status lines from what it prints as it prints them.
fn run_one(command: &mut Command, words: &[String], watch: &Watch) -> Result<(), String> {
    let spawn_failed = |err: std::io::Error| format!("{}: {err}", words[0]);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_failed)?;
    let (stdout, mut stderr) = (child.stdout.take(), child.stderr.take());
    // Read apart, so that neither pipe fills while the other is read.
    let said = std::thread::scope(|scope| {
        let errors = scope.spawn(move || {
            let mut text = String::new();
            if let Some(stderr) = stderr.as_mut() {
                if let Err(err) = stderr.read_to_string(&mut text) {
                    eprintln!("{}: {err}", words[0]);
                }
            }
            text
        });
        for line in stdout.into_iter().flat_map(|out| BufReader::new(out).lines().map_while(Result::ok)) {
            if let Some(progress) = apt_progress(&line) {
                report(watch, progress);
            }
        }
        errors.join().unwrap_or_default()
    });
    let status = child.wait().map_err(spawn_failed)?;
    if status.success() {
        return Ok(());
    }
    // The password dialog was cancelled: not a failure to report.
    if said.contains("no password was provided") || said.contains("a password is required") {
        return Err(String::new());
    }
    Err(explain_failure(&said))
}

/// Why a program failed, from what it said: in plain words where apt or dpkg said something
/// known, else the last thing it said, which is usually why.
pub fn explain_failure(said: &str) -> String {
    plain_apt_error(said).unwrap_or_else(|| {
        said.lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("W:"))
            .unwrap_or("失敗")
            .trim_start_matches("E: ")
            .trim_start_matches("error: ")
            .to_string()
    })
}

/// Failures of apt and dpkg, by what they say of them in English, in plain words.
const PLAIN_APT_ERRORS: [(&str, &str); 11] = [
    ("not a Debian format archive", "檔案損毀或不是 .deb 套件"),
    ("not a debian format archive", "檔案損毀或不是 .deb 套件"),
    ("Invalid archive signature", "檔案損毀或不是 .deb 套件"),
    ("unexpected end of file", "檔案不完整"),
    ("does not match system", "套件的架構與這部電腦不符"),
    ("No space left on device", "磁碟空間不足"),
    ("dpkg was interrupted", "先前的安裝沒有完成，請在終端機執行 sudo dpkg --configure -a 後重試"),
    ("Could not get lock", "另一個程式正在安裝軟體"),
    ("Unable to acquire the dpkg frontend lock", "另一個程式正在安裝軟體"),
    ("unmet dependencies", "相依套件無法安裝"),
    ("Unable to satisfy dependencies", "相依套件無法安裝"),
];

/// What apt or dpkg said of a failure a person can act on, in plain words; None for the rest.
pub fn plain_apt_error(said: &str) -> Option<String> {
    if let Some(dependency) = unmet_dependency(said) {
        return Some(format!("無法安裝相依套件 {dependency}"));
    }
    if let Some(owner) = overwritten_package(said) {
        return Some(format!("與已安裝的 {owner} 有檔案衝突"));
    }
    PLAIN_APT_ERRORS
        .iter()
        .find(|(english, _)| said.contains(english))
        .map(|(_, plain)| plain.to_string())
}

/// The first dependency apt could not satisfy, from ` package : Depends: name (>= 2) but …`.
fn unmet_dependency(said: &str) -> Option<&str> {
    said.lines()
        .filter(|line| line.contains(" : ") || line.starts_with("  "))
        .find_map(|line| line.split_once("Depends: "))
        .and_then(|(_, rest)| rest.split([' ', ',']).next())
        .filter(|name| !name.is_empty())
}

/// The installed package whose file dpkg would not overwrite, from `trying to overwrite
/// '/usr/bin/x', which is also in package name 1.0`.
fn overwritten_package(said: &str) -> Option<&str> {
    said.lines()
        .filter(|line| line.contains("trying to overwrite"))
        .find_map(|line| line.split_once("which is also in package "))
        .and_then(|(_, rest)| rest.split(' ').next())
}

/// The commands an installed command-line program put on the PATH.
pub fn commands(offer: &Offer) -> Vec<String> {
    let files = match offer.kind {
        Some(Kind::Apt) => output("dpkg", &["-L", &offer.id]),
        Some(Kind::Brew) => output("brew", &["list", "--formula", &offer.id]),
        _ => None,
    };
    let mut found: Vec<String> = files
        .unwrap_or_default()
        .lines()
        .filter(|path| path.contains("/bin/") && !path.ends_with('/'))
        .filter_map(|path| path.rsplit('/').next().map(str::to_string))
        .collect();
    found.dedup();
    found
}

/// A screenshot's bytes, fetched by curl, which also follows the site's redirects.
pub fn fetch(url: &str) -> Option<Vec<u8>> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "20", "--max-filesize", "20000000", url])
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Every source's search at once, put together.
pub fn search(query: &str) -> Vec<App> {
    let (appstream, snaps, brew, names) = std::thread::scope(|scope| {
        let appstream = scope.spawn(|| search_appstream(query));
        let snaps = scope.spawn(|| search_snaps(query));
        let brew = scope.spawn(|| search_brew(query));
        let names = scope.spawn(|| search_apt_names(query));
        (
            appstream.join().unwrap_or_default(),
            snaps.join().unwrap_or_default(),
            brew.join().unwrap_or_default(),
            names.join().unwrap_or_default(),
        )
    });
    let repos = repositories();
    // A package AppStream already knows, and its data, help and translation packages, are
    // not shown again by their names.
    let known: Vec<String> = appstream
        .iter()
        .flat_map(|app| &app.offers)
        .filter(|o| o.kind == Some(Kind::Apt))
        .map(|o| o.id.clone())
        .collect();
    let names = names.into_iter().filter(|app| {
        !known
            .iter()
            .any(|k| app.key == *k || app.key.starts_with(&format!("{k}-")))
    });
    let mut apps = merge(
        appstream
            .into_iter()
            .chain(snaps)
            .chain(names)
            .chain(search_vendors(query, &repos))
            .chain(brew),
    );
    fill_apt(&mut apps);
    // One whose offers all dropped out, a package apt no longer has, is not shown.
    apps.retain(|app| !app.offers.is_empty());
    // Closer names first: the query itself, then names that start with it.
    let lower = query.to_lowercase();
    apps.sort_by_key(|app| {
        let name = app.name.to_lowercase();
        let exact = name == lower
            || app.key == lower
            || app.key.rsplit('.').next() == Some(&lower)
            || app.offers.iter().any(|o| o.id.to_lowercase() == lower);
        (!exact, !name.starts_with(&lower), app.cli)
    });
    apps
}

/// Everything installed, by source, in the order the Installed page shows them.
pub fn installed() -> BTreeMap<u8, Vec<App>> {
    let (flatpak, snap, apt, brew) = std::thread::scope(|scope| {
        let flatpak = scope.spawn(flatpak_installed);
        let snap = scope.spawn(snaps_installed);
        let apt = scope.spawn(apt_installed);
        let brew = scope.spawn(brew_installed);
        (
            flatpak.join().unwrap_or_default(),
            snap.join().unwrap_or_default(),
            apt.join().unwrap_or_default(),
            brew.join().unwrap_or_default(),
        )
    });
    BTreeMap::from([(0, flatpak), (1, snap), (2, apt), (3, brew)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appstream_blocks_become_offers_merged_by_id() {
        let text = "Identifier: org.inkscape.Inkscape [desktop-application]\nName: Inkscape\nSummary: Vector graphics\nPackage: inkscape\nDeveloper: The Inkscape Community\nDescription: \n  Draw freely.\n  \n  Shapes too.\nCategories: \n  - Graphics\n---\nIdentifier: org.inkscape.Inkscape [desktop-application]\nName: Inkscape\nBundle: flatpak:app/org.inkscape.Inkscape/x86_64/stable\nDeveloper: The Inkscape Community\n---\nIdentifier: fonts-x [font]\nName: X\nPackage: fonts-x\n";
        let blocks = parse_appstream(text);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["Description"], "Draw freely.\n\nShapes too.");
        assert_eq!(blocks[0]["Categories"], "Graphics");
        let apps = merge(blocks.iter().filter_map(appstream_app));
        assert_eq!(apps.len(), 1);
        let kinds: Vec<_> = apps[0].offers.iter().map(|o| o.kind).collect();
        assert_eq!(kinds, [Some(Kind::Flatpak), Some(Kind::Apt)]);
        assert_eq!(apps[0].summary, "Vector graphics");
    }

    #[test]
    fn same_name_from_different_makers_stays_apart() {
        let ookla = search_vendors("speedtest", &[]);
        assert_eq!(ookla.len(), 1);
        let python = App {
            key: "speedtest-cli".into(),
            ..Default::default()
        };
        assert_eq!(merge([ookla[0].clone(), python]).len(), 2);
    }

    #[test]
    fn policy_gives_versions_and_where_from() {
        let text = "google-chrome-stable:\n  Installed: 157.0-1\n  Candidate: 157.0-1\n  Version table:\n *** 157.0-1 500\n        500 https://dl.google.com/linux/chrome-stable/deb stable/main amd64 Packages\n        100 /var/lib/dpkg/status\nripgrep:\n  Installed: (none)\n  Candidate: 15.2.0-1\n  Version table:\n     15.2.0-1 500\n        500 http://archive.ubuntu.com/ubuntu stonking/universe amd64 Packages\ngone:\n  Installed: (none)\n  Candidate: (none)\n";
        let policy = parse_policy(text);
        assert_eq!(
            policy["google-chrome-stable"],
            (
                Some("157.0-1".into()),
                "157.0-1".into(),
                "https://dl.google.com/linux/chrome-stable/deb".into()
            )
        );
        assert_eq!(policy["ripgrep"].0, None);
        assert!(!policy.contains_key("gone"));
    }

    #[test]
    fn sources_files_name_their_repositories() {
        let deb822 = "X-Repolib-Name: Google Chrome\nTypes: deb\nURIs: https://dl.google.com/linux/chrome-stable/deb/\nSuites: stable\nComponents: main\n";
        let repos = parse_sources(Path::new("/x/google-chrome.sources"), deb822);
        assert_eq!(repos[0].name, "Google Chrome");
        let list =
            "# comment\ndeb [arch=amd64 signed-by=/k.gpg] https://downloads.claude.ai/apt stable main\n";
        assert_eq!(
            parse_sources(Path::new("/x/claude-desktop.list"), list)[0].name,
            "Claude Desktop"
        );
        let ubuntu = "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu/\nSuites: stonking\n";
        assert!(parse_sources(Path::new("/x/ubuntu.sources"), ubuntu).is_empty());
        // Ubuntu's archive from a mirror elsewhere is still not anyone else's to remove.
        let mirror = "Types: deb\nURIs: http://free.nchc.org.tw/ubuntu/\nSuites: stonking\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n";
        assert!(parse_sources(Path::new("/x/ubuntu.sources"), mirror).is_empty());
        assert!(parse_sources(Path::new("/x/mirror.sources"), mirror).is_empty());
        let two = "deb https://a.example/apt stable main\ndeb https://b.example/apt stable main\n";
        assert_eq!(every_source(Path::new("/x/two.list"), two).len(), 2);
        let ppa = "deb https://ppa.launchpadcontent.net/mozillateam/ppa/ubuntu/ noble main\n";
        assert_eq!(
            parse_sources(Path::new("/x/m.list"), ppa)[0].name,
            "PPA mozillateam/ppa"
        );
    }

    #[test]
    fn upgradable_lines_give_both_versions() {
        let text = "Listing...\nfirefox/stonking-updates 146.0.1 amd64 [upgradable from: 146.0]\n";
        assert_eq!(
            parse_upgradable(text),
            [("firefox".into(), "146.0".into(), "146.0.1".into())]
        );
    }

    #[test]
    fn plans_ask_for_the_password_only_where_needed() {
        let flatpak = Offer {
            kind: Some(Kind::Flatpak),
            id: "org.gimp.GIMP".into(),
            ..Default::default()
        };
        assert!(install_plan(&flatpak).admin.is_empty());
        let vendor = &VENDORS[0];
        let apt = Offer {
            kind: Some(Kind::Apt),
            id: "speedtest".into(),
            repo: Some(vendor),
            ..Default::default()
        };
        let plan = install_plan(&apt);
        assert!(plan.user.is_empty());
        assert_eq!(plan.admin.last().unwrap().last().unwrap(), "speedtest");
        assert!(plan
            .admin
            .iter()
            .any(|c| c.join(" ").contains("packagecloud.io/ookla/speedtest-cli/gpgkey")));
    }

    #[test]
    fn shell_quoting_keeps_words_whole() {
        assert_eq!(shell_quote("apt-get"), "apt-get");
        assert_eq!(shell_quote("it's here"), r"'it'\''s here'");
    }

    #[test]
    fn apt_failures_are_said_plainly() {
        let unmet = "The following packages have unmet dependencies:\n neko-missing : Depends: nonexistent-pkg-xyz (>= 2) but it is not installable\nE: Unable to satisfy dependencies. Reached two conflicting assignments:\n   1. neko-missing:amd64=1.0 is selected for install\n      [no choices]\n";
        assert_eq!(explain_failure(unmet), "無法安裝相依套件 nonexistent-pkg-xyz");
        let overwrite = "dpkg: error processing archive /tmp/x.deb (--unpack):\n trying to overwrite '/usr/bin/hello', which is also in package hello-traditional 2.10-5\nE: Sub-process /usr/bin/dpkg returned an error code (1)\n";
        assert_eq!(explain_failure(overwrite), "與已安裝的 hello-traditional 有檔案衝突");
        assert_eq!(
            explain_failure("dpkg-deb: error: 'bad.deb' is not a Debian format archive\n"),
            "檔案損毀或不是 .deb 套件"
        );
        // Anything else is shown as the program said it.
        assert_eq!(explain_failure("W: a warning\nE: Something odd\nW: later\n"), "Something odd");
        assert_eq!(explain_failure(""), "失敗");
    }

    #[test]
    fn apt_status_lines_give_the_stage_and_share_done() {
        assert_eq!(
            apt_progress("dlstatus:3:42.5:Retrieving file 3 of 9"),
            Some(Progress { downloading: true, fraction: 0.425 })
        );
        assert_eq!(
            apt_progress("pmstatus:epiphany-browser:100:Installed epiphany-browser"),
            Some(Progress { downloading: false, fraction: 1.0 })
        );
        assert_eq!(apt_progress("Reading package lists..."), None);
    }
}
