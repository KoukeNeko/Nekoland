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
            share_gtk_look_with_flatpaks();
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

/// Flatpak applications read GTK 4's user stylesheet from a folder of their own: they are
/// let read the person's, where the look is, as the applications outside read it. It stays
/// when the look is turned off, as the stylesheet then has no look in it.
pub(crate) fn share_gtk_look_with_flatpaks() {
    let overrides = glib::user_data_dir().join("flatpak/overrides/global");
    let shared = std::fs::read_to_string(overrides).is_ok_and(|text| {
        text.lines().any(|line| line.starts_with("filesystems=") && line.contains(FLATPAK_GTK4))
    });
    if !shared && glib::find_program_in_path("flatpak").is_some() {
        run("flatpak", &["override", "--user", &format!("--filesystem={FLATPAK_GTK4}")]);
    }
}

const FLATPAK_GTK4: &str = "xdg-config/gtk-4.0:ro";

pub(crate) const GTK3_THEME: &str = "Nekoland";
