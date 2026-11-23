//! 儲存空間: the disks, and the space that can be freed: the trash, and the caches programs
//! make again by themselves. Caches are found where each program says it keeps them, and one
//! is left alone while a program has a file open in it or in its app's folder.

use super::*;
use std::{collections::HashSet, os::unix::fs::MetadataExt};

/// The groups the caches are shown in, by `Cache::section`.
const SECTIONS: [&str; 3] = ["應用程式快取", "開發工具快取", "Flatpak 快取"];
/// The folders a Chromium-based browser or Electron app writes its caches to, in its own
/// folder or in a profile within it, as they are named on disk.
const CHROMIUM_CACHES: [&str; 6] =
    ["Cache", "Code Cache", "GPUCache", "GPUPersistentCache", "DawnGraphiteCache", "DawnWebGPUCache"];

/// Folders whose contents a program makes again, shown as one row.
#[derive(Clone, Debug, Default)]
struct Cache {
    section: usize,
    label: String,
    roots: Vec<PathBuf>,
    /// A file open in one of these means the program that uses the cache runs.
    watched: Vec<PathBuf>,
    /// So does a process of one of these names.
    programs: Vec<String>,
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

/// The caches on this computer, measured, and the trash's size.
fn find(home: &Path) -> Found {
    let busy = Busy::now();
    let flatpaks = busy.flatpak_known.then(|| flatpak_apps(home));
    let mut caches = caches(home, &glib::user_cache_dir(), flatpaks.as_deref());
    for cache in &mut caches {
        cache.size = cache.roots.iter().map(|root| size_of(root)).sum();
        cache.running = busy.uses(cache);
    }
    Found { caches, trash: size_of(&glib::user_data_dir().join("Trash")) }
}

/// Every cache under `home` this page knows how to find; `flatpaks` are the ids of the
/// Flatpak apps, when Flatpak can tell which of them run.
fn caches(home: &Path, cache_home: &Path, flatpaks: Option<&[String]>) -> Vec<Cache> {
    let mut found = Vec::new();
    let mut add = |cache: Cache| {
        let roots: Vec<PathBuf> =
            cache.roots.into_iter().filter(|root| root.starts_with(home) && is_real_dir(root)).collect();
        if !roots.is_empty() {
            found.push(Cache { roots, ..cache });
        }
    };

    // The freedesktop thumbnail specification's folder, which viewers fill again as needed.
    add(Cache { label: "縮圖".into(), roots: vec![cache_home.join("thumbnails")], ..Cache::default() });

    // A Chromium-based program keeps "Local State" in its folder in ~/.config, and its disk
    // cache in the folder of the same name in ~/.cache.
    for app in subfolders(&home.join(".config")).into_iter().filter(|app| app.join("Local State").is_file()) {
        let Some(name) = app.file_name().map(|name| name.to_string_lossy().into_owned()) else { continue };
        let twin = cache_home.join(&name);
        let mut roots = Vec::new();
        for base in [&app, &twin] {
            for folder in std::iter::once(base.clone()).chain(subfolders(base)) {
                roots.extend(CHROMIUM_CACHES.iter().map(|leaf| folder.join(leaf)));
            }
        }
        add(Cache { label: name, roots, watched: vec![app.clone(), twin], ..Cache::default() });
    }

    // Firefox keeps each profile's disk cache in cache2 of the profile's folder under the cache
    // home; the Snap of Firefox has a home of its own.
    for base in [home.to_path_buf(), home.join("snap/firefox/common")] {
        let profiles = base.join(".mozilla/firefox");
        let caches = base.join(".cache/mozilla/firefox");
        let roots = subfolders(&caches).into_iter().map(|profile| profile.join("cache2")).collect();
        add(Cache { label: "Firefox".into(), roots, watched: vec![profiles, caches], ..Cache::default() });
    }

    for tool in tools(home, cache_home) {
        add(tool);
    }

    for id in flatpaks.unwrap_or_default() {
        let app = home.join(".var/app").join(id);
        add(Cache {
            section: 2,
            label: id.clone(),
            roots: vec![app.join("cache")],
            programs: vec![format!("flatpak:{id}")],
            ..Cache::default()
        });
    }
    found
}

/// The download and build caches of developer tools: where the tool says, asked, or where
/// its documentation says it keeps it when the tool cannot be run.
fn tools(home: &Path, cache_home: &Path) -> Vec<Cache> {
    let cargo_home = std::env::var_os("CARGO_HOME").map_or_else(|| home.join(".cargo"), PathBuf::from);
    let tool = |label: &str, programs: &[&str], root: PathBuf| Cache {
        section: 1,
        label: label.into(),
        watched: vec![root.clone()],
        roots: vec![root],
        programs: programs.iter().map(|program| program.to_string()).collect(),
        ..Cache::default()
    };
    vec![
        tool("Go", &["go"], asked("go", &["env", "GOCACHE"]).unwrap_or_else(|| cache_home.join("go-build"))),
        tool("npm", &["npm"], asked("npm", &["config", "get", "cache"]).unwrap_or_else(|| home.join(".npm")).join("_cacache")),
        tool("pip", &["pip", "pip3"], asked("pip3", &["cache", "dir"]).unwrap_or_else(|| cache_home.join("pip"))),
        tool("Bun", &["bun"], asked("bun", &["pm", "cache"]).unwrap_or_else(|| home.join(".bun/install/cache"))),
        // The .crate files Cargo downloaded; what they unpack to stays.
        tool("Cargo", &["cargo"], cargo_home.join("registry/cache")),
    ]
}

/// The folder `program` prints for `args`, if it runs and prints a whole path.
fn asked(program: &str, args: &[&str]) -> Option<PathBuf> {
    let ran = Command::new(program).args(args).stderr(std::process::Stdio::null()).output().ok()?;
    let said = String::from_utf8(ran.stdout).ok()?;
    let path = PathBuf::from(said.trim());
    (ran.status.success() && path.is_absolute()).then_some(path)
}

fn subfolders(folder: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
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

/// What runs at one moment: the files processes have open, their names, and the Flatpak
/// apps that run.
struct Busy {
    open: Vec<PathBuf>,
    names: HashSet<String>,
    flatpak_known: bool,
}

impl Busy {
    fn now() -> Busy {
        let mut open = Vec::new();
        let mut names = HashSet::new();
        for process in std::fs::read_dir("/proc").into_iter().flatten().flatten().map(|entry| entry.path()) {
            if let Ok(name) = std::fs::read_to_string(process.join("comm")) {
                names.insert(name.trim_end().to_string());
            }
            // Only this user's processes can be looked into, which are the ones using the home.
            open.extend(std::fs::read_link(process.join("cwd")));
            for fd in std::fs::read_dir(process.join("fd")).into_iter().flatten().flatten() {
                open.extend(std::fs::read_link(fd.path()));
            }
        }
        let flatpak = Command::new("flatpak")
            .args(["ps", "--columns=application"])
            .output()
            .ok()
            .filter(|ran| ran.status.success());
        let flatpak_known = flatpak.is_some();
        if let Some(ran) = flatpak {
            names.extend(String::from_utf8_lossy(&ran.stdout).lines().map(|id| format!("flatpak:{}", id.trim())));
        }
        Busy { open, names, flatpak_known }
    }

    fn uses(&self, cache: &Cache) -> bool {
        // Which Flatpak app runs cannot be told: it could be this one.
        let unknown = !self.flatpak_known && cache.programs.iter().any(|program| program.starts_with("flatpak:"));
        unknown
            || cache.programs.iter().any(|program| self.names.contains(program))
            || self.open.iter().any(|file| cache.watched.iter().chain(&cache.roots).any(|folder| file.starts_with(folder)))
    }
}

/// The ids of the Flatpak apps that keep files in the home.
fn flatpak_apps(home: &Path) -> Vec<String> {
    subfolders(&home.join(".var/app"))
        .into_iter()
        .filter_map(|app| app.file_name()?.to_str().map(str::to_string))
        .collect()
}

/// Empties each cache no program uses now, and the trash if `trash`; what could not be, as
/// it is told.
fn remove(caches: &[Cache], trash: bool) -> Vec<String> {
    let busy = Busy::now();
    let mut failed = Vec::new();
    for cache in caches {
        if busy.uses(cache) {
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
    fn a_chromium_app_has_its_cache_folders_only_in_its_own_and_its_profiles() {
        let home = scratch("chromium");
        let (app, cache_home) = (home.join(".config/SomeApp"), home.join(".cache"));
        std::fs::create_dir_all(app.join("Default/GPUCache")).unwrap();
        std::fs::create_dir_all(app.join("Code Cache")).unwrap();
        std::fs::create_dir_all(app.join("Local Storage")).unwrap();
        std::fs::write(app.join("Local State"), "{}").unwrap();
        std::fs::create_dir_all(cache_home.join("SomeApp/Default/Cache")).unwrap();
        // Not Chromium's: no Local State.
        std::fs::create_dir_all(home.join(".config/Other/Cache")).unwrap();

        let found = caches(&home, &cache_home, None);
        let app_cache = found.iter().find(|cache| cache.label == "SomeApp").expect("SomeApp found");
        let mut roots = app_cache.roots.clone();
        roots.sort();
        assert_eq!(roots, [cache_home.join("SomeApp/Default/Cache"), app.join("Code Cache"), app.join("Default/GPUCache")]);
        assert!(found.iter().all(|cache| cache.label != "Other"));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_file_open_in_the_app_folder_means_it_runs() {
        let cache = Cache {
            roots: vec![PathBuf::from("/h/.cache/App/Cache")],
            watched: vec![PathBuf::from("/h/.config/App")],
            ..Cache::default()
        };
        let busy = |open: &str| Busy { open: vec![PathBuf::from(open)], names: HashSet::new(), flatpak_known: true };
        assert!(busy.clone()("/h/.config/App/Cookies").uses(&cache));
        assert!(busy.clone()("/h/.cache/App/Cache/index").uses(&cache));
        assert!(!busy("/h/.config/Application/Cookies").uses(&cache));
    }

    #[test]
    fn a_flatpak_cache_is_kept_when_flatpak_cannot_say_what_runs() {
        let cache = Cache { programs: vec!["flatpak:org.example.App".into()], ..Cache::default() };
        let busy = Busy { open: Vec::new(), names: HashSet::new(), flatpak_known: false };
        assert!(busy.uses(&cache));
    }
}
