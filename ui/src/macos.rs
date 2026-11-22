//! Settings' macOS App page: Mac programs through Darling, which loads them on Linux itself
//! with a Darwin of its own, no macOS involved. Darling is installed by the person, there
//! being no package of it for Ubuntu: the page finds it, starts a program's bundle in it, and
//! makes launchers of the bundles to keep among the applications.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{
    alert, group,
    windows::{ENTRY_GROUP, button, entries, entries_dir, entry_id, entry_path},
};

/// How Darling is built and installed, where it is not yet.
const INSTRUCTIONS: &str = "https://docs.darlinghq.org/build-instructions.html";
/// How the launchers' desktop entries are named, and the key keeping the bundle each opens.
const ENTRY_PREFIX: &str = "nekoland-macos-";
const BUNDLE_KEY: &str = "X-Nekoland-macOS-Bundle";

pub(crate) fn macos_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.add(&darling_group());
    let (apps, added) = app_group();
    page.add(&apps);
    // Without a Darling to start them in no launcher is made; those there are stay, to be
    // taken away. Looked at again when the page is shown, Darling coming and going outside it.
    let show = move || {
        let installed = glib::find_program_in_path("darling").is_some();
        added.set_visible(installed);
        apps.set_visible(installed || !entries(ENTRY_PREFIX, BUNDLE_KEY).is_empty());
    };
    show();
    page.connect_map(move |_| show());
    page
}

/// The Darling installed, as `darling --version` says it: on the error stream, after a word
/// about the prefix the first time it runs, and before whose it is.
fn darling_version() -> Option<String> {
    glib::find_program_in_path("darling")?;
    let said = Command::new("darling")
        .arg("--version")
        .output()
        .inspect_err(|err| eprintln!("darling: {err}"))
        .ok()?;
    let said = String::from_utf8_lossy(&said.stderr);
    let version = said.lines().find(|line| line.starts_with("darling "));
    Some(version.unwrap_or("已安裝").trim().to_string())
}

/// What a bundle's Info.plist says of it.
struct Bundle {
    /// The program in it, and that program's name, which its windows go by.
    program: PathBuf,
    class: String,
    name: String,
    /// Its icon's file, if it names one.
    icon: Option<PathBuf>,
}

/// Reads a bundle's Info.plist, a binary plist as often as XML: Python reads both.
fn read_bundle(bundle: &Path) -> Result<Bundle, String> {
    let plist = bundle.join("Contents/Info.plist");
    let said = Command::new("/usr/bin/python3")
        .args([
            "-I",
            "-c",
            "import plistlib, sys\nsaid = plistlib.load(open(sys.argv[1], 'rb'))\n\
             for key in ('CFBundleExecutable', 'CFBundleDisplayName', 'CFBundleName', 'CFBundleIconFile'):\n\
             \x20   print(said.get(key) or '')",
        ])
        .arg(&plist)
        .output()
        .map_err(|err| format!("python3: {err}"))?;
    if !said.status.success() {
        let why = String::from_utf8_lossy(&said.stderr);
        return Err(format!(
            "{}: {}",
            plist.display(),
            why.lines().last().unwrap_or("not read")
        ));
    }
    let said = String::from_utf8_lossy(&said.stdout);
    let mut said = said.lines().map(str::trim);
    let mut next = || said.next().unwrap_or_default().to_string();
    let (class, shown, short, icon) = (next(), next(), next(), next());
    // A name, not a way out of the bundle.
    let is_name = |name: &str| !(name.is_empty() || name.contains('/') || name == "..");
    if !is_name(&class) {
        return Err(format!("{}: no CFBundleExecutable", plist.display()));
    }
    let name = [shown, short]
        .into_iter()
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| class.clone());
    let icon = is_name(&icon).then(|| {
        let file = bundle.join("Contents/Resources").join(icon);
        // Named with or without what an icon's file ends in.
        match file.extension() {
            Some(_) => file,
            None => file.with_extension("icns"),
        }
    });
    Ok(Bundle {
        program: bundle.join("Contents/MacOS").join(&class),
        class,
        name,
        icon,
    })
}

/// The picture in an icon's file that is nearest an icon's size here: the file holds one
/// after another, each behind its kind and its length, in sizes from 16 to 1024.
// shortcut: only the PNGs in it, which icons older than OS X 10.7's have none of; decode the
// packed pictures when a program turns up with no icon.
fn icns_png(icns: &[u8]) -> Option<&[u8]> {
    const WANTED: u32 = 256;
    let mut rest = icns.strip_prefix(b"icns")?.get(4..)?;
    let mut best: Option<(u32, &[u8])> = None;
    while let Some(length) = rest.get(4..8) {
        let length = u32::from_be_bytes([length[0], length[1], length[2], length[3]]) as usize;
        let Some(picture) = rest.get(8..length) else {
            break;
        };
        rest = &rest[length..];
        // A PNG says how wide it is right after the header of its first chunk.
        let Some(width) = picture
            .strip_prefix(b"\x89PNG\r\n\x1a\n")
            .and_then(|png| png.get(8..12))
        else {
            continue;
        };
        let width = u32::from_be_bytes([width[0], width[1], width[2], width[3]]);
        let better = match best {
            Some((kept, _)) if kept >= WANTED => (WANTED..kept).contains(&width),
            Some((kept, _)) => width > kept,
            None => true,
        };
        if better {
            best = Some((width, picture));
        }
    }
    best.map(|(_, picture)| picture)
}

/// Starts a bundle's program in Darling, left to run by itself.
fn start_bundle(bundle: &Path) -> Result<(), String> {
    let program = read_bundle(bundle)?.program;
    gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE)
        .spawn(&[OsStr::new("darling"), program.as_os_str()])
        .map(drop)
        .map_err(|err| err.to_string())
}

/// One word of a desktop entry's Exec, whatever is in it: within double quotes, with a
/// backslash before what a shell would take there, and a % doubled, one alone being a field's.
fn exec_quoted(word: &str) -> String {
    let mut quoted = String::from('"');
    for c in word.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        if c == '%' {
            quoted.push('%');
        }
        quoted.push(c);
    }
    quoted + "\""
}

/// Where a launcher's icon is kept, out of its bundle's.
fn icon_path(id: &str) -> PathBuf {
    glib::user_data_dir()
        .join("nekoland/macos/icons")
        .join(format!("{id}.png"))
}

/// Writes a desktop entry starting a bundle's program in Darling, so that it is among the
/// applications, and says its ID and name. The entry claims the program's name as its
/// windows' class, which is what Darling gives them: the Dock knows them by it.
fn add_launcher(bundle: &Path) -> Result<(String, String), String> {
    let failed = |err: std::io::Error| err.to_string();
    let app = read_bundle(bundle)?;
    let (Some(kept_bundle), Some(program)) = (bundle.to_str(), app.program.to_str()) else {
        return Err(format!("{}: not UTF-8", bundle.display()));
    };
    let id = entry_id(ENTRY_PREFIX, &app.class, |id| entry_path(id).exists());
    let drawn = app.icon.and_then(|icon| std::fs::read(icon).ok());
    let icon = match drawn.as_deref().and_then(icns_png) {
        Some(picture) => {
            let kept = icon_path(&id);
            std::fs::create_dir_all(kept.parent().unwrap_or(&kept)).map_err(failed)?;
            std::fs::write(&kept, picture).map_err(failed)?;
            kept.to_string_lossy().into_owned()
        }
        None => "application-x-executable".into(),
    };
    let file = glib::KeyFile::new();
    file.set_string(ENTRY_GROUP, "Type", "Application");
    file.set_string(ENTRY_GROUP, "Name", &app.name);
    file.set_string(ENTRY_GROUP, "Exec", &format!("darling {}", exec_quoted(program)));
    file.set_string(ENTRY_GROUP, "Icon", &icon);
    file.set_string(ENTRY_GROUP, "StartupWMClass", &app.class);
    file.set_string(ENTRY_GROUP, BUNDLE_KEY, kept_bundle);
    std::fs::create_dir_all(entries_dir()).map_err(failed)?;
    file.save_to_file(entry_path(&id))
        .map_err(|err| err.to_string())?;
    Ok((id, app.name))
}

/// Takes a launcher away, with its icon.
fn remove_launcher(id: &str) -> std::io::Result<()> {
    std::fs::remove_file(entry_path(id))?;
    match std::fs::remove_file(icon_path(id)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// The bundle a chosen folder stands for: itself, or the one it is in, a bundle being a
/// folder that the chooser opens as readily as it picks.
fn bundle_of(chosen: &Path) -> &Path {
    chosen
        .ancestors()
        .find(|folder| folder.extension().is_some_and(|end| end == "app"))
        .unwrap_or(chosen)
}

/// Asks for a program's bundle, a folder to Linux, and does `then` with the one chosen.
fn choose_bundle(button: &gtk::Button, accept: &str, then: impl Fn(&gtk::Button, &Path) + 'static) {
    let dialog = gtk::FileDialog::builder()
        .title("選擇 macOS 程式")
        .accept_label(accept)
        .build();
    let window = button.root().and_downcast::<gtk::Window>();
    let button = button.clone();
    dialog.select_folder(
        window.as_ref(),
        gio::Cancellable::NONE,
        move |chosen| match chosen.map(|file| file.path()) {
            Ok(Some(chosen)) => then(&button, bundle_of(&chosen)),
            Ok(None) => {}
            Err(err) if err.matches(gtk::DialogError::Dismissed) => {}
            Err(err) => eprintln!("macOS program: {err}"),
        },
    );
}

fn start_or_say(button: &gtk::Button, bundle: &Path) {
    if let Err(why) = start_bundle(bundle) {
        alert::notice(button, "無法執行程式", Some(&why));
    }
}

fn darling_group() -> adw::PreferencesGroup {
    let darling = group("Darling");
    let state = adw::ActionRow::builder().title("版本").build();
    let instructions = button("安裝說明");
    state.add_suffix(&instructions);
    let run = adw::ActionRow::builder().title("執行程式").build();
    let choose = button("選擇⋯");
    run.add_suffix(&choose);
    run.set_activatable_widget(Some(&choose));
    darling.add(&state);
    darling.add(&run);

    let show = glib::clone!(
        #[weak]
        state,
        #[weak]
        instructions,
        #[weak]
        run,
        move || {
            let version = darling_version();
            state.set_subtitle(version.as_deref().unwrap_or("未安裝"));
            instructions.set_visible(version.is_none());
            run.set_visible(version.is_some());
        }
    );
    show();
    // Installed from a terminal, which the page has no word of: it looks again when shown.
    darling.connect_map(move |_| show());

    instructions.connect_clicked(|button| {
        if let Err(err) = gio::AppInfo::launch_default_for_uri(INSTRUCTIONS, gio::AppLaunchContext::NONE) {
            alert::notice(button, "無法開啟", Some(&err.to_string()));
        }
    });
    choose.connect_clicked(|button| choose_bundle(button, "執行", start_or_say));
    darling
}

/// A launcher's row among the others: it opens its bundle, or takes the launcher and
/// itself away.
fn app_row(apps: &adw::PreferencesGroup, id: String, name: &str, bundle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(name)
        .subtitle(bundle)
        .use_markup(false)
        .build();
    let icon = icon_path(&id);
    if icon.exists() {
        row.add_prefix(
            &gtk::Image::builder()
                .file(icon.to_string_lossy())
                .pixel_size(32)
                .build(),
        );
    }
    let open = button("開啟");
    let opened = PathBuf::from(bundle);
    open.connect_clicked(move |button| start_or_say(button, &opened));
    let remove = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("移除")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    remove.connect_clicked(glib::clone!(
        #[weak]
        apps,
        #[weak]
        row,
        move |button| match remove_launcher(&id) {
            Ok(()) => apps.remove(&row),
            Err(err) => alert::notice(button, "無法移除 App", Some(&err.to_string())),
        }
    ));
    row.add_suffix(&open);
    row.add_suffix(&remove);
    row
}

/// The launchers, above the row that makes another, which is given with them.
fn app_group() -> (adw::PreferencesGroup, adw::ActionRow) {
    let apps = group("App");
    for (id, name, bundle) in entries(ENTRY_PREFIX, BUNDLE_KEY) {
        apps.add(&app_row(&apps, id, &name, &bundle));
    }
    let added = adw::ActionRow::builder().title("新增 App").build();
    let choose = button("選擇⋯");
    added.add_suffix(&choose);
    added.set_activatable_widget(Some(&choose));
    apps.add(&added);

    choose.connect_clicked(glib::clone!(
        #[weak]
        apps,
        #[weak]
        added,
        move |button| {
            choose_bundle(button, "新增", move |button, bundle| {
                match add_launcher(bundle) {
                    Ok((id, name)) => {
                        // The row that adds stays the last.
                        apps.remove(&added);
                        apps.add(&app_row(&apps, id, &name, &bundle.to_string_lossy()));
                        apps.add(&added);
                    }
                    Err(why) => alert::notice(button, "無法新增 App", Some(&why)),
                }
            });
        }
    ));
    (apps, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_is_what_its_info_plist_says() {
        let bundle = std::env::temp_dir().join(format!("nekoland-test-{}.app", std::process::id()));
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        let plist = |keys: &str| {
            std::fs::write(
                bundle.join("Contents/Info.plist"),
                format!("<?xml version=\"1.0\"?>\n<plist version=\"1.0\"><dict>{keys}</dict></plist>"),
            )
            .unwrap();
        };
        let key = |key: &str, value: &str| format!("<key>{key}</key><string>{value}</string>");

        plist(&(key("CFBundleExecutable", "TextEdit") + &key("CFBundleIconFile", "Edit")));
        let read = read_bundle(&bundle).unwrap();
        assert_eq!(read.program, bundle.join("Contents/MacOS/TextEdit"));
        assert_eq!(
            (read.class.as_str(), read.name.as_str()),
            ("TextEdit", "TextEdit")
        );
        assert_eq!(read.icon, Some(bundle.join("Contents/Resources/Edit.icns")));

        plist(
            &(key("CFBundleExecutable", "pages")
                + &key("CFBundleName", "Pages")
                + &key("CFBundleIconFile", "../x.icns")),
        );
        let read = read_bundle(&bundle).unwrap();
        assert_eq!((read.class.as_str(), read.name.as_str()), ("pages", "Pages"));
        assert_eq!(read.icon, None);

        plist(&key("CFBundleExecutable", "../../evil"));
        assert!(read_bundle(&bundle).is_err());
        std::fs::remove_dir_all(&bundle).unwrap();
        assert!(read_bundle(&bundle).is_err());
    }

    #[test]
    fn a_folder_in_a_bundle_stands_for_the_bundle() {
        let bundle = Path::new("/home/me/Apps/TextEdit.app");
        assert_eq!(bundle_of(bundle), bundle);
        assert_eq!(bundle_of(&bundle.join("Contents/MacOS")), bundle);
        assert_eq!(bundle_of(Path::new("/home/me/Apps")), Path::new("/home/me/Apps"));
    }

    #[test]
    fn a_program_s_path_is_one_word_of_an_entry_s_line() {
        assert_eq!(
            exec_quoted("/Apps/Final Cut Pro.app/x"),
            "\"/Apps/Final Cut Pro.app/x\""
        );
        assert_eq!(
            exec_quoted(r#"/a/$HOME 100% "new"`x`\"#),
            r#""/a/\$HOME 100%% \"new\"\`x\`\\""#
        );
    }

    #[test]
    fn an_icon_is_the_png_nearest_its_size() {
        let picture = |kind: &[u8; 4], width: u32| {
            let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            png.extend(width.to_be_bytes());
            [kind.as_slice(), &(png.len() as u32 + 8).to_be_bytes(), &png].concat()
        };
        let width = |png: &[u8]| u32::from_be_bytes(png[16..20].try_into().unwrap());
        let icns = |pictures: &[Vec<u8>]| [b"icns\0\0\0\0".as_slice(), &pictures.concat()].concat();

        let packed = [b"il32".as_slice(), &12u32.to_be_bytes(), b"\x01\x02\x03\x04"].concat();
        let all = icns(&[
            picture(b"ic07", 128),
            packed.clone(),
            picture(b"ic10", 1024),
            picture(b"ic08", 256),
            picture(b"ic09", 512),
        ]);
        assert_eq!(icns_png(&all).map(width), Some(256));
        assert_eq!(
            icns_png(&icns(&[picture(b"ic07", 128), picture(b"icp4", 16)])).map(width),
            Some(128)
        );
        // One cut short still gives what came whole; one with no PNG, or not an icon, none.
        assert_eq!(icns_png(&all[..all.len() - 3]).map(width), Some(256));
        assert_eq!(icns_png(&icns(&[packed])), None);
        assert_eq!(icns_png(b"not an icon"), None);
    }
}
