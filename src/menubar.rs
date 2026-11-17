//! The menu bar along the top, after the Mac's: the system menu, the menus of the active
//! application, the status items programs put there, the sound and the network, and the
//! clock.
//!
//! An application's own menus are shown when it exports them (Qt over KDE's appmenu
//! protocol, GTK over gtk_shell1). Otherwise its menus are built from the actions it
//! exports and what Nekoland can do for any window, as the Mac gives every application an
//! Edit and a Window menu. The Help menu searches all the others.
//!
//! A click opens a menu; after that the menus follow the pointer across the bar until one
//! is chosen or dismissed. A press dragged onto an entry and let go chooses it in one move.
//! Submenus open to the side, and a pointer on its way into one does not close it. The
//! keyboard works too: the arrows move, Return chooses, Escape closes, and Ctrl+F2 opens
//! the first menu.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use cairo::{Context, Format, ImageSurface};
use smithay::{
    backend::{input::KeyState, renderer::element::Id},
    desktop::{Window, WindowSurface},
    input::keyboard::{FilterResult, Keycode, Keysym},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::State as ToplevelState,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Logical, Point, Rectangle, Size, SERIAL_COUNTER},
    wayland::seat::WaylandFocus,
};

use crate::{
    config,
    dbus::{AppMenuAddress, Bus, Event, MenuAction, MenuEntry, Request, StatusItem},
    control::{self, ControlCenter},
    desktop::DesktopCommand,
    dock::DockCommand,
    glass::Tier,
    system::{self, Network, Volume},
    text::{self, Picture},
    windows::{is_managed, window_names, Side, LAUNCHER_APP_ID},
    Nekoland,
};

pub const MENU_BAR_HEIGHT: i32 = 30;
const TITLE_PADDING: i32 = 10;
const TITLE_HIGHLIGHT_HEIGHT: i32 = 22;
const STATUS_ICON: i32 = 18;
const LOGO: f64 = 16.0;
/// A menu's rows, separators and text are as GTK's popover menus are, so that every menu
/// is one size.
const ROW_HEIGHT: i32 = 32;
/// The Help menu's search row: a field and room around it.
const SEARCH_HEIGHT: i32 = FIELD_HEIGHT + 6;
const SEPARATOR_HEIGHT: i32 = 12;
const MENU_POINTS: f64 = 11.0;
const MENU_PADDING: i32 = 6;
const MENU_MIN_WIDTH: i32 = 220;
pub(crate) const MENU_RADIUS: i32 = 12;
/// Room on the left of an entry for its check mark.
const CHECK_COLUMN: i32 = 22;
/// Where a label starts in a menu with no check marks or icons, as in GTK's.
const LABEL_INSET: i32 = 12;
const GAP_BELOW_BAR: i32 = 4;
/// How far a submenu tucks under the menu it opens from.
const SUBMENU_OVERLAP: i32 = 4;
const MAX_SEARCH_RESULTS: usize = 14;
const NETWORK_POLL: Duration = Duration::from_secs(3);

const WEEKDAYS: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
/// What Help menus are called, by the programs that bring their own.
const HELP_TITLES: [&str; 6] = ["help", "說明", "輔助說明", "幫助", "帮助", "ヘルプ"];

// xkb key codes, which are evdev's plus 8, for the Edit menu's shortcuts.
const KEY_CTRL: u32 = 37;
const KEY_SHIFT: u32 = 50;
const KEY_A: u32 = 38;
const KEY_Z: u32 = 52;
const KEY_X: u32 = 53;
const KEY_C: u32 = 54;
const KEY_V: u32 = 55;

/// What choosing a menu entry does.
#[derive(Clone, PartialEq)]
pub enum Command {
    Settings,
    Sleep,
    Restart,
    ShutDown,
    Lock,
    LogOut,
    /// One of the actions the active application exports, by name.
    Action(String),
    /// An entry of a menu a program exported.
    Invoke(MenuAction),
    /// Keys pressed together for the active window, as its own shortcut would be.
    Keys(&'static [u32]),
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    CloseWindow,
    FullScreen,
    Minimize,
    Zoom,
    TileLeft,
    TileRight,
    MoveToDesktop(usize),
    BringAllToFront,
    /// What the Dock's menu for an application offers.
    Dock(DockCommand),
    Focus(Window),
    /// What the desktop's menus offer.
    Desktop(crate::desktop::DesktopCommand),
    VolumeUp,
    VolumeDown,
    Mute,
}

#[derive(Clone)]
pub struct Entry {
    label: String,
    shortcut: String,
    enabled: bool,
    separator: bool,
    /// The Help menu's search field, whose label is what was typed.
    search: bool,
    checked: Option<bool>,
    command: Option<Command>,
    children: Vec<Entry>,
    /// The names of an icon drawn before the label, the first the themes have.
    icon: Vec<String>,
}

impl Entry {
    pub(crate) fn item(label: impl Into<String>, command: Command) -> Self {
        Self {
            label: label.into(),
            shortcut: String::new(),
            enabled: true,
            separator: false,
            search: false,
            checked: None,
            command: Some(command),
            children: Vec::new(),
            icon: Vec::new(),
        }
    }

    /// An entry opening a submenu of these.
    pub(crate) fn submenu(label: impl Into<String>, children: Vec<Entry>) -> Self {
        Self {
            command: None,
            children,
            ..Self::note(label)
        }
        .enabled(true)
    }

    pub(crate) fn with_icon(mut self, names: Vec<String>) -> Self {
        self.icon = names;
        self
    }

    /// A line of information, not something to choose.
    pub(crate) fn note(label: impl Into<String>) -> Self {
        Self {
            command: None,
            enabled: false,
            ..Self::item(label, Command::Sleep)
        }
    }

    pub(crate) fn separator() -> Self {
        Self {
            separator: true,
            ..Self::note("")
        }
    }

    fn search_field(query: &str) -> Self {
        Self {
            search: true,
            ..Self::note(query)
        }
    }

    pub(crate) fn shortcut(mut self, shortcut: &str) -> Self {
        self.shortcut = shortcut.to_string();
        self
    }

    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub(crate) fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    fn from_menu(entry: &MenuEntry) -> Self {
        Self {
            label: entry.label.clone(),
            shortcut: entry.shortcut.clone(),
            enabled: entry.enabled,
            separator: entry.separator,
            search: false,
            checked: entry.checked,
            command: entry.action.clone().map(Command::Invoke),
            children: entry.children.iter().map(Entry::from_menu).collect(),
            icon: Vec::new(),
        }
    }

    /// Whether the pointer or the keyboard can land on it.
    fn reachable(&self) -> bool {
        self.enabled
            && !self.separator
            && !self.search
            && (self.command.is_some() || !self.children.is_empty())
    }
}

/// What a place on the bar opens.
#[derive(Clone, PartialEq)]
enum Source {
    System,
    App,
    /// One of the menus the application exported, by its place among them.
    AppMenu(usize),
    File,
    Edit,
    View,
    Window,
    Help,
    Status(String),
    Sound,
    Network,
    Control,
}

struct Title {
    rect: Rectangle<i32, Logical>,
    source: Source,
}

/// One menu on screen: the one under the bar, or a submenu beside another.
struct Level {
    entries: Vec<Entry>,
    rect: Rectangle<i32, Logical>,
    rows: Vec<Rectangle<i32, Logical>>,
    highlighted: Option<usize>,
    picture: Option<Picture>,
    drawn: Option<(Option<usize>, bool)>,
}

pub struct OpenMenu {
    /// The bar's title it opened from; none for one opened elsewhere, as the Dock's.
    pub(crate) title: Option<usize>,
    levels: Vec<Level>,
    /// For a Help menu: what has been typed into its search field, and what it shows
    /// when nothing has.
    search: Option<(String, Vec<Entry>)>,
    /// Where the pointer last was over the menus, to tell a move into a submenu.
    last_pointer: Option<Point<i32, Logical>>,
}

/// What the bar shows; it is drawn again when any of it changes.
#[derive(Clone, PartialEq)]
pub(crate) struct Drawn {
    width: i32,
    app: Option<(String, String)>,
    menus: u64,
    open: Option<usize>,
    control: bool,
    dark: bool,
    background: bool,
    minute: i64,
    items: u64,
}

pub struct MenuBar {
    pub bus: Option<Bus>,
    pub(crate) dark: bool,
    /// The actions each application exports, as far as they are known.
    actions: HashMap<String, Vec<String>>,
    /// Where each window said its menus are, and the menus found there.
    addresses: HashMap<WlSurface, AppMenuAddress>,
    /// The same for X11 windows, by window ID, from the AppMenu registrar.
    x11_addresses: HashMap<u32, AppMenuAddress>,
    app_menus: HashMap<AppMenuAddress, Vec<MenuEntry>>,
    menus_revision: u64,
    items: Vec<StatusItem>,
    pub(crate) items_revision: u64,
    pub(crate) sound: Option<Volume>,
    pub(crate) in_use: crate::privacy::InUse,
    pub(crate) network: Option<Network>,
    network_read: Option<Instant>,
    icons: HashMap<String, Option<ImageSurface>>,
    titles: Vec<Title>,
    pub open: Option<OpenMenu>,
    pub(crate) control: Option<ControlCenter>,
    picture: Option<Picture>,
    pub(crate) drawn: Option<Drawn>,
    /// Whether the wallpaper is dark under the bar, for the wallpaper, how it is drawn and
    /// the output's size it was found for.
    wallpaper_dark: Option<(WallpaperKey, bool)>,
    bar_id: Id,
    menu_ids: Vec<Id>,
}

impl Default for MenuBar {
    fn default() -> Self {
        Self {
            bus: None,
            dark: true,
            actions: HashMap::new(),
            addresses: HashMap::new(),
            x11_addresses: HashMap::new(),
            app_menus: HashMap::new(),
            menus_revision: 0,
            items: Vec::new(),
            items_revision: 0,
            sound: None,
            in_use: Default::default(),
            network: None,
            network_read: None,
            icons: HashMap::new(),
            titles: Vec::new(),
            open: None,
            control: None,
            picture: None,
            drawn: None,
            wallpaper_dark: None,
            bar_id: Id::new(),
            menu_ids: Vec::new(),
        }
    }
}

/// Something the compositor draws over everything, on glass.
pub struct Overlay<'a> {
    pub id: &'a Id,
    pub rect: Rectangle<i32, Logical>,
    /// The glass under it and its corner radius, or None for none.
    pub glass: Option<(Tier, i32)>,
    /// How opaque the picture is drawn.
    pub alpha: f32,
    /// How far it has come in, glass and all, as while it fades in or out.
    pub fade: f32,
    pub picture: &'a Picture,
}

type Rgb = (f64, f64, f64);

pub(crate) struct Colors {
    pub(crate) ink: Rgb,
    pub(crate) ink_3: Rgb,
    pub(crate) on_ink: Rgb,
    pub(crate) sheet: Rgb,
    pub(crate) edge: (f64, f64, f64, f64),
}

/// The design language's field, as GTK's are in Nekoland's look.
pub(crate) const FIELD_HEIGHT: i32 = 36;
const FIELD_RADIUS: f64 = 8.0;

/// A field of the design language being typed into: sheet on 8px corners, with a line of
/// ink just inside its edge.
pub(crate) fn draw_field(cr: &Context, (x, y, width, height): (f64, f64, f64, f64), colors: &Colors) {
    text::rounded_rectangle(cr, x, y, width, height, FIELD_RADIUS);
    cr.set_source_rgb(colors.sheet.0, colors.sheet.1, colors.sheet.2);
    let _ = cr.fill();
    text::rounded_rectangle(cr, x + 0.75, y + 0.75, width - 1.5, height - 1.5, FIELD_RADIUS - 0.75);
    cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
}

// The design language's ink, paper and glass, in both themes.
/// The wallpaper, how it is drawn, and the output it is drawn on.
type WallpaperKey = (Option<String>, config::WallpaperMode, Size<i32, Logical>);

/// Whether the strip of `output` the bar covers is dark on average, with `image`, a copy of
/// one `original` pixels in size, drawn as swaybg draws it in `mode`; what it leaves bare
/// is swaybg's black.
fn strip_is_dark(
    image: &gdk_pixbuf::Pixbuf,
    (width, height): (i32, i32),
    mode: config::WallpaperMode,
    output: Size<i32, Logical>,
) -> bool {
    let (w, h) = (f64::from(output.w), f64::from(output.h));
    let (iw, ih) = (f64::from(width), f64::from(height));
    // shortcut: centred images are taken at one pixel a point, which a scaled output is not.
    let scale = match mode {
        config::WallpaperMode::Fill => (w / iw).max(h / ih),
        config::WallpaperMode::Fit => (w / iw).min(h / ih),
        config::WallpaperMode::Center => 1.0,
    };
    let (left, top) = ((w - iw * scale) / 2.0, (h - ih * scale) / 2.0);
    let to_small = f64::from(image.width()) / iw;
    let pixels = image.read_pixel_bytes();
    let (stride, channels) = (image.rowstride() as usize, image.n_channels() as usize);
    let (columns, rows) = (64, 4);
    let mut sum = 0.0;
    for row in 0..rows {
        for column in 0..columns {
            let x = (f64::from(column) + 0.5) / f64::from(columns) * w;
            let y = (f64::from(row) + 0.5) / f64::from(rows) * f64::from(MENU_BAR_HEIGHT);
            let (ix, iy) = (((x - left) / scale * to_small), ((y - top) / scale * to_small));
            if ix < 0.0 || iy < 0.0 || ix >= f64::from(image.width()) || iy >= f64::from(image.height()) {
                continue;
            }
            let at = iy as usize * stride + ix as usize * channels;
            let [r, g, b] = [0, 1, 2].map(|c| f64::from(pixels[at + c]) / 255.0);
            sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
        }
    }
    sum / f64::from(columns * rows) < 0.5
}

pub(crate) fn colors(dark: bool) -> Colors {
    if dark {
        Colors {
            ink: (0.945, 0.937, 0.918),
            ink_3: (0.576, 0.569, 0.592),
            on_ink: (0.090, 0.090, 0.102),
            sheet: (0.133, 0.133, 0.149),
            edge: (1.0, 1.0, 1.0, 0.16),
        }
    } else {
        Colors {
            ink: (0.090, 0.090, 0.102),
            ink_3: (0.435, 0.427, 0.447),
            on_ink: (0.984, 0.980, 0.973),
            sheet: (0.984, 0.980, 0.973),
            edge: (0.0, 0.0, 0.0, 0.10),
        }
    }
}

/// A menu leaves room before its labels only if one of them has a check mark or an icon.
fn label_indent(entries: &[Entry]) -> i32 {
    let marked = entries
        .iter()
        .any(|entry| entry.checked.is_some() || !entry.icon.is_empty());
    if marked {
        CHECK_COLUMN
    } else {
        LABEL_INSET
    }
}

pub(crate) fn font(weight: &str, points: f64) -> pango::FontDescription {
    text::font(&format!("{weight} {points}"))
}

fn text_layout(cr: &Context, text: &str, font: &pango::FontDescription) -> pango::Layout {
    let layout = text::new_layout(cr);
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

pub(crate) fn show_text(cr: &Context, text: &str, font: &pango::FontDescription, x: f64, center_y: f64, color: Rgb) {
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

/// The minute it is now, or the second when seconds are shown, and the clock as the bar
/// shows it, as the configuration asks.
fn clock(config: &config::Config) -> (i64, String) {
    // SAFETY: localtime_r only writes the tm it is given.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    let minute = ((i64::from(tm.tm_yday) * 24 + i64::from(tm.tm_hour)) * 60) + i64::from(tm.tm_min);
    let key = if config.clock_seconds || config.clock_flash {
        minute * 60 + i64::from(tm.tm_sec)
    } else {
        minute
    };
    let weekday = if config.clock_weekday {
        format!("週{} ", WEEKDAYS[tm.tm_wday.rem_euclid(7) as usize])
    } else {
        String::new()
    };
    // A flashing separator is a punctuation space every other second, about as wide.
    let colon = if config.clock_flash && tm.tm_sec % 2 == 1 { '\u{2008}' } else { ':' };
    let time = if config.clock_24h {
        format!("{:02}{colon}{:02}", tm.tm_hour, tm.tm_min)
    } else {
        let half = if tm.tm_hour < 12 { "上午" } else { "下午" };
        format!("{half} {}{colon}{:02}", (tm.tm_hour + 11) % 12 + 1, tm.tm_min)
    };
    let seconds = if config.clock_seconds {
        format!(":{:02}", tm.tm_sec)
    } else {
        String::new()
    };
    let date = if config.clock_date {
        format!("{}月{}日 ", tm.tm_mon + 1, tm.tm_mday)
    } else {
        String::new()
    };
    (key, format!("{date}{weekday}{time}{seconds}"))
}

fn is_help(label: &str) -> bool {
    HELP_TITLES.contains(&label.trim().to_lowercase().as_str())
}

pub(crate) fn sound_icon(volume: Volume) -> &'static str {
    match (volume.muted, volume.percent) {
        (true, _) | (false, 0) => "audio-volume-muted-symbolic",
        (false, 1..=33) => "audio-volume-low-symbolic",
        (false, 34..=66) => "audio-volume-medium-symbolic",
        _ => "audio-volume-high-symbolic",
    }
}

pub(crate) fn network_icon(network: &Network) -> &'static str {
    match (network.connected, network.wireless) {
        (false, _) => "network-offline-symbolic",
        (true, true) => "network-wireless-signal-good-symbolic",
        (true, false) => "network-wired-symbolic",
    }
}

/// Is `point` inside the triangle `a`, `b`, `c`?
fn in_triangle(
    point: Point<i32, Logical>,
    a: Point<i32, Logical>,
    b: Point<i32, Logical>,
    c: Point<i32, Logical>,
) -> bool {
    let side = |p: Point<i32, Logical>, q: Point<i32, Logical>, r: Point<i32, Logical>| {
        i64::from(p.x - r.x) * i64::from(q.y - r.y) - i64::from(q.x - r.x) * i64::from(p.y - r.y)
    };
    let (d1, d2, d3) = (side(point, a, b), side(point, b, c), side(point, c, a));
    let negative = d1 < 0 || d2 < 0 || d3 < 0;
    let positive = d1 > 0 || d2 > 0 || d3 > 0;
    !(negative && positive)
}

/// Every entry that can be chosen, with the way to it, for the Help menu's search.
fn searchable(menus: &[(String, Vec<Entry>)]) -> Vec<Entry> {
    fn walk(entries: &[Entry], path: &str, found: &mut Vec<Entry>) {
        for entry in entries {
            if entry.separator || entry.search {
                continue;
            }
            let here = format!("{path} › {}", entry.label);
            if entry.children.is_empty() {
                if entry.command.is_some() {
                    found.push(Entry {
                        label: here,
                        ..entry.clone()
                    });
                }
            } else {
                walk(&entry.children, &here, found);
            }
        }
    }
    let mut found = Vec::new();
    for (title, entries) in menus {
        walk(entries, title, &mut found);
    }
    found
}

impl MenuBar {
    fn icon(
        &mut self,
        key: &str,
        name: &str,
        theme_path: &str,
        pixmap: Option<&(i32, i32, Vec<u8>)>,
        tint: Rgb,
    ) -> Option<ImageSurface> {
        let cache_key = format!("{key}|{name}|{tint:?}");
        if let Some(icon) = self.icons.get(&cache_key) {
            return icon.clone();
        }
        let icon = (!name.is_empty())
            .then(|| text::named_icon(name, theme_path, STATUS_ICON, tint))
            .flatten()
            .or_else(|| {
                let (width, height, argb) = pixmap?;
                text::pixmap_icon(*width, *height, argb)
            });
        self.icons.insert(cache_key, icon.clone());
        icon
    }

    /// Lays the bar out and draws it, its text for a `dark` background, on glass when it has
    /// a `background`.
    fn draw_bar(
        &mut self,
        width: i32,
        titles: Vec<(Source, Option<String>)>,
        clock: &str,
        (dark, background): (bool, bool),
        config: &config::Config,
    ) {
        let colors = colors(dark);
        let open = self.open.as_ref().and_then(|open| open.title);
        let title_font = font("Medium", 10.0);
        let app_font = font("Bold", 10.0);

        let Ok(surface) = text::canvas(width.max(1), MENU_BAR_HEIGHT) else {
            return;
        };
        let Ok(cr) = Context::new(&surface) else {
            return;
        };
        if background {
            let (r, g, b, a) = crate::glass::tint(dark, Tier::Bar);
            cr.set_source_rgba(r, g, b, a);
            let _ = cr.paint();
        }

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

        // Left: the mark, then the application's menus.
        self.titles.clear();
        let mut x = 8;
        for (source, label) in titles {
            let font = if source == Source::App {
                &app_font
            } else {
                &title_font
            };
            let content = label
                .as_deref()
                .map_or(LOGO as i32, |label| text_size(label, font).0);
            let rect = Rectangle::new(
                (x, 0).into(),
                (content + TITLE_PADDING * 2, MENU_BAR_HEIGHT).into(),
            );
            let lit = open == Some(self.titles.len());
            if lit {
                highlight(&cr, rect);
            }
            let color = if lit { colors.on_ink } else { colors.ink };
            match &label {
                None => draw_logo(&cr, f64::from(x + TITLE_PADDING), center - LOGO / 2.0, color),
                Some(label) => show_text(&cr, label, font, f64::from(x + TITLE_PADDING), center, color),
            }
            self.titles.push(Title { rect, source });
            x += rect.size.w;
        }

        // Right, from the edge in: the clock, the system's items, then the programs' items.
        let clock_font = font("Medium", 10.0);
        let clock_width = text_size(clock, &clock_font).0;
        let mut right = width - 12 - clock_width;
        show_text(&cr, clock, &clock_font, f64::from(right), center, colors.ink);
        right -= 8;
        right = crate::privacy::draw_dots(&cr, self.in_use, right, center);

        type Icon = (Source, String, String, String, Option<(i32, i32, Vec<u8>)>);
        let mut icons: Vec<Icon> = vec![(Source::Control, String::new(), String::new(), String::new(), None)];
        if let Some(network) = self.network.as_ref().filter(|_| config.menu_bar_network) {
            icons.push((
                Source::Network,
                "network".into(),
                network_icon(network).into(),
                String::new(),
                None,
            ));
        }
        if let Some(volume) = self.sound.filter(|_| config.menu_bar_sound) {
            icons.push((
                Source::Sound,
                "sound".into(),
                sound_icon(volume).into(),
                String::new(),
                None,
            ));
        }
        let items: Vec<StatusItem> = self.items.iter().filter(|item| !item.passive).cloned().collect();
        icons.extend(items.into_iter().map(|item| {
            (
                Source::Status(item.key.clone()),
                item.key,
                item.icon_name,
                item.icon_theme_path,
                item.pixmap,
            )
        }));
        for (source, key, name, theme_path, pixmap) in icons {
            let rect = Rectangle::new(
                (right - STATUS_ICON - TITLE_PADDING * 2 + 8, 0).into(),
                (STATUS_ICON + TITLE_PADDING * 2 - 8, MENU_BAR_HEIGHT).into(),
            );
            let lit = open == Some(self.titles.len()) || (source == Source::Control && self.control.is_some());
            if lit {
                highlight(&cr, rect);
            }
            let tint = if lit { colors.on_ink } else { colors.ink };
            if source == Source::Control {
                let at = |side| f64::from(side - STATUS_ICON) / 2.0;
                control::draw_bar_icon(&cr, f64::from(rect.loc.x) + at(rect.size.w), at(MENU_BAR_HEIGHT), f64::from(STATUS_ICON), tint);
            } else if let Some(icon) = self.icon(&key, &name, &theme_path, pixmap.as_ref(), tint) {
                let (w, h) = text::points(&icon);
                let scale = f64::from(STATUS_ICON) / w.max(h).max(1.0);
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
            self.titles.push(Title { rect, source });
            right = rect.loc.x;
        }
        drop(cr);
        self.picture = text::picture(surface);
    }

    /// Where a menu of these entries goes and where each entry is in it: under a title of
    /// the bar, or beside the row of another menu.
    fn place_menu(
        entries: &[Entry],
        anchor: Rectangle<i32, Logical>,
        beside: bool,
        output: Size<i32, Logical>,
    ) -> (Rectangle<i32, Logical>, Vec<Rectangle<i32, Logical>>) {
        let label_font = font("", MENU_POINTS);
        let widest = entries
            .iter()
            .map(|entry| {
                let label = text_size(&entry.label, &label_font).0;
                let trailing = if !entry.children.is_empty() {
                    24
                } else if entry.shortcut.is_empty() {
                    0
                } else {
                    24 + text_size(&entry.shortcut, &label_font).0
                };
                label + trailing
            })
            .max()
            .unwrap_or(0);
        let width = (widest + label_indent(entries) + MENU_PADDING * 2 + 16).max(MENU_MIN_WIDTH);

        let mut rows = Vec::with_capacity(entries.len());
        let mut y = MENU_PADDING;
        for entry in entries {
            let height = match (entry.separator, entry.search) {
                (true, _) => SEPARATOR_HEIGHT,
                (_, true) => SEARCH_HEIGHT,
                _ => ROW_HEIGHT,
            };
            rows.push(Rectangle::new(
                (MENU_PADDING, y).into(),
                (width - MENU_PADDING * 2, height).into(),
            ));
            y += height;
        }
        let height = y + MENU_PADDING;
        let (x, top) = if beside {
            let right = anchor.loc.x + anchor.size.w + MENU_PADDING - SUBMENU_OVERLAP;
            // On the right if it fits there, else on the left.
            let x = if right + width <= output.w - 6 {
                right
            } else {
                anchor.loc.x - MENU_PADDING + SUBMENU_OVERLAP - width
            };
            (x, anchor.loc.y - MENU_PADDING)
        } else {
            (anchor.loc.x, MENU_BAR_HEIGHT + GAP_BELOW_BAR)
        };
        let x = x.min(output.w - width - 6).max(6);
        let top = top
            .min(output.h - height - 6)
            .max(MENU_BAR_HEIGHT + GAP_BELOW_BAR);
        let rect = Rectangle::new((x, top).into(), (width, height).into());
        let rows = rows
            .into_iter()
            .map(|row| Rectangle::new(row.loc + rect.loc, row.size))
            .collect();
        (rect, rows)
    }

    fn draw_menu(dark: bool, level: &mut Level) {
        let colors = colors(dark);
        let key = (level.highlighted, dark);
        if level.drawn == Some(key) && level.picture.is_some() {
            return;
        }
        level.drawn = Some(key);
        let size: Size<i32, Logical> = level.rect.size;
        let Ok(surface) = text::canvas(size.w, size.h) else {
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
        let (r, g, b, a) = crate::glass::tint(dark, Tier::Menu);
        cr.set_source_rgba(r, g, b, a);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(colors.edge.0, colors.edge.1, colors.edge.2, colors.edge.3);
        cr.set_line_width(1.0);
        let _ = cr.stroke();

        let label_font = font("", MENU_POINTS);
        let indent = f64::from(label_indent(&level.entries));
        for (index, (entry, row)) in level.entries.iter().zip(&level.rows).enumerate() {
            let local = Rectangle::<i32, Logical>::new(row.loc - level.rect.loc, row.size);
            let (x, y, w, h) = (
                f64::from(local.loc.x),
                f64::from(local.loc.y),
                f64::from(local.size.w),
                f64::from(local.size.h),
            );
            let center = y + h / 2.0;
            if entry.separator {
                cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.12);
                cr.rectangle(x + 4.0, center, w - 8.0, 1.0);
                let _ = cr.fill();
                continue;
            }
            if entry.search {
                let field = f64::from(FIELD_HEIGHT);
                draw_field(&cr, (x, y + 2.0, w, field), &colors);
                let center = y + 2.0 + field / 2.0;
                let (text, color) = if entry.label.is_empty() {
                    ("搜尋", colors.ink_3)
                } else {
                    (entry.label.as_str(), colors.ink)
                };
                show_text(&cr, text, &label_font, x + 12.0, center, color);
                if !entry.label.is_empty() {
                    // The caret, after what was typed.
                    let typed = text_size(&entry.label, &label_font).0;
                    cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
                    cr.rectangle(x + 13.0 + f64::from(typed), center - 8.0, 1.0, 16.0);
                    let _ = cr.fill();
                }
                continue;
            }
            let lit = level.highlighted == Some(index);
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
            if entry.checked == Some(true) {
                show_text(&cr, "✓", &label_font, x + 6.0, center, color);
            } else if let Some(icon) = entry
                .icon
                .iter()
                .find_map(|name| text::named_icon(name, "", 16, color))
            {
                let _ = cr.set_source_surface(&icon, x + 3.0, center - 8.0);
                let _ = cr.paint();
            }
            show_text(&cr, &entry.label, &label_font, x + indent, center, color);
            let trailing_color = if lit { colors.on_ink } else { colors.ink_3 };
            if !entry.children.is_empty() {
                // The same arrow GTK's menus show, from shell/gtk/nekoland.css.
                if let Some(arrow) = text::named_icon("nekoland-submenu-symbolic", "", 16, trailing_color) {
                    let _ = cr.set_source_surface(&arrow, x + w - 22.0, center - 8.0);
                    let _ = cr.paint();
                }
            } else if !entry.shortcut.is_empty() {
                let width = text_size(&entry.shortcut, &label_font).0;
                show_text(
                    &cr,
                    &entry.shortcut,
                    &label_font,
                    x + w - 10.0 - f64::from(width),
                    center,
                    trailing_color,
                );
            }
        }
        drop(cr);
        level.picture = text::picture(surface);
    }
}

impl Nekoland {
    pub(crate) fn output_area(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.space.outputs().next()?;
        self.space.output_geometry(output)
    }

    /// The active application: its ID and the name it goes by.
    fn active_app(&mut self) -> Option<(String, String)> {
        let window = self.focused_window().filter(is_managed)?;
        let (_, app_id) = window_names(&window);
        // The launcher is the system's, not an application of its own.
        if app_id.is_empty() || app_id == LAUNCHER_APP_ID {
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

    /// Where the active window's own menus are, if it said; for a dialog that did not,
    /// where another window of its program said, as the Mac keeps an application's menus.
    fn active_address(&self) -> Option<AppMenuAddress> {
        let focused = self.focused_window()?;
        let address_of = |window: &Window| match window.x11_surface() {
            Some(surface) => self.menu_bar.x11_addresses.get(&surface.window_id()).cloned(),
            None => window
                .wl_surface()
                .and_then(|surface| self.menu_bar.addresses.get(&*surface).cloned()),
        };
        address_of(&focused).or_else(|| {
            let app_id = window_names(&focused).1;
            self.space
                .elements()
                .filter(|window| *window != &focused && window_names(window).1 == app_id)
                .find_map(address_of)
        })
    }

    /// The active window's own menus, if it has any.
    fn active_menus(&self) -> Vec<MenuEntry> {
        self.active_address()
            .and_then(|address| self.menu_bar.app_menus.get(&address).cloned())
            .unwrap_or_default()
    }

    pub(crate) fn app_windows(&self, app_id: &str) -> Vec<Window> {
        self.space
            .elements()
            .filter(|window| is_managed(window) && window_names(window).1 == app_id)
            .cloned()
            .collect()
    }

    /// A window said where its menus are.
    pub fn set_app_menu_address(&mut self, surface: &WlSurface, address: AppMenuAddress) {
        self.menu_bar.addresses.retain(|surface, _| surface.alive());
        self.menu_bar.addresses.insert(surface.clone(), address.clone());
        if let Some(bus) = &self.menu_bar.bus {
            bus.send(Request::AppMenus(address));
        }
    }

    /// Whether the desktop is dark, as the menu bar learned it.
    pub fn menu_bar_dark(&self) -> bool {
        self.menu_bar.dark
    }

    /// Whether the bar is there to see: not in the overview, nor over a fullscreen window.
    pub fn menu_bar_shown(&self) -> bool {
        self.overview.is_none() && self.fullscreen_window().is_none()
    }

    /// Whether the bar or an open menu is at a position, so that it is not a window's.
    pub fn menu_bar_covers(&self, pos: Point<f64, Logical>) -> bool {
        let Some(output) = self.output_area() else {
            return false;
        };
        self.menu_bar_shown()
            && (self.menu_bar.open.is_some() || self.menu_bar.control.is_some() || pos.y < f64::from(output.loc.y + MENU_BAR_HEIGHT))
    }

    /// The titles of the bar's left side for the active application.
    fn left_titles(&mut self) -> Vec<(Source, Option<String>)> {
        let mut titles = vec![(Source::System, None)];
        let Some((app_id, name)) = self.active_app() else {
            // The desktop in front, as the Finder: its Edit menu copies and pastes its files.
            if self.desktop_edit_targets().is_some() {
                titles.push((Source::Edit, Some("編輯".into())));
            }
            return titles;
        };
        titles.push((Source::App, Some(name)));
        let menus = self.active_menus();
        if menus.is_empty() {
            let actions = self.menu_bar.actions.get(&app_id).cloned().unwrap_or_default();
            if actions.iter().any(|action| action == "new-window") || self.focused_window().is_some() {
                titles.push((Source::File, Some("檔案".into())));
            }
            titles.push((Source::Edit, Some("編輯".into())));
            titles.push((Source::View, Some("顯示方式".into())));
            titles.push((Source::Window, Some("視窗".into())));
            titles.push((Source::Help, Some("輔助說明".into())));
            return titles;
        }
        // The program's menus, with Nekoland's Window menu before its Help, as on the Mac.
        let help = menus.iter().position(|menu| is_help(&menu.label));
        for (index, menu) in menus.iter().enumerate() {
            if Some(index) != help && !menu.separator {
                titles.push((Source::AppMenu(index), Some(menu.label.clone())));
            }
        }
        titles.push((Source::Window, Some("視窗".into())));
        match help {
            Some(index) => titles.push((Source::AppMenu(index), Some(menus[index].label.clone()))),
            None => titles.push((Source::Help, Some("輔助說明".into()))),
        }
        titles
    }

    /// Brings the bar and any open menu up to date before a frame.
    /// Whether the wallpaper is dark where the bar is over it, found again only when the
    /// wallpaper, how it is drawn or the output changes. Without a wallpaper, swaybg's black.
    fn wallpaper_dark_under_bar(&mut self, output: Size<i32, Logical>) -> bool {
        let key = (self.config.wallpaper.clone(), self.config.wallpaper_mode, output);
        if let Some((found, dark)) = &self.menu_bar.wallpaper_dark {
            if *found == key {
                return *dark;
            }
        }
        let dark = key.0.as_ref().is_none_or(|path| {
            let home = std::env::var("HOME").unwrap_or_default();
            let path = path.replacen('~', &home, 1);
            // A smaller copy is enough to tell light from dark.
            let image = gdk_pixbuf::Pixbuf::file_info(&path).and_then(|(_, width, height)| {
                let small = gdk_pixbuf::Pixbuf::from_file_at_scale(&path, 512, -1, true).ok()?;
                Some((small, (width, height)))
            });
            match image {
                Some((image, original)) => strip_is_dark(&image, original, key.1, output),
                None => {
                    tracing::warn!("{path}: could not read the wallpaper");
                    self.menu_bar.dark
                }
            }
        });
        self.menu_bar.wallpaper_dark = Some((key, dark));
        dark
    }

    pub fn refresh_menu_bar(&mut self) {
        self.refresh_sharing();
        let Some(output) = self.output_area() else {
            return;
        };
        let app = self.active_app();
        if let (Some((app_id, _)), Some(bus)) = (&app, &self.menu_bar.bus) {
            if !self.menu_bar.actions.contains_key(app_id) {
                self.menu_bar.actions.insert(app_id.clone(), Vec::new());
                bus.send(Request::Actions(app_id.clone()));
            }
        }
        if self
            .menu_bar
            .network_read
            .is_none_or(|read| read.elapsed() >= NETWORK_POLL)
        {
            self.menu_bar.network_read = Some(Instant::now());
            let network = system::read_network();
            if network != self.menu_bar.network {
                self.menu_bar.network = network;
                self.menu_bar.items_revision += 1;
            }
        }

        let (minute, clock) = clock(&self.config);
        let background = self.config.menu_bar_background;
        let dark = if background {
            self.menu_bar.dark
        } else {
            self.wallpaper_dark_under_bar(output.size)
        };
        let drawn = Drawn {
            width: output.size.w,
            app: app.clone(),
            menus: self.menu_bar.menus_revision,
            open: self.menu_bar.open.as_ref().and_then(|open| open.title),
            control: self.menu_bar.control.is_some(),
            dark,
            background,
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
            let titles = self.left_titles();
            self.menu_bar.draw_bar(output.size.w, titles, &clock, (dark, background), &self.config);
            self.menu_bar.drawn = Some(Drawn {
                open: self.menu_bar.open.as_ref().and_then(|open| open.title),
                ..drawn
            });
        }

        let dark = self.menu_bar.dark;
        let levels = self.menu_bar.open.as_ref().map_or(0, |open| open.levels.len());
        while self.menu_bar.menu_ids.len() < levels {
            self.menu_bar.menu_ids.push(Id::new());
        }
        if let Some(open) = self.menu_bar.open.as_mut() {
            for level in &mut open.levels {
                MenuBar::draw_menu(dark, level);
            }
        }
        self.refresh_control_center();
    }

    /// What the bar and its open menus put on screen, front first.
    pub fn menu_bar_overlays(&self) -> Vec<Overlay<'_>> {
        let Some(output) = self.output_area() else {
            return Vec::new();
        };
        if !self.menu_bar_shown() {
            return Vec::new();
        }
        let bar = &self.menu_bar;
        let mut overlays = Vec::new();
        if let Some(open) = &bar.open {
            for (level, id) in open.levels.iter().zip(&bar.menu_ids).rev() {
                if let Some(picture) = &level.picture {
                    overlays.push(Overlay {
                        fade: 1.0,
                        id,
                        rect: Rectangle::new(level.rect.loc + output.loc, level.rect.size),
                        glass: Some((Tier::Menu, MENU_RADIUS)),
                        alpha: 1.0,
                        picture,
                    });
                }
            }
        }
        overlays.extend(self.control_center_overlay());
        if let Some(picture) = &bar.picture {
            overlays.push(Overlay {
                fade: 1.0,
                id: &bar.bar_id,
                rect: Rectangle::new(output.loc, (output.size.w, MENU_BAR_HEIGHT).into()),
                glass: self.config.menu_bar_background.then_some((Tier::Bar, 0)),
                alpha: 1.0,
                picture,
            });
        }
        overlays
    }

    /// Takes in what the D-Bus thread and the sound watcher learned.
    pub fn menu_bar_event(&mut self, event: Event) {
        let bar = &mut self.menu_bar;
        match event {
            Event::Actions { app_id, actions } => {
                bar.actions.insert(app_id, actions);
                bar.menus_revision += 1;
            }
            Event::AppMenus { address, menus } => {
                if bar.app_menus.get(&address) == Some(&menus) {
                    return;
                }
                bar.app_menus.insert(address.clone(), menus);
                bar.menus_revision += 1;
                // An open menu of the program shows what it says now, unless a submenu
                // is open, which would lose its place.
                if self.active_address() == Some(address) {
                    self.reopen_app_menu();
                }
            }
            Event::X11Menu { window, address } => {
                match address {
                    Some(address) => {
                        bar.x11_addresses.insert(window, address.clone());
                        if let Some(bus) = &bar.bus {
                            bus.send(Request::AppMenus(address));
                        }
                    }
                    None => {
                        bar.x11_addresses.remove(&window);
                    }
                }
                bar.menus_revision += 1;
            }
            Event::MenusChanged(bus_name) => {
                let addresses: Vec<AppMenuAddress> = bar
                    .addresses
                    .values()
                    .chain(bar.x11_addresses.values())
                    .filter(|address| match address {
                        AppMenuAddress::Dbusmenu { bus, .. } | AppMenuAddress::Gtk { bus, .. } => {
                            *bus == bus_name
                        }
                    })
                    .cloned()
                    .collect();
                if let Some(bus) = &bar.bus {
                    for address in addresses {
                        bus.send(Request::AppMenus(address));
                    }
                }
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
                bar.open = None;
            }
            Event::Dark(dark) => bar.dark = dark,
            // Banners, which the notices take.
            Event::Notify(_) | Event::CloseNotification(_) => {}
            // What is typed into the applications' search field.
            Event::Ime(event) => self.ime_event(event),
            Event::Launcher { sender, app_id, update } => self.dock_launcher(sender, app_id, update),
            Event::LauncherGone(sender) => self.dock_launcher_gone(&sender),
            Event::Sound(volume) => {
                bar.sound = volume;
                bar.items_revision += 1;
            }
            Event::InUse(devices) => {
                bar.in_use = crate::privacy::InUse { screen: bar.in_use.screen, ..devices };
                bar.items_revision += 1;
            }
        }
    }

    fn reopen_app_menu(&mut self) {
        let Some((open, title)) = self
            .menu_bar
            .open
            .as_ref()
            .and_then(|open| Some((open, open.title?)))
        else {
            return;
        };
        let source = self.menu_bar.titles.get(title).map(|title| title.source.clone());
        if open.levels.len() == 1 && matches!(source, Some(Source::AppMenu(_))) {
            let highlighted = open.levels[0].highlighted;
            let search = open.search.as_ref().map(|(query, _)| query.clone());
            self.open_menu(title, false);
            if let Some(open) = self.menu_bar.open.as_mut() {
                if let Some(query) = search {
                    if let Some((typed, _)) = open.search.as_mut() {
                        *typed = query;
                    }
                }
                if let Some(level) = open.levels.first_mut() {
                    level.highlighted =
                        highlighted.filter(|&index| level.entries.get(index).is_some_and(Entry::reachable));
                }
            }
            self.update_search();
        }
    }

    fn local(&self, pos: Point<f64, Logical>) -> Option<Point<i32, Logical>> {
        let output = self.output_area()?;
        Some(pos.to_i32_floor() - output.loc)
    }

    fn title_at(&self, local: Point<i32, Logical>) -> Option<usize> {
        self.menu_bar
            .titles
            .iter()
            .position(|title| title.rect.contains(local))
    }

    /// The deepest open menu at a position, and the entry there if one can be landed on.
    fn level_at(&self, local: Point<i32, Logical>) -> Option<(usize, Option<usize>)> {
        let open = self.menu_bar.open.as_ref()?;
        open.levels.iter().enumerate().rev().find_map(|(index, level)| {
            level.rect.contains(local).then(|| {
                let row = level
                    .rows
                    .iter()
                    .position(|row| row.contains(local))
                    .filter(|&row| level.entries[row].reachable());
                (index, row)
            })
        })
    }

    pub fn close_menu(&mut self) {
        self.menu_bar.open = None;
        self.menu_bar.control = None;
    }

    pub(crate) fn output_size(&self) -> Size<i32, Logical> {
        self.output_area().map(|area| area.size).unwrap_or_default()
    }

    /// Opens the menu of a title, or for a status item without one, activates it.
    fn open_menu(&mut self, title: usize, highlight_first: bool) {
        let Some(source) = self.menu_bar.titles.get(title).map(|title| title.source.clone()) else {
            return;
        };
        self.menu_bar.control = None;
        if source == Source::Control {
            self.toggle_control_center(self.menu_bar.titles[title].rect);
            return;
        }
        let help = match &source {
            Source::Help => true,
            Source::AppMenu(index) => self
                .active_menus()
                .get(*index)
                .is_some_and(|menu| is_help(&menu.label)),
            _ => false,
        };
        let mut entries = self.menu_entries(&source);
        // Help holds the search of every menu, so it opens even with nothing else in it.
        if entries.is_empty() && !help {
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
        // Programs' menus are asked for again on opening, for entries that come and go.
        if matches!(source, Source::AppMenu(_)) {
            if let (Some(address), Some(bus)) = (self.active_address(), &self.menu_bar.bus) {
                bus.send(Request::AppMenus(address));
            }
        }
        let search = help.then(|| {
            let base = entries.clone();
            entries.insert(0, Entry::search_field(""));
            (String::new(), base)
        });
        let anchor = self.menu_bar.titles[title].rect;
        let (rect, rows) = MenuBar::place_menu(&entries, anchor, false, self.output_size());
        let highlighted = highlight_first
            .then(|| entries.iter().position(Entry::reachable))
            .flatten();
        self.menu_bar.open = Some(OpenMenu {
            title: Some(title),
            levels: vec![Level {
                entries,
                rect,
                rows,
                highlighted,
                picture: None,
                drawn: None,
            }],
            search,
            last_pointer: None,
        });
    }

    /// Opens a menu of these entries beside `anchor`, the strip across the Dock at an icon,
    /// on the side away from the Dock's edge.
    pub fn open_dock_menu(&mut self, entries: Vec<Entry>, anchor: Rectangle<i32, Logical>, gap: i32) {
        let output = self.output_size();
        let (rect, rows) = MenuBar::place_menu(&entries, anchor, false, output);
        let loc = crate::dock::beside(
            self.config.dock_position,
            Rectangle::from_size(output),
            anchor,
            rect.size,
            gap,
        );
        let shift = loc - rect.loc;
        self.menu_bar.open = Some(OpenMenu {
            title: None,
            levels: vec![Level {
                entries,
                rect: Rectangle::new(loc, rect.size),
                rows: rows
                    .into_iter()
                    .map(|row| Rectangle::new(row.loc + shift, row.size))
                    .collect(),
                highlighted: None,
                picture: None,
                drawn: None,
            }],
            search: None,
            last_pointer: None,
        });
    }

    /// Opens a menu of these entries with a corner at `point`, as a right click on the
    /// desktop does: its top left one, or, where it would not fit, the one on the other
    /// side, as the Mac's menus turn.
    pub fn open_menu_at(&mut self, entries: Vec<Entry>, point: Point<i32, Logical>) {
        let output = self.output_size();
        let (rect, rows) =
            MenuBar::place_menu(&entries, Rectangle::new(point, Size::default()), false, output);
        let loc = Point::from((
            menu_side(point.x, rect.size.w, 6, output.w - 6),
            menu_side(point.y, rect.size.h, MENU_BAR_HEIGHT, output.h - 6),
        ));
        let shift = loc - rect.loc;
        self.menu_bar.open = Some(OpenMenu {
            title: None,
            levels: vec![Level {
                entries,
                rect: Rectangle::new(loc, rect.size),
                rows: rows
                    .into_iter()
                    .map(|row| Rectangle::new(row.loc + shift, row.size))
                    .collect(),
                highlighted: None,
                picture: None,
                drawn: None,
            }],
            search: None,
            last_pointer: None,
        });
    }

    /// Opens the submenu of a row of a level beside it, closing any deeper one.
    fn open_submenu(&mut self, level: usize, row: usize, highlight_first: bool) {
        let size = self.output_size();
        let Some(open) = self.menu_bar.open.as_mut() else {
            return;
        };
        open.levels.truncate(level + 1);
        let parent = &open.levels[level];
        let entries = parent.entries[row].children.clone();
        if entries.is_empty() {
            return;
        }
        let (rect, rows) = MenuBar::place_menu(&entries, parent.rows[row], true, size);
        let highlighted = highlight_first
            .then(|| entries.iter().position(Entry::reachable))
            .flatten();
        open.levels.push(Level {
            entries,
            rect,
            rows,
            highlighted,
            picture: None,
            drawn: None,
        });
    }

    /// Every menu of the active application but Help, with its title, for searching.
    fn all_menus(&mut self) -> Vec<(String, Vec<Entry>)> {
        let titles = self.left_titles();
        titles
            .into_iter()
            .filter(|(source, _)| !matches!(source, Source::System | Source::Help))
            .filter_map(|(source, label)| {
                let label = label?;
                if matches!(source, Source::AppMenu(index) if self.active_menus().get(index).is_some_and(|menu| is_help(&menu.label)))
                {
                    return None;
                }
                Some((label, self.menu_entries(&source)))
            })
            .collect()
    }

    /// Shows what the Help menu's search finds for what was typed.
    fn update_search(&mut self) {
        let Some((query, base)) = self.menu_bar.open.as_ref().and_then(|open| open.search.clone()) else {
            return;
        };
        let mut entries = vec![Entry::search_field(&query)];
        if query.is_empty() {
            entries.extend(base);
        } else {
            let wanted = query.to_lowercase();
            let found: Vec<Entry> = searchable(&self.all_menus())
                .into_iter()
                .filter(|entry| entry.label.to_lowercase().contains(&wanted))
                .take(MAX_SEARCH_RESULTS)
                .collect();
            if found.is_empty() {
                entries.push(Entry::note("沒有相符的項目"));
            } else {
                entries.extend(found);
            }
        }
        let size = self.output_size();
        let Some(open) = self.menu_bar.open.as_mut() else {
            return;
        };
        let Some(anchor) = open
            .title
            .and_then(|title| self.menu_bar.titles.get(title))
            .map(|title| title.rect)
        else {
            return;
        };
        let (rect, rows) = MenuBar::place_menu(&entries, anchor, false, size);
        let highlighted = (!query.is_empty())
            .then(|| entries.iter().position(Entry::reachable))
            .flatten();
        open.levels = vec![Level {
            entries,
            rect,
            rows,
            highlighted,
            picture: None,
            drawn: None,
        }];
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
        let has_window = focused.is_some();

        match source {
            Source::System => vec![
                Entry::item("系統設定⋯", Command::Settings),
                Entry::separator(),
                // From an xrdp session, asleep would end the session; see Nekoland::sleep.
                Entry::item("睡眠", Command::Sleep).enabled(!self.remote),
                Entry::item("重新開機⋯", Command::Restart),
                Entry::item("關機⋯", Command::ShutDown),
                Entry::separator(),
                Entry::item("鎖定螢幕", Command::Lock).shortcut("Super L"),
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
            Source::AppMenu(index) => self
                .active_menus()
                .get(*index)
                .map(|menu| menu.children.iter().map(Entry::from_menu).collect())
                .unwrap_or_default(),
            Source::File => {
                let mut entries = Vec::new();
                if has("new-window") {
                    entries.push(Entry::item("新增視窗", Command::Action("new-window".into())));
                }
                entries.push(
                    Entry::item("關閉視窗", Command::CloseWindow)
                        .shortcut("Super Q")
                        .enabled(has_window),
                );
                entries
            }
            Source::Edit => {
                // With no window in front, copying, pasting and choosing all act on the
                // desktop's files, as the Finder's do.
                let desktop = self.desktop_edit_targets().filter(|_| !has_window);
                let chosen = desktop.map(<[_]>::to_vec).unwrap_or_default();
                let on_desktop = |keys, command| match desktop {
                    Some(_) => Command::Desktop(command),
                    None => Command::Keys(keys),
                };
                let copy = on_desktop(&[KEY_CTRL, KEY_C], DesktopCommand::Copy(chosen.clone()));
                let paste = on_desktop(&[KEY_CTRL, KEY_V], DesktopCommand::Paste);
                let all = on_desktop(&[KEY_CTRL, KEY_A], DesktopCommand::SelectAll);
                vec![
                    Entry::item("還原", Command::Keys(&[KEY_CTRL, KEY_Z]))
                        .shortcut("Ctrl Z")
                        .enabled(has_window),
                    Entry::item("重做", Command::Keys(&[KEY_CTRL, KEY_SHIFT, KEY_Z]))
                        .shortcut("Shift Ctrl Z")
                        .enabled(has_window),
                    Entry::separator(),
                    Entry::item("剪下", Command::Keys(&[KEY_CTRL, KEY_X]))
                        .shortcut("Ctrl X")
                        .enabled(has_window),
                    Entry::item("拷貝", copy)
                        .shortcut("Ctrl C")
                        .enabled(has_window || !chosen.is_empty()),
                    Entry::item("貼上", paste)
                        .shortcut("Ctrl V")
                        .enabled(has_window || desktop.is_some()),
                    Entry::item("全選", all)
                        .shortcut("Ctrl A")
                        .enabled(has_window || desktop.is_some()),
                ]
            }
            Source::View => {
                let fullscreen = focused.as_ref().is_some_and(|window| self.is_fullscreen(window));
                let label = if fullscreen {
                    "結束全螢幕"
                } else {
                    "進入全螢幕"
                };
                vec![Entry::item(label, Command::FullScreen)
                    .shortcut("F11")
                    .enabled(has_window)]
            }
            Source::Window => {
                let mut entries = vec![
                    Entry::item("縮到最小", Command::Minimize)
                        .shortcut("Super H")
                        .enabled(has_window),
                    Entry::item("縮放", Command::Zoom)
                        .shortcut("Super ↑")
                        .enabled(has_window),
                    Entry {
                        children: vec![
                            Entry::item("左半邊", Command::TileLeft).shortcut("Super ←"),
                            Entry::item("右半邊", Command::TileRight).shortcut("Super →"),
                        ],
                        ..Entry::note("移動與調整大小").enabled(has_window)
                    },
                ];
                let desktops = self.workspaces.count();
                let active = self.workspaces.active;
                if desktops > 1 && has_window {
                    entries.push(Entry {
                        children: (0..desktops)
                            .filter(|&index| index != active)
                            .map(|index| {
                                Entry::item(format!("桌面 {}", index + 1), Command::MoveToDesktop(index))
                            })
                            .collect(),
                        ..Entry::note("移到").enabled(true)
                    });
                }
                entries.push(Entry::separator());
                entries.push(Entry::item("全部移到最前面", Command::BringAllToFront));
                if let Some((app_id, _)) = &app {
                    // Minimized ones too, as the Mac's Window menu lists them.
                    let windows: Vec<Window> = self
                        .all_windows()
                        .filter(|window| is_managed(window) && window_names(window).1 == *app_id)
                        .cloned()
                        .collect();
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
            Source::Help => {
                if has("help") {
                    vec![Entry::item(
                        format!("「{name}」輔助說明"),
                        Command::Action("help".into()),
                    )]
                } else {
                    Vec::new()
                }
            }
            Source::Status(key) => self
                .menu_bar
                .items
                .iter()
                .find(|item| item.key == *key)
                .map(|item| item.menu.iter().map(Entry::from_menu).collect())
                .unwrap_or_default(),
            Source::Sound => match self.menu_bar.sound {
                Some(volume) => vec![
                    Entry::note(format!("音量 {}%", volume.percent)),
                    Entry::separator(),
                    Entry::item("調高音量", Command::VolumeUp),
                    Entry::item("調低音量", Command::VolumeDown),
                    Entry::item("靜音", Command::Mute).checked(volume.muted),
                ],
                None => Vec::new(),
            },
            Source::Network => match &self.menu_bar.network {
                Some(network) => {
                    let kind = if network.wireless { "Wi-Fi" } else { "乙太網路" };
                    let state = if network.connected {
                        "已連線"
                    } else {
                        "未連線"
                    };
                    let mut entries = vec![
                        Entry::note(format!("{kind}：{state}")),
                        Entry::note(network.interface.clone()),
                    ];
                    if let Some(address) = network.address {
                        entries.push(Entry::note(format!("IP 位址 {address}")));
                    }
                    entries
                }
                None => Vec::new(),
            },
            Source::Control => Vec::new(),
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
            let control = self.menu_bar.control.is_some()
                && self.menu_bar.titles.get(title).is_some_and(|title| title.source == Source::Control);
            if control
                || self
                    .menu_bar
                    .open
                    .as_ref()
                    .is_some_and(|open| open.title == Some(title))
            {
                self.close_menu();
            } else {
                self.open_menu(title, false);
            }
            return true;
        }
        if self.menu_bar.control.is_some() {
            if !self.control_press(local) {
                self.close_menu();
            }
            return true;
        }
        // A click anywhere else only dismisses the menus; it does not reach a window.
        if self.level_at(local).is_none() {
            self.close_menu();
        }
        true
    }

    /// The pointer moved: open menus follow it across the bar, light the entry under it,
    /// and open the submenu of that entry.
    pub fn menu_bar_motion(&mut self, pos: Point<f64, Logical>) {
        if self.menu_bar.control.is_some() && self.menu_bar_shown() {
            let Some(local) = self.local(pos) else {
                return;
            };
            let dragging = self.menu_bar.control.as_ref().is_some_and(|control| control.dragging());
            // As from one menu to the next, the pointer goes from the panel to another menu.
            match self.title_at(local) {
                Some(title) if !dragging && self.menu_bar.titles[title].source != Source::Control => {
                    self.open_menu(title, false)
                }
                _ => self.control_motion(local),
            }
            return;
        }
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
                .is_some_and(|open| open.title.is_some_and(|open_title| open_title != title))
            {
                self.open_menu(title, false);
            }
            return;
        }
        let Some((level, row)) = self.level_at(local) else {
            return;
        };
        let Some(open) = self.menu_bar.open.as_mut() else {
            return;
        };
        let previous = open.last_pointer.replace(local);

        // On the way from a row into its submenu, the pointer may cross other rows; while it
        // heads for the submenu, they do not take over.
        if let (Some(child), Some(from)) = (open.levels.get(level + 1), previous) {
            let parent_row = open.levels[level].highlighted;
            if row != parent_row {
                let edge_x = if child.rect.loc.x > open.levels[level].rect.loc.x {
                    child.rect.loc.x
                } else {
                    child.rect.loc.x + child.rect.size.w
                };
                let top = Point::from((edge_x, child.rect.loc.y));
                let bottom = Point::from((edge_x, child.rect.loc.y + child.rect.size.h));
                if in_triangle(local, from, top, bottom) {
                    return;
                }
            }
        }

        open.levels.truncate(level + 1);
        open.levels[level].highlighted = row;
        let has_children = row.is_some_and(|row| !open.levels[level].entries[row].children.is_empty());
        if let (Some(row), true) = (row, has_children) {
            self.open_submenu(level, row, false);
        }
    }

    /// A button came up: over an entry, that entry is chosen, whether it was clicked or
    /// the press that opened the menu was dragged onto it.
    pub fn menu_bar_release(&mut self, pos: Point<f64, Logical>) {
        if self.menu_bar.control.is_some() {
            if let Some(local) = self.local(pos) {
                self.control_release(local);
            }
            return;
        }
        if self.menu_bar.open.is_none() {
            return;
        }
        let Some(local) = self.local(pos) else {
            return;
        };
        if let Some((level, Some(row))) = self.level_at(local) {
            self.choose(level, row);
        }
    }

    /// A key while a menu is open. Every key goes to the menus then; in a Help menu,
    /// typing searches.
    pub fn menu_bar_key(&mut self, sym: Keysym, typed: Option<char>) {
        let Some(open) = self.menu_bar.open.as_ref() else {
            return;
        };
        let title = open.title;
        let titles = self.menu_bar.titles.len();
        let depth = open.levels.len() - 1;
        let searching = open.search.is_some() && depth == 0;

        if searching {
            let edit = match (sym, typed) {
                (Keysym::BackSpace, _) => Some(None),
                (Keysym::Return | Keysym::KP_Enter | Keysym::Escape | Keysym::Up | Keysym::Down, _) => None,
                (_, Some(character)) if !character.is_control() => Some(Some(character)),
                _ => None,
            };
            if let Some(edit) = edit {
                if let Some((query, _)) = self.menu_bar.open.as_mut().and_then(|open| open.search.as_mut()) {
                    match edit {
                        Some(character) => query.push(character),
                        None => {
                            query.pop();
                        }
                    }
                }
                self.update_search();
                return;
            }
        }

        let step = |level: &Level, forward: bool| {
            let count = level.entries.len();
            let mut index = level.highlighted;
            for _ in 0..count {
                let next = match (index, forward) {
                    (None, true) => 0,
                    (None, false) => count - 1,
                    (Some(at), true) => (at + 1) % count,
                    (Some(at), false) => (at + count - 1) % count,
                };
                if level.entries[next].reachable() {
                    return Some(next);
                }
                index = Some(next);
            }
            level.highlighted
        };
        let current = &open.levels[depth];
        let highlighted = current.highlighted;
        let opens_submenu = highlighted.is_some_and(|row| !current.entries[row].children.is_empty());

        match sym {
            Keysym::Escape if depth > 0 => {
                if let Some(open) = self.menu_bar.open.as_mut() {
                    open.levels.truncate(depth);
                }
            }
            Keysym::Escape => self.close_menu(),
            Keysym::Left if depth > 0 => {
                if let Some(open) = self.menu_bar.open.as_mut() {
                    open.levels.truncate(depth);
                }
            }
            Keysym::Left => {
                if let Some(title) = title {
                    self.open_menu((title + titles - 1) % titles, true);
                }
            }
            Keysym::Right if opens_submenu => {
                if let Some(row) = highlighted {
                    self.open_submenu(depth, row, true);
                }
            }
            Keysym::Right => {
                if let Some(title) = title {
                    self.open_menu((title + 1) % titles, true);
                }
            }
            Keysym::Down | Keysym::Up => {
                if let Some(open) = self.menu_bar.open.as_mut() {
                    let level = &mut open.levels[depth];
                    level.highlighted = step(level, sym == Keysym::Down);
                }
            }
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                if let Some(row) = highlighted {
                    if opens_submenu {
                        self.open_submenu(depth, row, true);
                    } else {
                        self.choose(depth, row);
                    }
                }
            }
            _ => {}
        }
    }

    /// Puts what `change` gives in place of entries of the open menu, by their commands,
    /// without closing it. The menu keeps the width it opened with.
    pub fn change_open_menu(&mut self, change: impl Fn(Option<&Command>) -> Option<Entry>) {
        let Some(level) = self.menu_bar.open.as_mut().and_then(|open| open.levels.first_mut()) else {
            return;
        };
        for entry in &mut level.entries {
            if let Some(changed) = change(entry.command.as_ref()) {
                *entry = changed;
                level.drawn = None;
            }
        }
    }

    /// Ctrl+F2: the first menu, its first entry lit, for the keyboard.
    pub fn open_first_menu(&mut self) {
        if self.menu_bar_shown() && !self.menu_bar.titles.is_empty() {
            self.open_menu(0, true);
        }
    }

    fn choose(&mut self, level: usize, row: usize) {
        let command = self
            .menu_bar
            .open
            .as_ref()
            .and_then(|open| open.levels.get(level))
            .and_then(|level| level.entries.get(row))
            .filter(|entry| entry.children.is_empty())
            .and_then(|entry| entry.command.clone());
        let Some(command) = command else {
            return;
        };
        self.close_menu();
        self.run_menu_command(command);
    }

    /// Presses keys for the window with the keyboard, then lets them go in reverse.
    fn press_keys(&mut self, keys: &[u32]) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let time = self.start_time.elapsed().as_millis() as u32;
        for (state, keys) in [
            (KeyState::Pressed, keys.to_vec()),
            (KeyState::Released, keys.iter().rev().copied().collect()),
        ] {
            for key in keys {
                keyboard.input::<(), _>(
                    self,
                    Keycode::new(key),
                    state,
                    SERIAL_COUNTER.next_serial(),
                    time,
                    |_, _, _| FilterResult::Forward,
                );
            }
        }
    }

    /// Pastes into the active window what the launcher put on the clipboard: Ctrl+V, and
    /// Ctrl+Shift+V for a terminal, where Ctrl+V is a key of its own.
    pub(crate) fn paste_into_active(&mut self) {
        let app_id = self.focused_window().map(|window| window_names(&window).1).unwrap_or_default();
        // shortcut: terminals known by name; read their desktop entry's TerminalEmulator
        // category once one is missed.
        let terminal = ["terminal", "foot", "kitty", "alacritty", "ptyxis", "konsole", "wezterm", "xterm"]
            .iter()
            .any(|name| app_id.to_lowercase().contains(name));
        if terminal {
            self.press_keys(&[KEY_CTRL, KEY_SHIFT, KEY_V]);
        } else {
            self.press_keys(&[KEY_CTRL, KEY_V]);
        }
    }

    /// Opens the Help menu's search over the active application's menus, with `query`
    /// typed in it.
    pub(crate) fn search_menus(&mut self, query: &str) {
        self.refresh_menu_bar();
        let menus = self.active_menus();
        let help = self.menu_bar.titles.iter().rposition(|title| match title.source {
            Source::Help => true,
            Source::AppMenu(index) => menus.get(index).is_some_and(|menu| is_help(&menu.label)),
            _ => false,
        });
        let Some(help) = help else {
            return;
        };
        self.open_menu(help, false);
        if let Some((typed, _)) = self.menu_bar.open.as_mut().and_then(|open| open.search.as_mut()) {
            *typed = query.to_string();
        }
        self.update_search();
    }

    /// Ends an application: through its quit action if it exports one, else by closing
    /// every window it has, which is how such a program ends.
    pub(crate) fn quit_app(&mut self, app_id: &str) {
        let quits = self
            .menu_bar
            .actions
            .get(app_id)
            .is_some_and(|actions| actions.iter().any(|action| action == "quit"));
        match &self.menu_bar.bus {
            Some(bus) if quits => bus.send(Request::Activate {
                app_id: app_id.to_string(),
                action: "quit".into(),
            }),
            _ => self
                .all_windows()
                .filter(|window| is_managed(window) && window_names(window).1 == app_id)
                .for_each(close),
        }
    }

    /// The process behind a window.
    pub(crate) fn window_pid(&self, window: &Window) -> Option<i32> {
        match window.underlying_surface() {
            WindowSurface::Wayland(toplevel) => {
                smithay::reexports::wayland_server::Resource::client(toplevel.wl_surface())?
                    .get_credentials(&self.display_handle)
                    .ok()
                    .map(|credentials| credentials.pid)
            }
            WindowSurface::X11(surface) => surface.pid().and_then(|pid| i32::try_from(pid).ok()),
        }
    }

    /// Ends an application at once, whether it answers or not: the processes behind its
    /// windows are killed.
    pub(crate) fn force_quit_app(&mut self, app_id: &str) {
        let mut pids: Vec<i32> = self
            .all_windows()
            .filter(|window| is_managed(window) && window_names(window).1 == app_id)
            .filter_map(|window| self.window_pid(window))
            .collect();
        pids.sort_unstable();
        pids.dedup();
        // Never the compositor itself, whose own windows are not an application's.
        let own = i32::try_from(std::process::id()).unwrap_or_default();
        for pid in pids.into_iter().filter(|pid| *pid > 1 && *pid != own) {
            // SAFETY: kill only sends a signal; a process that is already gone is an error
            // it reports, not undefined behaviour.
            if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
                tracing::warn!("{app_id}: could not end {pid}: {}", std::io::Error::last_os_error());
            }
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
            Command::Sleep => self.sleep(),
            Command::Lock => self.lock_screen(),
            // Asked first, in a dialog of its own: these lose what is not saved.
            Command::Settings => config::spawn("nekoland-settings"),
            Command::Restart => config::spawn("nekoland-confirm restart"),
            Command::ShutDown => config::spawn("nekoland-confirm shutdown"),
            Command::LogOut => config::spawn("nekoland-confirm logout"),
            Command::Action(action) => {
                if let Some((app_id, _)) = app {
                    send(&self.menu_bar, Request::Activate { app_id, action });
                }
            }
            Command::Invoke(action) => send(&self.menu_bar, Request::Invoke(action)),
            Command::Keys(keys) => self.press_keys(keys),
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
                if let Some((app_id, _)) = app {
                    self.quit_app(&app_id);
                }
            }
            Command::CloseWindow => {
                if let Some(window) = focused {
                    close(&window);
                }
            }
            Command::FullScreen => {
                if let Some(window) = focused {
                    if self.is_fullscreen(&window) {
                        self.restore(&window);
                    } else {
                        self.fullscreen(&window);
                    }
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
            Command::Focus(window) => self.restore_from_dock(&window),
            Command::Dock(command) => self.run_dock_command(command),
            Command::Desktop(command) => self.run_desktop_command(command),
            Command::VolumeUp => config::spawn(&self.config.volume_up),
            Command::VolumeDown => config::spawn(&self.config.volume_down),
            Command::Mute => config::spawn(&self.config.volume_mute),
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

pub(crate) fn close(window: &Window) {
    match window.underlying_surface() {
        WindowSurface::Wayland(toplevel) => toplevel.send_close(),
        WindowSurface::X11(surface) => {
            let _ = surface.close();
        }
    }
}

/// Where a menu `length` long starts along one axis to have its edge at `point`: after it,
/// or before it where it would pass `end`, kept from `start`.
fn menu_side(point: i32, length: i32, start: i32, end: i32) -> i32 {
    let at = if point + length <= end { point } else { point - length };
    at.max(start).min((end - length).max(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_turns_to_keep_its_corner_at_the_pointer() {
        // Room after the pointer: it starts there.
        assert_eq!(menu_side(100, 200, 6, 1914), 100);
        // None: it ends there.
        assert_eq!(menu_side(1850, 200, 6, 1914), 1650);
        // Room on neither side: kept on the output.
        assert_eq!(menu_side(150, 200, 6, 300), 6);
    }

    fn output() -> Size<i32, Logical> {
        Size::from((1920, 1080))
    }

    /// The bar reads the strip of wallpaper under it, wherever the way it is drawn puts it.
    #[test]
    fn the_wallpaper_under_the_bar_is_told_light_or_dark() {
        // Light along its top tenth, dark below, at the output's shape.
        let image = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 192, 108).unwrap();
        image.fill(0x101010ff);
        image.new_subpixbuf(0, 0, 192, 10).fill(0xf0f0f0ff);
        let original = (1920, 1080);
        assert!(!strip_is_dark(&image, original, config::WallpaperMode::Fill, output()));
        assert!(!strip_is_dark(&image, original, config::WallpaperMode::Fit, output()));
        // Centred on a taller output, its light top is below the bar, which is over black.
        let tall = Size::from((1920, 1600));
        assert!(strip_is_dark(&image, original, config::WallpaperMode::Center, tall));
    }

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
        let (rect, _) = MenuBar::place_menu(&entries(), title, false, output());
        assert!(rect.loc.x + rect.size.w <= 1920 - 6);
        assert_eq!(rect.loc.y, MENU_BAR_HEIGHT + GAP_BELOW_BAR);
    }

    #[test]
    fn rows_are_stacked_with_thin_separators() {
        let title = Rectangle::new((8, 0).into(), (36, MENU_BAR_HEIGHT).into());
        let (rect, rows) = MenuBar::place_menu(&entries(), title, false, output());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].size.h, SEPARATOR_HEIGHT);
        assert_eq!(rows[2].loc.y, rows[1].loc.y + SEPARATOR_HEIGHT);
        assert_eq!(rect.size.h, MENU_PADDING * 2 + ROW_HEIGHT * 2 + SEPARATOR_HEIGHT);
    }

    #[test]
    fn submenus_open_beside_their_row_and_flip_at_the_edge() {
        let row = Rectangle::new((100, 200).into(), (208, ROW_HEIGHT).into());
        let (rect, _) = MenuBar::place_menu(&entries(), row, true, output());
        assert!(rect.loc.x > row.loc.x + row.size.w - SUBMENU_OVERLAP * 2);
        assert_eq!(rect.loc.y, row.loc.y - MENU_PADDING);

        let at_edge = Rectangle::new((1700, 200).into(), (208, ROW_HEIGHT).into());
        let (rect, _) = MenuBar::place_menu(&entries(), at_edge, true, output());
        assert!(rect.loc.x + rect.size.w <= at_edge.loc.x);
    }

    #[test]
    fn only_entries_that_do_something_can_be_reached() {
        let entries = entries();
        assert!(entries[0].reachable());
        assert!(!entries[1].reachable());
        assert!(!Entry::item("三", Command::Sleep).enabled(false).reachable());
        assert!(!Entry::note("音量 40%").reachable());
        assert!(!Entry::search_field("").reachable());
    }

    #[test]
    fn search_finds_entries_in_submenus_by_their_way_there() {
        let menus = vec![(
            "視窗".to_string(),
            vec![Entry {
                children: vec![Entry::item("左半邊", Command::TileLeft)],
                ..Entry::note("移動與調整大小")
            }],
        )];
        let found = searchable(&menus);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "視窗 › 移動與調整大小 › 左半邊");
    }

    #[test]
    fn a_point_on_the_way_to_a_submenu_is_in_the_triangle() {
        let from = Point::from((100, 100));
        let top = Point::from((200, 50));
        let bottom = Point::from((200, 250));
        assert!(in_triangle(Point::from((150, 110)), from, top, bottom));
        assert!(!in_triangle(Point::from((150, 30)), from, top, bottom));
    }
}
