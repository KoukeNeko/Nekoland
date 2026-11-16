//! The menu bar along the top, after the Mac's: the system menu, the menus of the active
//! application, the status items programs put there, and the clock.
//!
//! A click opens a menu; after that the menus follow the pointer across the bar until one
//! is chosen or dismissed. A press dragged onto an entry and let go chooses it in one move.
//! The keyboard works too: the arrows move, Return chooses, Escape closes, and Ctrl+F2
//! opens the first menu.

use std::collections::HashMap;

use cairo::{Context, Format, ImageSurface};
use smithay::{
    backend::renderer::element::Id,
    desktop::{Window, WindowSurface},
    input::keyboard::Keysym,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as ToplevelState,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    config,
    dbus::{Bus, Event, MenuEntry, Request, StatusItem},
    text::{self, Picture},
    windows::{is_managed, window_names, Side},
    Nekoland,
};

pub const MENU_BAR_HEIGHT: i32 = 30;
const TITLE_PADDING: i32 = 10;
const TITLE_HIGHLIGHT_HEIGHT: i32 = 22;
const STATUS_ICON: i32 = 18;
const LOGO: f64 = 16.0;
const ROW_HEIGHT: i32 = 24;
const SEPARATOR_HEIGHT: i32 = 11;
const MENU_PADDING: i32 = 6;
const MENU_MIN_WIDTH: i32 = 220;
const MENU_RADIUS: i32 = 12;
/// Room on the left of an entry for its check mark, and per level of a submenu.
const CHECK_COLUMN: i32 = 22;
const INDENT: i32 = 14;
const GAP_BELOW_BAR: i32 = 4;

const FONT: &str = "Ubuntu Sans, Noto Sans CJK TC";
const MONO: &str = "Ubuntu Sans Mono";
const WEEKDAYS: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];

/// What choosing a menu entry does.
#[derive(Clone, PartialEq)]
pub enum Command {
    Sleep,
    Restart,
    ShutDown,
    LogOut,
    /// One of the actions the active application exports, by name.
    Action(String),
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Minimize,
    Zoom,
    TileLeft,
    TileRight,
    MoveToDesktop(usize),
    BringAllToFront,
    Focus(Window),
    Status {
        item: String,
        id: i32,
    },
}

#[derive(Clone)]
pub struct Entry {
    label: String,
    shortcut: &'static str,
    enabled: bool,
    separator: bool,
    checked: Option<bool>,
    depth: usize,
    command: Option<Command>,
}

impl Entry {
    fn item(label: impl Into<String>, command: Command) -> Self {
        Self {
            label: label.into(),
            shortcut: "",
            enabled: true,
            separator: false,
            checked: None,
            depth: 0,
            command: Some(command),
        }
    }

    fn separator() -> Self {
        Self {
            label: String::new(),
            shortcut: "",
            enabled: false,
            separator: true,
            checked: None,
            depth: 0,
            command: None,
        }
    }

    fn shortcut(mut self, shortcut: &'static str) -> Self {
        self.shortcut = shortcut;
        self
    }

    fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    fn chooseable(&self) -> bool {
        self.enabled && !self.separator && self.command.is_some()
    }
}

/// What a place on the bar opens.
#[derive(Clone, PartialEq)]
enum Source {
    System,
    App,
    File,
    Window,
    Help,
    Status(String),
}

struct Title {
    rect: Rectangle<i32, Logical>,
    source: Source,
}

pub struct OpenMenu {
    title: usize,
    entries: Vec<Entry>,
    rect: Rectangle<i32, Logical>,
    rows: Vec<Rectangle<i32, Logical>>,
    highlighted: Option<usize>,
    picture: Option<Picture>,
    drawn: Option<(Option<usize>, bool)>,
}

/// What the bar shows; it is drawn again when any of it changes.
#[derive(Clone, PartialEq)]
struct Drawn {
    width: i32,
    app: Option<(String, String)>,
    actions: usize,
    open: Option<usize>,
    dark: bool,
    minute: i64,
    items: u64,
}

pub struct MenuBar {
    pub bus: Option<Bus>,
    dark: bool,
    /// The actions each application exports, as far as they are known.
    actions: HashMap<String, Vec<String>>,
    items: Vec<StatusItem>,
    items_revision: u64,
    icons: HashMap<String, Option<ImageSurface>>,
    titles: Vec<Title>,
    pub open: Option<OpenMenu>,
    picture: Option<Picture>,
    drawn: Option<Drawn>,
    bar_id: Id,
    menu_id: Id,
}

impl Default for MenuBar {
    fn default() -> Self {
        Self {
            bus: None,
            dark: true,
            actions: HashMap::new(),
            items: Vec::new(),
            items_revision: 0,
            icons: HashMap::new(),
            titles: Vec::new(),
            open: None,
            picture: None,
            drawn: None,
            bar_id: Id::new(),
            menu_id: Id::new(),
        }
    }
}

/// Something the compositor draws over everything, on glass.
pub struct Overlay<'a> {
    pub id: &'a Id,
    pub rect: Rectangle<i32, Logical>,
    pub radius: i32,
    pub picture: &'a Picture,
}

type Rgb = (f64, f64, f64);

struct Colors {
    ink: Rgb,
    ink_3: Rgb,
    on_ink: Rgb,
    bar: (f64, f64, f64, f64),
    menu: (f64, f64, f64, f64),
    edge: (f64, f64, f64, f64),
}

// The design language's ink, paper and glass, in both themes.
fn colors(dark: bool) -> Colors {
    if dark {
        Colors {
            ink: (0.945, 0.937, 0.918),
            ink_3: (0.576, 0.569, 0.592),
            on_ink: (0.090, 0.090, 0.102),
            bar: (0.133, 0.133, 0.149, 0.45),
            menu: (0.133, 0.133, 0.149, 0.78),
            edge: (1.0, 1.0, 1.0, 0.16),
        }
    } else {
        Colors {
            ink: (0.090, 0.090, 0.102),
            ink_3: (0.435, 0.427, 0.447),
            on_ink: (0.984, 0.980, 0.973),
            bar: (0.984, 0.980, 0.973, 0.55),
            menu: (0.984, 0.980, 0.973, 0.80),
            edge: (0.0, 0.0, 0.0, 0.10),
        }
    }
}

fn font(weight: &str, points: f64) -> pango::FontDescription {
    pango::FontDescription::from_string(&format!("{FONT} {weight} {points}"))
}

fn text_layout(cr: &Context, text: &str, font: &pango::FontDescription) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    layout.set_font_description(Some(font));
    layout.set_text(text);
    layout
}

fn text_size(text: &str, font: &pango::FontDescription) -> (i32, i32) {
    let Ok(surface) = ImageSurface::create(Format::ARgb32, 1, 1) else {
        return (0, 0);
    };
    let Ok(cr) = Context::new(&surface) else {
        return (0, 0);
    };
    text_layout(&cr, text, font).pixel_size()
}

fn show_text(cr: &Context, text: &str, font: &pango::FontDescription, x: f64, center_y: f64, color: Rgb) {
    let layout = text_layout(cr, text, font);
    let (_, height) = layout.pixel_size();
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.move_to(x, center_y - f64::from(height) / 2.0);
    pangocairo::functions::show_layout(cr, &layout);
}

/// Nekoland's mark: a cat's head, where the Mac has its apple.
fn draw_logo(cr: &Context, x: f64, y: f64, color: Rgb) {
    let s = LOGO / 16.0;
    cr.move_to(x + 2.0 * s, y + 6.5 * s);
    cr.line_to(x + 3.0 * s, y + 1.0 * s);
    cr.line_to(x + 6.5 * s, y + 4.0 * s);
    cr.line_to(x + 9.5 * s, y + 4.0 * s);
    cr.line_to(x + 13.0 * s, y + 1.0 * s);
    cr.line_to(x + 14.0 * s, y + 6.5 * s);
    cr.curve_to(
        x + 15.5 * s,
        y + 11.5 * s,
        x + 12.0 * s,
        y + 15.0 * s,
        x + 8.0 * s,
        y + 15.0 * s,
    );
    cr.curve_to(
        x + 4.0 * s,
        y + 15.0 * s,
        x + 0.5 * s,
        y + 11.5 * s,
        x + 2.0 * s,
        y + 6.5 * s,
    );
    cr.close_path();
    cr.set_source_rgb(color.0, color.1, color.2);
    let _ = cr.fill();
}

/// The minute it is now, and the clock as the bar shows it.
fn clock() -> (i64, String) {
    // SAFETY: localtime_r only writes the tm it is given.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    let minute = ((i64::from(tm.tm_yday) * 24 + i64::from(tm.tm_hour)) * 60) + i64::from(tm.tm_min);
    let weekday = WEEKDAYS[tm.tm_wday.rem_euclid(7) as usize];
    let text = format!(
        "{}月{}日 週{} {:02}:{:02}",
        tm.tm_mon + 1,
        tm.tm_mday,
        weekday,
        tm.tm_hour,
        tm.tm_min
    );
    (minute, text)
}

impl MenuBar {
    fn status_icon(&mut self, item: &StatusItem, tint: Rgb) -> Option<ImageSurface> {
        let key = format!("{}|{}|{:?}", item.key, item.icon_name, tint);
        if let Some(icon) = self.icons.get(&key) {
            return icon.clone();
        }
        let icon = (!item.icon_name.is_empty())
            .then(|| text::named_icon(&item.icon_name, &item.icon_theme_path, STATUS_ICON, tint))
            .flatten()
            .or_else(|| {
                let (width, height, argb) = item.pixmap.as_ref()?;
                text::pixmap_icon(*width, *height, argb)
            });
        self.icons.insert(key, icon.clone());
        icon
    }

    /// Lays the bar out and draws it.
    fn draw_bar(&mut self, width: i32, app: Option<&(String, String)>, actions: &[String], clock: &str) {
        let colors = colors(self.dark);
        let open = self.open.as_ref().map(|open| open.title);
        let title_font = font("Medium", 10.0);
        let app_font = font("Bold", 10.0);

        // Left: the mark, then the application's menus.
        let mut titles: Vec<(Source, Option<String>, i32)> = vec![(Source::System, None, LOGO as i32)];
        if let Some((_, name)) = app {
            titles.push((Source::App, Some(name.clone()), text_size(name, &app_font).0));
            if actions.iter().any(|action| action == "new-window") {
                titles.push((
                    Source::File,
                    Some("檔案".into()),
                    text_size("檔案", &title_font).0,
                ));
            }
            titles.push((
                Source::Window,
                Some("視窗".into()),
                text_size("視窗", &title_font).0,
            ));
            if actions.iter().any(|action| action == "help") {
                titles.push((
                    Source::Help,
                    Some("輔助說明".into()),
                    text_size("輔助說明", &title_font).0,
                ));
            }
        }

        let Ok(surface) = ImageSurface::create(Format::ARgb32, width.max(1), MENU_BAR_HEIGHT) else {
            return;
        };
        let Ok(cr) = Context::new(&surface) else {
            return;
        };
        cr.set_source_rgba(colors.bar.0, colors.bar.1, colors.bar.2, colors.bar.3);
        let _ = cr.paint();

        let center = f64::from(MENU_BAR_HEIGHT) / 2.0;
        let highlight = |cr: &Context, rect: Rectangle<i32, Logical>| {
            let y = (MENU_BAR_HEIGHT - TITLE_HIGHLIGHT_HEIGHT) / 2;
            text::rounded_rectangle(
                cr,
                f64::from(rect.loc.x),
                f64::from(y),
                f64::from(rect.size.w),
                f64::from(TITLE_HIGHLIGHT_HEIGHT),
                6.0,
            );
            cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
            let _ = cr.fill();
        };

        self.titles.clear();
        let mut x = 8;
        for (source, label, content_width) in titles {
            let rect = Rectangle::new(
                (x, 0).into(),
                (content_width + TITLE_PADDING * 2, MENU_BAR_HEIGHT).into(),
            );
            let lit = open == Some(self.titles.len());
            if lit {
                highlight(&cr, rect);
            }
            let color = if lit { colors.on_ink } else { colors.ink };
            match &label {
                None => draw_logo(&cr, f64::from(x + TITLE_PADDING), center - LOGO / 2.0, color),
                Some(label) => {
                    let font = if source == Source::App {
                        &app_font
                    } else {
                        &title_font
                    };
                    show_text(&cr, label, font, f64::from(x + TITLE_PADDING), center, color);
                }
            }
            self.titles.push(Title { rect, source });
            x += rect.size.w;
        }

        // Right: the clock, and the status items left of it.
        let clock_font = font("Medium", 10.0);
        let clock_width = text_size(clock, &clock_font).0;
        let mut right = width - 12 - clock_width;
        show_text(&cr, clock, &clock_font, f64::from(right), center, colors.ink);
        right -= 8;

        let items: Vec<StatusItem> = self.items.iter().filter(|item| !item.passive).cloned().collect();
        for item in items.iter().rev() {
            let rect = Rectangle::new(
                (right - STATUS_ICON - TITLE_PADDING * 2 + 8, 0).into(),
                (STATUS_ICON + TITLE_PADDING * 2 - 8, MENU_BAR_HEIGHT).into(),
            );
            let lit = open == Some(self.titles.len());
            if lit {
                highlight(&cr, rect);
            }
            let tint = if lit { colors.on_ink } else { colors.ink };
            if let Some(icon) = self.status_icon(item, tint) {
                let scale = f64::from(STATUS_ICON) / f64::from(icon.width().max(icon.height()).max(1));
                cr.save().ok();
                cr.translate(
                    f64::from(rect.loc.x + (rect.size.w - STATUS_ICON) / 2),
                    f64::from((MENU_BAR_HEIGHT - STATUS_ICON) / 2),
                );
                cr.scale(scale, scale);
                let _ = cr.set_source_surface(&icon, 0.0, 0.0);
                let _ = cr.paint();
                cr.restore().ok();
            }
            self.titles.push(Title {
                rect,
                source: Source::Status(item.key.clone()),
            });
            right = rect.loc.x;
        }
        drop(cr);
        self.picture = text::picture(surface);
    }

    /// Where a menu of these entries goes under a title, and where each entry is in it.
    fn place_menu(
        entries: &[Entry],
        under: Rectangle<i32, Logical>,
        output_width: i32,
    ) -> (Rectangle<i32, Logical>, Vec<Rectangle<i32, Logical>>) {
        let label_font = font("", 10.0);
        let shortcut_font = pango::FontDescription::from_string(&format!("{MONO} 9"));
        let widest = entries
            .iter()
            .map(|entry| {
                let label = text_size(&entry.label, &label_font).0 + entry.depth as i32 * INDENT;
                let shortcut = if entry.shortcut.is_empty() {
                    0
                } else {
                    24 + text_size(entry.shortcut, &shortcut_font).0
                };
                label + shortcut
            })
            .max()
            .unwrap_or(0);
        let width = (widest + CHECK_COLUMN + MENU_PADDING * 2 + 16).max(MENU_MIN_WIDTH);

        let mut rows = Vec::with_capacity(entries.len());
        let mut y = MENU_PADDING;
        for entry in entries {
            let height = if entry.separator {
                SEPARATOR_HEIGHT
            } else {
                ROW_HEIGHT
            };
            rows.push(Rectangle::new(
                (MENU_PADDING, y).into(),
                (width - MENU_PADDING * 2, height).into(),
            ));
            y += height;
        }
        let height = y + MENU_PADDING;
        let x = under.loc.x.min(output_width - width - 6).max(6);
        let rect = Rectangle::new(
            (x, MENU_BAR_HEIGHT + GAP_BELOW_BAR).into(),
            (width, height).into(),
        );
        let rows = rows
            .into_iter()
            .map(|row| Rectangle::new(row.loc + rect.loc, row.size))
            .collect();
        (rect, rows)
    }

    fn draw_menu(dark: bool, open: &mut OpenMenu) {
        let colors = colors(dark);
        let key = (open.highlighted, dark);
        if open.drawn == Some(key) && open.picture.is_some() {
            return;
        }
        open.drawn = Some(key);
        let size: Size<i32, Logical> = open.rect.size;
        let Ok(surface) = ImageSurface::create(Format::ARgb32, size.w, size.h) else {
            return;
        };
        let Ok(cr) = Context::new(&surface) else {
            return;
        };
        let radius = f64::from(MENU_RADIUS);
        text::rounded_rectangle(
            &cr,
            0.5,
            0.5,
            f64::from(size.w) - 1.0,
            f64::from(size.h) - 1.0,
            radius,
        );
        cr.set_source_rgba(colors.menu.0, colors.menu.1, colors.menu.2, colors.menu.3);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(colors.edge.0, colors.edge.1, colors.edge.2, colors.edge.3);
        cr.set_line_width(1.0);
        let _ = cr.stroke();

        let label_font = font("", 10.0);
        let shortcut_font = pango::FontDescription::from_string(&format!("{MONO} 9"));
        for (index, (entry, row)) in open.entries.iter().zip(&open.rows).enumerate() {
            let local = Rectangle::<i32, Logical>::new(row.loc - open.rect.loc, row.size);
            let (x, y, w, h) = (
                f64::from(local.loc.x),
                f64::from(local.loc.y),
                f64::from(local.size.w),
                f64::from(local.size.h),
            );
            if entry.separator {
                cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.12);
                cr.rectangle(x + 4.0, y + h / 2.0, w - 8.0, 1.0);
                let _ = cr.fill();
                continue;
            }
            let lit = open.highlighted == Some(index);
            if lit {
                text::rounded_rectangle(&cr, x, y, w, h, 6.0);
                cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
                let _ = cr.fill();
            }
            let color = match (lit, entry.enabled) {
                (true, _) => colors.on_ink,
                (false, true) => colors.ink,
                (false, false) => colors.ink_3,
            };
            let center = y + h / 2.0;
            if entry.checked == Some(true) {
                show_text(&cr, "✓", &label_font, x + 6.0, center, color);
            }
            let indent = f64::from(CHECK_COLUMN + entry.depth as i32 * INDENT);
            show_text(&cr, &entry.label, &label_font, x + indent, center, color);
            if !entry.shortcut.is_empty() {
                let width = text_size(entry.shortcut, &shortcut_font).0;
                let shortcut_color = if lit { colors.on_ink } else { colors.ink_3 };
                show_text(
                    &cr,
                    entry.shortcut,
                    &shortcut_font,
                    x + w - 10.0 - f64::from(width),
                    center,
                    shortcut_color,
                );
            }
        }
        drop(cr);
        open.picture = text::picture(surface);
    }
}

impl Nekoland {
    fn output_origin(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.space.outputs().next()?;
        self.space.output_geometry(output)
    }

    /// The active application: its ID and the name it goes by.
    fn active_app(&mut self) -> Option<(String, String)> {
        let window = self.focused_window().filter(is_managed)?;
        let (_, app_id) = window_names(&window);
        if app_id.is_empty() {
            return None;
        }
        let name = self
            .apps
            .info(&app_id)
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| app_id.rsplit('.').next().unwrap_or(&app_id).to_string());
        Some((app_id, name))
    }

    fn app_windows(&self, app_id: &str) -> Vec<Window> {
        self.space
            .elements()
            .filter(|window| is_managed(window) && window_names(window).1 == app_id)
            .cloned()
            .collect()
    }

    /// Whether the bar is there to see: not in the overview, nor over a fullscreen window.
    pub fn menu_bar_shown(&self) -> bool {
        self.overview.is_none() && self.fullscreen_window().is_none()
    }

    /// Whether the bar or an open menu is at a position, so that it is not a window's.
    pub fn menu_bar_covers(&self, pos: Point<f64, Logical>) -> bool {
        let Some(output) = self.output_origin() else {
            return false;
        };
        self.menu_bar_shown()
            && (self.menu_bar.open.is_some() || pos.y < f64::from(output.loc.y + MENU_BAR_HEIGHT))
    }

    /// Brings the bar and any open menu up to date before a frame.
    pub fn refresh_menu_bar(&mut self) {
        let Some(output) = self.output_origin() else {
            return;
        };
        let app = self.active_app();
        if let Some((app_id, _)) = &app {
            if !self.menu_bar.actions.contains_key(app_id) {
                self.menu_bar.actions.insert(app_id.clone(), Vec::new());
                if let Some(bus) = &self.menu_bar.bus {
                    bus.send(Request::Actions(app_id.clone()));
                }
            }
        }
        let actions = app
            .as_ref()
            .and_then(|(app_id, _)| self.menu_bar.actions.get(app_id))
            .cloned()
            .unwrap_or_default();
        let (minute, clock) = clock();
        let drawn = Drawn {
            width: output.size.w,
            app: app.clone(),
            actions: actions.len(),
            open: self.menu_bar.open.as_ref().map(|open| open.title),
            dark: self.menu_bar.dark,
            minute,
            items: self.menu_bar.items_revision,
        };
        if self.menu_bar.drawn.as_ref() != Some(&drawn) || self.menu_bar.picture.is_none() {
            // A menu of an application that is no longer active goes with it.
            if self
                .menu_bar
                .drawn
                .as_ref()
                .is_some_and(|before| before.app != drawn.app)
            {
                self.menu_bar.open = None;
            }
            self.menu_bar
                .draw_bar(output.size.w, app.as_ref(), &actions, &clock);
            self.menu_bar.drawn = Some(Drawn {
                open: self.menu_bar.open.as_ref().map(|open| open.title),
                ..drawn
            });
        }
        let dark = self.menu_bar.dark;
        if let Some(open) = self.menu_bar.open.as_mut() {
            MenuBar::draw_menu(dark, open);
        }
    }

    /// What the bar and its open menu put on screen, front first.
    pub fn menu_bar_overlays(&self) -> Vec<Overlay<'_>> {
        let Some(output) = self.output_origin() else {
            return Vec::new();
        };
        if !self.menu_bar_shown() {
            return Vec::new();
        }
        let bar = &self.menu_bar;
        let mut overlays = Vec::new();
        if let Some(open) = &bar.open {
            if let Some(picture) = &open.picture {
                overlays.push(Overlay {
                    id: &bar.menu_id,
                    rect: Rectangle::new(open.rect.loc + output.loc, open.rect.size),
                    radius: MENU_RADIUS,
                    picture,
                });
            }
        }
        if let Some(picture) = &bar.picture {
            overlays.push(Overlay {
                id: &bar.bar_id,
                rect: Rectangle::new(output.loc, (output.size.w, MENU_BAR_HEIGHT).into()),
                radius: 0,
                picture,
            });
        }
        overlays
    }

    /// Takes in what the D-Bus thread learned.
    pub fn menu_bar_event(&mut self, event: Event) {
        let bar = &mut self.menu_bar;
        match event {
            Event::Actions { app_id, actions } => {
                bar.actions.insert(app_id, actions);
            }
            Event::Item(item) => {
                match bar.items.iter_mut().find(|known| known.key == item.key) {
                    Some(known) => *known = item,
                    None => bar.items.push(item),
                }
                bar.items_revision += 1;
                bar.icons.clear();
            }
            Event::ItemGone(key) => {
                bar.items.retain(|item| item.key != key);
                bar.items_revision += 1;
                if bar.open.is_some() {
                    bar.open = None;
                }
            }
            Event::Dark(dark) => bar.dark = dark,
        }
    }

    fn local(&self, pos: Point<f64, Logical>) -> Option<Point<i32, Logical>> {
        let output = self.output_origin()?;
        Some(pos.to_i32_floor() - output.loc)
    }

    fn title_at(&self, local: Point<i32, Logical>) -> Option<usize> {
        self.menu_bar
            .titles
            .iter()
            .position(|title| title.rect.contains(local))
    }

    fn row_at(&self, local: Point<i32, Logical>) -> Option<usize> {
        let open = self.menu_bar.open.as_ref()?;
        open.rows
            .iter()
            .position(|row| row.contains(local))
            .filter(|&index| open.entries[index].chooseable())
    }

    pub fn close_menu(&mut self) {
        self.menu_bar.open = None;
    }

    /// Opens the menu of a title, or for a status item without one, activates it.
    fn open_menu(&mut self, title: usize, highlight_first: bool) {
        let Some(source) = self.menu_bar.titles.get(title).map(|title| title.source.clone()) else {
            return;
        };
        let entries = self.menu_entries(&source);
        if entries.is_empty() {
            self.menu_bar.open = None;
            if let (Source::Status(item), Some(bus)) = (&source, &self.menu_bar.bus) {
                let rect = self.menu_bar.titles[title].rect;
                bus.send(Request::ItemActivate {
                    item: item.clone(),
                    x: rect.loc.x,
                    y: MENU_BAR_HEIGHT,
                });
            }
            return;
        }
        let width = self.output_origin().map_or(0, |output| output.size.w);
        let (rect, rows) = MenuBar::place_menu(&entries, self.menu_bar.titles[title].rect, width);
        let highlighted = highlight_first
            .then(|| entries.iter().position(Entry::chooseable))
            .flatten();
        self.menu_bar.open = Some(OpenMenu {
            title,
            entries,
            rect,
            rows,
            highlighted,
            picture: None,
            drawn: None,
        });
    }

    fn menu_entries(&mut self, source: &Source) -> Vec<Entry> {
        let app = self.active_app();
        let actions = app
            .as_ref()
            .and_then(|(app_id, _)| self.menu_bar.actions.get(app_id))
            .cloned()
            .unwrap_or_default();
        let has = |action: &str| actions.iter().any(|known| known == action);
        let name = app.as_ref().map(|(_, name)| name.clone()).unwrap_or_default();
        let focused = self.focused_window().filter(is_managed);

        match source {
            Source::System => vec![
                Entry::item("睡眠", Command::Sleep),
                Entry::item("重新開機⋯", Command::Restart),
                Entry::item("關機⋯", Command::ShutDown),
                Entry::separator(),
                Entry::item("登出⋯", Command::LogOut),
            ],
            Source::App => {
                let mut entries = Vec::new();
                if has("about") {
                    entries.push(Entry::item(
                        format!("關於「{name}」"),
                        Command::Action("about".into()),
                    ));
                    entries.push(Entry::separator());
                }
                if has("preferences") {
                    entries.push(Entry::item("設定⋯", Command::Action("preferences".into())));
                }
                if has("shortcuts") {
                    entries.push(Entry::item("鍵盤快速鍵", Command::Action("shortcuts".into())));
                }
                if has("preferences") || has("shortcuts") {
                    entries.push(Entry::separator());
                }
                entries.push(Entry::item(format!("隱藏「{name}」"), Command::Hide).shortcut("Super H"));
                entries.push(Entry::item("隱藏其他", Command::HideOthers));
                entries.push(Entry::item("顯示全部", Command::ShowAll).enabled(!self.minimized.is_empty()));
                entries.push(Entry::separator());
                entries.push(Entry::item(format!("結束「{name}」"), Command::Quit));
                entries
            }
            Source::File => vec![Entry::item("新增視窗", Command::Action("new-window".into()))],
            Source::Window => {
                let has_window = focused.is_some();
                let mut entries = vec![
                    Entry::item("縮到最小", Command::Minimize)
                        .shortcut("Super H")
                        .enabled(has_window),
                    Entry::item("縮放", Command::Zoom)
                        .shortcut("Super ↑")
                        .enabled(has_window),
                    Entry::item("移到螢幕左半邊", Command::TileLeft)
                        .shortcut("Super ←")
                        .enabled(has_window),
                    Entry::item("移到螢幕右半邊", Command::TileRight)
                        .shortcut("Super →")
                        .enabled(has_window),
                ];
                let desktops = self.workspaces.count();
                let active = self.workspaces.active;
                if desktops > 1 && has_window {
                    entries.push(Entry::separator());
                    entries.extend((0..desktops).filter(|&index| index != active).map(|index| {
                        Entry::item(format!("移到桌面 {}", index + 1), Command::MoveToDesktop(index))
                    }));
                }
                entries.push(Entry::separator());
                entries.push(Entry::item("全部移到最前面", Command::BringAllToFront));
                if let Some((app_id, _)) = &app {
                    let windows = self.app_windows(app_id);
                    if !windows.is_empty() {
                        entries.push(Entry::separator());
                    }
                    for window in windows {
                        let title = window_names(&window).0;
                        let title = if title.is_empty() { name.clone() } else { title };
                        let current = focused.as_ref() == Some(&window);
                        entries.push(Entry::item(title, Command::Focus(window)).checked(current));
                    }
                }
                entries
            }
            Source::Help => vec![Entry::item(
                format!("「{name}」輔助說明"),
                Command::Action("help".into()),
            )],
            Source::Status(key) => self
                .menu_bar
                .items
                .iter()
                .find(|item| item.key == *key)
                .map(|item| {
                    item.menu
                        .iter()
                        .map(|entry: &MenuEntry| {
                            if entry.separator {
                                return Entry::separator();
                            }
                            let mut converted = Entry::item(
                                entry.label.clone(),
                                Command::Status {
                                    item: key.clone(),
                                    id: entry.id,
                                },
                            )
                            .enabled(entry.enabled);
                            converted.checked = entry.checked;
                            converted.depth = entry.depth;
                            converted
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// A button went down. Returns whether the bar took it.
    pub fn menu_bar_press(&mut self, pos: Point<f64, Logical>) -> bool {
        if !self.menu_bar_covers(pos) {
            return false;
        }
        let Some(local) = self.local(pos) else {
            return false;
        };
        if let Some(title) = self.title_at(local) {
            if self
                .menu_bar
                .open
                .as_ref()
                .is_some_and(|open| open.title == title)
            {
                self.close_menu();
            } else {
                self.open_menu(title, false);
            }
            return true;
        }
        let in_menu = self
            .menu_bar
            .open
            .as_ref()
            .is_some_and(|open| open.rect.contains(local));
        // A click anywhere else only dismisses the menu; it does not reach a window.
        if !in_menu {
            self.close_menu();
        }
        true
    }

    /// The pointer moved: an open menu follows it across the bar and lights the entry under it.
    pub fn menu_bar_motion(&mut self, pos: Point<f64, Logical>) {
        if self.menu_bar.open.is_none() || !self.menu_bar_shown() {
            return;
        }
        let Some(local) = self.local(pos) else {
            return;
        };
        if let Some(title) = self.title_at(local) {
            if self
                .menu_bar
                .open
                .as_ref()
                .is_some_and(|open| open.title != title)
            {
                self.open_menu(title, false);
            }
            return;
        }
        let row = self.row_at(local);
        if let Some(open) = self.menu_bar.open.as_mut() {
            open.highlighted = row;
        }
    }

    /// A button came up: over an entry, that entry is chosen, whether it was clicked or
    /// the press that opened the menu was dragged onto it.
    pub fn menu_bar_release(&mut self, pos: Point<f64, Logical>) {
        if self.menu_bar.open.is_none() {
            return;
        }
        let Some(local) = self.local(pos) else {
            return;
        };
        if let Some(row) = self.row_at(local) {
            self.choose(row);
        }
    }

    /// A key while a menu is open. Every key goes to the menu then.
    pub fn menu_bar_key(&mut self, sym: Keysym) {
        let Some(open) = self.menu_bar.open.as_ref() else {
            return;
        };
        let title = open.title;
        let titles = self.menu_bar.titles.len();
        let step = |from: Option<usize>, forward: bool, open: &OpenMenu| {
            let count = open.entries.len();
            let mut index = from;
            for _ in 0..count {
                let next = match (index, forward) {
                    (None, true) => 0,
                    (None, false) => count - 1,
                    (Some(at), true) => (at + 1) % count,
                    (Some(at), false) => (at + count - 1) % count,
                };
                if open.entries[next].chooseable() {
                    return Some(next);
                }
                index = Some(next);
            }
            from
        };
        match sym {
            Keysym::Escape => self.close_menu(),
            Keysym::Left => self.open_menu((title + titles - 1) % titles, true),
            Keysym::Right => self.open_menu((title + 1) % titles, true),
            Keysym::Down | Keysym::Up => {
                if let Some(open) = self.menu_bar.open.as_mut() {
                    open.highlighted = step(open.highlighted, sym == Keysym::Down, open);
                }
            }
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                if let Some(row) = open.highlighted {
                    self.choose(row);
                }
            }
            _ => {}
        }
    }

    /// Ctrl+F2: the first menu, its first entry lit, for the keyboard.
    pub fn open_first_menu(&mut self) {
        if self.menu_bar_shown() && !self.menu_bar.titles.is_empty() {
            self.open_menu(0, true);
        }
    }

    fn choose(&mut self, row: usize) {
        let command = self
            .menu_bar
            .open
            .as_ref()
            .and_then(|open| open.entries.get(row))
            .and_then(|entry| entry.command.clone());
        self.close_menu();
        if let Some(command) = command {
            self.run_menu_command(command);
        }
    }

    fn run_menu_command(&mut self, command: Command) {
        let app = self.active_app();
        let focused = self.focused_window().filter(is_managed);
        let send = |bar: &MenuBar, request: Request| {
            if let Some(bus) = &bar.bus {
                bus.send(request);
            }
        };
        match command {
            Command::Sleep => config::spawn("systemctl suspend"),
            // Asked first, in a dialog of its own: these lose what is not saved.
            Command::Restart => config::spawn("nekoland-confirm restart"),
            Command::ShutDown => config::spawn("nekoland-confirm shutdown"),
            Command::LogOut => config::spawn("nekoland-confirm logout"),
            Command::Action(action) => {
                if let Some((app_id, _)) = app {
                    send(&self.menu_bar, Request::Activate { app_id, action });
                }
            }
            Command::Hide => {
                if let Some((app_id, _)) = app {
                    for window in self.app_windows(&app_id) {
                        self.minimize(&window);
                    }
                }
            }
            Command::HideOthers => {
                let app_id = app.map(|(app_id, _)| app_id).unwrap_or_default();
                let others: Vec<Window> = self
                    .space
                    .elements()
                    .filter(|window| is_managed(window) && window_names(window).1 != app_id)
                    .cloned()
                    .collect();
                for window in others {
                    self.minimize(&window);
                }
                if let Some(window) = focused {
                    self.focus_window(&window);
                }
            }
            Command::ShowAll => {
                let minimized: Vec<Window> =
                    self.minimized.iter().map(|(window, _)| window.clone()).collect();
                for window in minimized {
                    self.unminimize(&window);
                }
            }
            Command::Quit => {
                let quits = app
                    .as_ref()
                    .and_then(|(app_id, _)| self.menu_bar.actions.get(app_id))
                    .is_some_and(|actions| actions.iter().any(|action| action == "quit"));
                match app {
                    Some((app_id, _)) if quits => send(
                        &self.menu_bar,
                        Request::Activate {
                            app_id,
                            action: "quit".into(),
                        },
                    ),
                    // Without a quit action, closing every window is how such a program ends.
                    Some((app_id, _)) => self.app_windows(&app_id).iter().for_each(close),
                    None => {}
                }
            }
            Command::Minimize => {
                if let Some(window) = focused {
                    self.minimize(&window);
                }
            }
            Command::Zoom => {
                if let Some(window) = focused {
                    if is_maximized(&window) {
                        self.restore(&window);
                    } else {
                        self.maximize(&window);
                    }
                }
            }
            Command::TileLeft => {
                if let Some(window) = focused {
                    self.tile(&window, Side::Left);
                }
            }
            Command::TileRight => {
                if let Some(window) = focused {
                    self.tile(&window, Side::Right);
                }
            }
            Command::MoveToDesktop(index) => {
                if let Some(window) = focused {
                    self.move_to_workspace(&window, index);
                }
            }
            Command::BringAllToFront => {
                if let Some((app_id, _)) = app {
                    for window in self.app_windows(&app_id).iter().rev() {
                        self.space.raise_element(window, false);
                    }
                }
                if let Some(window) = focused {
                    self.focus_window(&window);
                }
            }
            Command::Focus(window) => {
                self.unminimize(&window);
                self.focus_window(&window);
            }
            Command::Status { item, id } => send(&self.menu_bar, Request::MenuEvent { item, id }),
        }
    }
}

fn is_maximized(window: &Window) -> bool {
    match window.underlying_surface() {
        WindowSurface::Wayland(toplevel) => {
            toplevel.current_state().states.contains(ToplevelState::Maximized)
        }
        WindowSurface::X11(surface) => surface.is_maximized(),
    }
}

fn close(window: &Window) {
    match window.underlying_surface() {
        WindowSurface::Wayland(toplevel) => toplevel.send_close(),
        WindowSurface::X11(surface) => {
            let _ = surface.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        vec![
            Entry::item("一", Command::Sleep),
            Entry::separator(),
            Entry::item("二", Command::Sleep).shortcut("Super H"),
        ]
    }

    #[test]
    fn menus_stay_on_the_output() {
        let title = Rectangle::new((1880, 0).into(), (30, MENU_BAR_HEIGHT).into());
        let (rect, _) = MenuBar::place_menu(&entries(), title, 1920);
        assert!(rect.loc.x + rect.size.w <= 1920 - 6);
        assert_eq!(rect.loc.y, MENU_BAR_HEIGHT + GAP_BELOW_BAR);
    }

    #[test]
    fn rows_are_stacked_with_thin_separators() {
        let title = Rectangle::new((8, 0).into(), (36, MENU_BAR_HEIGHT).into());
        let (rect, rows) = MenuBar::place_menu(&entries(), title, 1920);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].size.h, SEPARATOR_HEIGHT);
        assert_eq!(rows[2].loc.y, rows[1].loc.y + SEPARATOR_HEIGHT);
        assert_eq!(rect.size.h, MENU_PADDING * 2 + ROW_HEIGHT * 2 + SEPARATOR_HEIGHT);
    }

    #[test]
    fn separators_and_disabled_entries_cannot_be_chosen() {
        let entries = entries();
        assert!(entries[0].chooseable());
        assert!(!entries[1].chooseable());
        assert!(!Entry::item("三", Command::Sleep).enabled(false).chooseable());
    }
}
