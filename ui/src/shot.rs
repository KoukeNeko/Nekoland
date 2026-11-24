//! 截圖: takes a picture of the screen, an area of it, a window, or a page longer than the
//! screen as it scrolls, and opens it to mark up, copy, save, pin or upload.
//!
//! `nekoland-shot area|tools|window|scroll|ocr|full|repeat|clipboard|delay [N]|open FILE|edit|history|prefs|pin FILE`,
//! `area-copy` and `full-copy` for the clipboard alone, and `record` or `record-area` to record
//! the screen or an area of it, or stop recording. `area` and `window` are the Mac's ⇧⌘4,
//! taken as the button comes up; `tools` is its ⇧⌘5, with the strip of tools.

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod alert;
#[path = "shot/editor.rs"]
mod editor;
#[path = "shot/img.rs"]
mod img;
mod ipc;
#[path = "shot/marks.rs"]
mod marks;
#[path = "shot/output.rs"]
mod output;
#[path = "shot/prefs.rs"]
mod prefs;
#[path = "shot/overlay.rs"]
mod overlay;
#[path = "shot/record.rs"]
mod record;
#[path = "shot/thumbnail.rs"]
mod thumbnail;

use editor::Ed;
use img::Img;
use marks::Doc;
use output::{After, Prefs};
use overlay::{Choice, Mode, Style};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Shot";
/// The compositor keeps this program's windows in front of the others.
const PIN_APP_ID: &str = "io.github.koukeneko.Nekoland.ShotPin";

type Hold = Rc<gio::ApplicationHoldGuard>;

fn css() -> &'static str {
    "
.shot-overlay { background: transparent; }
.shot-strip {
  padding: 6px;
  border-radius: 14px;
  background: alpha(#1e1e22, .82);
  color: white;
  box-shadow: 0 0 0 1px alpha(white, .1), 0 8px 24px alpha(black, .35);
}
.shot-strip separator { background: alpha(white, .15); margin: 4px 4px; }
.shot-strip button.flat:checked { background: alpha(white, .18); }
.shot-pin { background: transparent; }
.shot-thumbnail { background: transparent; }
.shot-thumbnail-picture {
  border-radius: 8px;
  box-shadow: 0 0 0 1px alpha(black, .25), 0 6px 18px alpha(black, .35);
}
.shot-pin picture { box-shadow: 0 0 0 1px alpha(black, .25); }
"
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().cloned().unwrap_or_else(|| "area".into());
    // The screen as the key was pressed, taken while GTK starts.
    if matches!(
        command.as_str(),
        "area" | "area-copy" | "tools" | "window" | "scroll" | "ocr" | "full" | "full-copy" | "record-area"
    ) {
        overlay::grab_early();
    }
    // GTK's Vulkan takes some 200 ms longer to start than its OpenGL, before the screen to
    // choose from can show. A choice already made, such as Nekoland's Cairo without a GPU, is kept.
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "gl");
    }
    let app = adw::Application::builder()
        .application_id(match command.as_str() {
            "pin" => PIN_APP_ID,
            "thumbnail" => thumbnail::APP_ID,
            _ => APP_ID,
        })
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|app| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css());
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        add_app_actions(app);
    });
    app.connect_activate(move |app| run(app, &command, &args[1.min(args.len())..]));
    // The arguments are 截圖's own, not GTK's.
    app.run_with_args::<&str>(&[])
}

fn add_app_actions(app: &adw::Application) {
    let action = |name: &str, f: fn(&adw::Application)| {
        let a = gio::SimpleAction::new(name, None);
        let app2 = app.clone();
        a.connect_activate(move |_, _| f(&app2));
        app.add_action(&a);
    };
    action("history", |app| history_window(app));
    action("prefs", |app| {
        prefs_dialog(app.active_window().as_ref().map(|w| w.upcast_ref()), None);
    });
    for (name, accels) in [
        ("win.copy", &["<Control>c"][..]),
        ("win.save", &["<Control>s"]),
        ("win.save-as", &["<Control><Shift>s"]),
        ("win.paste", &["<Control>v"]),
        ("win.undo", &["<Control>z"]),
        ("win.redo", &["<Control><Shift>z", "<Control>y"]),
        ("win.zoom-in", &["<Control>plus", "<Control>equal"]),
        ("win.zoom-out", &["<Control>minus"]),
        ("win.zoom-fit", &["<Control>0"]),
        ("win.zoom-100", &["<Control>1"]),
        ("win.zoom-sel", &["<Control>2"]),
        ("win.pin", &["<Control>p"]),
        ("win.ocr", &["<Control>t"]),
        ("window.close", &["<Control>w"]),
    ] {
        app.set_accels_for_action(name, accels);
    }
}

fn run(app: &adw::Application, command: &str, rest: &[String]) {
    let hold: Hold = Rc::new(app.hold());
    let path = rest.first().map(PathBuf::from);
    match command {
        "area" => choose(app, Mode::Area, Style::Quick { copy: false }, hold),
        "area-copy" => choose(app, Mode::Area, Style::Quick { copy: true }, hold),
        "tools" => choose(app, Mode::Area, Style::Tools, hold),
        "window" => choose(app, Mode::Window, Style::Quick { copy: false }, hold),
        "scroll" => choose(app, Mode::Scroll, Style::Tools, hold),
        "ocr" => choose(app, Mode::Ocr, Style::Tools, hold),
        "record" | "record-area" if record::is_recording() => record::stop(),
        "record" => record::start(None),
        "record-stop" => record::stop(),
        "record-area" => choose(app, Mode::Record, Style::Tools, hold),
        "full" => match overlay::freeze() {
            Ok(frozen) => taken(app, (*frozen.img).clone(), frozen.scale()),
            Err(err) => failed(&err),
        },
        "full-copy" => match overlay::freeze() {
            Ok(frozen) => copied((*frozen.img).clone(), frozen.scale()),
            Err(err) => failed(&err),
        },
        "repeat" => repeat(app, hold),
        "clipboard" => from_clipboard(app),
        "delay" => {
            let seconds = rest
                .first()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| Prefs::load().delay);
            countdown(app, seconds, hold);
        }
        "open" => match path.as_deref().map(Img::load) {
            Some(Ok(img)) => {
                let entry = output::new_entry(&img);
                let title = path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string());
                Ed::open(app, Doc::new(img, 1.0), entry, title.as_deref().unwrap_or("截圖"));
            }
            Some(Err(err)) => failed(&err),
            None => failed("沒有指定檔案"),
        },
        "edit" => match path
            .as_deref()
            .or(output::history().first().map(|(entry, _)| entry.as_path()))
        {
            Some(entry) => open_entry(app, entry),
            None => history_window(app),
        },
        "thumbnail" => match path {
            Some(entry) => thumbnail::show(app, entry, rest.get(1).map(PathBuf::from)),
            None => failed("沒有指定截圖"),
        },
        "history" => history_window(app),
        "prefs" => {
            let dialog = prefs_dialog(None, None);
            dialog.connect_closed(move |_| {
                let _ = &hold;
            });
        }
        "pin" => match path {
            Some(path) => pin_window(app, &path),
            None => failed("沒有指定檔案"),
        },
        other => eprintln!("nekoland-shot: unknown command {other}"),
    }
}

fn failed(err: &str) {
    eprintln!("nekoland-shot: {err}");
    output::notify("無法截圖", err, None, 5000, None);
}

/// Freezes the screen and lets an area, a window or all of it be chosen from it.
fn choose(app: &adw::Application, mode: Mode, style: Style, hold: Hold) {
    let frozen = match overlay::freeze() {
        Ok(frozen) => Rc::new(frozen),
        Err(err) => return failed(&err),
    };
    let app2 = app.clone();
    let shown = frozen.clone();
    overlay::choose(app.upcast_ref(), frozen, mode, style, move |choice| {
        let app = &app2;
        match choice {
            Choice::Area(Mode::Scroll, area) => scroll(app, area, hold.clone()),
            Choice::Area(Mode::Ocr, area) => read_text(shown.crop(area), hold.clone()),
            Choice::Area(Mode::Record, area) => record::start(Some(area)),
            Choice::Area(_, area) => {
                output::remember_area(area);
                taken(app, shown.crop(area), shown.scale());
            }
            Choice::Copied(area) => copied(shown.crop(area), shown.scale()),
            Choice::Full => taken(app, (*shown.img).clone(), shown.scale()),
            Choice::Delay(seconds) => countdown(app, seconds, hold.clone()),
            Choice::Repeat => repeat(app, hold.clone()),
            Choice::Clipboard => from_clipboard(app),
            Choice::Cancel => {}
        }
    });
}

/// A screenshot for 加入截圖 in a window already open: `None` when nothing was taken.
pub fn capture_into(app: &gtk::Application, done: impl Fn(Option<Img>) + 'static) {
    let frozen = match overlay::freeze() {
        Ok(frozen) => Rc::new(frozen),
        Err(err) => {
            failed(&err);
            return done(None);
        }
    };
    let shown = frozen.clone();
    overlay::choose(
        app,
        frozen,
        Mode::Area,
        Style::Tools,
        move |choice| match choice {
            Choice::Area(_, area) | Choice::Copied(area) => done(Some(shown.crop(area))),
            Choice::Full => done(Some((*shown.img).clone())),
            _ => done(None),
        },
    );
}

fn repeat(app: &adw::Application, hold: Hold) {
    let Some(area) = output::last_area() else {
        return choose(app, Mode::Area, Style::Tools, hold);
    };
    match overlay::grab(Some(area)) {
        Ok(img) => {
            let scale = img.w as f64 / area.2.max(1.0);
            taken(app, img, scale);
        }
        Err(err) => failed(&err),
    }
}

fn from_clipboard(app: &adw::Application) {
    match output::clipboard_image() {
        Some(img) => {
            let entry = output::new_entry(&img);
            Ed::open(app, Doc::new(img, 1.0), entry, "截圖");
        }
        None => output::notify("剪貼簿沒有圖片", "", None, 4000, None),
    }
}

/// Waits, and then lets an area be chosen. The notice goes before the screen is frozen.
fn countdown(app: &adw::Application, seconds: u32, hold: Hold) {
    let seconds = seconds.max(1);
    output::notify(
        &format!("{seconds} 秒後截圖"),
        "",
        None,
        (seconds * 1000).saturating_sub(700).max(800) as i32,
        None,
    );
    let app = app.clone();
    glib::timeout_add_local_once(Duration::from_secs(seconds as u64), move || {
        choose(&app, Mode::Area, Style::Tools, hold)
    });
}

/// What happens to a picture once it is taken, as the preferences say.
fn taken(app: &adw::Application, img: Img, scale: f64) {
    output::shutter();
    let prefs = Prefs::load();
    let entry = output::new_entry(&img);
    let doc = Doc::new(img, scale);
    output::keep(&entry, &doc);
    let flat = output::export(&doc, &prefs);
    let mut done = Vec::new();
    let mut saved = None;
    if prefs.auto_copy {
        match output::copy_image(&flat) {
            Ok(()) => done.push("已複製到剪貼簿".to_string()),
            Err(err) => eprintln!("nekoland-shot: {err}"),
        }
    }
    if prefs.auto_save {
        match output::save(&flat, &prefs) {
            Ok(path) => {
                done.push(format!(
                    "已儲存 {}",
                    path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                ));
                saved = Some(path);
            }
            Err(err) => output::notify("無法儲存", &err, None, 5000, None),
        }
    }
    let summary = done.first().cloned().unwrap_or_else(|| "已截圖".into());
    let body = done.get(1).cloned().unwrap_or_default();
    match prefs.after {
        After::Editor => {
            Ed::open(app, doc, entry, "截圖");
        }
        After::Thumbnail => thumbnail::open(&entry, saved.as_deref()),
        After::Nothing => output::notify(&summary, &body, Some(&flat), 3000, None),
    }
}

/// A picture for the clipboard alone, as Control asks: not kept or saved.
fn copied(img: Img, scale: f64) {
    output::shutter();
    let flat = output::export(&Doc::new(img, scale), &Prefs::load());
    match output::copy_image(&flat) {
        Ok(()) => output::notify("已複製到剪貼簿", "", Some(&flat), 3000, None),
        Err(err) => failed(&err),
    }
}

fn open_entry(app: &adw::Application, entry: &Path) {
    match output::restore(entry) {
        Ok(doc) => {
            Ed::open(app, doc, entry.to_path_buf(), "截圖");
        }
        Err(err) => failed(&err),
    }
}

// ---- Text in a picture ----

/// Reads the text out of an area and copies it.
fn read_text(img: Img, hold: Hold) {
    let Some(path) = output::scratch(&img, "ocr.png") else {
        return failed("無法寫入暫存檔");
    };
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || output::recognize(&path))
            .await
            .unwrap_or_else(|_| Err("辨識中斷".into()));
        match result {
            Ok((text, codes)) => {
                let all: Vec<&str> = codes
                    .iter()
                    .map(String::as_str)
                    .chain((!text.is_empty()).then_some(text.as_str()))
                    .collect();
                if all.is_empty() {
                    output::notify("沒有辨識到文字", "", None, 4000, None);
                } else {
                    let joined = all.join("\n");
                    match output::copy_text(&joined) {
                        Ok(()) => output::notify(
                            "已複製文字",
                            &joined.chars().take(200).collect::<String>(),
                            None,
                            5000,
                            None,
                        ),
                        Err(err) => failed(&err),
                    }
                }
            }
            Err(err) => output::notify("無法辨識文字", &err, None, 5000, None),
        }
        drop(hold);
    });
}

/// Shows what was read out of a picture, to correct and copy.
pub fn show_recognized(parent: &impl IsA<gtk::Widget>, text: &str, codes: &[String]) {
    if text.is_empty() && codes.is_empty() {
        return alert::notice(parent, "沒有辨識到文字", None);
    }
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for code in codes {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&gtk::Image::from_icon_name("view-grid-symbolic"));
        if code.starts_with("http://") || code.starts_with("https://") {
            let link = gtk::LinkButton::with_label(code, code);
            link.set_halign(gtk::Align::Start);
            row.append(&link);
        } else {
            row.append(
                &gtk::Label::builder()
                    .label(code)
                    .selectable(true)
                    .wrap(true)
                    .xalign(0.0)
                    .build(),
            );
        }
        content.append(&row);
    }
    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(text);
    if !text.is_empty() {
        let view = gtk::TextView::builder()
            .buffer(&buffer)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .min_content_height(160)
            .max_content_height(360)
            .propagate_natural_height(true)
            .width_request(alert::FIELD_WIDTH)
            .build();
        scroller.add_css_class("card");
        content.append(&scroller);
    }
    let dialog = adw::AlertDialog::new(Some("辨識結果"), None);
    dialog.set_extra_child(Some(&content));
    dialog.add_responses(&[("close", "關閉"), ("copy", "複製")]);
    dialog.set_response_appearance("copy", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("copy"));
    let codes = codes.to_vec();
    dialog.connect_response(Some("copy"), move |_, _| {
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let all: Vec<String> = codes
            .iter()
            .cloned()
            .chain((!text.is_empty()).then_some(text))
            .collect();
        if let Err(err) = output::copy_text(&all.join("\n")) {
            eprintln!("nekoland-shot: {err}");
        }
    });
    dialog.present(Some(parent));
}

// ---- A page longer than the screen ----

/// How long the area has to stay still for the capture to end.
const SCROLL_IDLE: Duration = Duration::from_millis(2500);
/// The tallest picture a scrolling capture makes, in pixels.
const SCROLL_MAX: i32 = 30000;

/// Takes the area again and again while it is scrolled, and joins what moved into it.
fn scroll(app: &adw::Application, area: (f64, f64, f64, f64), hold: Hold) {
    output::notify("捲動擷取", "停止捲動後完成", None, 1500, None);
    let app = app.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            // Past the notice.
            std::thread::sleep(Duration::from_millis(1600));
            scroll_capture(area)
        })
        .await
        .unwrap_or_else(|_| Err("擷取中斷".into()));
        match result {
            Ok((img, cut)) => {
                if cut {
                    output::notify("捲動過快", "只接到接不上的地方為止", None, 5000, None);
                }
                let scale = img.w as f64 / area.2.max(1.0);
                taken(&app, img, scale);
            }
            Err(err) => failed(&err),
        }
        drop(hold);
    });
}

/// The picture joined, and whether it was cut short where the page moved too far between two
/// frames to be joined.
fn scroll_capture(area: (f64, f64, f64, f64)) -> Result<(Img, bool), String> {
    let first = overlay::grab(Some(area))?;
    let mut result = first.clone();
    // The last frame joined on, and the last one taken: the page has stopped once the frames
    // taken stay the same, whether or not the last of them could be joined.
    let mut joined_last = first.clone();
    let mut previous = first;
    let mut still = Instant::now();
    loop {
        // A grab takes about 150 ms itself; the shorter the wait, the faster a page can be
        // scrolled and still overlap the frame before.
        std::thread::sleep(Duration::from_millis(30));
        let frame = overlay::grab(Some(area))?;
        if frame.px == previous.px {
            if still.elapsed() >= SCROLL_IDLE {
                // Stopped on a frame never joined: the page moved too far between two frames.
                let cut = scrolled_by(&joined_last, &previous).is_none();
                return Ok((result, cut));
            }
            continue;
        }
        still = Instant::now();
        match scrolled_by(&joined_last, &frame) {
            Some(0) => {}
            Some(dy) => {
                let added = frame.crop(img::Area::new(0, frame.h - dy, frame.w, dy));
                let mut joined = Img::new(result.w, result.h + dy, [255, 255, 255, 255]);
                joined.paste(&result, 0, 0);
                joined.paste(&added, 0, result.h);
                result = joined;
                joined_last = frame.clone();
                if result.h >= SCROLL_MAX {
                    break;
                }
            }
            // The page went on from a frame that was never joined: what lay between is lost,
            // and joining later frames would leave a gap no one sees.
            None if scrolled_by(&previous, &frame).is_some_and(|dy| dy > 0) => {
                return Ok((result, true));
            }
            // A mid-scroll frame that matches nothing; the next one will.
            None => {}
        }
        previous = frame;
    }
    Ok((result, false))
}

/// How many rows `next` moved up from `prev`, 0 if it did not move: the shift under which their rows differ the least,
/// if little enough. Faint differences do not count: the row a list lights under a pointer that
/// stays where it is differs a little everywhere, and would outweigh rows that differ only in a
/// digit of their text, so a list would seem to have scrolled whole rows less than it did.
fn scrolled_by(prev: &Img, next: &Img) -> Option<i32> {
    if prev.w != next.w || prev.h != next.h {
        return None;
    }
    // Each row as the brightness of every other pixel.
    let rows = |img: &Img| -> Vec<Vec<u8>> {
        img.px
            .chunks((img.w * 4) as usize)
            .map(|row| {
                row.chunks_exact(8)
                    .map(|p| ((p[0] as u16 + 2 * p[1] as u16 + p[2] as u16) / 4) as u8)
                    .collect()
            })
            .collect()
    };
    let (a, b) = (rows(prev), rows(next));
    let off = |x: &[u8], y: &[u8]| -> u64 {
        x.iter()
            .zip(y)
            .map(|(p, q)| p.abs_diff(*q).saturating_sub(16) as u64)
            .sum()
    };
    let (h, len) = (a.len(), a.first().map_or(1, Vec::len).max(1));
    let mut best: Option<((f64, f64), usize)> = None;
    // Not moving is weighed too: a frame can differ from the last only where a lit row or a
    // scroll bar fades, and the same rows one row further on would otherwise come close.
    // A fast flick can leave only an eighth of the frame in common.
    for dy in 0..h - h / 8 {
        let overlap = h - dy;
        let mut offs: Vec<u64> = (0..overlap).map(|i| off(&b[i], &a[i + dy])).collect();
        offs.sort_unstable();
        // The tenth of rows least alike is left out: a header that stays where it is.
        // Between shifts alike without them, all the rows decide.
        let kept = overlap - overlap / 10;
        let mean = offs[..kept].iter().sum::<u64>() as f64 / (kept * len) as f64;
        let all = offs.iter().sum::<u64>() as f64 / (overlap * len) as f64;
        if best.is_none_or(|(m, _)| (mean, all) < m) {
            best = Some(((mean, all), dy));
        }
    }
    best.filter(|((mean, _), _)| *mean <= 3.0)
        .map(|(_, dy)| dy as i32)
}

// ---- Pinned pictures ----

fn pin_window(app: &adw::Application, path: &Path) {
    let texture = match gdk::Texture::from_filename(path) {
        Ok(texture) => texture,
        Err(err) => return failed(&err.to_string()),
    };
    let scale = gdk::Display::default()
        .and_then(|d| d.monitors().item(0))
        .and_downcast::<gdk::Monitor>()
        .map_or(1, |m| m.scale_factor())
        .max(1) as f64;
    let (w, h) = (texture.width() as f64 / scale, texture.height() as f64 / scale);
    let picture = gtk::Picture::builder()
        .paintable(&texture)
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Fill)
        .build();
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("釘選的截圖")
        .decorated(false)
        .resizable(true)
        .default_width(w.round() as i32)
        .default_height(h.round() as i32)
        .child(&picture)
        .build();
    window.add_css_class("shot-pin");
    let size = Rc::new(Cell::new(1.0f64));

    let press = gtk::GestureClick::new();
    press.set_button(gdk::BUTTON_PRIMARY);
    let win = window.clone();
    let file = path.to_path_buf();
    press.connect_pressed(move |g, n, x, y| {
        if n == 2 {
            open_in_editor(&file);
            return;
        }
        if let (Some(toplevel), Some(device)) = (win.surface().and_downcast::<gdk::Toplevel>(), g.device()) {
            toplevel.begin_move(&device, g.current_button() as i32, x, y, g.current_event_time());
        }
    });
    picture.add_controller(press);

    // Scrolling makes it larger or smaller; with Alt, clearer or fainter.
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let win = window.clone();
    let sz = size.clone();
    scroll.connect_scroll(move |c, _, dy| {
        if c.current_event_state().contains(gdk::ModifierType::ALT_MASK) {
            win.set_opacity((win.opacity() - dy * 0.05).clamp(0.2, 1.0));
        } else {
            let k = (sz.get() * (1.0 - dy * 0.05)).clamp(0.1, 4.0);
            sz.set(k);
            win.set_default_size((w * k).round() as i32, (h * k).round() as i32);
        }
        glib::Propagation::Stop
    });
    window.add_controller(scroll);

    let actions = gio::SimpleActionGroup::new();
    let add = |name: &str, f: Box<dyn Fn()>| {
        let a = gio::SimpleAction::new(name, None);
        a.connect_activate(move |_, _| f());
        actions.add_action(&a);
    };
    let file = path.to_path_buf();
    add(
        "copy",
        Box::new(move || {
            if let Err(err) = Img::load(&file).and_then(|img| output::copy_image(&img)) {
                eprintln!("nekoland-shot: {err}");
            }
        }),
    );
    let file = path.to_path_buf();
    add("edit", Box::new(move || open_in_editor(&file)));
    let (sz, win) = (size.clone(), window.clone());
    add(
        "actual",
        Box::new(move || {
            sz.set(1.0);
            win.set_opacity(1.0);
            win.set_default_size(w.round() as i32, h.round() as i32);
        }),
    );
    let win = window.clone();
    add("close", Box::new(move || win.close()));
    window.insert_action_group("pin", Some(&actions));
    let menu = gio::Menu::new();
    menu.append(Some("複製"), Some("pin.copy"));
    menu.append(Some("在截圖中開啟"), Some("pin.edit"));
    menu.append(Some("實際大小"), Some("pin.actual"));
    menu.append(Some("關閉"), Some("pin.close"));
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(&picture);
    popover.set_has_arrow(false);
    let context = gtk::GestureClick::new();
    context.set_button(gdk::BUTTON_SECONDARY);
    context.connect_pressed(move |_, _, x, y| {
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.popup();
    });
    picture.add_controller(context);

    let keys = gtk::EventControllerKey::new();
    let win = window.clone();
    keys.connect_key_pressed(move |_, key, _, state| {
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        match key {
            gdk::Key::Escape => win.close(),
            gdk::Key::w if ctrl => win.close(),
            gdk::Key::c if ctrl => {
                let _ = gtk::prelude::WidgetExt::activate_action(&win, "pin.copy", None);
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    let file = path.to_path_buf();
    window.connect_destroy(move |_| {
        // Only the copy 釘選 made; a file named on the command line is the user's.
        if file.starts_with(output::scratch_dir()) {
            let _ = std::fs::remove_file(&file);
        }
    });
    window.present();
}

/// Opens a pinned picture in 截圖's window, which is another program: a pin's windows stay in front.
fn open_in_editor(path: &Path) {
    let exe = std::env::current_exe().unwrap_or_else(|_| "nekoland-shot".into());
    // A copy, as the pin's file goes with the pin.
    let copy = output::scratch_dir().join(format!("open-{}.png", glib::monotonic_time()));
    if let Err(err) = std::fs::create_dir_all(output::scratch_dir())
        .and_then(|_| std::fs::copy(path, &copy))
        .and_then(|_| {
            std::process::Command::new(exe)
                .arg("open")
                .arg(&copy)
                .spawn()
                .map(|_| ())
        })
    {
        eprintln!("nekoland-shot: {err}");
    }
}

// ---- The history ----

fn history_window(app: &adw::Application) {
    let entries = output::history();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("截圖紀錄")
        .default_width(760)
        .default_height(560)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    if entries.is_empty() {
        view.set_content(Some(
            &adw::StatusPage::builder()
                .icon_name("shot-area-symbolic")
                .title("沒有截圖")
                .build(),
        ));
    } else {
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(true)
            .homogeneous(true)
            .min_children_per_line(3)
            .max_children_per_line(6)
            .row_spacing(12)
            .column_spacing(12)
            .margin_start(18)
            .margin_end(18)
            .margin_top(18)
            .margin_bottom(18)
            .valign(gtk::Align::Start)
            .build();
        let paths: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
        for (entry, when) in &entries {
            let tile = gtk::Box::new(gtk::Orientation::Vertical, 6);
            let picture = gtk::Picture::builder()
                .can_shrink(true)
                .content_fit(gtk::ContentFit::Contain)
                .width_request(200)
                .height_request(130)
                .build();
            match gdk::Texture::from_filename(entry.join("base.png")) {
                Ok(texture) => picture.set_paintable(Some(&texture)),
                Err(err) => eprintln!("nekoland-shot: {err}"),
            }
            picture.add_css_class("card");
            tile.append(&picture);
            tile.append(
                &gtk::Label::builder()
                    .label(when)
                    .css_classes(["caption", "numeric"])
                    .build(),
            );
            flow.append(&tile);
            paths.borrow_mut().push(entry.clone());
        }
        let app2 = app.clone();
        flow.connect_child_activated(move |_, child| {
            if let Some(entry) = paths.borrow().get(child.index() as usize) {
                open_entry(&app2, entry);
            }
        });
        view.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .child(&flow)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build(),
        ));
    }
    window.set_content(Some(&view));
    window.present();
}

// ---- Preferences ----

/// The preferences; `page` is "upload" to open on 上傳.
pub fn prefs_dialog(parent: Option<&gtk::Widget>, page: Option<&str>) -> adw::PreferencesDialog {
    let prefs = Rc::new(RefCell::new(Prefs::load()));
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("偏好設定");

    let general = adw::PreferencesPage::builder()
        .title("一般")
        .icon_name("preferences-system-symbolic")
        .name("general")
        .build();
    let after_group = adw::PreferencesGroup::builder().title("截圖後").build();
    let after = adw::ComboRow::builder()
        .title("顯示")
        .model(&gtk::StringList::new(&["編輯視窗", "縮圖", "不顯示"]))
        .build();
    after.set_selected(match prefs.borrow().after {
        After::Editor => 0,
        After::Thumbnail => 1,
        After::Nothing => 2,
    });
    let p = prefs.clone();
    after.connect_selected_notify(move |row| {
        p.borrow_mut().after =
            [After::Editor, After::Thumbnail, After::Nothing][row.selected().min(2) as usize];
        p.borrow().save();
    });
    after_group.add(&after);
    let switch = |title: &str, on: bool, set: fn(&mut Prefs, bool)| {
        let row = adw::SwitchRow::builder().title(title).active(on).build();
        let p = prefs.clone();
        row.connect_active_notify(move |row| {
            set(&mut p.borrow_mut(), row.is_active());
            p.borrow().save();
        });
        row
    };
    let pr = prefs.borrow().clone();
    after_group.add(&switch("複製到剪貼簿", pr.auto_copy, |p, v| {
        p.auto_copy = v
    }));
    after_group.add(&switch("儲存檔案", pr.auto_save, |p, v| p.auto_save = v));
    general.add(&after_group);

    let file_group = adw::PreferencesGroup::builder().title("檔案").build();
    let folder = adw::ActionRow::builder()
        .title("位置")
        .subtitle(pr.folder().display().to_string())
        .build();
    let choose_folder = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .tooltip_text("選擇資料夾")
        .build();
    folder.add_suffix(&choose_folder);
    let (p, row) = (prefs.clone(), folder.clone());
    choose_folder.connect_clicked(move |button| {
        let dialog = gtk::FileDialog::builder()
            .title("選擇資料夾")
            .initial_folder(&gio::File::for_path(p.borrow().folder()))
            .build();
        let (p, row) = (p.clone(), row.clone());
        let parent = button.root().and_downcast::<gtk::Window>();
        dialog.select_folder(parent.as_ref(), gio::Cancellable::NONE, move |file| {
            if let Some(path) = file.ok().and_then(|f| f.path()) {
                row.set_subtitle(&path.display().to_string());
                p.borrow_mut().folder = path.display().to_string();
                p.borrow().save();
            }
        });
    });
    file_group.add(&folder);
    let template = adw::EntryRow::builder()
        .title("檔名")
        .text(&pr.template)
        .show_apply_button(true)
        .build();
    let p = prefs.clone();
    template.connect_apply(move |row| {
        let text = row.text().trim().to_string();
        if !text.is_empty() {
            p.borrow_mut().template = text;
            p.borrow().save();
        }
    });
    file_group.add(&template);
    let format = adw::ComboRow::builder()
        .title("格式")
        .model(&gtk::StringList::new(&["PNG", "JPEG"]))
        .build();
    format.set_selected(if pr.format == "jpg" { 1 } else { 0 });
    let p = prefs.clone();
    format.connect_selected_notify(move |row| {
        p.borrow_mut().format = if row.selected() == 1 { "jpg" } else { "png" }.into();
        p.borrow().save();
    });
    file_group.add(&format);
    file_group.add(&switch("以邏輯像素儲存", pr.downscale, |p, v| {
        p.downscale = v
    }));
    general.add(&file_group);

    let capture_group = adw::PreferencesGroup::builder().title("擷取").build();
    let delay = adw::SpinRow::with_range(1.0, 30.0, 1.0);
    delay.set_title("延遲秒數");
    delay.set_value(pr.delay as f64);
    let p = prefs.clone();
    delay.connect_value_notify(move |row| {
        p.borrow_mut().delay = row.value() as u32;
        p.borrow().save();
    });
    capture_group.add(&delay);
    general.add(&capture_group);

    let keys_group = adw::PreferencesGroup::builder().title("快速鍵").build();
    for (title, accel) in [
        ("擷取區域", "<Super><Shift>4"),
        ("擷取全螢幕", "<Super><Shift>3"),
        ("擷取視窗", "<Super><Shift>5"),
        ("捲動擷取", "<Super><Shift>6"),
        ("重複上次區域", "<Super><Shift>r"),
        ("文字辨識", "<Super><Shift>o"),
        ("延遲擷取", "<Super><Shift>d"),
        ("從剪貼簿載入", "<Super><Shift>v"),
    ] {
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(
            &gtk::ShortcutLabel::builder()
                .accelerator(accel)
                .valign(gtk::Align::Center)
                .build(),
        );
        keys_group.add(&row);
    }
    general.add(&keys_group);
    dialog.add(&general);

    let upload = adw::PreferencesPage::builder()
        .title("上傳")
        .icon_name("send-to-symbolic")
        .name("upload")
        .build();
    let s3_group = adw::PreferencesGroup::builder().title("S3 相容儲存").build();
    let s3 = pr.s3.clone();
    for (title, value, set, secret) in [
        (
            "端點",
            s3.endpoint,
            (|p: &mut Prefs, v: String| p.s3.endpoint = v) as fn(&mut Prefs, String),
            false,
        ),
        ("儲存貯體", s3.bucket, |p, v| p.s3.bucket = v, false),
        ("區域", s3.region, |p, v| p.s3.region = v, false),
        ("存取金鑰", s3.access_key, |p, v| p.s3.access_key = v, false),
        ("私密金鑰", s3.secret_key, |p, v| p.s3.secret_key = v, true),
        ("公開網址", s3.public_url, |p, v| p.s3.public_url = v, false),
    ] {
        let row: adw::EntryRow = if secret {
            adw::PasswordEntryRow::builder().title(title).build().upcast()
        } else {
            adw::EntryRow::builder().title(title).build()
        };
        row.set_text(&value);
        let p = prefs.clone();
        row.connect_changed(move |row| {
            set(&mut p.borrow_mut(), row.text().trim().to_string());
            p.borrow().save();
        });
        s3_group.add(&row);
    }
    upload.add(&s3_group);
    dialog.add(&upload);
    if let Some(page) = page {
        dialog.set_visible_page_name(page);
    }
    dialog.present(parent);
    dialog
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page of noise, each row its own, seen through a window `h` rows tall at `top`.
    fn view(top: i32, h: i32) -> Img {
        let mut img = Img::new(8, h, [255, 255, 255, 255]);
        for y in 0..h {
            for x in 0..8 {
                let mut v = (((top + y) * 8 + x) as u32).wrapping_mul(0x9e37_79b1);
                v ^= v >> 15;
                v = v.wrapping_mul(0x85eb_ca77);
                v ^= v >> 13;
                let shade = (v >> 24) as u8;
                img.set(x, y, [shade, shade, shade, 255]);
            }
        }
        img
    }

    /// `cargo test -- --ignored joined` in a Nekoland session; GTK runs once a process, so alone.
    #[test]
    #[ignore = "needs a Nekoland session"]
    fn a_scrolling_page_is_joined_into_one_picture() {
        assert_joined_whole(24.0);
    }

    /// Faster than a frame taken every 270 ms could keep overlapping:
    /// `cargo test -- --ignored fast_scrolled` in a Nekoland session.
    #[test]
    #[ignore = "needs a Nekoland session"]
    fn a_fast_scrolled_page_is_whole_too() {
        assert_joined_whole(110.0);
    }

    fn assert_joined_whole(step: f64) {
        let (text, cut) = capture_scrolling_window(step);
        let v: Vec<f64> = text.split(' ').map(|s| s.parse().unwrap()).collect();
        let (h, expected, k) = (v[0], v[1], v[2]);
        assert!(!cut, "cut short: {text}");
        assert!(h > 3.0 * 300.0 * k, "too short to be joined: {text}");
        assert!(
            (h - expected).abs() <= 2.0 * k,
            "joined {h} rows, the page scrolled to {expected}"
        );
    }

    /// `cargo test -- --ignored cut_short` in a Nekoland session.
    #[test]
    #[ignore = "needs a Nekoland session"]
    fn a_page_scrolled_past_a_whole_frame_is_cut_short() {
        let (text, cut) = capture_scrolling_window(450.0);
        let v: Vec<f64> = text.split(' ').map(|s| s.parse().unwrap()).collect();
        assert!(cut, "not cut short: {text}");
        // Nothing past the first frame is joined across the gap.
        assert!(v[0] <= 300.0 * v[2], "joined across the gap: {text}");
    }

    /// A window of numbered lines that scrolls itself down `step` pixels at a time while the
    /// area over it is taken: the joined height, the height scrolled to, the scale, and whether
    /// the capture was cut short.
    fn capture_scrolling_window(step: f64) -> (String, bool) {
        const TITLE: &str = "捲動測試";
        let app = adw::Application::builder()
            .application_id("io.github.koukeneko.Nekoland.ShotScrollTest")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        let outcome: Rc<RefCell<Option<Result<(String, bool), String>>>> = Rc::default();
        let out = outcome.clone();
        app.connect_activate(move |app| {
            let text: Vec<String> = (1..=160)
                .map(|n| format!("第 {n} 行 {}", "■".repeat(n % 23)))
                .collect();
            let label = gtk::Label::builder()
                .label(text.join("\n"))
                .xalign(0.0)
                .margin_start(12)
                .build();
            let scroller = gtk::ScrolledWindow::builder()
                .child(&label)
                .vscrollbar_policy(gtk::PolicyType::External)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build();
            let window = gtk::ApplicationWindow::builder()
                .application(app)
                .title(TITLE)
                .decorated(false)
                .default_width(420)
                .default_height(300)
                .child(&scroller)
                .build();
            window.present();
            let (app, out) = (app.clone(), out.clone());
            glib::spawn_future_local(async move {
                let mut area = None;
                for _ in 0..50 {
                    glib::timeout_future(Duration::from_millis(100)).await;
                    area = ipc::msg("window-areas")
                        .unwrap_or_default()
                        .lines()
                        .find_map(|line| {
                            let (a, title) = line.split_once('\t')?;
                            let v: Vec<f64> = a.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                            (title == TITLE && v.len() == 4)
                                .then(|| (v[0] + 4.0, v[1] + 4.0, v[2] - 8.0, v[3] - 8.0))
                        });
                    if area.is_some() {
                        break;
                    }
                }
                let Some(area) = area else {
                    *out.borrow_mut() = Some(Err("the window never showed".into()));
                    return app.quit();
                };
                // Past the window's opening animation, as a person choosing the area would be.
                glib::timeout_future(Duration::from_millis(600)).await;
                // Down a little at a time, as a wheel does, until the end.
                let adj = scroller.vadjustment();
                let moving = adj.clone();
                // Once the first frame is taken, which is of the top.
                glib::timeout_add_local_once(Duration::from_millis(1000), move || {
                    glib::timeout_add_local(Duration::from_millis(90), move || {
                        let next = (moving.value() + step).min(moving.upper() - moving.page_size());
                        moving.set_value(next);
                        glib::ControlFlow::from(next < moving.upper() - moving.page_size())
                    });
                });
                let taken = gio::spawn_blocking(move || scroll_capture(area)).await.unwrap();
                let result = taken.map(|(img, cut)| {
                    let k = img.w as f64 / area.2;
                    let expected = (area.3 + adj.value()) * k;
                    (format!("{} {} {}", img.h, expected.round(), k), cut)
                });
                *out.borrow_mut() = Some(result);
                app.quit();
            });
        });
        app.run_with_args::<&str>(&[]);
        let result = outcome.borrow_mut().take().expect("an outcome");
        result.unwrap_or_else(|err| panic!("{err}"))
    }

    #[test]
    fn a_scrolled_page_is_found_by_how_far_it_moved() {
        assert_eq!(scrolled_by(&view(0, 60), &view(17, 60)), Some(17));
        assert_eq!(scrolled_by(&view(0, 60), &view(0, 60)), Some(0));
        // Too far to overlap; the frame after it still matches it, so the capture ends there.
        assert_eq!(scrolled_by(&view(0, 60), &view(200, 60)), None);
        assert_eq!(scrolled_by(&view(200, 60), &view(230, 60)), Some(30));
    }

    /// A list whose 20-line rows look alike but for a number drawn in a few of their lines; the
    /// row under a pointer that stays at line 50 of the view is faintly lit, as lists light it.
    fn list(top: i32, h: i32) -> Img {
        let mut img = Img::new(1000, h, [30, 30, 30, 255]);
        let hovered = (top + 50) / 20;
        for y in 0..h {
            let (row, line) = ((top + y) / 20, (top + y) % 20);
            for x in 0..1000 {
                let label = x < 600 && x % 50 < 30 && (6..14).contains(&line);
                let digit =
                    (700..712).contains(&x) && (6..14).contains(&line) && row >> ((x - 700) / 2) & 1 == 1;
                if label || digit {
                    img.set(x, y, [230, 230, 230, 255]);
                } else if row == hovered {
                    img.set(x, y, [39, 39, 39, 255]);
                }
            }
        }
        img
    }

    #[test]
    fn a_scrolled_list_is_not_taken_for_one_scrolled_whole_rows_less() {
        assert_eq!(scrolled_by(&list(0, 300), &list(47, 300)), Some(47));
        assert_eq!(scrolled_by(&list(100, 300), &list(160, 300)), Some(60));
        // Stopped: the same rows, not the same rows again one or more rows further.
        assert_eq!(scrolled_by(&list(100, 300), &list(100, 300)), Some(0));
    }
}
