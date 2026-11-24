//! The Appearance page, with the themes and GTK look it sets.

use super::*;

pub(crate) fn appearance_page(config: &Config, provider: &gtk::CssProvider) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let style = group("");
    let dark = output(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "color-scheme"],
    )
    .is_some_and(|scheme| scheme.contains("dark"));
    let scheme = adw::ComboRow::builder()
        .title("配色")
        .model(&gtk::StringList::new(&["淺色", "深色"]))
        .selected(u32::from(dark))
        .build();
    scheme.connect_selected_notify(|row| {
        let value = if row.selected() == 1 {
            "prefer-dark"
        } else {
            "default"
        };
        run(
            "gsettings",
            &["set", "org.gnome.desktop.interface", "color-scheme", value],
        );
    });
    style.add(&scheme);

    let look = adw::SwitchRow::builder()
        .title("Nekoland 外觀")
        .subtitle("GTK app 重新開啟後套用")
        .active(gtk_look_on())
        .build();
    if gtk::minor_version() < GTK_LOOK_MINOR {
        look.set_sensitive(false);
        look.set_subtitle(&format!("需要 GTK 4.{GTK_LOOK_MINOR} 以上"));
    }
    let provider = provider.clone();
    let read_at_start = look_read_at_start();
    let glass_look = provider.clone();
    look.connect_active_notify(move |row| {
        let on = row.is_active();
        if let Err(err) = set_gtk_look(on) {
            eprintln!("GTK look: {err}");
            return;
        }
        set_gtk3_theme(on);
        if on {
            share_look_with_flatpaks();
            copy_gtk3_theme_to_snaps();
        } else {
            withdraw_kvantum_theme();
            withdraw_gtk3_theme_from_snaps();
        }
        let display = row.display();
        if on {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        } else {
            gtk::style_context_remove_provider_for_display(&display, &provider);
        }
        // What GTK read as Settings started cannot be taken back but by starting again.
        if !on && read_at_start {
            RESTART.set(Some("外觀"));
            if let Some(app) = row
                .root()
                .and_downcast::<gtk::Window>()
                .and_then(|window| window.application())
            {
                app.quit();
            }
        }
    });
    style.add(&look);
    let scroll_bars = adw::SwitchRow::builder()
        .title("一律顯示捲軸")
        .active(interface_setting("overlay-scrolling") == "false")
        .build();
    scroll_bars.connect_active_notify(|row| set_interface("overlay-scrolling", &(!row.is_active()).to_string()));
    style.add(&scroll_bars);
    page.add(&style);

    let themes = group("");
    themes.add(&theme_row(config, "圖示主題", "icon_theme", "icon-theme", false));
    themes.add(&converted_icons_row());
    themes.add(&own_icons_row());
    themes.add(&theme_row(config, "游標主題", "cursor_theme", "cursor-theme", true));
    page.add(&themes);

    let fonts = fonts_page(&glass_look);
    let bars = glass_group(config, "選單列與 Dock", ("bar", 0.5), |_| {});
    bars.add(&switch_row(config, "選單列背景", "menu_bar_background"));
    // GTK's menus and sidebars take their tint from the look, written again as it changes.
    let glass = glass_group(config, "選單、通知與視窗", ("glass", 0.8), move |opacity| {
        GLASS_OPACITY.set(opacity);
        if gtk_look_on() {
            if let Err(err) = set_gtk_look(true) {
                eprintln!("GTK look: {err}");
            }
            glass_look.load_from_string(&look_for(adw::StyleManager::default().is_dark()));
        }
    });
    let more = group("");
    more.add(&subpage_row("字型", || UI_FONT.with_borrow(Clone::clone), fonts));
    more.add(&subpage_row(
        "毛玻璃",
        || format!("不透明度 {:.0}%", GLASS_OPACITY.get() * 100.0),
        page_of_groups(&[bars, glass]),
    ));
    page.add(&more);
    page
}

/// A row choosing a theme for Nekoland's `key` and GNOME's `gsetting` together; 預設 leaves
/// both to their own choice.
pub(crate) fn theme_row(config: &Config, title: &str, key: &'static str, gsetting: &'static str, cursors: bool) -> adw::ComboRow {
    let themes: Vec<String> = themes::installed(cursors).into_iter().map(|theme| theme.name).collect();
    let labels: Vec<&str> = std::iter::once("預設").chain(themes.iter().map(String::as_str)).collect();
    let current = config.word(key);
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&labels))
        .selected(themes.iter().position(|theme| theme == current).map_or(0, |at| at + 1) as u32)
        .build();
    row.connect_selected_notify(move |row| match row.selected().checked_sub(1) {
        None => {
            unset(key);
            run("gsettings", &["reset", INTERFACE, gsetting]);
        }
        Some(at) => {
            if let Some(theme) = themes.get(at as usize) {
                set(&format!("{key} = {}", toml_string(theme)));
                set_interface(gsetting, &format!("'{theme}'"));
            }
        }
    });
    row
}

/// The hand-drawn icons of Nekoland's own icon theme, as install.sh puts them.
pub(crate) const DRAWN_ICONS: &str = "/usr/local/share/icons/Nekoland/scalable/apps";
/// The SVG icons in `dir`, by name.
pub(crate) fn icons_in(dir: &Path) -> Vec<PathBuf> {
    let mut icons: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
        .collect();
    icons.sort();
    icons
}

/// Whether other applications' icons are redrawn on Nekoland's tile; turning it on makes
/// them, off removes them, and either way the compositor looks its icons up again.
pub(crate) fn converted_icons_row() -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title("轉換其他應用程式的圖示")
        .active(!themes::converted_icons().is_empty())
        .build();
    row.connect_active_notify(|row| {
        let remove = !row.is_active();
        row.set_sensitive(false);
        let row = row.clone();
        glib::spawn_future_local(async move {
            // Several seconds for a few hundred icons, so not on the main loop.
            let _ = gio::spawn_blocking(move || {
                run("nekoland-conform-icons", if remove { &["--remove"] } else { &[] });
                run("nekoland", &["msg", "reload"]);
            })
            .await;
            row.set_sensitive(true);
        });
    });
    row
}

/// Opens the list of Nekoland's icons: which are drawn by hand, which are converted.
pub(crate) fn own_icons_row() -> adw::ActionRow {
    let row = adw::ActionRow::builder().title("Nekoland 圖示").activatable(true).build();
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row.connect_activated(own_icons_dialog);
    row
}

pub(crate) fn own_icons_dialog(parent: &adw::ActionRow) {
    let page = adw::PreferencesPage::new();
    for (title, icons) in [
        ("手繪", icons_in(Path::new(DRAWN_ICONS))),
        ("自動演算", themes::converted_icons()),
    ] {
        let icons_group = adw::PreferencesGroup::builder()
            .title(title)
            .description(icons.len().to_string())
            .build();
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(12)
            .min_children_per_line(6)
            .column_spacing(6)
            .row_spacing(6)
            .homogeneous(true)
            .build();
        icons_group.add(&flow);
        page.add(&icons_group);
        // A few at a time: each is an SVG with filters, and there may be hundreds.
        let mut icons = icons.into_iter();
        glib::idle_add_local(move || {
            for path in icons.by_ref().take(8) {
                let name = path.file_stem().map(|stem| stem.to_string_lossy().into_owned());
                let image = gtk::Image::builder().file(path.to_string_lossy()).pixel_size(48).build();
                image.set_tooltip_text(name.as_deref());
                flow.append(&image);
            }
            if icons.len() == 0 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    adw::Dialog::builder()
        .title("Nekoland 圖示")
        .content_width(760)
        .content_height(640)
        .child(&view)
        .build()
        .present(Some(parent));
}

/// The opacity and blur of one kind of glass, Nekoland's `<key>_opacity` and `<key>_blur`;
/// `opacity_changed` is told each new opacity once Nekoland has it.
pub(crate) fn glass_group(
    config: &Config,
    title: &str,
    (key, opacity): (&'static str, f64),
    opacity_changed: impl Fn(f64) + 'static,
) -> adw::PreferencesGroup {
    let glass = group(title);
    glass.add(&scale_row(
        "不透明度",
        config.number(&format!("{key}_opacity"), opacity),
        (0.0, 1.0, 0.05),
        percent,
        move |opacity| {
            set(&format!("{key}_opacity = {opacity:.2}"));
            opacity_changed(opacity);
        },
    ));
    glass.add(&scale_row(
        "模糊程度",
        config.number(&format!("{key}_blur"), 0.5),
        (0.0, 1.0, 0.05),
        percent,
        move |blur| set(&format!("{key}_blur = {blur:.2}")),
    ));
    glass
}

/// Nekoland's design language for GTK 4 and libadwaita applications, as libadwaita's
/// variables; see the file for what it reaches.
pub(crate) const GTK_LOOK: &str = include_str!("../../../shell/gtk/nekoland.css");
/// Where Nekoland's look goes, in a folder of its own beside GTK's user stylesheet.
pub(crate) const GTK_LOOK_FILE: &str = "nekoland/look.css";
/// What GTK's own user stylesheet imports it with, the rest of that file left as it is.
pub(crate) const GTK_IMPORT: &str = "@import url(\"nekoland/look.css\");";
/// The import of earlier versions, from beside that stylesheet; taken out for the one above.
pub(crate) const OLD_GTK_IMPORT: &str = "@import url(\"nekoland.css\");";
/// The first GTK whose stylesheets read the look's variables and its dark media query.
pub(crate) const GTK_LOOK_MINOR: u32 = 20;

/// The look as GTK's user stylesheet takes it, with Nekoland's glass opacity.
pub(crate) fn gtk_look() -> String {
    let opacity = GLASS_OPACITY.get() * 100.0;
    let font = UI_FONT.with_borrow(|font| css_family(font));
    format!(
        "{GTK_LOOK}\n:root {{ --nekoland-glass-opacity: {opacity:.0}%; \
         --nekoland-font: {font}, \"Noto Sans CJK TC\", sans-serif; }}\n"
    )
}

/// A family's name as CSS quotes it; quotes and backslashes in it are left out.
pub(crate) fn css_family(family: &str) -> String {
    format!("\"{}\"", family.replace(['"', '\\'], ""))
}

/// The look for a stylesheet Settings loads itself, which GTK reads light whatever the
/// system is: when dark, its dark colours are put in outside their media query.
pub(crate) fn look_for(dark: bool) -> String {
    let dark_part = GTK_LOOK
        .split_once("@media (prefers-color-scheme: dark) {")
        .and_then(|(_, part)| part.trim_end().strip_suffix('}'))
        .filter(|_| dark)
        .unwrap_or_default();
    format!("{}{dark_part}", gtk_look())
}

/// Where GTK 4 reads the user's stylesheet, and where Nekoland's goes beside it.
pub(crate) fn gtk_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("gtk-4.0")
}

pub(crate) fn gtk_look_on() -> bool {
    std::fs::read_to_string(gtk_dir().join("gtk.css")).is_ok_and(|text| {
        text.lines().any(|line| [GTK_IMPORT, OLD_GTK_IMPORT].contains(&line.trim()))
    })
}

/// Writes a file whole or not at all, so a crash midway leaves the one there before.
pub(crate) fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    // A stylesheet linked from a dotfiles folder is written there, the link kept.
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let partial = path.with_extension("css.partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)
}

/// Imports Nekoland's look into GTK's user stylesheet, or takes it out again, keeping
/// whatever else is there.
pub(crate) fn set_gtk_look(on: bool) -> std::io::Result<()> {
    let dir = gtk_dir();
    let user = dir.join("gtk.css");
    let text = match std::fs::read_to_string(&user) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|line| ![GTK_IMPORT, OLD_GTK_IMPORT].contains(&line.trim()))
        .collect();
    let look = dir.join(GTK_LOOK_FILE);
    if on {
        std::fs::create_dir_all(look.parent().expect("a folder"))?;
        write_atomic(&look, &gtk_look())?;
        // An import counts only before any rule.
        lines.insert(0, GTK_IMPORT);
    } else if let Err(err) = std::fs::remove_file(&look) {
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err);
        }
    }
    if lines.is_empty() && !on {
        return match std::fs::remove_file(&user) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    write_atomic(&user, &(lines.join("\n") + "\n"))
}

/// The GTK 3 theme of the look, which install.sh puts in /usr/local/share/themes, is
/// GNOME's `gtk-theme` while the look is on: GTK 3 programs read it on Wayland, and the
/// compositor gives it to X11 ones as XSettings. Off, it is put back to the default, unless
/// another was chosen.
pub(crate) fn set_gtk3_theme(on: bool) {
    let current = interface_setting("gtk-theme");
    if on {
        if current != GTK3_THEME {
            set_interface("gtk-theme", &format!("'{GTK3_THEME}'"));
        }
    } else if current == GTK3_THEME {
        run("gsettings", &["reset", INTERFACE, "gtk-theme"]);
    }
}

/// Flatpak applications read GTK 4's user stylesheet, and Kvantum's choice of theme for Qt,
/// from folders of their own: they are let read the person's, as the applications outside
/// read them, and given the look's Kvantum theme there, as /usr/share is not theirs. What
/// they are let read stays when the look is turned off, as there is no look in it then.
pub(crate) fn share_look_with_flatpaks() {
    if glib::find_program_in_path("flatpak").is_none() {
        return;
    }
    let overrides = std::fs::read_to_string(glib::user_data_dir().join("flatpak/overrides/global"));
    let overrides = overrides.unwrap_or_default();
    let granted = overrides.lines().find_map(|line| line.strip_prefix("filesystems=")).unwrap_or_default();
    let missing: Vec<String> = FLATPAK_FOLDERS
        .iter()
        .filter(|folder| !granted.split(';').any(|granted| granted == **folder))
        .map(|folder| format!("--filesystem={folder}"))
        .collect();
    if !missing.is_empty() {
        let mut args = vec!["override", "--user"];
        args.extend(missing.iter().map(String::as_str));
        run("flatpak", &args);
    }
    if let Err(err) = copy_kvantum_theme() {
        eprintln!("Kvantum theme: {err}");
    }
    std::thread::spawn(install_kvantum_styles);
}

const FLATPAK_FOLDERS: [&str; 2] = ["xdg-config/gtk-4.0:ro", "xdg-config/Kvantum:ro"];
/// Where install.sh puts the look's Kvantum theme, for Qt programs outside Flatpak.
const KVANTUM_THEME: &str = "/usr/share/Kvantum/Nekoland";

fn kvantum_theme_copy() -> std::path::PathBuf {
    glib::user_config_dir().join("Kvantum/Nekoland")
}

/// Copies the look's Kvantum theme to the person's Kvantum folder, which Flatpak's Qt
/// applications can read, when it is not there as it is.
fn copy_kvantum_theme() -> std::io::Result<()> {
    let theme = std::path::Path::new(KVANTUM_THEME);
    // Not installed: Qt applications keep their own style.
    if !theme.is_dir() {
        return Ok(());
    }
    copy_folder(theme, &kvantum_theme_copy())
}

/// Copies `from` and what is in it to `to`, leaving alone the files already the same.
fn copy_folder(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let files = std::fs::read_dir(from)?;
    std::fs::create_dir_all(to)?;
    for file in files {
        let file = file?;
        let copy = to.join(file.file_name());
        if file.file_type()?.is_dir() {
            copy_folder(&file.path(), &copy)?;
            continue;
        }
        let text = std::fs::read(file.path())?;
        if std::fs::read(&copy).ok().as_ref() != Some(&text) {
            std::fs::write(&copy, text)?;
        }
    }
    Ok(())
}

/// Where install.sh puts the look's GTK 3 theme.
const GTK3_THEME_DIR: &str = "/usr/local/share/themes/Nekoland";

/// The copy of the GTK 3 theme in each snap's own data folder, one of those GTK in the snap
/// looks for themes in: a snap sees neither /usr/local nor the person's own folders, and
/// draws in Adwaita without it. Its ~/.themes is a link the snap makes to its own themes.
fn snap_theme_copies() -> Vec<std::path::PathBuf> {
    let Ok(snaps) = std::fs::read_dir(glib::home_dir().join("snap")) else {
        return Vec::new();
    };
    snaps
        .filter_map(Result::ok)
        .map(|snap| snap.path().join("current"))
        .filter(|home| home.is_dir())
        .map(|home| home.join("themes").join(GTK3_THEME))
        .collect()
}

/// Gives each snap the look's GTK 3 theme, as the programs outside have it.
pub(crate) fn copy_gtk3_theme_to_snaps() {
    if !std::path::Path::new(GTK3_THEME_DIR).is_dir() {
        return;
    }
    for copy in snap_theme_copies() {
        if let Err(err) = copy_folder(std::path::Path::new(GTK3_THEME_DIR), &copy) {
            eprintln!("GTK 3 theme for {}: {err}", copy.display());
        }
    }
}

/// Takes the snaps' copies of the GTK 3 theme back, with the look.
pub(crate) fn withdraw_gtk3_theme_from_snaps() {
    for copy in snap_theme_copies() {
        match std::fs::remove_dir_all(&copy) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                eprintln!("GTK 3 theme for {}: {err}", copy.display())
            }
            _ => {}
        }
    }
}

/// Takes the copy of the look's Kvantum theme back, with the look.
pub(crate) fn withdraw_kvantum_theme() {
    match std::fs::remove_dir_all(kvantum_theme_copy()) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => eprintln!("Kvantum theme: {err}"),
        _ => {}
    }
}

/// Installs Kvantum's style for each KDE runtime Flatpak applications run on, from Flathub,
/// which has it for some versions only; on the rest they keep their own style.
fn install_kvantum_styles() {
    let Some(runtimes) = output("flatpak", &["list", "--runtime", "--columns=application,branch"]) else {
        return;
    };
    let installed: Vec<(&str, &str)> = runtimes.lines().filter_map(|line| line.split_once('\t')).collect();
    let user_flathub = output("flatpak", &["remotes", "--user", "--columns=name"])
        .is_some_and(|remotes| remotes.lines().any(|remote| remote.trim() == "flathub"));
    let installation = if user_flathub { "--user" } else { "--system" };
    for (_, branch) in installed.iter().filter(|(runtime, _)| *runtime == "org.kde.Platform") {
        if installed.contains(&(KVANTUM_STYLE, branch)) {
            continue;
        }
        let style = format!("{KVANTUM_STYLE}//{branch}");
        let args = ["install", installation, "--noninteractive", "-y", "flathub", &style];
        match Command::new("flatpak").args(args).output() {
            Ok(done) if done.status.success() => {}
            Ok(done) => eprintln!("{style}: {}", String::from_utf8_lossy(&done.stderr).trim()),
            Err(err) => eprintln!("flatpak: {err}"),
        }
    }
}

const KVANTUM_STYLE: &str = "org.kde.KStyle.Kvantum";

pub(crate) const GTK3_THEME: &str = "Nekoland";

#[cfg(test)]
mod tests {
    #[test]
    fn a_theme_is_copied_with_its_subfolders() {
        let dir = std::env::temp_dir().join(format!("nekoland-appearance-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (from, to) = (dir.join("Nekoland"), dir.join("snap/themes/Nekoland"));
        std::fs::create_dir_all(from.join("gtk-3.0")).unwrap();
        std::fs::write(from.join("index.theme"), "[Desktop Entry]").unwrap();
        std::fs::write(from.join("gtk-3.0/gtk.css"), "window {}").unwrap();

        super::copy_folder(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(to.join("index.theme")).unwrap(), "[Desktop Entry]");
        assert_eq!(std::fs::read_to_string(to.join("gtk-3.0/gtk.css")).unwrap(), "window {}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
