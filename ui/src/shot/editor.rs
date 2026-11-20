//! 截圖's window: the screenshot on a canvas that zooms and pans, the tools along the top, the
//! chosen tool's or mark's style down the right, and where the pointer is with the colour
//! under it along the bottom. What is copied, saved, pinned or uploaded is the picture with
//! everything on it.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use adw::prelude::*;
use gtk::{cairo, gdk, gio, glib};

use super::img::{apca, hex, hsl, oklch, Area, Img};
use super::marks::{self, ArrowStyle, Backdrop, Censor, Doc, Kind, Mark, Point, Style, GRADIENTS};
use super::output::{self, Prefs};

/// The colours offered for marks: red, orange, yellow, green, blue, purple, ink and white.
pub const SWATCHES: [[f64; 4]; 8] = [
    [0.898, 0.282, 0.302, 1.0],
    [0.961, 0.647, 0.141, 1.0],
    [1.0, 0.851, 0.239, 1.0],
    [0.188, 0.643, 0.424, 1.0],
    [0.208, 0.518, 0.894, 1.0],
    [0.557, 0.306, 0.776, 1.0],
    [0.141, 0.122, 0.192, 1.0],
    [1.0, 1.0, 1.0, 1.0],
];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tool {
    Select,
    Arrow,
    Rect,
    Ellipse,
    Line,
    Pen,
    Highlight,
    Spotlight,
    Text,
    Step,
    Magnifier,
    Censor,
    Ruler,
    Picker,
}

/// The tools in the bar's order, with their icons and names; `None` parts the groups.
const TOOLS: [Option<(Tool, &str, &str)>; 17] = [
    Some((Tool::Select, "shot-select-symbolic", "選取與裁切")),
    None,
    Some((Tool::Arrow, "shot-arrow-symbolic", "箭頭")),
    Some((Tool::Rect, "shot-rect-symbolic", "矩形")),
    Some((Tool::Ellipse, "shot-ellipse-symbolic", "橢圓")),
    Some((Tool::Line, "shot-line-symbolic", "線條")),
    Some((Tool::Pen, "shot-pen-symbolic", "手繪")),
    Some((Tool::Highlight, "shot-highlight-symbolic", "螢光筆")),
    Some((Tool::Spotlight, "shot-spotlight-symbolic", "聚光燈")),
    None,
    Some((Tool::Text, "insert-text-symbolic", "文字")),
    Some((Tool::Step, "shot-step-symbolic", "步驟")),
    Some((Tool::Magnifier, "shot-magnifier-symbolic", "放大鏡")),
    None,
    Some((Tool::Censor, "shot-censor-symbolic", "打碼")),
    Some((Tool::Ruler, "shot-ruler-symbolic", "尺規")),
    Some((Tool::Picker, "color-select-symbolic", "取色")),
];

impl Tool {
    fn kind(self) -> Option<Kind> {
        Some(match self {
            Tool::Arrow => Kind::Arrow,
            Tool::Rect => Kind::Rect,
            Tool::Ellipse => Kind::Ellipse,
            Tool::Line => Kind::Line,
            Tool::Pen => Kind::Pen,
            Tool::Highlight => Kind::Highlight,
            Tool::Spotlight => Kind::Spotlight,
            Tool::Text => Kind::Text,
            Tool::Step => Kind::Step,
            Tool::Magnifier => Kind::Magnifier,
            Tool::Censor => Kind::Censor,
            Tool::Ruler => Kind::Measure,
            Tool::Select | Tool::Picker => return None,
        })
    }

    fn of(kind: Kind) -> Tool {
        match kind {
            Kind::Arrow => Tool::Arrow,
            Kind::Rect => Tool::Rect,
            Kind::Ellipse => Tool::Ellipse,
            Kind::Line => Tool::Line,
            Kind::Pen => Tool::Pen,
            Kind::Highlight => Tool::Highlight,
            Kind::Spotlight => Tool::Spotlight,
            Kind::Text => Tool::Text,
            Kind::Step => Tool::Step,
            Kind::Magnifier => Tool::Magnifier,
            Kind::Censor => Tool::Censor,
            Kind::Measure => Tool::Ruler,
            Kind::Picture => Tool::Select,
        }
    }

    fn name(self) -> &'static str {
        TOOLS
            .iter()
            .flatten()
            .find(|(t, _, _)| *t == self)
            .map_or("", |(_, _, name)| name)
    }
}

enum Drag {
    None,
    Pan((f64, f64)),
    NewSel(Point),
    MoveSel(Point, Area),
    SelHandle(usize, Area),
    Create,
    MoveMark(Point, Vec<Point>),
    /// A mark just put down by a click, which can still be dragged to its place.
    Place(Point, Vec<Point>),
    /// One of the chosen mark's points: an end, or an arrow's bend at 2.
    MarkPoint(usize),
}

/// The widgets of the style panel, kept to show the chosen mark's or tool's values.
struct Inspector {
    panel: gtk::Box,
    title: gtk::Label,
    colours: gtk::Box,
    custom: gtk::ColorDialogButton,
    rows: HashMap<&'static str, gtk::Widget>,
    width: gtk::Scale,
    arrow: gtk::DropDown,
    fill: gtk::Switch,
    shadow: gtk::Switch,
    sketchy: gtk::Switch,
    size: gtk::Scale,
    censor: gtk::DropDown,
    text_only: gtk::Switch,
    strength: gtk::Scale,
    dim: gtk::Scale,
    zoom: gtk::Scale,
    opacity: gtk::Scale,
    step_start: gtk::SpinButton,
}

pub struct Ed {
    window: adw::ApplicationWindow,
    canvas: gtk::DrawingArea,
    doc: RefCell<Doc>,
    surface: RefCell<Option<(*const Img, cairo::ImageSurface)>>,
    undo: RefCell<Vec<Doc>>,
    redo: RefCell<Vec<Doc>>,
    last_style_undo: Cell<Option<Instant>>,
    tool: Cell<Tool>,
    styles: RefCell<HashMap<Tool, Style>>,
    selected: Cell<Option<usize>>,
    sel: Cell<Option<Area>>,
    zoom: Cell<f64>,
    off: Cell<(f64, f64)>,
    fitted: Cell<bool>,
    drag: RefCell<Drag>,
    pressed: Cell<(f64, f64)>,
    pointer: Cell<Option<Point>>,
    measuring: Cell<Option<(Point, Point)>>,
    space: Cell<bool>,
    zoom_click: Cell<bool>,
    physical: Cell<bool>,
    prefs: RefCell<Prefs>,
    entry: PathBuf,
    status: gtk::Label,
    coords: gtk::Label,
    colour: gtk::Label,
    zoom_label: gtk::Label,
    size_button: gtk::MenuButton,
    tool_buttons: RefCell<Vec<(Tool, gtk::ToggleButton)>>,
    insp: Inspector,
    syncing: Cell<bool>,
}

fn rgba(c: [f64; 4]) -> gdk::RGBA {
    gdk::RGBA::new(c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32)
}

fn row(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let text = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .build();
    row.append(&text);
    row.append(widget);
    row
}

fn column(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    col.append(widget);
    col
}

fn scale(min: f64, max: f64, step: f64) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_hexpand(true);
    scale
}

fn tool_icon_button(icon: &str, tip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tip)
        .css_classes(["flat"])
        .build()
}

impl Ed {
    /// Opens a window on a screenshot; `entry` is its place in the history.
    pub fn open(app: &adw::Application, doc: Doc, entry: PathBuf, title: &str) -> Rc<Ed> {
        let prefs = Prefs::load();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(title)
            .default_width(1180)
            .default_height(760)
            .build();
        let canvas = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .focusable(true)
            .build();
        let size_button = gtk::MenuButton::builder()
            .css_classes(["flat", "numeric"])
            .tooltip_text("尺寸")
            .build();
        let ed = Rc::new(Ed {
            window,
            canvas,
            doc: RefCell::new(doc),
            surface: RefCell::new(None),
            undo: RefCell::default(),
            redo: RefCell::default(),
            last_style_undo: Cell::new(None),
            tool: Cell::new(Tool::Arrow),
            styles: RefCell::default(),
            selected: Cell::new(None),
            sel: Cell::new(None),
            zoom: Cell::new(1.0),
            off: Cell::new((0.0, 0.0)),
            fitted: Cell::new(false),
            drag: RefCell::new(Drag::None),
            pressed: Cell::new((0.0, 0.0)),
            pointer: Cell::new(None),
            measuring: Cell::new(None),
            space: Cell::new(false),
            zoom_click: Cell::new(false),
            physical: Cell::new(false),
            prefs: RefCell::new(prefs),
            entry,
            status: gtk::Label::builder()
                .xalign(1.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Start)
                .build(),
            coords: gtk::Label::builder()
                .css_classes(["numeric"])
                .width_chars(14)
                .xalign(0.0)
                .build(),
            colour: gtk::Label::builder().css_classes(["numeric"]).xalign(0.0).build(),
            zoom_label: gtk::Label::builder().css_classes(["numeric"]).build(),
            size_button,
            tool_buttons: RefCell::default(),
            insp: build_inspector(),
            syncing: Cell::new(false),
        });
        ed.build();
        ed.select_tool(Tool::Arrow);
        ed.refresh();
        ed.window.present();
        ed.canvas.grab_focus();
        ed
    }

    fn style_for(&self, tool: Tool) -> Style {
        let doc = self.doc.borrow();
        let s = doc.scale;
        let colour = SWATCHES[self.prefs.borrow().color.min(SWATCHES.len() - 1)];
        self.styles
            .borrow_mut()
            .entry(tool)
            .or_insert_with(|| {
                let mut style = Style::new(s, colour);
                match tool {
                    Tool::Highlight => {
                        style.color = SWATCHES[2];
                        style.shadow = false;
                    }
                    Tool::Pen => style.shadow = false,
                    Tool::Step => style.size = 13.0 * s,
                    Tool::Magnifier => {
                        style.size = 70.0 * s;
                        style.width = 3.0 * s;
                    }
                    Tool::Ruler => {
                        style.width = 2.0 * s;
                        style.shadow = false;
                    }
                    Tool::Text => style.shadow = false,
                    _ => {}
                }
                style
            })
            .clone()
    }

    fn build(self: &Rc<Self>) {
        let header = adw::HeaderBar::new();
        let copy = gtk::Button::builder()
            .label("複製")
            .css_classes(["suggested-action"])
            .action_name("win.copy")
            .build();
        header.pack_end(&copy);
        header.pack_end(&self.size_button);
        let menu = gio::Menu::new();
        let file = gio::Menu::new();
        file.append(Some("儲存"), Some("win.save"));
        file.append(Some("另存新檔…"), Some("win.save-as"));
        file.append(Some("拖曳輸出…"), Some("win.drag-hint"));
        menu.append_section(None, &file);
        let more = gio::Menu::new();
        more.append(Some("歷史紀錄"), Some("app.history"));
        more.append(Some("偏好設定"), Some("app.prefs"));
        menu.append_section(None, &more);
        header.pack_start(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .build(),
        );

        // The tools.
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        bar.add_css_class("toolbar");
        for entry in TOOLS {
            let Some((tool, icon, tip)) = entry else {
                bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
                continue;
            };
            let button = gtk::ToggleButton::builder()
                .icon_name(icon)
                .tooltip_text(tip)
                .css_classes(["flat"])
                .build();
            let ed = self.clone();
            button.connect_clicked(move |b| {
                if b.is_active() {
                    ed.select_tool(tool);
                } else if ed.tool.get() == tool {
                    b.set_active(true);
                }
            });
            self.tool_buttons.borrow_mut().push((tool, button.clone()));
            bar.append(&button);
        }
        bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        let ocr = tool_icon_button("shot-ocr-symbolic", "文字辨識");
        ocr.set_action_name(Some("win.ocr"));
        bar.append(&ocr);
        bar.append(&self.backdrop_button());
        let overlay_menu = gio::Menu::new();
        overlay_menu.append(Some("貼上圖片"), Some("win.paste"));
        overlay_menu.append(Some("開啟圖片…"), Some("win.open-picture"));
        overlay_menu.append(Some("輸出前後對照動畫"), Some("win.before-after"));
        bar.append(
            &gtk::MenuButton::builder()
                .icon_name("shot-overlay-symbolic")
                .tooltip_text("疊圖")
                .menu_model(&overlay_menu)
                .css_classes(["flat"])
                .build(),
        );
        let add_menu = gio::Menu::new();
        add_menu.append(Some("並排加入"), Some("win.add-capture::beside"));
        add_menu.append(Some("上下加入"), Some("win.add-capture::below"));
        bar.append(
            &gtk::MenuButton::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text("加入截圖")
                .menu_model(&add_menu)
                .css_classes(["flat"])
                .build(),
        );
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        bar.append(&spacer);
        for (icon, tip, action) in [
            ("edit-undo-symbolic", "復原", "win.undo"),
            ("edit-redo-symbolic", "重做", "win.redo"),
        ] {
            let b = tool_icon_button(icon, tip);
            b.set_action_name(Some(action));
            bar.append(&b);
        }
        bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        for (icon, tip, action) in [
            ("view-pin-symbolic", "釘選", "win.pin"),
            ("send-to-symbolic", "上傳", "win.upload"),
        ] {
            let b = tool_icon_button(icon, tip);
            b.set_action_name(Some(action));
            bar.append(&b);
        }

        // The canvas and the style panel.
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&self.canvas);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&self.insp.panel);

        // Along the bottom.
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        foot.add_css_class("toolbar");
        foot.append(&self.coords);
        foot.append(&self.colour);
        foot.append(&self.status);
        foot.append(&self.zoom_label);
        let drag_icon = gtk::Image::builder()
            .icon_name("image-x-generic-symbolic")
            .tooltip_text("拖曳到其他應用程式")
            .build();
        drag_icon.set_cursor_from_name(Some("grab"));
        self.drag_source(&drag_icon);
        foot.append(&drag_icon);
        let save = gtk::Button::builder()
            .label("儲存")
            .action_name("win.save")
            .css_classes(["flat"])
            .build();
        foot.append(&save);

        // All the tools are wider than half of a small display: there they scroll sideways,
        // so that the window still fits a half.
        let tools = gtk::ScrolledWindow::builder()
            .child(&bar)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.add_top_bar(&tools);
        view.set_content(Some(&body));
        view.add_bottom_bar(&foot);
        self.window.set_content(Some(&view));

        self.wire_inspector();
        self.wire_canvas();
        self.add_actions();
        let ed = self.clone();
        self.window.connect_close_request(move |_| {
            output::keep(&ed.entry, &ed.doc.borrow());
            glib::Propagation::Proceed
        });
    }

    // ---- Changes and taking them back ----

    fn push_undo(&self) {
        self.undo.borrow_mut().push(self.doc.borrow().clone());
        if self.undo.borrow().len() > 100 {
            self.undo.borrow_mut().remove(0);
        }
        self.redo.borrow_mut().clear();
        self.last_style_undo.set(None);
    }

    /// A step to take back for a run of style changes, not one for each.
    fn push_style_undo(&self) {
        let recent = self
            .last_style_undo
            .get()
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1));
        if !recent {
            self.push_undo();
        }
        self.last_style_undo.set(Some(Instant::now()));
    }

    fn undo(&self, back: bool) {
        let (from, to) = if back {
            (&self.undo, &self.redo)
        } else {
            (&self.redo, &self.undo)
        };
        let Some(doc) = from.borrow_mut().pop() else {
            return;
        };
        let current = self.doc.replace(doc);
        to.borrow_mut().push(current);
        self.selected.set(None);
        self.refresh();
    }

    /// Everything that shows the document again after a change.
    fn refresh(&self) {
        let doc = self.doc.borrow();
        let (w, h) = doc.canvas_size();
        let (w, h) = if self.physical.get() {
            (w as f64, h as f64)
        } else {
            (w as f64 / doc.scale, h as f64 / doc.scale)
        };
        self.size_button.set_label(&format!(
            "{} × {}{}",
            w.round(),
            h.round(),
            if self.physical.get() { " px" } else { "" }
        ));
        drop(doc);
        if self
            .selected
            .get()
            .is_some_and(|i| i >= self.doc.borrow().marks.len())
        {
            self.selected.set(None);
        }
        self.zoom_label.set_label(&format!(
            "{}%",
            (self.zoom.get() * self.doc.borrow().scale * 100.0).round()
        ));
        self.sync_inspector();
        self.canvas.queue_draw();
    }

    fn say(&self, text: &str) {
        self.status.set_label(text);
    }

    // ---- The view ----

    fn margin(&self) -> f64 {
        self.doc.borrow().margin()
    }

    /// A point on the widget as a point in the picture's pixels.
    fn to_image(&self, x: f64, y: f64) -> Point {
        let (ox, oy) = self.off.get();
        let z = self.zoom.get();
        let m = self.margin();
        ((x - ox) / z - m, (y - oy) / z - m)
    }

    fn to_widget(&self, p: Point) -> Point {
        let (ox, oy) = self.off.get();
        let z = self.zoom.get();
        let m = self.margin();
        ((p.0 + m) * z + ox, (p.1 + m) * z + oy)
    }

    fn fit(&self) {
        let (w, h) = (self.canvas.width() as f64, self.canvas.height() as f64);
        if w < 10.0 || h < 10.0 {
            return;
        }
        let (cw, ch) = self.doc.borrow().canvas_size();
        let scale = self.doc.borrow().scale;
        let z = ((w - 48.0) / cw as f64)
            .min((h - 48.0) / ch as f64)
            .min(1.0 / scale)
            .max(0.02);
        self.zoom.set(z);
        self.off
            .set(((w - cw as f64 * z) / 2.0, (h - ch as f64 * z) / 2.0));
        self.fitted.set(true);
        self.refresh();
    }

    /// Zooms to `z`, keeping the point of the widget at `at` where it is.
    fn zoom_to(&self, z: f64, at: (f64, f64)) {
        let z = z.clamp(0.02, 32.0);
        let old = self.zoom.get();
        let (ox, oy) = self.off.get();
        self.off
            .set((at.0 - (at.0 - ox) * z / old, at.1 - (at.1 - oy) * z / old));
        self.zoom.set(z);
        self.refresh();
    }

    fn centre(&self) -> (f64, f64) {
        (
            self.canvas.width() as f64 / 2.0,
            self.canvas.height() as f64 / 2.0,
        )
    }

    /// Zooms so that a point of the picture is in the middle at `z`.
    fn look_at(&self, p: Point, z: f64) {
        self.zoom.set(z);
        let m = self.margin();
        let (cx, cy) = self.centre();
        self.off.set((cx - (p.0 + m) * z, cy - (p.1 + m) * z));
        self.refresh();
    }

    // ---- Tools ----

    fn select_tool(&self, tool: Tool) {
        self.tool.set(tool);
        for (t, b) in self.tool_buttons.borrow().iter() {
            b.set_active(*t == tool);
        }
        if tool != Tool::Select {
            self.selected.set(None);
        }
        self.canvas.set_cursor_from_name(Some(match tool {
            Tool::Select => "default",
            Tool::Text => "text",
            Tool::Picker => "cell",
            _ => "crosshair",
        }));
        self.sync_inspector();
        self.canvas.queue_draw();
    }

    /// The style panel shows the chosen mark's style, or the tool's.
    fn sync_inspector(&self) {
        let doc = self.doc.borrow();
        let (kind, style, title) = match self.selected.get().and_then(|i| doc.marks.get(i)) {
            Some(mark) => (Some(mark.kind), mark.style.clone(), Tool::of(mark.kind).name()),
            None => {
                let tool = self.tool.get();
                drop(doc);
                let style = self.style_for(tool);
                return self.show_inspector(tool.kind(), &style, tool.name());
            }
        };
        drop(doc);
        self.show_inspector(
            kind,
            &style,
            if kind == Some(Kind::Picture) {
                "圖片"
            } else {
                title
            },
        );
    }

    fn show_inspector(&self, kind: Option<Kind>, style: &Style, title: &str) {
        self.syncing.set(true);
        let i = &self.insp;
        i.title.set_label(title);
        let has = |names: &[Kind]| kind.is_some_and(|k| names.contains(&k));
        let show = |name: &str, on: bool| {
            if let Some(w) = i.rows.get(name) {
                w.set_visible(on);
            }
        };
        let drawn = [
            Kind::Arrow,
            Kind::Rect,
            Kind::Ellipse,
            Kind::Line,
            Kind::Pen,
            Kind::Measure,
            Kind::Magnifier,
        ];
        show(
            "colour",
            kind.is_some_and(|k| !matches!(k, Kind::Censor | Kind::Spotlight | Kind::Picture)),
        );
        show("width", has(&drawn));
        show("arrow", has(&[Kind::Arrow]));
        show(
            "fill",
            has(&[Kind::Rect, Kind::Ellipse, Kind::Text, Kind::Spotlight]),
        );
        show(
            "shadow",
            has(&[
                Kind::Arrow,
                Kind::Rect,
                Kind::Ellipse,
                Kind::Line,
                Kind::Pen,
                Kind::Step,
                Kind::Text,
                Kind::Magnifier,
            ]),
        );
        show(
            "sketchy",
            has(&[Kind::Arrow, Kind::Rect, Kind::Ellipse, Kind::Line, Kind::Pen]),
        );
        show("size", has(&[Kind::Text, Kind::Step, Kind::Magnifier]));
        show("censor", has(&[Kind::Censor]));
        show("text_only", has(&[Kind::Censor]));
        show("strength", has(&[Kind::Censor]));
        show("dim", has(&[Kind::Spotlight]));
        show("zoom", has(&[Kind::Magnifier]));
        show("opacity", has(&[Kind::Picture]));
        show("step_start", has(&[Kind::Step]));
        show("none", kind.is_none());
        let s = self.doc.borrow().scale;
        i.custom.set_rgba(&rgba(style.color));
        i.width.set_value(style.width / s);
        i.arrow.set_selected(match style.arrow {
            ArrowStyle::Straight => 0,
            ArrowStyle::Thin => 1,
            ArrowStyle::Curved => 2,
        });
        i.fill.set_active(style.fill);
        i.shadow.set_active(style.shadow);
        i.sketchy.set_active(style.sketchy);
        i.size.set_value(style.size / s);
        i.censor.set_selected(match style.censor {
            Censor::Pixelate => 0,
            Censor::Blur => 1,
            Censor::Erase => 2,
        });
        i.text_only.set_active(style.text_only);
        i.strength.set_value(style.strength);
        i.dim.set_value(style.dim * 100.0);
        i.zoom.set_value(style.zoom);
        i.opacity.set_value(style.opacity * 100.0);
        i.step_start.set_value(self.doc.borrow().step_start as f64);
        // The fill switch means a pill under text and an oval for a spotlight.
        if let Some(fill) = i.rows.get("fill") {
            if let Some(label) = fill.first_child().and_downcast::<gtk::Label>() {
                label.set_label(match kind {
                    Some(Kind::Text) => "底色",
                    Some(Kind::Spotlight) => "橢圓",
                    _ => "填滿",
                });
            }
        }
        self.syncing.set(false);
    }

    /// A change made in the style panel: to the tool's style, and the chosen mark's.
    fn restyle(&self, change: impl Fn(&mut Style, f64)) {
        if self.syncing.get() {
            return;
        }
        let s = self.doc.borrow().scale;
        let tool = match self
            .selected
            .get()
            .and_then(|i| self.doc.borrow().marks.get(i).map(|m| Tool::of(m.kind)))
        {
            Some(tool) => tool,
            None => self.tool.get(),
        };
        let mut style = self.style_for(tool);
        change(&mut style, s);
        self.styles.borrow_mut().insert(tool, style);
        if let Some(i) = self.selected.get() {
            self.push_style_undo();
            if let Some(mark) = self.doc.borrow_mut().marks.get_mut(i) {
                change(&mut mark.style, s);
                mark.shift(0.0, 0.0);
            }
        }
        self.canvas.queue_draw();
    }

    fn wire_inspector(self: &Rc<Self>) {
        let i = &self.insp;
        let mut child = i.colours.first_child();
        let mut index = 0;
        while let Some(button) = child {
            let colour = SWATCHES[index];
            let ed = self.clone();
            if let Some(button) = button.downcast_ref::<gtk::Button>() {
                button.connect_clicked(move |_| {
                    ed.restyle(move |style, _| style.color = colour);
                    ed.sync_inspector();
                });
            }
            index += 1;
            child = button.next_sibling();
            if index >= SWATCHES.len() {
                break;
            }
        }
        let ed = self.clone();
        i.custom.connect_rgba_notify(move |b| {
            let c = b.rgba();
            let colour = [
                c.red() as f64,
                c.green() as f64,
                c.blue() as f64,
                c.alpha() as f64,
            ];
            ed.restyle(move |style, _| style.color = colour);
        });
        let ed = self.clone();
        i.width.connect_value_changed(move |w| {
            let v = w.value();
            ed.restyle(move |style, s| style.width = v * s);
        });
        let ed = self.clone();
        i.arrow.connect_selected_notify(move |d| {
            let a =
                [ArrowStyle::Straight, ArrowStyle::Thin, ArrowStyle::Curved][d.selected().min(2) as usize];
            ed.restyle(move |style, _| style.arrow = a);
        });
        for (switch, set) in [
            (&i.fill, (|s: &mut Style, v| s.fill = v) as fn(&mut Style, bool)),
            (&i.shadow, |s: &mut Style, v| s.shadow = v),
            (&i.sketchy, |s: &mut Style, v| s.sketchy = v),
            (&i.text_only, |s: &mut Style, v| s.text_only = v),
        ] {
            let ed = self.clone();
            switch.connect_active_notify(move |sw| {
                let v = sw.is_active();
                ed.restyle(move |style, _| set(style, v));
            });
        }
        let ed = self.clone();
        i.size.connect_value_changed(move |w| {
            let v = w.value();
            ed.restyle(move |style, s| style.size = v * s);
        });
        let ed = self.clone();
        i.censor.connect_selected_notify(move |d| {
            let c = [Censor::Pixelate, Censor::Blur, Censor::Erase][d.selected().min(2) as usize];
            ed.restyle(move |style, _| style.censor = c);
        });
        let ed = self.clone();
        i.strength.connect_value_changed(move |w| {
            let v = w.value();
            ed.restyle(move |style, _| style.strength = v);
        });
        let ed = self.clone();
        i.dim.connect_value_changed(move |w| {
            let v = w.value() / 100.0;
            ed.restyle(move |style, _| style.dim = v);
        });
        let ed = self.clone();
        i.zoom.connect_value_changed(move |w| {
            let v = w.value();
            ed.restyle(move |style, _| style.zoom = v);
        });
        let ed = self.clone();
        i.opacity.connect_value_changed(move |w| {
            let v = w.value() / 100.0;
            ed.restyle(move |style, _| style.opacity = v);
        });
        let ed = self.clone();
        i.step_start.connect_value_changed(move |w| {
            if ed.syncing.get() {
                return;
            }
            ed.push_style_undo();
            ed.doc.borrow_mut().step_start = w.value() as i32;
            ed.canvas.queue_draw();
        });
    }

    // ---- The canvas ----

    fn wire_canvas(self: &Rc<Self>) {
        let ed = Rc::downgrade(self);
        self.canvas.set_draw_func(move |_, cr, w, h| {
            if let Some(ed) = ed.upgrade() {
                ed.draw(cr, w as f64, h as f64);
            }
        });
        let ed = self.clone();
        self.canvas.connect_resize(move |_, _, _| {
            if !ed.fitted.get() {
                ed.fit();
            }
        });

        let primary = gtk::GestureDrag::new();
        primary.set_button(gdk::BUTTON_PRIMARY);
        let ed = self.clone();
        primary.connect_drag_begin(move |g, x, y| {
            let state = g.current_event_state();
            ed.canvas.grab_focus();
            ed.press(x, y, state);
        });
        let ed = self.clone();
        primary.connect_drag_update(move |g, dx, dy| {
            let shift = g.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
            let (x0, y0) = ed.pressed.get();
            ed.drag_to(x0 + dx, y0 + dy, shift);
        });
        let ed = self.clone();
        primary.connect_drag_end(move |_, dx, dy| ed.release(dx.abs() + dy.abs() < 3.0));
        self.canvas.add_controller(primary);

        // Panning with the right button.
        let pan = gtk::GestureDrag::new();
        pan.set_button(gdk::BUTTON_SECONDARY);
        let ed = self.clone();
        pan.connect_drag_begin(move |_, _, _| *ed.drag.borrow_mut() = Drag::Pan(ed.off.get()));
        let ed = self.clone();
        pan.connect_drag_update(move |_, dx, dy| {
            if let Drag::Pan(o) = *ed.drag.borrow() {
                ed.off.set((o.0 + dx, o.1 + dy));
            }
            ed.canvas.queue_draw();
        });
        let ed = self.clone();
        pan.connect_drag_end(move |_, _, _| *ed.drag.borrow_mut() = Drag::None);
        self.canvas.add_controller(pan);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let ed = self.clone();
        scroll.connect_scroll(move |c, dx, dy| {
            let ctrl = c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
            if ctrl {
                let at = ed
                    .pointer
                    .get()
                    .map(|p| ed.to_widget(p))
                    .unwrap_or_else(|| ed.centre());
                ed.zoom_to(ed.zoom.get() * (1.0 - dy * 0.1), at);
            } else {
                let (ox, oy) = ed.off.get();
                ed.off.set((ox - dx * 30.0, oy - dy * 30.0));
                ed.canvas.queue_draw();
            }
            glib::Propagation::Stop
        });
        self.canvas.add_controller(scroll);

        let pinch = gtk::GestureZoom::new();
        let start = Rc::new(Cell::new(1.0));
        let (ed, s) = (self.clone(), start.clone());
        pinch.connect_begin(move |_, _| s.set(ed.zoom.get()));
        let ed = self.clone();
        pinch.connect_scale_changed(move |g, k| {
            let at = g.bounding_box_center().unwrap_or_else(|| ed.centre());
            ed.zoom_to(start.get() * k, at);
        });
        self.canvas.add_controller(pinch);

        let motion = gtk::EventControllerMotion::new();
        let ed = self.clone();
        motion.connect_motion(move |_, x, y| ed.hover(x, y));
        let ed = self.clone();
        motion.connect_leave(move |_| {
            ed.pointer.set(None);
            ed.canvas.queue_draw();
        });
        self.canvas.add_controller(motion);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let ed = self.clone();
        keys.connect_key_pressed(move |_, key, _, state| ed.key(key, state));
        let ed = self.clone();
        keys.connect_key_released(move |_, key, _, _| {
            if key == gdk::Key::space {
                ed.space.set(false);
            }
        });
        self.window.add_controller(keys);
    }

    fn hover(&self, x: f64, y: f64) {
        let p = self.to_image(x, y);
        self.pointer.set(Some(p));
        let doc = self.doc.borrow();
        let inside = p.0 >= 0.0 && p.1 >= 0.0 && p.0 < doc.base.w as f64 && p.1 < doc.base.h as f64;
        if inside {
            let c = doc.base.rgba(p.0 as i32, p.1 as i32);
            let (px, py) = if self.physical.get() {
                (p.0, p.1)
            } else {
                (p.0 / doc.scale, p.1 / doc.scale)
            };
            self.coords
                .set_label(&format!("x {}　y {}", px.floor(), py.floor()));
            self.colour.set_label(&hex(c));
        }
        drop(doc);
        if matches!(self.tool.get(), Tool::Picker) || self.measuring.get().is_some() {
            self.canvas.queue_draw();
        }
    }

    /// The mark under a point, the topmost.
    fn mark_at(&self, p: Point) -> Option<usize> {
        let slack = 6.0 / self.zoom.get();
        self.doc.borrow().marks.iter().rposition(|m| m.hit(p, slack))
    }

    /// The chosen mark's point near `p`: its ends, or an arrow's middle, which bends it.
    fn mark_point_at(&self, p: Point) -> Option<usize> {
        let i = self.selected.get()?;
        let doc = self.doc.borrow();
        let mark = doc.marks.get(i)?;
        if !mark.has_ends() {
            return None;
        }
        let near = 8.0 / self.zoom.get();
        let close = |q: Point| (q.0 - p.0).abs() < near && (q.1 - p.1).abs() < near;
        if let Some(j) = mark.pts.iter().take(2).position(|q| close(*q)) {
            return Some(j);
        }
        if matches!(mark.kind, Kind::Arrow | Kind::Line) {
            let mid = mark_middle(mark);
            if close(mid) {
                return Some(2);
            }
        }
        None
    }

    fn press(self: &Rc<Self>, x: f64, y: f64, state: gdk::ModifierType) {
        self.pressed.set((x, y));
        let p = self.to_image(x, y);
        if self.space.get() {
            *self.drag.borrow_mut() = Drag::Pan(self.off.get());
            return;
        }
        if self.zoom_click.replace(false) {
            self.zoom_to(self.zoom.get() * 2.0, (x, y));
            *self.drag.borrow_mut() = Drag::None;
            return;
        }
        // Clicking where the arrow keys measured puts the measurement on the picture.
        if let Some((a, b)) = self.measuring.take() {
            self.push_undo();
            let style = self.style_for(Tool::Ruler);
            self.doc
                .borrow_mut()
                .marks
                .push(Mark::new(Kind::Measure, vec![a, b], style));
            *self.drag.borrow_mut() = Drag::None;
            self.refresh();
            return;
        }
        let tool = self.tool.get();
        // A chosen mark's handles and the marks themselves come first, whatever the tool.
        if let Some(j) = self.mark_point_at(p) {
            self.push_undo();
            *self.drag.borrow_mut() = Drag::MarkPoint(j);
            return;
        }
        let hit = self.mark_at(p);
        if tool == Tool::Select || (hit.is_some() && hit == self.selected.get()) {
            if let Some(i) = hit {
                self.selected.set(Some(i));
                self.push_undo();
                let pts = self.doc.borrow().marks[i].pts.clone();
                *self.drag.borrow_mut() = Drag::MoveMark(p, pts);
                self.refresh();
                return;
            }
        }
        match tool {
            Tool::Select => {
                self.selected.set(None);
                if state.contains(gdk::ModifierType::CONTROL_MASK) {
                    let area = self.doc.borrow().base.flood_bounds(p.0 as i32, p.1 as i32);
                    self.sel.set(Some(area));
                    *self.drag.borrow_mut() = Drag::None;
                } else if let Some(sel) = self.sel.get() {
                    let near = 8.0 / self.zoom.get();
                    let hs = area_handles(sel);
                    *self.drag.borrow_mut() = match hs
                        .iter()
                        .position(|h| (h.0 - p.0).abs() < near && (h.1 - p.1).abs() < near)
                    {
                        Some(i) => Drag::SelHandle(i, sel),
                        None if area_contains(sel, p) => Drag::MoveSel(p, sel),
                        None => Drag::NewSel(p),
                    };
                } else {
                    *self.drag.borrow_mut() = Drag::NewSel(p);
                }
                self.refresh();
            }
            Tool::Picker => {
                self.copy_colour(self.doc.borrow().base.rgba(p.0 as i32, p.1 as i32));
                *self.drag.borrow_mut() = Drag::None;
            }
            Tool::Text => {
                self.push_undo();
                let style = self.style_for(Tool::Text);
                let mut mark = Mark::new(Kind::Text, vec![p], style);
                mark.text = String::new();
                self.doc.borrow_mut().marks.push(mark);
                let i = self.doc.borrow().marks.len() - 1;
                self.selected.set(Some(i));
                *self.drag.borrow_mut() = Drag::None;
                self.edit_text(i);
            }
            Tool::Step | Tool::Magnifier => {
                self.push_undo();
                let style = self.style_for(tool);
                self.doc.borrow_mut().marks.push(Mark::new(
                    tool.kind().expect("a mark's tool"),
                    vec![p],
                    style,
                ));
                let i = self.doc.borrow().marks.len() - 1;
                self.selected.set(Some(i));
                let pts = vec![p];
                *self.drag.borrow_mut() = Drag::Place(p, pts);
                self.refresh();
            }
            _ => {
                self.push_undo();
                let style = self.style_for(tool);
                let kind = tool.kind().expect("a mark's tool");
                let pts = if kind == Kind::Pen { vec![p] } else { vec![p, p] };
                self.doc.borrow_mut().marks.push(Mark::new(kind, pts, style));
                self.selected.set(Some(self.doc.borrow().marks.len() - 1));
                *self.drag.borrow_mut() = Drag::Create;
            }
        }
    }

    fn drag_to(&self, x: f64, y: f64, shift: bool) {
        let p = self.to_image(x, y);
        let drag = std::mem::replace(&mut *self.drag.borrow_mut(), Drag::None);
        match &drag {
            Drag::Pan(o) => {
                let (x0, y0) = self.pressed.get();
                self.off.set((o.0 + x - x0, o.1 + y - y0));
            }
            Drag::NewSel(a) => self.sel.set(Some(Area::between(*a, p))),
            Drag::MoveSel(a, sel) => {
                let mut s = *sel;
                s.x += (p.0 - a.0).round() as i32;
                s.y += (p.1 - a.1).round() as i32;
                self.sel.set(Some(s));
            }
            Drag::SelHandle(i, s) => {
                let (mut l, mut t, mut r, mut b) =
                    (s.x as f64, s.y as f64, (s.x + s.w) as f64, (s.y + s.h) as f64);
                if matches!(i, 0 | 6 | 7) {
                    l = p.0;
                }
                if matches!(i, 2..=4) {
                    r = p.0;
                }
                if matches!(i, 0..=2) {
                    t = p.1;
                }
                if matches!(i, 4..=6) {
                    b = p.1;
                }
                self.sel.set(Some(Area::between((l, t), (r, b))));
            }
            Drag::Create => {
                if let Some(i) = self.selected.get() {
                    let mut doc = self.doc.borrow_mut();
                    let mark = &mut doc.marks[i];
                    if mark.kind == Kind::Pen {
                        mark.pts.push(p);
                    } else {
                        let a = mark.pts[0];
                        mark.pts[1] = if shift { constrain(mark.kind, a, p) } else { p };
                    }
                    mark.shift(0.0, 0.0);
                }
            }
            Drag::MoveMark(a, pts) | Drag::Place(a, pts) => {
                if let Some(i) = self.selected.get() {
                    let mut doc = self.doc.borrow_mut();
                    let mark = &mut doc.marks[i];
                    let (dx, dy) = (p.0 - a.0, p.1 - a.1);
                    mark.pts = pts.iter().map(|q| (q.0 + dx, q.1 + dy)).collect();
                    mark.shift(0.0, 0.0);
                }
            }
            Drag::MarkPoint(j) => {
                if let Some(i) = self.selected.get() {
                    let mut doc = self.doc.borrow_mut();
                    let mark = &mut doc.marks[i];
                    if *j == 2 && mark.pts.len() < 3 {
                        mark.pts.push(p);
                    } else if *j < mark.pts.len() {
                        let other = mark.pts[1 - j.min(&1)];
                        mark.pts[*j] = if shift && *j < 2 {
                            constrain(mark.kind, other, p)
                        } else {
                            p
                        };
                    }
                    mark.shift(0.0, 0.0);
                }
            }
            Drag::None => {}
        }
        *self.drag.borrow_mut() = drag;
        self.canvas.queue_draw();
    }

    fn release(&self, click: bool) {
        let drag = std::mem::replace(&mut *self.drag.borrow_mut(), Drag::None);
        match drag {
            Drag::NewSel(_) if click => self.sel.set(None),
            Drag::Create if click => {
                // A click with a drawing tool: nothing to draw, except the ruler, which
                // measures what is under it.
                if let Some(i) = self.selected.take() {
                    self.doc.borrow_mut().marks.remove(i);
                    self.undo.borrow_mut().pop();
                }
                if self.tool.get() == Tool::Ruler {
                    let (x, y) = self.pressed.get();
                    let p = self.to_image(x, y);
                    if let Some(m) = self.auto_measure(p, false) {
                        self.push_undo();
                        let style = self.style_for(Tool::Ruler);
                        self.doc
                            .borrow_mut()
                            .marks
                            .push(Mark::new(Kind::Measure, vec![m.0, m.1], style));
                    }
                }
            }
            Drag::MoveMark(_, _) | Drag::MarkPoint(_) if click => {
                // Nothing moved: no step to take back.
                self.undo.borrow_mut().pop();
            }
            _ => {}
        }
        self.refresh();
    }

    /// The stretch of one colour under a point, across or down, as a measurement's ends.
    fn auto_measure(&self, p: Point, vertical: bool) -> Option<(Point, Point)> {
        let doc = self.doc.borrow();
        let (x, y) = (p.0 as i32, p.1 as i32);
        if x < 0 || y < 0 || x >= doc.base.w || y >= doc.base.h {
            return None;
        }
        let (a, b) = doc.base.span(x, y, vertical);
        Some(if vertical {
            ((x as f64 + 0.5, a as f64), (x as f64 + 0.5, b as f64))
        } else {
            ((a as f64, y as f64 + 0.5), (b as f64, y as f64 + 0.5))
        })
    }

    fn edit_text(self: &Rc<Self>, i: usize) {
        let Some((at, text, size)) = self
            .doc
            .borrow()
            .marks
            .get(i)
            .map(|m| (m.pts[0], m.text.clone(), m.style.size))
        else {
            return;
        };
        let (x, y) = self.to_widget(at);
        let entry = gtk::Entry::builder().text(text).width_chars(24).build();
        let popover = gtk::Popover::builder()
            .child(&entry)
            .pointing_to(&gdk::Rectangle::new(
                x as i32,
                y as i32,
                1,
                (size * self.zoom.get()) as i32,
            ))
            .position(gtk::PositionType::Bottom)
            .build();
        popover.set_parent(&self.canvas);
        let ed = self.clone();
        entry.connect_changed(move |e| {
            if let Some(mark) = ed.doc.borrow_mut().marks.get_mut(i) {
                mark.text = e.text().to_string();
            }
            ed.canvas.queue_draw();
        });
        let p = popover.clone();
        entry.connect_activate(move |_| p.popdown());
        let ed = self.clone();
        popover.connect_closed(move |p| {
            let empty = ed
                .doc
                .borrow()
                .marks
                .get(i)
                .is_some_and(|m| m.kind == Kind::Text && m.text.trim().is_empty());
            if empty {
                ed.doc.borrow_mut().marks.remove(i);
                ed.selected.set(None);
            }
            p.unparent();
            ed.refresh();
            ed.canvas.grab_focus();
        });
        popover.popup();
        entry.grab_focus();
    }

    fn key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        // Typing into a field is left to the field.
        if gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::Entry>() || w.is::<gtk::SpinButton>())
        {
            return glib::Propagation::Proceed;
        }
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        if ctrl || state.contains(gdk::ModifierType::ALT_MASK) {
            return glib::Propagation::Proceed;
        }
        let step = if shift { 10.0 } else { 1.0 };
        match key {
            gdk::Key::Escape => {
                if self.measuring.take().is_none()
                    && self.selected.take().is_none()
                    && self.sel.take().is_none()
                {
                    self.select_tool(Tool::Select);
                }
            }
            gdk::Key::Delete | gdk::Key::BackSpace => {
                if let Some(i) = self.selected.take() {
                    self.push_undo();
                    self.doc.borrow_mut().marks.remove(i);
                }
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                if let Some(i) = self
                    .selected
                    .get()
                    .filter(|i| self.doc.borrow().marks[*i].kind == Kind::Text)
                {
                    self.edit_text(i);
                } else {
                    self.crop();
                }
            }
            gdk::Key::a | gdk::Key::A => {
                if let Some(sel) = self.sel.get() {
                    self.sel.set(Some(self.doc.borrow().base.trim(sel)));
                }
            }
            gdk::Key::c | gdk::Key::C => {
                if let Some(sel) = self.sel.get() {
                    let c = self.doc.borrow().base.average(sel);
                    self.copy_colour(c);
                }
            }
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                if let Some(p) = self.pointer.get() {
                    let base = self.doc.borrow().base.clone();
                    let c = if shift || key == gdk::Key::ISO_Left_Tab {
                        base.darkest_near(p.0 as i32, p.1 as i32)
                    } else {
                        base.rgba(p.0 as i32, p.1 as i32)
                    };
                    self.copy_colour(c);
                }
            }
            gdk::Key::z | gdk::Key::Z => self.zoom_click.set(true),
            gdk::Key::q | gdk::Key::Q | gdk::Key::w | gdk::Key::W => {
                if let Some(s) = self.sel.get() {
                    let corner = if matches!(key, gdk::Key::q | gdk::Key::Q) {
                        (s.x as f64, s.y as f64)
                    } else {
                        ((s.x + s.w) as f64, (s.y + s.h) as f64)
                    };
                    self.look_at(corner, 4.0 / self.doc.borrow().scale);
                }
            }
            gdk::Key::space => self.space.set(true),
            gdk::Key::bracketleft | gdk::Key::bracketright | gdk::Key::braceleft | gdk::Key::braceright => {
                if let Some(s) = self.sel.get() {
                    let g = if matches!(key, gdk::Key::bracketright | gdk::Key::braceright) {
                        step
                    } else {
                        -step
                    } as i32;
                    self.sel.set(Some(Area::new(
                        s.x - g,
                        s.y - g,
                        (s.w + g * 2).max(1),
                        (s.h + g * 2).max(1),
                    )));
                }
            }
            gdk::Key::Left | gdk::Key::Right | gdk::Key::Up | gdk::Key::Down => {
                let (dx, dy) = match key {
                    gdk::Key::Left => (-step, 0.0),
                    gdk::Key::Right => (step, 0.0),
                    gdk::Key::Up => (0.0, -step),
                    _ => (0.0, step),
                };
                let s = self.doc.borrow().scale;
                if let Some(i) = self.selected.get() {
                    self.push_style_undo();
                    self.doc.borrow_mut().marks[i].shift(dx * s, dy * s);
                } else if let Some(a) = self.sel.get() {
                    self.sel.set(Some(Area::new(
                        a.x + (dx * s) as i32,
                        a.y + (dy * s) as i32,
                        a.w,
                        a.h,
                    )));
                } else if let Some(p) = self.pointer.get() {
                    // Up and down measure the height under the pointer, left and right its width.
                    self.measuring.set(self.auto_measure(p, dy != 0.0));
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        self.refresh();
        glib::Propagation::Stop
    }

    /// Copies a colour as its hex, and shows it in the other forms.
    fn copy_colour(&self, c: [u8; 4]) {
        let text = hex(c);
        if let Err(err) = output::copy_text(&text) {
            return self.say(&err);
        }
        let (h, s, l) = hsl(c);
        let (ol, oc, oh) = oklch(c);
        self.say(&format!(
            "已複製 {text}　rgb({} {} {})　hsl({:.0} {:.0}% {:.0}%)　oklch({:.2} {:.3} {:.0})　APCA 白底 Lc {:.0}　黑底 Lc {:.0}",
            c[0],
            c[1],
            c[2],
            h,
            s,
            l,
            ol,
            oc,
            oh,
            apca(c, [255, 255, 255, 255]),
            apca(c, [0, 0, 0, 255]),
        ));
    }

    // ---- Drawing ----

    fn draw(&self, cr: &cairo::Context, w: f64, h: f64) {
        let dark = adw::StyleManager::default().is_dark();
        let g = if dark { 0.13 } else { 0.86 };
        cr.set_source_rgb(g, g, g);
        let _ = cr.paint();
        let doc = self.doc.borrow();
        let ptr = Rc::as_ptr(&doc.base);
        if self.surface.borrow().as_ref().is_none_or(|(p, _)| *p != ptr) {
            *self.surface.borrow_mut() = Some((ptr, doc.base.surface()));
        }
        let surface = self.surface.borrow();
        let (_, base) = surface.as_ref().expect("the picture's surface");
        let (ox, oy) = self.off.get();
        let z = self.zoom.get();
        let (cw, ch) = doc.canvas_size();
        cr.save().ok();
        cr.translate(ox, oy);
        cr.scale(z, z);
        // The picture's edge, as a sheet on the canvas.
        cr.rectangle(-1.0 / z, -1.0 / z, cw as f64 + 2.0 / z, ch as f64 + 2.0 / z);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
        let _ = cr.fill();
        if z * doc.scale >= 2.0 {
            cr.set_antialias(cairo::Antialias::None);
        }
        marks::draw(cr, &doc, base, false);
        cr.restore().ok();
        let m = doc.margin();
        drop(surface);

        // What is chosen, over the picture, at the screen's size.
        let blue = (0.21, 0.52, 0.89);
        let to_w = |p: Point| ((p.0 + m) * z + ox, (p.1 + m) * z + oy);
        if let Some(sel) = self.sel.get() {
            let (x0, y0) = to_w((sel.x as f64, sel.y as f64));
            let (x1, y1) = to_w(((sel.x + sel.w) as f64, (sel.y + sel.h) as f64));
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            cr.rectangle(0.0, 0.0, w, h);
            cr.rectangle(x0, y0, x1 - x0, y1 - y0);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
            let _ = cr.fill();
            cr.set_fill_rule(cairo::FillRule::Winding);
            cr.rectangle(
                x0.floor() + 0.5,
                y0.floor() + 0.5,
                (x1 - x0).round(),
                (y1 - y0).round(),
            );
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_line_width(1.0);
            cr.set_dash(&[4.0, 4.0], 0.0);
            let _ = cr.stroke_preserve();
            cr.set_source_rgb(0.0, 0.0, 0.0);
            cr.set_dash(&[4.0, 4.0], 4.0);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
            for hp in area_handles(sel) {
                handle(cr, to_w(hp), blue);
            }
            let (sw, sh) = if self.physical.get() {
                (sel.w as f64, sel.h as f64)
            } else {
                (sel.w as f64 / doc.scale, sel.h as f64 / doc.scale)
            };
            marks::chip(
                cr,
                &format!("{} × {}", sw.round(), sh.round()),
                (x0 + 36.0, (y0 - 14.0).max(12.0)),
                11.0,
            );
        }
        if let Some(mark) = self.selected.get().and_then(|i| doc.marks.get(i)) {
            let (x, y, bw, bh) = mark.bounds();
            let (x0, y0) = to_w((x, y));
            let (x1, y1) = to_w((x + bw, y + bh));
            cr.rectangle(x0 - 3.0, y0 - 3.0, x1 - x0 + 6.0, y1 - y0 + 6.0);
            cr.set_source_rgba(blue.0, blue.1, blue.2, 0.9);
            cr.set_line_width(1.0);
            cr.set_dash(&[3.0, 3.0], 0.0);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
            if mark.has_ends() {
                for p in mark.pts.iter().take(2) {
                    handle(cr, to_w(*p), blue);
                }
                if matches!(mark.kind, Kind::Arrow | Kind::Line) {
                    let (mx, my) = to_w(mark_middle(mark));
                    cr.arc(mx, my, 4.0, 0.0, std::f64::consts::TAU);
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgb(blue.0, blue.1, blue.2);
                    let _ = cr.stroke();
                }
            }
        }
        if let Some((a, b)) = self.measuring.get() {
            let (ax, ay) = to_w(a);
            let (bx, by) = to_w(b);
            cr.move_to(ax, ay);
            cr.line_to(bx, by);
            cr.set_source_rgb(0.9, 0.28, 0.3);
            cr.set_line_width(1.5);
            let _ = cr.stroke();
            let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
            let len = if self.physical.get() { len } else { len / doc.scale };
            marks::chip(
                cr,
                &format!("{} px　點一下標上", len.round()),
                ((ax + bx) / 2.0, (ay + by) / 2.0 - 16.0),
                11.0,
            );
        }
        if self.tool.get() == Tool::Picker {
            if let Some(p) = self.pointer.get() {
                let (px, py) = to_w(p);
                let c = doc.base.rgba(p.0 as i32, p.1 as i32);
                cr.arc(px + 30.0, py + 30.0, 16.0, 0.0, std::f64::consts::TAU);
                cr.set_source_rgb(c[0] as f64 / 255.0, c[1] as f64 / 255.0, c[2] as f64 / 255.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.set_line_width(2.0);
                let _ = cr.stroke();
                marks::chip(cr, &hex(c), (px + 30.0, py + 60.0), 11.0);
            }
        }
    }

    // ---- What changes the picture itself ----

    /// Changes the picture and moves the marks with it.
    fn transform(&self, base: Img, map: &dyn Fn(Point) -> Point, scale: f64) {
        self.push_undo();
        let mut doc = self.doc.borrow_mut();
        doc.base = Rc::new(base);
        for mark in &mut doc.marks {
            mark.map(map, scale);
        }
        drop(doc);
        self.sel.set(None);
        self.selected.set(None);
        self.fitted.set(false);
        self.fit();
    }

    fn crop(&self) {
        let Some(sel) = self.sel.get() else {
            return;
        };
        let doc = self.doc.borrow();
        let sel = sel.clamp(doc.base.w, doc.base.h);
        if sel.is_empty() {
            return;
        }
        let base = doc.base.crop(sel);
        drop(doc);
        let (dx, dy) = (sel.x as f64, sel.y as f64);
        self.transform(base, &|(x, y)| (x - dx, y - dy), 1.0);
    }

    fn turn(&self, quarters: i32) {
        let doc = self.doc.borrow();
        let (w, h) = (doc.base.w as f64, doc.base.h as f64);
        let base = doc.base.turned(quarters);
        drop(doc);
        if quarters.rem_euclid(4) == 1 {
            self.transform(base, &|(x, y)| (h - y, x), 1.0);
        } else {
            self.transform(base, &|(x, y)| (y, w - x), 1.0);
        }
    }

    fn flip(&self, horizontal: bool) {
        let doc = self.doc.borrow();
        let (w, h) = (doc.base.w as f64, doc.base.h as f64);
        let base = doc.base.flipped(horizontal);
        drop(doc);
        if horizontal {
            self.transform(base, &|(x, y)| (w - x, y), 1.0);
        } else {
            self.transform(base, &|(x, y)| (x, h - y), 1.0);
        }
    }

    fn resize(&self, w: i32, h: i32) {
        let doc = self.doc.borrow();
        let (fx, fy) = (w as f64 / doc.base.w as f64, h as f64 / doc.base.h as f64);
        let base = doc.base.scaled(w, h);
        drop(doc);
        self.transform(base, &|(x, y)| (x * fx, y * fy), (fx + fy) / 2.0);
    }

    fn expand(&self, left: i32, top: i32, right: i32, bottom: i32, clear: bool) {
        let doc = self.doc.borrow();
        let fill = if clear {
            [255, 255, 255, 0]
        } else {
            [255, 255, 255, 255]
        };
        let mut base = Img::new(doc.base.w + left + right, doc.base.h + top + bottom, fill);
        base.paste(&doc.base, left, top);
        drop(doc);
        let (dx, dy) = (left as f64, top as f64);
        self.transform(base, &|(x, y)| (x + dx, y + dy), 1.0);
    }

    /// Puts another screenshot beside the picture or under it.
    pub fn combine(&self, other: &Img, below: bool) {
        let doc = self.doc.borrow();
        let gap = (12.0 * doc.scale).round() as i32;
        let (w, h) = if below {
            (doc.base.w.max(other.w), doc.base.h + gap + other.h)
        } else {
            (doc.base.w + gap + other.w, doc.base.h.max(other.h))
        };
        let mut base = Img::new(w, h, [255, 255, 255, 255]);
        base.paste(&doc.base, 0, 0);
        if below {
            base.paste(other, 0, doc.base.h + gap);
        } else {
            base.paste(other, doc.base.w + gap, 0);
        }
        drop(doc);
        self.transform(base, &|p| p, 1.0);
        self.window.present();
    }

    /// Pastes a picture over the screenshot, as a mark of its own.
    fn add_picture(&self, picture: Img) {
        self.push_undo();
        let mut doc = self.doc.borrow_mut();
        let name = format!("{}", glib::monotonic_time());
        let (w, h) = (picture.w as f64, picture.h as f64);
        let k = (doc.base.w as f64 * 0.8 / w)
            .min(doc.base.h as f64 * 0.8 / h)
            .min(1.0);
        let (x, y) = (
            (doc.base.w as f64 - w * k) / 2.0,
            (doc.base.h as f64 - h * k) / 2.0,
        );
        doc.pictures.insert(name.clone(), Rc::new(picture));
        let mut style = Style::new(doc.scale, SWATCHES[0]);
        style.opacity = 0.5;
        let mut mark = Mark::new(Kind::Picture, vec![(x, y), (x + w * k, y + h * k)], style);
        mark.text = name;
        doc.marks.push(mark);
        let i = doc.marks.len() - 1;
        drop(doc);
        self.select_tool(Tool::Select);
        self.selected.set(Some(i));
        self.refresh();
    }

    // ---- Out of 截圖 ----

    fn export(&self) -> Img {
        output::export(&self.doc.borrow(), &self.prefs.borrow())
    }

    fn file_name(&self) -> String {
        let prefs = self.prefs.borrow();
        output::file_name(&prefs.template, &prefs.format)
    }

    fn drag_source(self: &Rc<Self>, widget: &impl IsA<gtk::Widget>) {
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::COPY);
        let ed = Rc::downgrade(self);
        source.connect_prepare(move |_, _, _| {
            let ed = ed.upgrade()?;
            let path = output::scratch(&ed.export(), &ed.file_name().replace(".jpg", ".png"))?;
            let files = gdk::FileList::from_array(&[gio::File::for_path(path)]);
            Some(gdk::ContentProvider::for_value(&files.to_value()))
        });
        widget.add_controller(source);
    }

    fn add_actions(self: &Rc<Self>) {
        let window = &self.window;
        let add = |name: &str, f: Box<dyn Fn(&Rc<Ed>)>| {
            let action = gio::SimpleAction::new(name, None);
            let ed = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(ed) = ed.upgrade() {
                    f(&ed);
                }
            });
            window.add_action(&action);
        };
        add(
            "copy",
            Box::new(|ed| match output::copy_image(&ed.export()) {
                Ok(()) => ed.say("已複製到剪貼簿"),
                Err(err) => ed.say(&err),
            }),
        );
        add(
            "save",
            Box::new(|ed| {
                let prefs = ed.prefs.borrow().clone();
                match output::save(&ed.export(), &prefs) {
                    Ok(path) => ed.say(&format!("已儲存 {}", path.display())),
                    Err(err) => crate::alert::notice(&ed.window, "無法儲存", Some(&err)),
                }
            }),
        );
        add(
            "save-as",
            Box::new(|ed| {
                let dialog = gtk::FileDialog::builder()
                    .title("另存新檔")
                    .initial_name(ed.file_name())
                    .build();
                let folder = ed.prefs.borrow().folder();
                let _ = std::fs::create_dir_all(&folder);
                dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
                let ed2 = ed.clone();
                dialog.save(Some(&ed.window), gio::Cancellable::NONE, move |file| {
                    let Some(path) = file.ok().and_then(|f| f.path()) else {
                        return;
                    };
                    match output::save_to(&ed2.export(), &path) {
                        Ok(()) => ed2.say(&format!("已儲存 {}", path.display())),
                        Err(err) => crate::alert::notice(&ed2.window, "無法儲存", Some(&err)),
                    }
                });
            }),
        );
        add(
            "drag-hint",
            Box::new(|ed| ed.say("從右下角的圖片圖示拖曳到其他應用程式")),
        );
        add("undo", Box::new(|ed| ed.undo(true)));
        add("redo", Box::new(|ed| ed.undo(false)));
        add(
            "delete",
            Box::new(|ed| {
                if let Some(i) = ed.selected.take() {
                    ed.push_undo();
                    ed.doc.borrow_mut().marks.remove(i);
                    ed.refresh();
                }
            }),
        );
        add(
            "paste",
            Box::new(|ed| match output::clipboard_image() {
                Some(picture) => ed.add_picture(picture),
                None => ed.say("剪貼簿沒有圖片"),
            }),
        );
        add(
            "open-picture",
            Box::new(|ed| {
                let dialog = gtk::FileDialog::builder().title("開啟圖片").build();
                let ed2 = ed.clone();
                dialog.open(Some(&ed.window), gio::Cancellable::NONE, move |file| {
                    if let Some(path) = file.ok().and_then(|f| f.path()) {
                        match Img::load(&path) {
                            Ok(picture) => ed2.add_picture(picture),
                            Err(err) => crate::alert::notice(&ed2.window, "無法開啟圖片", Some(&err)),
                        }
                    }
                });
            }),
        );
        add("before-after", Box::new(|ed| ed.before_after()));
        add(
            "zoom-in",
            Box::new(|ed| ed.zoom_to(ed.zoom.get() * 1.25, ed.centre())),
        );
        add(
            "zoom-out",
            Box::new(|ed| ed.zoom_to(ed.zoom.get() / 1.25, ed.centre())),
        );
        add(
            "zoom-100",
            Box::new(|ed| {
                let z = 1.0 / ed.doc.borrow().scale;
                ed.zoom_to(z, ed.centre());
            }),
        );
        add("zoom-fit", Box::new(|ed| ed.fit()));
        add(
            "zoom-sel",
            Box::new(|ed| {
                if let Some(s) = ed.sel.get() {
                    let z = (ed.canvas.width() as f64 * 0.8 / s.w.max(1) as f64)
                        .min(ed.canvas.height() as f64 * 0.8 / s.h.max(1) as f64);
                    ed.look_at((s.x as f64 + s.w as f64 / 2.0, s.y as f64 + s.h as f64 / 2.0), z);
                }
            }),
        );
        add("crop", Box::new(|ed| ed.crop()));
        add("turn-left", Box::new(|ed| ed.turn(3)));
        add("turn-right", Box::new(|ed| ed.turn(1)));
        add("flip-h", Box::new(|ed| ed.flip(true)));
        add("flip-v", Box::new(|ed| ed.flip(false)));
        add("resize", Box::new(|ed| ed.resize_dialog()));
        add("expand", Box::new(|ed| ed.expand_dialog()));
        add(
            "physical",
            Box::new(|ed| {
                ed.physical.set(!ed.physical.get());
                ed.refresh();
            }),
        );
        add("pin", Box::new(|ed| output::pin(&ed.export())));
        add("upload", Box::new(|ed| ed.upload()));
        add("ocr", Box::new(|ed| ed.recognize()));
        let capture = gio::SimpleAction::new("add-capture", Some(glib::VariantTy::STRING));
        let ed = Rc::downgrade(self);
        capture.connect_activate(move |_, arg| {
            let below = arg.and_then(|a| a.get::<String>()).as_deref() == Some("below");
            if let Some(ed) = ed.upgrade() {
                ed.add_capture(below);
            }
        });
        window.add_action(&capture);

        // The size button's menu.
        let size_menu = gio::Menu::new();
        let size = gio::Menu::new();
        size.append(Some("調整大小…"), Some("win.resize"));
        size.append(Some("畫布擴張…"), Some("win.expand"));
        size.append(Some("裁切到選取範圍"), Some("win.crop"));
        size_menu.append_section(None, &size);
        let turn = gio::Menu::new();
        turn.append(Some("向左旋轉"), Some("win.turn-left"));
        turn.append(Some("向右旋轉"), Some("win.turn-right"));
        turn.append(Some("水平翻轉"), Some("win.flip-h"));
        turn.append(Some("垂直翻轉"), Some("win.flip-v"));
        size_menu.append_section(None, &turn);
        let units = gio::Menu::new();
        units.append(Some("切換邏輯／實體像素"), Some("win.physical"));
        size_menu.append_section(None, &units);
        self.size_button.set_menu_model(Some(&size_menu));
    }

    fn add_capture(self: &Rc<Self>, below: bool) {
        self.window.set_visible(false);
        let ed = self.clone();
        // Once the window is off the screen.
        glib::timeout_add_local_once(Duration::from_millis(350), move || {
            let app = ed.window.application().expect("the application");
            super::capture_into(&app, move |img| match img {
                Some(img) => ed.combine(&img, below),
                None => ed.window.present(),
            });
        });
    }

    fn before_after(&self) {
        let doc = self.doc.borrow();
        if !doc.marks.iter().any(|m| m.kind == Kind::Picture) {
            drop(doc);
            return self.say("先貼上要對照的圖片");
        }
        let before = doc.flatten(true);
        let after = doc.flatten(false);
        drop(doc);
        let dir = glib::user_cache_dir().join("nekoland/shot/frames");
        let _ = std::fs::create_dir_all(&dir);
        let (a, b) = (dir.join("1.png"), dir.join("2.png"));
        if std::fs::write(&a, before.png())
            .and_then(|_| std::fs::write(&b, after.png()))
            .is_err()
        {
            return self.say("無法寫入暫存檔");
        }
        let prefs = self.prefs.borrow().clone();
        let folder = prefs.folder();
        let _ = std::fs::create_dir_all(&folder);
        let name = output::file_name(&prefs.template, "gif");
        let out = output::free_path(&folder, &name);
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-framerate", "1", "-i"])
            .arg(dir.join("%d.png"))
            .args([
                "-vf",
                "split[a][b];[a]palettegen[p];[b][p]paletteuse",
                "-loop",
                "0",
            ])
            .arg(&out)
            .status();
        match status {
            Ok(s) if s.success() => self.say(&format!("已儲存 {}", out.display())),
            _ => self.say("無法輸出動畫（需要 ffmpeg）"),
        }
    }

    fn upload(self: &Rc<Self>) {
        let prefs = self.prefs.borrow().clone();
        if !prefs.s3.is_set() {
            let alert = adw::AlertDialog::new(
                Some("未設定上傳位置"),
                Some("在偏好設定的「上傳」填入 S3 相容儲存的位置與金鑰。"),
            );
            alert.add_responses(&[("cancel", "取消"), ("prefs", "偏好設定")]);
            alert.set_default_response(Some("prefs"));
            alert.set_response_appearance("prefs", adw::ResponseAppearance::Suggested);
            let ed = self.clone();
            alert.choose(&self.window, gio::Cancellable::NONE, move |r| {
                if r == "prefs" {
                    super::prefs_dialog(Some(ed.window.upcast_ref()), Some("upload"));
                }
            });
            return;
        }
        let Some(path) = output::scratch(&self.export(), &self.file_name()) else {
            return self.say("無法寫入暫存檔");
        };
        self.say("上傳中…");
        let ed = self.clone();
        glib::spawn_future_local(async move {
            let s3 = prefs.s3.clone();
            let result = gio::spawn_blocking(move || output::upload(&path, &s3))
                .await
                .unwrap_or_else(|_| Err("上傳中斷".into()));
            match result {
                Ok(url) => {
                    let _ = output::copy_text(&url);
                    ed.say(&format!("已複製連結 {url}"));
                }
                Err(err) => {
                    ed.say("");
                    crate::alert::notice(&ed.window, "無法上傳", Some(&err));
                }
            }
        });
    }

    fn recognize(self: &Rc<Self>) {
        let doc = self.doc.borrow();
        let part = match self.sel.get() {
            Some(sel) => doc.base.crop(sel),
            None => (*doc.base).clone(),
        };
        drop(doc);
        let Some(path) = output::scratch(&part, "ocr.png") else {
            return;
        };
        self.say("辨識中…");
        let ed = self.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || output::recognize(&path))
                .await
                .unwrap_or_else(|_| Err("辨識中斷".into()));
            ed.say("");
            match result {
                Ok((text, codes)) => super::show_recognized(&ed.window, &text, &codes),
                Err(err) => crate::alert::notice(&ed.window, "無法辨識文字", Some(&err)),
            }
        });
    }

    fn resize_dialog(self: &Rc<Self>) {
        let doc = self.doc.borrow();
        let (w0, h0, s) = (doc.base.w, doc.base.h, doc.scale);
        drop(doc);
        let physical = self.physical.get();
        let unit = if physical { 1.0 } else { s };
        let width = gtk::SpinButton::with_range(1.0, 20000.0, 1.0);
        width.set_value((w0 as f64 / unit).round());
        let height = gtk::SpinButton::with_range(1.0, 20000.0, 1.0);
        height.set_value((h0 as f64 / unit).round());
        let keep = gtk::Switch::builder()
            .active(true)
            .valign(gtk::Align::Center)
            .build();
        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 8);
        box_.append(&row("寬度", &width));
        box_.append(&row("高度", &height));
        box_.append(&row("等比例", &keep));
        let presets = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        presets.add_css_class("linked");
        for pct in [100, 75, 50, 25] {
            let b = gtk::Button::with_label(&format!("{pct}%"));
            let (w, h) = (width.clone(), height.clone());
            b.connect_clicked(move |_| {
                w.set_value((w0 as f64 / unit * pct as f64 / 100.0).round());
                h.set_value((h0 as f64 / unit * pct as f64 / 100.0).round());
            });
            presets.append(&b);
        }
        box_.append(&presets);
        let busy = Rc::new(Cell::new(false));
        {
            let (h, k, busy) = (height.clone(), keep.clone(), busy.clone());
            width.connect_value_changed(move |w| {
                if k.is_active() && !busy.replace(true) {
                    h.set_value((w.value() * h0 as f64 / w0 as f64).round());
                    busy.set(false);
                }
            });
        }
        {
            let (w, k, busy) = (width.clone(), keep.clone(), busy.clone());
            height.connect_value_changed(move |h| {
                if k.is_active() && !busy.replace(true) {
                    w.set_value((h.value() * w0 as f64 / h0 as f64).round());
                    busy.set(false);
                }
            });
        }
        let alert = adw::AlertDialog::new(Some("調整大小"), None);
        alert.set_extra_child(Some(&box_));
        alert.add_responses(&[("cancel", "取消"), ("do", "調整")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("do"));
        let ed = self.clone();
        alert.choose(&self.window, gio::Cancellable::NONE, move |r| {
            if r == "do" {
                ed.resize(
                    (width.value() * unit).round() as i32,
                    (height.value() * unit).round() as i32,
                );
            }
        });
    }

    fn expand_dialog(self: &Rc<Self>) {
        let s = self.doc.borrow().scale;
        let spin = || {
            let b = gtk::SpinButton::with_range(0.0, 2000.0, 1.0);
            b.set_value(40.0);
            b
        };
        let (top, bottom, left, right) = (spin(), spin(), spin(), spin());
        let clear = gtk::Switch::builder().valign(gtk::Align::Center).build();
        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 8);
        box_.append(&row("上", &top));
        box_.append(&row("下", &bottom));
        box_.append(&row("左", &left));
        box_.append(&row("右", &right));
        box_.append(&row("透明", &clear));
        let alert = adw::AlertDialog::new(Some("畫布擴張"), None);
        alert.set_extra_child(Some(&box_));
        alert.add_responses(&[("cancel", "取消"), ("do", "擴張")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("do"));
        let ed = self.clone();
        alert.choose(&self.window, gio::Cancellable::NONE, move |r| {
            if r == "do" {
                let px = |b: &gtk::SpinButton| (b.value() * s).round() as i32;
                ed.expand(px(&left), px(&top), px(&right), px(&bottom), clear.is_active());
            }
        });
    }

    fn backdrop_button(self: &Rc<Self>) -> gtk::MenuButton {
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
        panel.set_margin_start(6);
        panel.set_margin_end(6);
        panel.set_margin_top(6);
        panel.set_margin_bottom(6);
        panel.set_width_request(240);
        let on = gtk::Switch::builder().valign(gtk::Align::Center).build();
        panel.append(&row("背景", &on));
        let grid = gtk::Grid::builder().row_spacing(6).column_spacing(6).build();
        for (i, (a, b)) in GRADIENTS.iter().enumerate() {
            let button = gtk::Button::builder()
                .width_request(44)
                .height_request(28)
                .build();
            button.add_css_class(&format!("shot-gradient-{i}"));
            let provider = gtk::CssProvider::new();
            provider.load_from_string(&format!(
                ".shot-gradient-{i} {{ background: linear-gradient(135deg, rgb({},{},{}), rgb({},{},{})); border-radius: 6px; }}",
                (a[0] * 255.0) as u8,
                (a[1] * 255.0) as u8,
                (a[2] * 255.0) as u8,
                (b[0] * 255.0) as u8,
                (b[1] * 255.0) as u8,
                (b[2] * 255.0) as u8
            ));
            gtk::style_context_add_provider_for_display(
                &button.display(),
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            let ed = self.clone();
            let on = on.clone();
            button.connect_clicked(move |_| {
                ed.set_backdrop(|b| b.gradient = i);
                on.set_active(true);
            });
            grid.attach(&button, (i % 4) as i32, (i / 4) as i32, 1, 1);
        }
        panel.append(&grid);
        let padding = scale(0.0, 160.0, 1.0);
        padding.set_value(Backdrop::default().padding);
        let radius = scale(0.0, 40.0, 1.0);
        radius.set_value(Backdrop::default().radius);
        let shadow = scale(0.0, 80.0, 1.0);
        shadow.set_value(Backdrop::default().shadow);
        panel.append(&column("邊距", &padding));
        panel.append(&column("圓角", &radius));
        panel.append(&column("陰影", &shadow));
        let ed = self.clone();
        on.connect_active_notify(move |sw| {
            let want = sw.is_active();
            if want != ed.doc.borrow().backdrop.is_some() {
                ed.push_undo();
                ed.doc.borrow_mut().backdrop = want.then(Backdrop::default);
                ed.fitted.set(false);
                ed.fit();
            }
        });
        for (s, set) in [
            (
                &padding,
                (|b: &mut Backdrop, v| b.padding = v) as fn(&mut Backdrop, f64),
            ),
            (&radius, |b: &mut Backdrop, v| b.radius = v),
            (&shadow, |b: &mut Backdrop, v| b.shadow = v),
        ] {
            let ed = self.clone();
            let on = on.clone();
            s.connect_value_changed(move |s| {
                let v = s.value();
                ed.set_backdrop(move |b| set(b, v));
                on.set_active(true);
            });
        }
        gtk::MenuButton::builder()
            .icon_name("shot-backdrop-symbolic")
            .tooltip_text("背景")
            .popover(&gtk::Popover::builder().child(&panel).build())
            .css_classes(["flat"])
            .build()
    }

    fn set_backdrop(&self, change: impl Fn(&mut Backdrop)) {
        self.push_style_undo();
        let mut doc = self.doc.borrow_mut();
        let mut b = doc.backdrop.unwrap_or_default();
        change(&mut b);
        doc.backdrop = Some(b);
        drop(doc);
        self.fit();
    }
}

fn mark_middle(mark: &Mark) -> Point {
    let pts = marks::arrow_points(mark);
    pts[pts.len() / 2]
}

/// Shift keeps a rectangle square, an ellipse round and a line at a multiple of 45°.
fn constrain(kind: Kind, a: Point, p: Point) -> Point {
    let (dx, dy) = (p.0 - a.0, p.1 - a.1);
    match kind {
        Kind::Rect | Kind::Ellipse | Kind::Highlight | Kind::Spotlight | Kind::Censor | Kind::Picture => {
            let d = dx.abs().max(dy.abs());
            (a.0 + d * dx.signum(), a.1 + d * dy.signum())
        }
        _ => {
            let angle = (dy.atan2(dx) / (std::f64::consts::PI / 4.0)).round() * (std::f64::consts::PI / 4.0);
            let len = (dx * dx + dy * dy).sqrt();
            (a.0 + len * angle.cos(), a.1 + len * angle.sin())
        }
    }
}

fn area_handles(a: Area) -> [Point; 8] {
    let (x, y, w, h) = (a.x as f64, a.y as f64, a.w as f64, a.h as f64);
    [
        (x, y),
        (x + w / 2.0, y),
        (x + w, y),
        (x + w, y + h / 2.0),
        (x + w, y + h),
        (x + w / 2.0, y + h),
        (x, y + h),
        (x, y + h / 2.0),
    ]
}

fn area_contains(a: Area, p: Point) -> bool {
    p.0 >= a.x as f64 && p.0 <= (a.x + a.w) as f64 && p.1 >= a.y as f64 && p.1 <= (a.y + a.h) as f64
}

fn handle(cr: &cairo::Context, (x, y): Point, blue: (f64, f64, f64)) {
    cr.arc(x, y, 4.5, 0.0, std::f64::consts::TAU);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(blue.0, blue.1, blue.2);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
}

fn build_inspector() -> Inspector {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
    panel.set_width_request(236);
    panel.set_hexpand(false);
    panel.set_margin_start(14);
    panel.set_margin_end(14);
    panel.set_margin_top(14);
    let title = gtk::Label::builder().xalign(0.0).css_classes(["heading"]).build();
    panel.append(&title);
    let mut rows: HashMap<&'static str, gtk::Widget> = HashMap::new();

    let colours = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    colours.set_homogeneous(true);
    let css = gtk::CssProvider::new();
    let mut rules = String::from(".shot-swatch { min-width: 20px; min-height: 20px; padding: 0; border-radius: 999px; box-shadow: inset 0 0 0 1px alpha(black, .15); }\n");
    for (i, c) in SWATCHES.iter().enumerate() {
        rules += &format!(
            ".shot-swatch-{i} {{ background: rgb({},{},{}); }}\n",
            (c[0] * 255.0) as u8,
            (c[1] * 255.0) as u8,
            (c[2] * 255.0) as u8
        );
        let b = gtk::Button::builder()
            .css_classes(["shot-swatch", &format!("shot-swatch-{i}")])
            .build();
        colours.append(&b);
    }
    css.load_from_string(&rules);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    let custom = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(true).build()));
    let colour_col = gtk::Box::new(gtk::Orientation::Vertical, 6);
    colour_col.append(
        &gtk::Label::builder()
            .label("顏色")
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    colour_col.append(&colours);
    colour_col.append(&row("自訂", &custom));
    panel.append(&colour_col);
    rows.insert("colour", colour_col.upcast());

    let width = scale(1.0, 24.0, 1.0);
    let w = column("粗細", &width);
    panel.append(&w);
    rows.insert("width", w.upcast());

    let arrow = gtk::DropDown::from_strings(&["直線", "細", "曲線"]);
    let a = row("樣式", &arrow);
    panel.append(&a);
    rows.insert("arrow", a.upcast());

    let switch = || gtk::Switch::builder().valign(gtk::Align::Center).build();
    let fill = switch();
    let shadow = switch();
    let sketchy = switch();
    let text_only = switch();
    for (name, label, sw) in [
        ("fill", "填滿", &fill),
        ("shadow", "陰影", &shadow),
        ("sketchy", "手繪風格", &sketchy),
    ] {
        let r = row(label, sw);
        panel.append(&r);
        rows.insert(name, r.upcast());
    }
    let size = scale(6.0, 160.0, 1.0);
    let s = column("大小", &size);
    panel.append(&s);
    rows.insert("size", s.upcast());

    let censor = gtk::DropDown::from_strings(&["像素化", "模糊", "移除"]);
    let c = row("方式", &censor);
    panel.append(&c);
    rows.insert("censor", c.upcast());
    let t = row("僅文字", &text_only);
    panel.append(&t);
    rows.insert("text_only", t.upcast());
    let strength = scale(2.0, 40.0, 1.0);
    let st = column("強度", &strength);
    panel.append(&st);
    rows.insert("strength", st.upcast());

    let dim = scale(10.0, 90.0, 1.0);
    let d = column("暗度", &dim);
    panel.append(&d);
    rows.insert("dim", d.upcast());
    let zoom = scale(1.5, 8.0, 0.5);
    let z = column("倍率", &zoom);
    panel.append(&z);
    rows.insert("zoom", z.upcast());
    let opacity = scale(5.0, 100.0, 1.0);
    let o = column("不透明度", &opacity);
    panel.append(&o);
    rows.insert("opacity", o.upcast());
    let step_start = gtk::SpinButton::with_range(0.0, 999.0, 1.0);
    let ss = row("起始數字", &step_start);
    panel.append(&ss);
    rows.insert("step_start", ss.upcast());
    let none = gtk::Label::builder()
        .label("拖曳框選範圍，Enter 裁切\nA 貼合內容　C 複製平均色\nTab 複製游標下的顏色\n方向鍵量測")
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label", "caption"])
        .build();
    panel.append(&none);
    rows.insert("none", none.upcast());

    Inspector {
        panel,
        title,
        colours,
        custom,
        rows,
        width,
        arrow,
        fill,
        shadow,
        sketchy,
        size,
        censor,
        text_only,
        strength,
        dim,
        zoom,
        opacity,
        step_start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn clipboard(args: &[&str]) -> String {
        let out = std::process::Command::new("wl-paste")
            .args(args)
            .output()
            .expect("wl-paste");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Opens the window in the running session and drives its handlers as the pointer and keys do.
    /// `cargo test -- --ignored the_window` in a session; GTK runs once a process, so alone.
    #[test]
    #[ignore = "needs a Wayland session"]
    fn the_window_marks_crops_and_copies_the_picture() {
        let app = adw::Application::builder()
            .application_id("io.github.koukeneko.Nekoland.ShotTest")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        let failed: Rc<RefCell<Option<String>>> = Rc::default();
        let f = failed.clone();
        app.connect_activate(move |app| {
            if let Err(err) = catch_unwind(AssertUnwindSafe(|| drive(app))) {
                let text = err
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()));
                *f.borrow_mut() = Some(text.unwrap_or_default());
            }
            for window in app.windows() {
                window.destroy();
            }
            app.quit();
        });
        app.run_with_args::<&str>(&[]);
        let err = failed.borrow_mut().take();
        if let Some(err) = err {
            panic!("{err}");
        }
    }

    fn drive(app: &adw::Application) {
        let entry = std::env::temp_dir().join(format!("shot-test-{}", std::process::id()));
        std::fs::create_dir_all(&entry).unwrap();
        let ed = Ed::open(
            app,
            Doc::new(Img::new(400, 300, [255, 255, 255, 255]), 1.0),
            entry.clone(),
            "測試",
        );
        ed.zoom.set(1.0);
        ed.off.set((0.0, 0.0));
        ed.fitted.set(true);
        let none = gdk::ModifierType::empty();
        let count = || ed.doc.borrow().marks.len();
        let kind = |i: usize| ed.doc.borrow().marks[i].kind;

        // Each drawing tool, dragged out side by side.
        let tools = [
            Tool::Arrow,
            Tool::Rect,
            Tool::Ellipse,
            Tool::Line,
            Tool::Pen,
            Tool::Highlight,
            Tool::Censor,
            Tool::Ruler,
        ];
        for (i, tool) in tools.into_iter().enumerate() {
            let x = 20.0 + 45.0 * i as f64;
            ed.select_tool(tool);
            ed.press(x, 20.0, none);
            ed.drag_to(x + 30.0, 120.0, false);
            ed.release(false);
            assert_eq!(count(), i + 1, "{tool:?} drew nothing");
            assert_eq!(kind(i), tool.kind().unwrap());
        }
        // Shift keeps a rectangle square.
        ed.select_tool(Tool::Rect);
        ed.press(20.0, 150.0, none);
        ed.drag_to(60.0, 170.0, true);
        ed.release(false);
        let pts = ed.doc.borrow().marks.last().unwrap().pts.clone();
        assert_eq!((pts[1].0 - pts[0].0).abs(), (pts[1].1 - pts[0].1).abs());
        ed.undo(true);

        // A click puts down a step, and taking it back removes it.
        ed.select_tool(Tool::Step);
        let before = count();
        ed.press(100.0, 200.0, none);
        ed.release(true);
        assert_eq!(count(), before + 1);
        ed.undo(true);
        assert_eq!(count(), before);
        ed.undo(false);
        assert_eq!(count(), before + 1);
        ed.select_tool(Tool::Magnifier);
        ed.press(160.0, 200.0, none);
        ed.release(true);
        ed.select_tool(Tool::Spotlight);
        ed.press(220.0, 180.0, none);
        ed.drag_to(280.0, 230.0, false);
        ed.release(false);
        assert_eq!(kind(count() - 1), Kind::Spotlight);

        // Text typed into the field it opens.
        ed.select_tool(Tool::Text);
        ed.press(300.0, 250.0, none);
        let i = count() - 1;
        ed.doc.borrow_mut().marks[i].text = "標記".into();
        assert_eq!(kind(i), Kind::Text);
        // Clicking away closes its field, as the pointer would.
        let field = GtkWindowExt::focus(&ed.window)
            .and_then(|w| w.ancestor(gtk::Popover::static_type()))
            .and_downcast::<gtk::Popover>();
        field.expect("the text's field").popdown();
        ed.canvas.grab_focus();

        // A click without a drag draws nothing.
        ed.select_tool(Tool::Arrow);
        let before = count();
        ed.press(390.0, 290.0, none);
        ed.release(true);
        assert_eq!(count(), before);

        // Choosing the rectangle by its edge and moving it; Delete takes it away.
        ed.select_tool(Tool::Select);
        let rect = ed.doc.borrow().marks[1].pts.clone();
        ed.press(65.0, 70.0, none);
        assert_eq!(ed.selected.get(), Some(1));
        ed.drag_to(75.0, 80.0, false);
        ed.release(false);
        let moved = ed.doc.borrow().marks[1].pts.clone();
        assert_eq!((moved[0].0 - rect[0].0, moved[0].1 - rect[0].1), (10.0, 10.0));
        ed.key(gdk::Key::Right, gdk::ModifierType::SHIFT_MASK);
        assert_eq!(ed.doc.borrow().marks[1].pts[0].0, moved[0].0 + 10.0);
        let before = count();
        ed.key(gdk::Key::Delete, none);
        assert_eq!(count(), before - 1);
        ed.undo(true);
        assert_eq!(count(), before);

        // The marks are on what is copied.
        let flat = ed.export();
        let [r, g, b, _] = flat.rgba(35, 70);
        assert!(
            r > g + 40 && r > b + 40,
            "no red arrow at its middle: {r} {g} {b}"
        );
        WidgetExt::activate_action(&ed.window, "win.copy", None).unwrap();
        assert!(clipboard(&["--list-types"]).lines().any(|t| t == "image/png"));

        // The colour picker copies the colour under it.
        ed.select_tool(Tool::Picker);
        ed.press(390.0, 290.0, none);
        ed.release(true);
        assert_eq!(clipboard(&["-n"]).to_lowercase(), "#ffffff");

        // Arrow keys measure under the pointer, and a click keeps the measurement.
        ed.selected.set(None);
        ed.pointer.set(Some((200.0, 280.0)));
        ed.key(gdk::Key::Right, none);
        assert!(ed.measuring.get().is_some());
        let before = count();
        ed.press(200.0, 280.0, none);
        ed.release(true);
        assert_eq!(kind(before), Kind::Measure);

        // A pasted picture, and the before and after of it as a GIF.
        ed.add_picture(Img::new(60, 40, [0, 0, 255, 255]));
        assert_eq!(kind(count() - 1), Kind::Picture);
        ed.before_after();
        let said = ed.status.label().to_string();
        let gif = said
            .strip_prefix("已儲存 ")
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("no GIF: {said}"));
        assert!(std::fs::read(&gif).unwrap().starts_with(b"GIF8"));
        std::fs::remove_file(&gif).unwrap();

        // Its history keeps the marks.
        output::keep(&entry, &ed.doc.borrow());
        let back = output::restore(&entry).unwrap();
        assert_eq!(back.marks.len(), ed.doc.borrow().marks.len());

        // The picture's size: background, turning, resizing, combining and cropping.
        ed.set_backdrop(|b| b.padding = 40.0);
        let (w, h) = ed.doc.borrow().canvas_size();
        assert!(w > 400 && h > 300);
        ed.doc.borrow_mut().backdrop = None;
        ed.turn(1);
        assert_eq!((ed.doc.borrow().base.w, ed.doc.borrow().base.h), (300, 400));
        ed.turn(3);
        ed.flip(true);
        ed.resize(200, 150);
        assert_eq!((ed.doc.borrow().base.w, ed.doc.borrow().base.h), (200, 150));
        ed.combine(&Img::new(100, 50, [0, 0, 0, 255]), true);
        assert_eq!(ed.doc.borrow().base.h, 150 + 12 + 50);
        ed.zoom.set(1.0);
        ed.off.set((0.0, 0.0));
        ed.doc.borrow_mut().marks.clear();
        ed.select_tool(Tool::Select);
        ed.selected.set(None);
        ed.press(150.0, 170.0, none);
        ed.drag_to(195.0, 205.0, false);
        ed.release(false);
        assert!(ed.sel.get().is_some());
        ed.key(gdk::Key::Return, none);
        assert_eq!((ed.doc.borrow().base.w, ed.doc.borrow().base.h), (45, 35));

        std::fs::remove_dir_all(&entry).unwrap();
    }
}
