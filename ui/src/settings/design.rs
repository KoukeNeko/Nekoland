//! The design language page, drawn with its own tokens.

use super::*;

/// The colours of the design language, by token, with their names and what they are for.
pub(crate) const TOKENS: [(&str, &str, &str); 8] = [
    ("paper", "紙", "視窗底色"),
    ("sheet", "頁", "卡片、清單"),
    ("ink", "墨", "文字、選中"),
    ("ink-2", "墨 2", "次要文字"),
    ("ink-3", "墨 3", "說明、停用"),
    ("line", "線", "分隔、外框"),
    ("fill", "填", "輸入框、未選中"),
    ("danger", "警示", "刪除、錯誤"),
];

/// A token's colour as the GTK look sets it, light or dark.
pub(crate) fn token_value(token: &str, dark: bool) -> &'static str {
    let (light, dark_part) = GTK_LOOK
        .split_once("@media (prefers-color-scheme: dark)")
        .unwrap_or((GTK_LOOK, ""));
    let part = if dark { dark_part } else { light };
    let key = format!("--nekoland-{token}: ");
    part.split_once(&key)
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value)
}

/// A stylesheet with every rule kept to what is inside `scope`, and what it sets on the
/// root set on `scope` itself: GTK has no way to give a stylesheet to one widget.
pub(crate) fn scoped(css: &str, scope: &str) -> String {
    let mut text = String::new();
    let mut rest = css;
    while let Some((before, comment)) = rest.split_once("/*") {
        text += before;
        rest = comment.split_once("*/").map_or("", |(_, after)| after);
    }
    text += rest;

    let mut out = String::new();
    let mut chunk = String::new();
    let mut in_declarations = false;
    for character in text.chars() {
        match character {
            '{' if !in_declarations => {
                let selectors = chunk.trim();
                // A media query: the rules in it follow.
                if selectors.starts_with('@') {
                    out += selectors;
                } else {
                    let kept: Vec<String> = selectors
                        .split(',')
                        .map(|selector| match selector.trim() {
                            ":root" => scope.to_string(),
                            selector => format!("{scope} {selector}"),
                        })
                        .collect();
                    out += &kept.join(", ");
                    in_declarations = true;
                }
                out += " { ";
                chunk.clear();
            }
            '}' => {
                out += &chunk;
                out += "}\n";
                chunk.clear();
                in_declarations = false;
            }
            _ => chunk.push(character),
        }
    }
    out
}

/// What the design language page is drawn with: Nekoland's look, kept to the page so that
/// it shows whether or not GTK applications have it, and what the page draws that GTK has
/// no widget for.
pub(crate) fn design_css(dark: bool) -> String {
    // Not the rules for sidebars on glass, which are the window's, not the page's.
    let look = look_for(dark);
    let (before, rest) = look
        .split_once("/* Sidebars on glass")
        .expect("the look has its sidebar rules");
    let after = &rest[rest.find("@media").expect("the look has a dark part")..];
    scoped(&format!("{before}{after}"), ".nk-design") + DESIGN_CSS
}

/// The parts of the design GTK has no widget for, and the page's own type and layout.
pub(crate) const DESIGN_CSS: &str = r#"
.nk-design { background-color: var(--nekoland-paper); color: var(--nekoland-ink); }
.nk-h1 { font-size: 32px; font-weight: 700; }
.nk-h2 { font-size: 20px; font-weight: 600; }
.nk-h3 { font-size: 12px; font-weight: 500; letter-spacing: 0.04em; color: var(--nekoland-ink-3); }
.nk-lead { color: var(--nekoland-ink-2); }
.nk-note { font-size: 12px; color: var(--nekoland-ink-3); }
.nk-small { font-size: 12px; color: var(--nekoland-ink-2); }
.nk-mono { font-family: "Ubuntu Sans Mono", monospace; }
/* The cards are laid out in a grid of cells; a cell is not something to point at or choose. */
.nk-design flowbox.nk-cards > flowboxchild, .nk-design flowbox.nk-cards > flowboxchild:hover, .nk-design flowbox.nk-cards > flowboxchild:active, .nk-design flowbox.nk-cards > flowboxchild:selected { padding: 0; background: none; box-shadow: none; outline: none; }
.nk-card { padding: 20px; border-radius: 12px; background-color: var(--nekoland-sheet); box-shadow: 0 0 0 1px var(--nekoland-line); }
.nk-rule { padding: 16px; }
.nk-rule-body { font-size: 13px; color: var(--nekoland-ink-2); }
.nk-swatch { min-height: 64px; border-radius: 8px; box-shadow: inset 0 0 0 1px var(--nekoland-line); }
.nk-type-row { padding: 10px 0; }
.nk-type-row.next { border-top: 1px solid var(--nekoland-line); }
.nk-spec { font-size: 12px; color: var(--nekoland-ink-3); }
.nk-step { background-color: var(--nekoland-ink); border-radius: 2px; }
.nk-corner { min-width: 72px; min-height: 56px; background-color: var(--nekoland-fill); box-shadow: inset 0 0 0 1.5px var(--nekoland-ink); }
.nk-design button.nk-large { min-height: 40px; padding: 0 20px; border-radius: 10px; }
.nk-label { font-size: 12px; font-weight: 500; }
.nk-err { font-size: 12px; color: var(--nekoland-danger); }
.nk-badge { min-height: 20px; padding: 0 8px; border-radius: 99px; font-size: 12px; font-weight: 500; background-color: var(--nekoland-fill); color: var(--nekoland-ink-2); }
.nk-badge.solid { background-color: var(--nekoland-ink); color: var(--nekoland-on-ink); }
.nk-badge.danger { background-color: color-mix(in srgb, var(--nekoland-danger) 14%, transparent); color: var(--nekoland-danger); }
.nk-tip { padding: 4px 8px; border-radius: 6px; background-color: var(--nekoland-ink); color: var(--nekoland-on-ink); font-size: 12px; font-weight: 500; }
.nk-toast { padding: 10px 10px 10px 16px; border-radius: 12px; background-color: var(--nekoland-ink); color: var(--nekoland-on-ink); box-shadow: 0 12px 30px -10px rgb(0 0 0 / 40%); }
.nk-design .nk-toast button { min-height: 28px; background-color: color-mix(in srgb, var(--nekoland-on-ink) 14%, transparent); color: var(--nekoland-on-ink); }
.nk-stage { border-radius: 12px; background: linear-gradient(140deg, #f3b47a, #d4577b 42%, #4b3f8f 78%, #1f2a5c); }
/* As the compositor draws it, by render.rs's BAND_FILL_ALPHA and BAND_EDGE_ALPHA. */
.nk-band { background-color: rgb(255 255 255 / 15%); box-shadow: inset 0 0 0 1px rgb(255 255 255 / 60%); }
.nk-glass { background-color: color-mix(in srgb, var(--nekoland-sheet) 50%, transparent); color: var(--nekoland-ink); box-shadow: inset 0 0 0 1px rgb(255 255 255 / 20%), 0 10px 30px -10px rgb(0 0 0 / 45%); }
.nk-bar { min-height: 30px; padding: 0 10px; border-radius: 12px 12px 0 0; box-shadow: none; font-size: 13px; }
.nk-bar .title { font-weight: 600; }
.nk-menu { padding: 6px; border-radius: 12px; font-size: 13px; }
.nk-menu-item { min-height: 24px; padding: 0 10px; border-radius: 6px; }
.nk-menu-item.lit { background-color: var(--nekoland-ink); color: var(--nekoland-on-ink); }
.nk-menu-item.danger { color: var(--nekoland-danger); }
.nk-keys { font-family: "Ubuntu Sans Mono", monospace; font-size: 11px; opacity: 0.6; }
.nk-menu-line { min-height: 1px; margin: 5px 4px; background-color: color-mix(in srgb, var(--nekoland-ink) 12%, transparent); }
.nk-notice { padding: 12px; border-radius: 14px; }
.nk-notice-title { font-weight: 600; }
.nk-notice-body { font-size: 13px; color: var(--nekoland-ink-2); }
.nk-dock { padding: 12px 12px 4px 12px; border-radius: 20px; }
.nk-dot { min-width: 4px; min-height: 4px; border-radius: 2px; background-color: var(--nekoland-ink); }
.nk-dot.active { min-width: 14px; }
.nk-dock-line { min-width: 1px; min-height: 32px; margin: 4px 0 12px 0; background-color: color-mix(in srgb, var(--nekoland-ink) 18%, transparent); }
.nk-window { border-radius: 14px; background-color: var(--nekoland-paper); box-shadow: 0 0 0 1px rgb(0 0 0 / 12%), 0 24px 50px -20px rgb(0 0 0 / 50%); }
.nk-titlebar { min-height: 44px; padding: 0 8px 0 16px; }
.nk-titlebar .title { font-weight: 600; }
.nk-design .nk-titlebar button { min-width: 24px; min-height: 24px; padding: 0; }
.nk-window.inactive .nk-titlebar .title, .nk-design .nk-window.inactive .nk-titlebar button { color: var(--nekoland-ink-3); }
.nk-window-body { min-height: 120px; border-top: 1px solid var(--nekoland-line); border-radius: 0 0 14px 14px; background-color: var(--nekoland-sheet); }
.nk-scrim { padding: 32px; border-radius: 12px; background-color: rgb(0 0 0 / 40%); }
.nk-dialog { padding: 15px 20px 20px; border-radius: 14px; background-color: var(--nekoland-sheet); box-shadow: inset 0 0 0 1px var(--nekoland-line), 0 30px 60px -20px rgb(0 0 0 / 50%); }
.nk-dialog-title { font-size: 16px; font-weight: 600; }
.nk-dialog .nk-lead { font-size: 14px; line-height: 21px; }
.nk-dialog button { min-height: 32px; padding: 0 14px; }
"#;

pub(crate) fn classes(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

/// A widget that is only its style classes, of the size they give it.
pub(crate) fn shape(names: &[&str]) -> gtk::Box {
    gtk::Box::builder().css_classes(classes(names)).build()
}

pub(crate) fn text(label: &str, names: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .css_classes(classes(names))
        .build()
}

pub(crate) fn column(spacing: i32, children: &[&gtk::Widget]) -> gtk::Box {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(spacing)
        .build();
    for child in children {
        column.append(*child);
    }
    column
}

pub(crate) fn line(spacing: i32, children: &[&gtk::Widget]) -> gtk::Box {
    let line = gtk::Box::builder().spacing(spacing).build();
    for child in children {
        line.append(*child);
    }
    line
}

/// A card, under a small heading if it has one.
pub(crate) fn card(heading: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["nk-card"])
        .build();
    if !heading.is_empty() {
        card.append(&text(heading, &["nk-h3"]));
    }
    card.append(child);
    card
}

/// Cards side by side, each at least `width` wide and at most `most` to a line, as the
/// design's grids are.
pub(crate) fn cards(width: i32, most: u32, children: &[&gtk::Widget]) -> gtk::FlowBox {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["nk-cards"])
        .column_spacing(16)
        .row_spacing(16)
        .max_children_per_line(most)
        .homogeneous(true)
        .build();
    for child in children {
        child.set_size_request(width, -1);
        flow.append(*child);
    }
    // The cards are not to be chosen, only what is in them.
    let mut cell = flow.first_child();
    while let Some(child) = cell {
        child.set_focusable(false);
        cell = child.next_sibling();
    }
    flow
}

pub(crate) fn section(title: &str, lead: &str, children: &[&gtk::Widget]) -> gtk::Box {
    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .build();
    let heading = column(4, &[text(title, &["nk-h2"]).upcast_ref()]);
    if !lead.is_empty() {
        let lead = text(lead, &["nk-lead"]);
        lead.set_wrap(true);
        heading.append(&lead);
    }
    section.append(&heading);
    for child in children {
        section.append(*child);
    }
    section
}

/// One of a few choices, the chosen one solid: libadwaita's toggle group.
// shortcut: made from its name, as the libadwaita crate in use stops at 1.5 and the group
// came with 1.7; its own type once the crate is moved up.
pub(crate) fn segmented(labels: &[&str], active: u32) -> gtk::Widget {
    let toggles: String = labels
        .iter()
        .map(|label| {
            format!(
                "<child><object class=\"AdwToggle\"><property name=\"label\">{}</property></object></child>",
                glib::markup_escape_text(label)
            )
        })
        .collect();
    let group: gtk::Widget = gtk::Builder::from_string(&format!(
        "<interface><object class=\"AdwToggleGroup\" id=\"group\">{toggles}</object></interface>"
    ))
    .object("group")
    .expect("the toggle group just described");
    group.set_property("active", active);
    group.set_halign(gtk::Align::Start);
    group
}

/// Nekoland's mark, as the menu bar draws it: a cat's head.
pub(crate) fn logo() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(16)
        .content_height(16)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(|area, cr, _, _| {
        cr.move_to(2.0, 6.5);
        cr.line_to(3.0, 1.0);
        cr.line_to(6.5, 4.0);
        cr.line_to(9.5, 4.0);
        cr.line_to(13.0, 1.0);
        cr.line_to(14.0, 6.5);
        cr.curve_to(15.5, 11.5, 12.0, 15.0, 8.0, 15.0);
        cr.curve_to(4.0, 15.0, 0.5, 11.5, 2.0, 6.5);
        cr.close_path();
        let color = area.color();
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        let _ = cr.fill();
    });
    area
}

/// Nekoland's design language, after its design, section by section: GTK's own widgets
/// in Nekoland's look where GTK has one, drawn here where it has none. Everything can be
/// tried, and the switch at the top shows it light or dark.
/// One of the page's rules: its name over a sentence or two.
pub(crate) fn rule_card(name: &str, body: &str) -> gtk::Widget {
    let name = text(name, &[]);
    name.set_markup(&format!("<b>{}</b>", name.label()));
    let body = text(body, &["nk-rule-body"]);
    body.set_wrap(true);
    // Narrow enough for three to a line.
    body.set_max_width_chars(20);
    let rule = column(4, &[name.upcast_ref(), body.upcast_ref()]);
    rule.set_css_classes(&["nk-card", "nk-rule"]);
    rule.upcast()
}

/// The construction of an icon, drawn as a sheet: the grid, the layers, the light, and
/// what a surface is and is not made of.
pub(crate) const ICON_SHEET: &[u8] = include_bytes!("../icon-sheet.svg");

/// How Nekoland's icons are built, with the theme's own icons as the examples.
pub(crate) fn icons_section() -> gtk::Box {
    let sheet = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .height_request(416)
        .build();
    match gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON_SHEET)) {
        Ok(drawing) => sheet.set_paintable(Some(&drawing)),
        Err(err) => eprintln!("icon sheet: {err}"),
    }
    // The first of each kind, a line's worth: the whole list is in Appearance.
    let strip = |name: &str, icons: Vec<PathBuf>| {
        let spec = text(&format!("{name} {}", icons.len()), &["nk-spec"]);
        spec.set_size_request(120, -1);
        let samples = gtk::Box::builder().spacing(10).build();
        for path in icons.iter().take(12) {
            samples.append(&gtk::Image::builder().file(path.to_string_lossy()).pixel_size(48).build());
        }
        line(16, &[spec.upcast_ref(), samples.upcast_ref()])
    };
    section(
        "圖示",
        "手繪的和從其他應用程式轉換的圖示照同一張圖製作，合成一個圖示主題。",
        &[
            card("", &sheet).upcast_ref(),
            strip("手繪", icons_in(Path::new(DRAWN_ICONS))).upcast_ref(),
            strip("自動演算", converted_icons()).upcast_ref(),
        ],
    )
}

pub(crate) fn design_page() -> gtk::ScrolledWindow {
    let provider = gtk::CssProvider::new();
    let styles = adw::StyleManager::default();
    // Light or dark as chosen on the page; as the system is until then.
    let chosen: Rc<std::cell::Cell<Option<bool>>> = Rc::default();
    provider.load_from_string(&design_css(styles.is_dark()));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
        );
    }

    // Colours: a swatch each, with the value it has now.
    let values: Vec<(gtk::Label, &str, &str)> = TOKENS
        .iter()
        .map(|(token, _, used)| (text("", &["nk-small"]), *token, *used))
        .collect();
    let show_values = {
        let values = values.clone();
        move |dark: bool| {
            for (label, token, used) in &values {
                label.set_markup(&format!(
                    "<span font_family='Ubuntu Sans Mono'>{}</span> · {used}",
                    token_value(token, dark)
                ));
            }
        }
    };
    show_values(styles.is_dark());
    let swatches: Vec<gtk::Widget> = TOKENS
        .iter()
        .zip(&values)
        .map(|((token, name, _), (value, ..))| {
            let swatch = shape(&["nk-swatch"]);
            // Each swatch takes its token from the page, so it turns with the page.
            let own = gtk::CssProvider::new();
            own.load_from_string(&format!(
                ".nk-swatch.nk-{token} {{ background-color: var(--nekoland-{token}); }}"
            ));
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &own,
                    gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
                );
            }
            swatch.add_css_class(&format!("nk-{token}"));
            let name = text(name, &[]);
            name.set_markup(&format!("<span weight='500'>{}</span>", name.label()));
            card(
                "",
                &column(
                    8,
                    &[
                        swatch.upcast_ref(),
                        column(0, &[name.upcast_ref(), value.upcast_ref()]).upcast_ref(),
                    ],
                ),
            )
            .upcast()
        })
        .collect();

    let theme = segmented(&["淺色", "深色"], u32::from(styles.is_dark()));
    theme.set_valign(gtk::Align::End);
    theme.connect_notify_local(
        Some("active"),
        glib::clone!(
            #[weak]
            provider,
            #[strong]
            chosen,
            #[strong]
            show_values,
            move |theme, _| {
                let dark = theme.property::<u32>("active") == 1;
                chosen.set(Some(dark));
                provider.load_from_string(&design_css(dark));
                show_values(dark);
            }
        ),
    );
    styles.connect_dark_notify(glib::clone!(
        #[weak]
        theme,
        #[strong]
        chosen,
        move |styles| {
            if chosen.get().is_none() {
                theme.set_property("active", u32::from(styles.is_dark()));
                chosen.set(None);
            }
        }
    ));

    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(56)
        .margin_top(40)
        .margin_bottom(80)
        .margin_start(32)
        .margin_end(32)
        .build();

    let intro = text(
        "介面只用墨色、紙色和灰階，選中就是實心墨色。顏色留給桌布、照片和視窗內容；需要警示時才出現紅色。",
        &["nk-lead"],
    );
    intro.set_wrap(true);
    intro.set_max_width_chars(40);
    let title = column(
        8,
        &[
            text("Nekoland 設計語言", &["nk-h1"]).upcast_ref(),
            intro.upcast_ref(),
        ],
    );
    title.set_hexpand(true);
    page.append(&line(24, &[title.upcast_ref(), &theme]));

    let rules: Vec<gtk::Widget> = [
        (
            "選中是墨色",
            "目前的分類、按下的選項、開著的開關，一律填實心墨色，不另用強調色。",
        ),
        (
            "顏色屬於內容",
            "桌布、App 圖示、照片和視窗是畫面上唯一的彩色。介面退到灰階，讓內容在前面。",
        ),
        (
            "一層玻璃",
            "只有浮在桌面上的東西用毛玻璃：選單列、Dock、通知、選單和側邊欄。視窗裡面是不透明的紙色。",
        ),
        (
            "動作有回彈",
            "按下時縮小一點，放開時回彈。開關的圓點按住會拉長，像被手指壓扁。",
        ),
    ]
    .iter()
    .map(|(name, body)| rule_card(name, body))
    .collect();
    page.append(&section(
        "原則",
        "",
        &[cards(320, 3, &rules.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    page.append(&section(
        "顏色",
        "每個名稱在淺色與深色各有一個值，切換右上角可以對照。",
        &[cards(240, 4, &swatches.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    page.append(&icons_section());

    let types = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    for (index, (spec, markup)) in [
        (
            "標題 32 / 700",
            "<span size='24pt' weight='bold'>桌面與 Dock</span>",
        ),
        (
            "頁面標題 20 / 600",
            "<span size='15pt' weight='600'>顯示器</span>",
        ),
        ("視窗標題 14 / 600", "<span weight='600'>設定</span>"),
        ("正文 14 / 400", "拖到螢幕邊緣時，視窗會貼齊到那一半。"),
        (
            "說明 12 / 400",
            "<span size='9pt' alpha='70%'>重新登入後生效</span>",
        ),
        (
            "群組標題 12 / 500",
            "<span size='9pt' weight='500' alpha='60%' letter_spacing='500'>系統</span>",
        ),
        (
            "數值 13 等寬",
            "<span font_family='Ubuntu Sans Mono' size='9.75pt'>6016 × 3384 · 60 Hz · Ctrl+Alt+T</span>",
        ),
    ]
    .iter()
    .enumerate()
    {
        let spec = text(spec, &["nk-spec"]);
        spec.set_size_request(120, -1);
        let sample = gtk::Label::builder()
            .label(*markup)
            .use_markup(true)
            .xalign(0.0)
            .build();
        let row = line(16, &[spec.upcast_ref(), sample.upcast_ref()]);
        row.set_css_classes(if index == 0 {
            &["nk-type-row"]
        } else {
            &["nk-type-row", "next"]
        });
        types.append(&row);
    }
    page.append(&section(
        "字體",
        "Ubuntu Sans 加 Noto Sans CJK TC；數值、路徑和快速鍵用 Ubuntu Sans Mono。",
        &[card("", &types).upcast_ref()],
    ));

    let steps = gtk::Box::builder().spacing(10).build();
    for step in [4, 8, 12, 16, 24, 32, 48] {
        let square = shape(&["nk-step"]);
        square.set_size_request(step, step);
        square.set_halign(gtk::Align::Center);
        square.set_valign(gtk::Align::End);
        square.set_vexpand(true);
        let number = text(&step.to_string(), &["nk-note"]);
        number.set_xalign(0.5);
        steps.append(&column(6, &[square.upcast_ref(), number.upcast_ref()]));
    }
    let corners = gtk::Box::builder().spacing(12).build();
    for (radius, used) in [
        ("6px", "6 · 小按鈕、提示"),
        ("8px", "8 · 按鈕、輸入框"),
        ("12px", "12 · 卡片、選單"),
        ("14px", "14 · 視窗、對話框"),
        ("99px", "全圓 · 開關、標籤"),
    ] {
        let corner = shape(&["nk-corner"]);
        corner.set_halign(gtk::Align::Start);
        let own = gtk::CssProvider::new();
        let class = format!("nk-corner-{}", radius.trim_end_matches("px"));
        own.load_from_string(&format!(".{class} {{ border-radius: {radius}; }}"));
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &own,
                gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
            );
        }
        corner.add_css_class(&class);
        corners.append(&column(
            12,
            &[corner.upcast_ref(), text(used, &["nk-note"]).upcast_ref()],
        ));
    }
    page.append(&section(
        "間距與圓角",
        "間距以 4 為單位；圓角依大小分級，越大的面越圓。",
        &[
            cards(240, 4, &[card("間距", &steps).upcast_ref()]).upcast_ref(),
            card("圓角", &corners).upcast_ref(),
        ],
    ));

    let button = |label: &str, names: &[&str]| {
        gtk::Button::builder()
            .label(label)
            .css_classes(classes(names))
            .build()
    };
    let disabled = button("儲存", &["suggested-action"]);
    disabled.set_sensitive(false);
    let add = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("新增")
                .build(),
        )
        .build();
    let kinds = line(
        12,
        &[
            button("儲存", &["suggested-action"]).upcast_ref(),
            button("取消", &[]).upcast_ref(),
            button("重設", &["flat"]).upcast_ref(),
            button("刪除", &["destructive-action"]).upcast_ref(),
            disabled.upcast_ref(),
            button("開始使用", &["suggested-action", "nk-large"]).upcast_ref(),
            add.upcast_ref(),
        ],
    );
    let mut kind = kinds.first_child();
    while let Some(widget) = kind {
        widget.set_valign(gtk::Align::Center);
        kind = widget.next_sibling();
    }
    let icon_button = |icon: &str, label: &str| {
        gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(label)
            .css_classes(["flat", "circular"])
            .build()
    };
    let icons = line(
        12,
        &[
            icon_button("system-search-symbolic", "搜尋").upcast_ref(),
            icon_button("view-more-horizontal-symbolic", "更多").upcast_ref(),
            icon_button("go-previous-symbolic", "返回").upcast_ref(),
        ],
    );
    let icon_card = card(
        "圖示按鈕",
        &column(
            12,
            &[
                icons.upcast_ref(),
                text("描邊 1.5，圓頭，16 格線。", &["nk-note"]).upcast_ref(),
            ],
        ),
    );
    page.append(&section(
        "按鈕",
        "一個畫面只有一個主要按鈕。按鈕寫出動作本身，不寫「確定」。",
        &[
            card("種類", &kinds).upcast_ref(),
            cards(240, 4, &[icon_card.upcast_ref()]).upcast_ref(),
        ],
    ));

    let field = |label: &str, widget: &gtk::Widget, error: &str| {
        let field = column(6, &[text(label, &["nk-label"]).upcast_ref(), widget]);
        if !error.is_empty() {
            field.append(&text(error, &["nk-err"]));
        }
        let card = card("", &field);
        card.upcast::<gtk::Widget>()
    };
    // Fields as wide as their cards, not as wide as GTK makes them by itself.
    let entry = |value: &str| {
        gtk::Entry::builder()
            .text(value)
            .width_chars(8)
            .max_width_chars(8)
            .build()
    };
    let wrong = entry("neko land");
    wrong.add_css_class("error");
    let search = gtk::SearchEntry::builder().placeholder_text("搜尋").build();
    search.set_width_chars(8);
    search.set_max_width_chars(8);
    let zone = gtk::DropDown::from_strings(&["台北（GMT+8）", "東京（GMT+9）", "倫敦（GMT+0）"]);
    let fields = [
        field("裝置名稱", entry("nekoland").upcast_ref(), ""),
        field("主機名稱", wrong.upcast_ref(), "不能有空格"),
        field("搜尋", search.upcast_ref(), ""),
        field("時區", zone.upcast_ref(), ""),
    ];
    page.append(&section(
        "輸入",
        "",
        &[cards(240, 4, &fields.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    let switches = line(
        12,
        &[
            gtk::Switch::builder()
                .active(true)
                .valign(gtk::Align::Center)
                .build()
                .upcast_ref(),
            gtk::Switch::builder()
                .valign(gtk::Align::Center)
                .build()
                .upcast_ref(),
        ],
    );
    let centre = gtk::CheckButton::builder().label("置中").active(true).build();
    let pointer = gtk::CheckButton::builder().label("游標所在處").build();
    pointer.set_group(Some(&centre));
    let checks = column(
        6,
        &[
            gtk::CheckButton::builder()
                .label("開機時啟動")
                .active(true)
                .build()
                .upcast_ref(),
            gtk::CheckButton::builder()
                .label("在 Dock 顯示")
                .build()
                .upcast_ref(),
            centre.upcast_ref(),
            pointer.upcast_ref(),
        ],
    );
    let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    slider.set_value(60.0);
    let level = text("60%", &["nk-note", "nk-mono"]);
    slider.connect_value_changed(glib::clone!(
        #[weak]
        level,
        move |slider| level.set_label(&format!("{}%", slider.value().round()))
    ));
    let choices = [
        card("開關", &switches).upcast::<gtk::Widget>(),
        card("核取方塊與單選", &checks).upcast(),
        card("分段選擇", &segmented(&["左側", "底部", "右側"], 1)).upcast(),
        card("滑桿", &column(8, &[slider.upcast_ref(), level.upcast_ref()])).upcast(),
    ];
    page.append(&section(
        "選擇",
        "",
        &[cards(240, 4, &choices.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    let rows = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    rows.append(&adw::SwitchRow::builder().title("毛玻璃").active(true).build());
    rows.append(
        &adw::SwitchRow::builder()
            .title("自動隱藏")
            .subtitle("游標移到螢幕底部時出現")
            .build(),
    );
    let resolution = adw::ActionRow::builder()
        .title("解析度")
        .activatable(true)
        .build();
    resolution.add_suffix(&text("6016 × 3384", &["nk-lead", "nk-mono"]));
    resolution.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    rows.append(&resolution);
    let bluetooth = adw::ActionRow::builder().title("藍牙").build();
    let closed = text("已關閉", &["nk-badge"]);
    closed.set_valign(gtk::Align::Center);
    bluetooth.add_suffix(&closed);
    rows.append(&bluetooth);
    let nav = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    for (name, detail) in [("桌面", ""), ("顯示器", "6016 × 3384"), ("輸入法", "Project Key")] {
        let name = text(name, &[]);
        name.set_hexpand(true);
        let detail = text(detail, &[]);
        detail.set_markup(&format!("<span size='9pt' alpha='60%'>{}</span>", detail.label()));
        nav.append(&line(8, &[name.upcast_ref(), detail.upcast_ref()]));
    }
    nav.select_row(nav.row_at_index(0).as_ref());
    page.append(&section(
        "清單與導覽",
        "",
        &[
            rows.upcast_ref(),
            cards(240, 4, &[card("側邊欄", &nav).upcast_ref()]).upcast_ref(),
        ],
    ));

    let badge = |label: &str, names: &[&str]| {
        let badge = text(label, names);
        badge.set_valign(gtk::Align::Center);
        badge
    };
    let badges = line(
        12,
        &[
            badge("3", &["nk-badge"]).upcast_ref(),
            badge("使用中", &["nk-badge", "solid"]).upcast_ref(),
            badge("離線", &["nk-badge", "danger"]).upcast_ref(),
        ],
    );
    let downloading = line(
        12,
        &[
            gtk::Spinner::builder().spinning(true).build().upcast_ref(),
            text("正在下載", &["nk-lead"]).upcast_ref(),
        ],
    );
    let progress = column(
        16,
        &[
            gtk::ProgressBar::builder().fraction(0.62).build().upcast_ref(),
            downloading.upcast_ref(),
        ],
    );
    let tip = text("Ctrl+Alt+T", &["nk-tip"]);
    tip.set_halign(gtk::Align::Start);
    let moved = text("已移到桌面 2", &[]);
    moved.set_hexpand(true);
    let toast = line(
        12,
        &[
            moved.upcast_ref(),
            gtk::Button::builder()
                .label("復原")
                .valign(gtk::Align::Center)
                .build()
                .upcast_ref(),
        ],
    );
    toast.add_css_class("nk-toast");
    let feedback = [
        card("標籤", &badges).upcast::<gtk::Widget>(),
        card("進度", &progress).upcast(),
        card("提示", &tip).upcast(),
        card("浮動訊息", &toast).upcast(),
    ];
    page.append(&section(
        "回饋",
        "",
        &[cards(240, 4, &feedback.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    page.append(&section(
        "浮在桌面上的",
        "選單列、選單、通知和 Dock 用同一層玻璃。模糊由合成器畫，這裡只有半透明。",
        &[desktop_glass().upcast_ref()],
    ));

    page.append(&section(
        "框選與並排預覽",
        "在桌面框選項目、把視窗拖到螢幕邊緣並排時，都畫同一種框：白色填色 15%，1 px 白邊 60%，直角。",
        &[desktop_bands().upcast_ref()],
    ));

    let window = |title: &str, active: bool| {
        let control = |icon: &str, label: &str| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(label)
                .valign(gtk::Align::Center)
                .css_classes(["circular"])
                .build()
        };
        let name = text(title, &["title"]);
        name.set_hexpand(true);
        let bar = line(
            8,
            &[
                name.upcast_ref(),
                control("window-minimize-symbolic", "最小化").upcast_ref(),
                control("window-maximize-symbolic", "最大化").upcast_ref(),
                control("window-close-symbolic", "關閉").upcast_ref(),
            ],
        );
        bar.add_css_class("nk-titlebar");
        let window = column(0, &[bar.upcast_ref(), shape(&["nk-window-body"]).upcast_ref()]);
        window.set_css_classes(if active {
            &["nk-window"]
        } else {
            &["nk-window", "inactive"]
        });
        window.upcast::<gtk::Widget>()
    };
    let windows = [window("設定", true), window("檔案", false)];
    page.append(&section(
        "視窗",
        "標題列和內容同一張紙，沒有分隔的色帶。非作用中的視窗只把標題退成灰色。",
        &[cards(240, 4, &windows.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    // An alert as nekoland.css has libadwaita draw it: 20px from each row's letters or edges
    // to the next. A line of the question stands 5px above its letters and 3px below, one of
    // the consequence 4px and 3px, and the icon's symbol 2px inside its image, so the margins
    // are that much less; a field and the buttons are spaced from their edges.
    let alert = |icon: Option<&str>,
                 title: &str,
                 body: Option<&str>,
                 field: Option<gtk::Widget>,
                 responses: &[(&str, &[&str])]| {
        let dialog = column(0, &[]);
        dialog.add_css_class("nk-dialog");
        let mut gap = 0;
        if let Some(icon) = icon {
            let image = gtk::Image::builder()
                .icon_name(icon)
                .pixel_size(32)
                .margin_top(3)
                .build();
            dialog.append(&image);
            gap = 13;
        }
        let centred = |label: &str, names: &[&str]| {
            let label = text(label, names);
            label.set_xalign(0.5);
            label.set_justify(gtk::Justification::Center);
            label.set_wrap(true);
            label
        };
        let question = centred(title, &["nk-dialog-title"]);
        question.set_margin_top(gap);
        dialog.append(&question);
        if let Some(body) = body {
            let body = centred(body, &["nk-lead"]);
            body.set_margin_top(13);
            dialog.append(&body);
        }
        gap = 17;
        if let Some(field) = field {
            field.set_margin_top(gap);
            dialog.append(&field);
            gap = 20;
        }
        let actions = line(12, &[]);
        actions.set_homogeneous(true);
        actions.set_margin_top(gap);
        for (label, names) in responses {
            actions.append(&button(label, names));
        }
        dialog.append(&actions);
        // An alert's width, as alert::FIELD_WIDTH and the 20px each side make it.
        dialog.set_size_request(360, -1);
        let scrim = column(0, &[dialog.upcast_ref()]);
        scrim.add_css_class("nk-scrim");
        scrim.set_halign(gtk::Align::Start);
        scrim.set_valign(gtk::Align::Start);
        scrim.upcast::<gtk::Widget>()
    };
    let alerts = [
        alert(
            None,
            "刪除桌面 2？",
            Some("上面的 3 個視窗會移到桌面 1。"),
            None,
            &[("取消", &[]), ("刪除", &["destructive-action"])],
        ),
        alert(
            None,
            "新增檔案夾",
            None,
            Some(gtk::Entry::builder().text("未命名檔案夾").build().upcast()),
            &[("取消", &[]), ("建立", &["suggested-action"])],
        ),
        alert(
            None,
            "無法建立檔案夾",
            Some("沒有寫入這個檔案夾的權限。"),
            None,
            &[("關閉", &[])],
        ),
        alert(
            Some("system-lock-screen-symbolic"),
            "需要管理員密碼",
            Some("Market 要安裝 GIMP。"),
            Some(
                gtk::PasswordEntry::builder()
                    .show_peek_icon(true)
                    .build()
                    .upcast(),
            ),
            &[("取消", &[]), ("認證", &["suggested-action"])],
        ),
    ];
    // Each scrim only as large as its alert, so it is 32px on every side.
    let grid = cards(424, 2, &alerts.iter().collect::<Vec<_>>());
    grid.set_homogeneous(false);
    page.append(&section(
        "對話框",
        "問題置中，後果用灰字。四邊和每一列之間都隔 20px，從字算起；按鈕平分一列，相隔 12px。",
        &[grid.upcast_ref()],
    ));
    let clamp = adw::Clamp::builder()
        .maximum_size(1120)
        .tightening_threshold(1120)
        .child(&page)
        .build();
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .css_classes(["nk-design"])
        .child(&clamp)
        .build()
}

/// The box the desktop draws to choose items, and the same box where a dragged window will
/// go, each over a wallpaper.
pub(crate) fn desktop_bands() -> gtk::FlowBox {
    let stage = |name: &str, bands: &[&gtk::Widget]| {
        let stage = gtk::Box::builder()
            .css_classes(["nk-stage"])
            .height_request(160)
            .homogeneous(true)
            .overflow(gtk::Overflow::Hidden)
            .build();
        for band in bands {
            stage.append(*band);
        }
        column(8, &[stage.upcast_ref(), text(name, &["nk-note"]).upcast_ref()]).upcast::<gtk::Widget>()
    };
    let choosing = shape(&["nk-band"]);
    choosing.set_size_request(120, 72);
    choosing.set_halign(gtk::Align::Start);
    choosing.set_valign(gtk::Align::Start);
    choosing.set_margin_start(36);
    choosing.set_margin_top(28);
    // The left half, the other half left empty.
    let preview = stage("並排預覽",&[shape(&["nk-band"]).upcast_ref(), shape(&[]).upcast_ref()]);
    cards(240, 2, &[&stage("框選", &[choosing.upcast_ref()]), &preview])
}

/// What floats over the desktop, as Nekoland draws it: the menu bar across the top with a
/// menu open under it, a notification under the bar's right end, and the Dock. Drawn over
/// a wallpaper, with translucency standing in for the compositor's blur.
pub(crate) fn desktop_glass() -> gtk::Box {
    let stage = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["nk-stage"])
        .build();

    let bar = gtk::Box::builder()
        .spacing(20)
        .css_classes(["nk-glass", "nk-bar"])
        .build();
    bar.append(&logo());
    bar.append(&text("檔案", &["title"]));
    for menu in ["檔案", "編輯", "顯示方式", "視窗", "輔助說明"] {
        bar.append(&text(menu, &[]));
    }
    let status = gtk::Box::builder()
        .spacing(14)
        .hexpand(true)
        .halign(gtk::Align::End)
        .build();
    for icon in [
        "input-keyboard-symbolic",
        "audio-volume-high-symbolic",
        "network-wired-symbolic",
    ] {
        status.append(&gtk::Image::builder().icon_name(icon).pixel_size(16).build());
    }
    status.append(&text("11月16日 週一 09:41", &[]));
    bar.append(&status);
    stage.append(&bar);

    // Under the bar: a menu where it opens from its title, a notification at the right.
    let item = |label: &str, keys: &str, names: &[&str]| {
        let name = text(label, &[]);
        name.set_hexpand(true);
        let row = line(16, &[name.upcast_ref(), text(keys, &["nk-keys"]).upcast_ref()]);
        row.set_css_classes(&[&["nk-menu-item"], names].concat());
        row
    };
    let menu = column(
        0,
        &[
            item("開新視窗", "Ctrl N", &[]).upcast_ref(),
            item("釘選到 Dock", "", &["lit"]).upcast_ref(),
            item("移到桌面 2", "", &[]).upcast_ref(),
            shape(&["nk-menu-line"]).upcast_ref(),
            item("結束", "Ctrl Q", &["danger"]).upcast_ref(),
        ],
    );
    menu.set_css_classes(&["nk-glass", "nk-menu"]);
    menu.set_size_request(220, -1);
    menu.set_valign(gtk::Align::Start);
    // As wide as it is, though what is in it would spread.
    menu.set_hexpand(false);
    menu.set_margin_start(36);

    let body = text("「設計稿.png」已複製到下載項目", &["nk-notice-body"]);
    body.set_wrap(true);
    let words = column(
        0,
        &[text("檔案", &["nk-notice-title"]).upcast_ref(), body.upcast_ref()],
    );
    words.set_hexpand(true);
    let notice = line(
        10,
        &[
            gtk::Image::builder()
                .icon_name("org.gnome.Nautilus")
                .pixel_size(36)
                .valign(gtk::Align::Start)
                .build()
                .upcast_ref(),
            words.upcast_ref(),
        ],
    );
    notice.set_css_classes(&["nk-glass", "nk-notice"]);
    notice.set_size_request(360, -1);
    notice.set_valign(gtk::Align::Start);
    notice.set_halign(gtk::Align::End);
    notice.set_hexpand(true);
    notice.set_margin_end(12);
    let under = line(20, &[menu.upcast_ref(), notice.upcast_ref()]);
    under.set_margin_top(4);
    stage.append(&under);

    // The Dock: all applications, the kept ones, a line, the Trash; a dot under those that
    // run, a longer one under the active one.
    let dock = gtk::Box::builder()
        .spacing(12)
        .css_classes(["nk-glass", "nk-dock"])
        .halign(gtk::Align::Center)
        .margin_top(96)
        .margin_bottom(8)
        .build();
    for (icon, running) in [
        ("view-app-grid-symbolic", None),
        ("org.gnome.Nautilus", Some(true)),
        ("org.gnome.TextEditor", Some(false)),
        ("org.gnome.Calculator", None),
        ("utilities-terminal", Some(false)),
        ("", None),
        ("user-trash", None),
    ] {
        if icon.is_empty() {
            dock.append(&shape(&["nk-dock-line"]));
            continue;
        }
        let dot = match running {
            Some(true) => shape(&["nk-dot", "active"]),
            Some(false) => shape(&["nk-dot"]),
            None => shape(&[]),
        };
        dot.set_halign(gtk::Align::Center);
        dot.set_size_request(-1, 4);
        dock.append(&column(
            4,
            &[
                gtk::Image::builder()
                    .icon_name(icon)
                    .pixel_size(40)
                    .build()
                    .upcast_ref(),
                dot.upcast_ref(),
            ],
        ));
    }
    stage.append(&dock);
    stage
}
