//! 儲存空間: the disks, and what can be cleaned the way Mole cleans: the caches apps make
//! again by themselves, from a fixed list, and the trash. A cache is left while a program
//! that uses it runs.

use super::*;
use std::{
    collections::HashSet,
    ffi::OsStr,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
};

/// The groups the caches are shown in, by `Cache::section`.
const SECTIONS: [&str; 4] = ["瀏覽器快取", "應用程式快取", "開發工具快取", "Flatpak 快取"];
/// What Electron apps keep in their folder in ~/.config that they make again (Mole's list).
const ELECTRON_LEAVES: [&str; 7] =
    ["Cache", "Code Cache", "GPUCache", "DawnCache", "DawnGraphiteCache", "DawnWebGPUCache", "Crashpad/completed"];
/// What editors built on VS Code keep besides.
const EDITOR_LEAVES: [&str; 3] = ["CachedData", "CachedExtensionVSIXs", "logs"];

/// Folders whose contents an app makes again, shown as one row.
#[derive(Clone, Debug)]
struct Cache {
    section: usize,
    label: String,
    roots: Vec<PathBuf>,
    /// Process names that use it; a name ending in `*` is any name it starts.
    owners: Vec<String>,
    size: u64,
    running: bool,
}

struct Found {
    caches: Vec<Cache>,
    trash: u64,
}

#[derive(Default)]
struct Shown {
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    scanning: Cell<bool>,
}

pub(crate) fn storage_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    if let Some(storage) = storage_group() {
        page.add(&storage);
    }
    let shown = Rc::new(Shown::default());
    // Measured each time the page is shown, as apps fill their caches again.
    page.connect_map(move |page| scan(page, &shown));
    page
}

fn scan(page: &adw::PreferencesPage, shown: &Rc<Shown>) {
    if shown.scanning.replace(true) {
        return;
    }
    let waiting = group("");
    waiting.add(&adw::ActionRow::builder().title("可清除的項目").subtitle("正在計算").build());
    replace(page, shown, vec![waiting]);
    let (page, shown) = (page.downgrade(), shown.clone());
    glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(|| find(&glib::home_dir())).await;
        shown.scanning.set(false);
        let Some(page) = page.upgrade() else { return };
        match found {
            Ok(found) => show(&page, &shown, found),
            Err(_) => {
                let failed = group("");
                failed.add(&adw::ActionRow::builder().title("可清除的項目").subtitle("無法計算").build());
                replace(&page, &shown, vec![failed]);
            }
        }
    });
}

fn replace(page: &adw::PreferencesPage, shown: &Shown, groups: Vec<adw::PreferencesGroup>) {
    for old in shown.groups.replace(Vec::new()) {
        page.remove(&old);
    }
    for new in &groups {
        page.add(new);
    }
    shown.groups.replace(groups);
}

fn show(page: &adw::PreferencesPage, shown: &Rc<Shown>, found: Found) {
    let caches: Vec<Cache> = found.caches.into_iter().filter(|cache| cache.size > 0).collect();
    if caches.is_empty() && found.trash == 0 {
        let none = group("");
        none.add(&adw::ActionRow::builder().title("沒有可清除的項目").css_classes(["dim-label"]).build());
        return replace(page, shown, vec![none]);
    }
    let clean = gtk::Button::builder()
        .halign(gtk::Align::Center)
        .css_classes(["destructive-action", "pill"])
        .build();
    // Each check with the cache it chooses, None for the trash.
    let checks: Rc<RefCell<Vec<(gtk::CheckButton, Option<Cache>)>>> = Rc::default();
    let total = {
        let (checks, clean) = (checks.clone(), clean.clone());
        let trash = found.trash;
        move || {
            let size: u64 = checks
                .borrow()
                .iter()
                .filter(|(check, _)| check.is_active())
                .map(|(_, cache)| cache.as_ref().map_or(trash, |cache| cache.size))
                .sum();
            clean.set_label(&format!("清理（{}）", glib::format_size(size)));
            clean.set_sensitive(size > 0);
        }
    };
    let mut groups = Vec::new();
    let add_row = |rows: &adw::PreferencesGroup, title: &str, size: u64, running: bool, active: bool, cache: Option<Cache>| {
        let check = gtk::CheckButton::builder()
            .active(active && !running)
            .sensitive(!running)
            .valign(gtk::Align::Center)
            .build();
        let subtitle = glib::format_size(size).to_string() + if running { "，執行中" } else { "" };
        let row = adw::ActionRow::builder()
            .title(title)
            .use_markup(false)
            .subtitle(subtitle)
            .activatable_widget(&check)
            .sensitive(!running)
            .build();
        row.add_prefix(&check);
        let total = total.clone();
        check.connect_toggled(move |_| total());
        rows.add(&row);
        checks.borrow_mut().push((check, cache));
    };
    if found.trash > 0 {
        let trash = group("");
        // What is in the trash was thrown away, but is not made again: it is chosen by hand.
        add_row(&trash, "垃圾桶", found.trash, false, false, None);
        groups.push(trash);
    }
    for (section, title) in SECTIONS.iter().enumerate() {
        let rows = group(title);
        let mut any = false;
        for cache in caches.iter().filter(|cache| cache.section == section) {
            add_row(&rows, &cache.label, cache.size, cache.running, true, Some(cache.clone()));
            any = true;
        }
        if any {
            groups.push(rows);
        }
    }
    total();
    let end = adw::PreferencesGroup::new();
    end.add(&clean);
    groups.push(end);
    replace(page, shown, groups);

    let (page, shown) = (page.downgrade(), shown.clone());
    clean.connect_clicked(move |clean| {
        let chosen: Vec<Option<Cache>> = checks
            .borrow()
            .iter()
            .filter(|(check, _)| check.is_active())
            .map(|(_, cache)| cache.clone())
            .collect();
        let trash = chosen.iter().any(Option::is_none);
        let caches: Vec<Cache> = chosen.into_iter().flatten().collect();
        let body = [
            (!caches.is_empty()).then_some("快取會直接刪除，應用程式下次開啟時會重新產生。"),
            trash.then_some("垃圾桶裡的項目會永久刪除，無法復原。"),
        ]
        .into_iter()
        .flatten()
        .collect::<String>();
        let (page, shown, button) = (page.clone(), shown.clone(), clean.clone());
        alert::confirm(clean, "清理選取的項目？", Some(&body), "清理", move || {
            button.set_sensitive(false);
            button.set_label("正在清理");
            glib::spawn_future_local(async move {
                let failed = gio::spawn_blocking(move || remove(&caches, trash))
                    .await
                    .unwrap_or_else(|_| vec!["清理時發生錯誤".to_string()]);
                let Some(page) = page.upgrade() else { return };
                if !failed.is_empty() {
                    alert::notice(&page, "有些項目無法清理", Some(&failed.join("\n")));
                }
                scan(&page, &shown);
            });
        });
    });
}

/// The caches of the list that are on this computer, measured, and the trash's size.
fn find(home: &Path) -> Found {
    let (names, flatpak_known) = running_names();
    let flatpaks = flatpak_known.then(|| flatpak_apps(home));
    let mut caches = rules(home, flatpaks.as_deref());
    for cache in &mut caches {
        cache.size = cache.roots.iter().map(|root| size_of(root)).sum();
        cache.running = runs(&cache.owners, &names);
    }
    Found { caches, trash: size_of(&glib::user_data_dir().join("Trash")) }
}

/// Mole's list of caches apps make again, those of them that are folders of their own
/// under `home`; `flatpaks` are the sandboxed apps' ids, when it can be told which run.
fn rules(home: &Path, flatpaks: Option<&[String]>) -> Vec<Cache> {
    let cache_home = home.join(".cache");
    let config = home.join(".config");
    let mut caches: Vec<Cache> = Vec::new();
    let mut add = |section: usize, label: &str, roots: Vec<PathBuf>, owners: &[&str]| {
        let roots: Vec<PathBuf> = roots.into_iter().filter(|root| is_real_dir(root)).collect();
        if !roots.is_empty() {
            caches.push(Cache {
                section,
                label: label.to_string(),
                roots,
                owners: owners.iter().map(|owner| owner.to_string()).collect(),
                size: 0,
                running: false,
            });
        }
    };
    let c = |path: &str| cache_home.join(path);
    let h = |path: &str| home.join(path);

    for (label, path, owners) in [
        ("Chromium", "chromium", &["chromium"][..]),
        ("Google Chrome", "google-chrome", &["chrome"]),
        ("Brave", "BraveSoftware", &["brave"]),
        ("Microsoft Edge", "microsoft-edge", &["msedge"]),
        ("Vivaldi", "vivaldi", &["vivaldi-bin"]),
        ("Zen", "zen", &["zen", "zen-bin"]),
    ] {
        add(0, label, vec![c(path)], owners);
    }
    // The Snap of Firefox, as Ubuntu installs it, keeps its own.
    add(0, "Firefox", vec![c("mozilla/firefox"), h("snap/firefox/common/.cache/mozilla/firefox")], &["firefox"]);

    add(1, "縮圖", vec![c("thumbnails")], &[]);
    add(1, "WebKitGTK", vec![c("webkitgtk/WebKitCache"), c("webkitgtk/MediaCache")], &["WebKitNetworkProcess", "WebKitWebProcess"]);
    add(1, "GNOME 軟體", vec![c("gnome-software")], &["gnome-software"]);
    for (folder, label, owners, editor) in [
        ("Code", "VS Code", &["code", "electron*"][..], true),
        ("Code - OSS", "Code - OSS", &["code-oss", "electron*"], true),
        ("VSCodium", "VSCodium", &["codium", "vscodium", "electron*"], true),
        ("Cursor", "Cursor", &["cursor"], true),
        ("obsidian", "Obsidian", &["obsidian", "electron*"], false),
        ("discord", "Discord", &["Discord", "discord"], false),
        ("Slack", "Slack", &["slack"], false),
        ("Signal", "Signal", &["signal-desktop", "electron*"], false),
    ] {
        let leaves = ELECTRON_LEAVES.iter().chain(if editor { &EDITOR_LEAVES[..] } else { &[] });
        add(1, label, leaves.map(|leaf| config.join(folder).join(leaf)).collect(), owners);
    }

    for (label, path, owners) in [
        ("npm", h(".npm/_cacache"), &["npm", "node"][..]),
        ("Yarn", c("yarn"), &["yarn", "node"]),
        ("pnpm", c("pnpm"), &["pnpm", "node"]),
        ("Bun", h(".bun/install/cache"), &["bun", "bunx"]),
        ("node-gyp", c("node-gyp"), &["node-gyp", "node"]),
        ("Electron 下載", c("electron"), &[]),
        ("TypeScript", c("typescript"), &["tsserver", "node"]),
        ("pip", c("pip"), &["pip", "pip3", "python", "python3"]),
        ("Poetry", c("pypoetry/cache"), &["poetry"]),
        ("pre-commit", c("pre-commit"), &["pre-commit"]),
        ("Ruff", c("ruff"), &["ruff"]),
        ("mypy", c("mypy"), &["mypy", "dmypy"]),
        ("Go", c("go-build"), &["go", "gopls"]),
        ("Cargo", h(".cargo/registry/cache"), &["cargo", "rustc", "rust-analyzer"]),
        ("rustup 下載", h(".rustup/downloads"), &["rustup", "cargo"]),
        ("sccache", c("sccache"), &["sccache"]),
        ("Zig", c("zig"), &["zig", "zls"]),
        ("Composer", c("composer"), &["composer", "php"]),
    ] {
        add(2, label, vec![path], owners);
    }

    for id in flatpaks.unwrap_or_default() {
        let owner = format!("flatpak:{id}");
        add(3, id, vec![home.join(".var/app").join(id).join("cache")], &[owner.as_str()]);
    }
    caches
}

/// A folder, not a link to one nor a disk mounted there.
fn is_real_dir(path: &Path) -> bool {
    let (Ok(meta), Some(Ok(parent))) = (std::fs::symlink_metadata(path), path.parent().map(std::fs::metadata)) else {
        return false;
    };
    meta.is_dir() && meta.dev() == parent.dev()
}

/// The space a file or a folder's contents take; links are not followed.
fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    let own = meta.blocks() * 512;
    if !meta.is_dir() {
        return own;
    }
    std::fs::read_dir(path)
        .map(|entries| entries.filter_map(Result::ok).map(|entry| size_of(&entry.path())).sum())
        .unwrap_or(0)
        + own
}

/// The names of the processes that run, as /proc tells them and as each was started, and
/// `flatpak:<id>` for each sandboxed app that runs; and whether Flatpak could tell that.
fn running_names() -> (HashSet<String>, bool) {
    let mut names = HashSet::new();
    for dir in std::fs::read_dir("/proc").into_iter().flatten().flatten().map(|entry| entry.path()) {
        if let Ok(comm) = std::fs::read_to_string(dir.join("comm")) {
            names.insert(comm.trim_end().to_string());
        }
        // comm is cut to 15 bytes; the command's first word is whole.
        if let Ok(cmdline) = std::fs::read(dir.join("cmdline")) {
            let first = cmdline.split(|byte| *byte == 0 || *byte == b' ').next().unwrap_or_default();
            if let Some(name) = Path::new(OsStr::from_bytes(first)).file_name() {
                names.insert(name.to_string_lossy().into_owned());
            }
        }
    }
    let flatpak = Command::new("flatpak")
        .args(["ps", "--columns=application"])
        .output()
        .ok()
        .filter(|ran| ran.status.success());
    let known = flatpak.is_some();
    if let Some(ran) = flatpak {
        names.extend(String::from_utf8_lossy(&ran.stdout).lines().map(|id| format!("flatpak:{}", id.trim())));
    }
    (names, known)
}

/// The ids of the Flatpak apps that keep files in the home.
fn flatpak_apps(home: &Path) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(home.join(".var/app"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids
}

fn runs(owners: &[String], names: &HashSet<String>) -> bool {
    owners.iter().any(|owner| match owner.strip_suffix('*') {
        Some(start) => names.iter().any(|name| name.starts_with(start)),
        None => names.contains(owner),
    })
}

/// Empties each cache whose programs still do not run, and the trash if `trash`; what
/// could not be, as it is told.
fn remove(caches: &[Cache], trash: bool) -> Vec<String> {
    let (names, flatpak_known) = running_names();
    let mut failed = Vec::new();
    for cache in caches {
        // Which sandboxed app runs cannot be told now: it could be this one.
        let unknown = !flatpak_known && cache.owners.iter().any(|owner| owner.starts_with("flatpak:"));
        if unknown || runs(&cache.owners, &names) {
            failed.push(format!("{}：執行中", cache.label));
            continue;
        }
        for root in &cache.roots {
            if let Err(err) = empty(root) {
                failed.push(format!("{}：{err}", root.display()));
            }
        }
    }
    if trash {
        match Command::new("gio").args(["trash", "--empty"]).output() {
            Ok(ran) if ran.status.success() => {}
            Ok(ran) => failed.push(format!("垃圾桶：{}", String::from_utf8_lossy(&ran.stderr).trim())),
            Err(err) => failed.push(format!("垃圾桶：{err}")),
        }
    }
    failed
}

/// Removes what is in `root`, never `root` itself, and nothing a link leads to.
fn empty(root: &Path) -> std::io::Result<()> {
    // Checked again: it could have become a link or a mount since it was measured.
    if !is_real_dir(root) {
        return Err(std::io::Error::other("不是一般資料夾"));
    }
    let mut first_error = None;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let removed = if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())
        } else {
            std::fs::remove_file(entry.path())
        };
        if let Err(err) = removed {
            first_error.get_or_insert(err);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nekoland-storage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn emptying_keeps_the_folder_and_what_links_lead_to() {
        let dir = scratch("empty");
        let (root, kept) = (dir.join("cache"), dir.join("kept"));
        std::fs::create_dir_all(root.join("deep/er")).unwrap();
        std::fs::write(root.join("deep/er/file"), "x").unwrap();
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(kept.join("file"), "x").unwrap();
        std::os::unix::fs::symlink(&kept, root.join("link")).unwrap();

        empty(&root).unwrap();
        assert!(root.is_dir());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert!(kept.join("file").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_link_to_a_folder_is_not_a_cache() {
        let dir = scratch("link");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/file"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();

        assert!(empty(&dir.join("link")).is_err());
        assert!(dir.join("real/file").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_folders_there_are_listed_and_leaves_join_one_row() {
        let home = scratch("rules");
        std::fs::create_dir_all(home.join(".config/Code/Cache")).unwrap();
        std::fs::create_dir_all(home.join(".config/Code/CachedData")).unwrap();
        std::fs::create_dir_all(home.join(".cache/real")).unwrap();
        std::os::unix::fs::symlink(home.join(".cache/real"), home.join(".cache/go-build")).unwrap();

        let caches = rules(&home, None);
        assert_eq!(caches.len(), 1, "{caches:?}");
        assert_eq!(caches[0].label, "VS Code");
        assert_eq!(caches[0].roots.len(), 2);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn an_owner_with_a_star_is_any_name_it_starts() {
        let names: HashSet<String> = ["electron37".to_string(), "bash".to_string()].into();
        assert!(runs(&["electron*".to_string()], &names));
        assert!(!runs(&["electron".to_string(), "code".to_string()], &names));
        assert!(!runs(&[], &names));
    }
}
