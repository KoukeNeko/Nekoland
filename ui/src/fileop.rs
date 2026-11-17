//! `nekoland-fileop copy|move <folder> <path>...`: copies or moves files and folders into a
//! folder, as dropping them on it does. A window shows how far it is when it takes more
//! than a moment, and asks when a name is taken there or something fails.

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};

use adw::prelude::*;
use browse::{free_name, EDGE, GAP};
use gtk::{gio, glib};

mod browse;

/// Work done sooner than this never shows a window.
const SHOW_AFTER: Duration = Duration::from_millis(500);
/// Between buttons side by side, as an alert's are.
const BUTTON_GAP: i32 = 12;

#[derive(Clone, Copy, PartialEq)]
enum Verb {
    Copy,
    Move,
}

impl Verb {
    fn label(self) -> &'static str {
        match self {
            Verb::Copy => "拷貝",
            Verb::Move => "移動",
        }
    }
}

/// One side of a name clash, as the window shows it.
struct Side {
    path: PathBuf,
    is_dir: bool,
    /// Bytes for a file, items for a folder.
    amount: u64,
    modified: Option<SystemTime>,
}

enum Update {
    Started {
        total: u64,
    },
    Progress {
        done: u64,
        file: String,
    },
    Conflict {
        folder: String,
        existing: Side,
        incoming: Side,
        more: bool,
    },
    Failed {
        message: String,
    },
    Finished,
}

#[derive(Clone, Copy, PartialEq)]
enum Answer {
    Skip,
    KeepBoth,
    /// Merges two folders; otherwise the existing item gives way.
    Replace,
    Retry,
    Stop,
}

fn main() -> glib::ExitCode {
    let mut args = std::env::args().skip(1);
    let verb = match args.next().as_deref() {
        Some("copy") => Verb::Copy,
        Some("move") => Verb::Move,
        _ => {
            eprintln!("usage: nekoland-fileop copy|move <folder> <path>...");
            return glib::ExitCode::FAILURE;
        }
    };
    let Some(folder) = args.next().map(PathBuf::from) else {
        eprintln!("usage: nekoland-fileop copy|move <folder> <path>...");
        return glib::ExitCode::FAILURE;
    };
    let sources: Vec<PathBuf> = args.map(PathBuf::from).collect();
    if sources.is_empty() {
        return glib::ExitCode::SUCCESS;
    }
    // Not unique: each drop is its own piece of work.
    let app = adw::Application::builder()
        .application_id("io.github.koukeneko.Nekoland.FileOperation")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| Window::start(app, verb, folder.clone(), sources.clone()));
    app.run_with_args::<&str>(&[])
}

struct Window {
    window: adw::Window,
    pages: gtk::Stack,
    progress: gtk::ProgressBar,
    file: gtk::Label,
    amount: gtk::Label,
    failure: gtk::Box,
    conflict_title: gtk::Label,
    existing: gtk::Box,
    incoming: gtk::Box,
    merge_hint: gtk::Label,
    apply_all: gtk::CheckButton,
    replace: gtk::Button,
    answers: mpsc::Sender<(Answer, bool)>,
    cancellable: gio::Cancellable,
    verb: Verb,
    started: Instant,
    total: Cell<u64>,
    done: Cell<u64>,
    /// Since when the rate is measured: reset after every question, which stops the clock.
    rate_from: Cell<(Instant, u64)>,
    finished: Cell<bool>,
    conflict_is_dir: Cell<bool>,
    shown: Cell<bool>,
    /// Whether how far it is goes to stdout too.
    report: bool,
}

impl Window {
    fn start(app: &adw::Application, verb: Verb, folder: PathBuf, sources: Vec<PathBuf>) {
        let (updates_tx, updates) = mpsc::channel();
        let (answers, answers_rx) = mpsc::channel();
        let cancellable = gio::Cancellable::new();

        let window = adw::Window::builder()
            .application(app)
            .title(verb.label())
            .default_width(460)
            // libadwaita's windows are at least 200 high, more than the progress needs.
            .height_request(0)
            .resizable(false)
            .build();

        // The work: what, where to, and how far.
        let what = match sources.as_slice() {
            [one] => format!("「{}」", display_name(one)),
            many => format!(" {} 個項目", many.len()),
        };
        let title = gtk::Label::builder()
            .label(format!("{}{what}到「{}」", verb.label(), display_name(&folder)))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["heading"])
            .build();
        let progress = gtk::ProgressBar::new();
        let file = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["dim-label", "caption"])
            .build();
        let amount = gtk::Label::builder()
            .css_classes(["dim-label", "caption", "numeric"])
            .build();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
        line.append(&file);
        line.append(&amount);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(GAP / 2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        text.append(&title);
        text.append(&progress);
        text.append(&line);
        let stop = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("停止")
            .valign(gtk::Align::Center)
            .css_classes(["circular"])
            .build();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, EDGE);
        row.append(&file_image(&folder, 40));
        row.append(&text);
        row.append(&stop);
        let skip_failed = gtk::Button::with_label("略過");
        let retry = gtk::Button::builder()
            .label("重試")
            .css_classes(["suggested-action"])
            .build();
        let failure = gtk::Box::builder()
            .spacing(BUTTON_GAP)
            .halign(gtk::Align::End)
            .visible(false)
            .build();
        failure.append(&skip_failed);
        failure.append(&retry);
        let working = padded(gtk::Orientation::Vertical);
        working.append(&row);
        working.append(&failure);

        // A name already taken where the work goes.
        let conflict_title = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["heading"])
            .build();
        let existing = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let incoming = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let sides = gtk::Box::builder().spacing(GAP).homogeneous(true).build();
        sides.append(&existing);
        sides.append(&incoming);
        let merge_hint = gtk::Label::builder()
            .label("合併後，同名的檔案會再逐一詢問")
            .css_classes(["dim-label", "caption"])
            .build();
        let apply_all = gtk::CheckButton::with_label("其餘同名項目也這樣處理");
        let skip = gtk::Button::with_label("略過");
        let keep_both = gtk::Button::with_label("保留兩者");
        let replace = gtk::Button::builder().css_classes(["suggested-action"]).build();
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, BUTTON_GAP);
        actions.append(&apply_all);
        actions.append(&gtk::Box::builder().hexpand(true).build());
        actions.append(&skip);
        actions.append(&keep_both);
        actions.append(&replace);
        let conflict = padded(gtk::Orientation::Vertical);
        conflict.append(&conflict_title);
        conflict.append(&sides);
        conflict.append(&merge_hint);
        conflict.append(&actions);

        let pages = gtk::Stack::builder().vhomogeneous(false).build();
        pages.add_named(&working, Some("working"));
        pages.add_named(&conflict, Some("conflict"));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&pages));
        window.set_content(Some(&view));

        let now = Instant::now();
        let this = Rc::new(Window {
            window,
            pages,
            progress,
            file,
            amount,
            failure,
            conflict_title,
            existing,
            incoming,
            merge_hint,
            apply_all,
            replace: replace.clone(),
            answers,
            cancellable: cancellable.clone(),
            verb,
            started: now,
            total: Cell::new(0),
            done: Cell::new(0),
            rate_from: Cell::new((now, 0)),
            report: std::env::var_os(browse::REPORT_PROGRESS).is_some(),
            finished: Cell::new(false),
            conflict_is_dir: Cell::new(false),
            shown: Cell::new(false),
        });

        let answer = |this: &Rc<Window>, button: &gtk::Button, answer: Answer| {
            let this = this.clone();
            button.connect_clicked(move |_| this.answer(answer));
        };
        answer(&this, &skip, Answer::Skip);
        answer(&this, &keep_both, Answer::KeepBoth);
        answer(&this, &replace, Answer::Replace);
        answer(&this, &skip_failed, Answer::Skip);
        answer(&this, &retry, Answer::Retry);
        let stopping = this.clone();
        stop.connect_clicked(move |_| stopping.window.close());
        let closing = this.clone();
        this.window.connect_close_request(move |_| {
            if !closing.finished.get() {
                closing.cancellable.cancel();
                // The work may be waiting for an answer.
                let _ = closing.answers.send((Answer::Stop, false));
            }
            glib::Propagation::Proceed
        });

        // The application lives as long as the work, its window not shown at first.
        let mut hold = Some(app.hold());
        std::thread::spawn(move || {
            let work = Work {
                verb,
                updates: updates_tx,
                answers: answers_rx,
                cancellable,
                done: 0,
                all_files: None,
                all_folders: None,
            };
            work.run(&folder, &sources)
        });
        let polling = this.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let keep = polling.poll(&updates);
            if keep == glib::ControlFlow::Break {
                hold.take();
            }
            keep
        });
    }

    fn poll(self: &Rc<Self>, updates: &mpsc::Receiver<Update>) -> glib::ControlFlow {
        loop {
            match updates.try_recv() {
                Ok(Update::Started { total }) => self.total.set(total),
                Ok(Update::Progress { done, file }) => {
                    self.done.set(done);
                    self.file.set_label(&file);
                }
                Ok(Update::Conflict {
                    folder,
                    existing,
                    incoming,
                    more,
                }) => {
                    self.ask_conflict(&folder, &existing, &incoming, more);
                    break;
                }
                Ok(Update::Failed { message }) => {
                    self.file.set_label(&message);
                    self.file.remove_css_class("dim-label");
                    self.file.add_css_class("error");
                    self.amount.set_visible(false);
                    self.failure.set_visible(true);
                    self.show();
                    break;
                }
                Ok(Update::Finished) | Err(mpsc::TryRecvError::Disconnected) => {
                    self.finished.set(true);
                    // Not close: a window never shown ignores it, and would keep this running.
                    self.window.destroy();
                    return glib::ControlFlow::Break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        self.show_progress();
        if self.report {
            use std::io::Write;
            // Whoever reads it may have quit; the work goes on without them.
            let _ = writeln!(std::io::stdout(), "{} {}", self.done.get(), self.total.get());
        }
        if self.started.elapsed() >= SHOW_AFTER {
            self.show();
        }
        glib::ControlFlow::Continue
    }

    fn show(&self) {
        if !self.shown.replace(true) {
            self.window.present();
            // It comes up unasked, so no key meant for another window may stop the work.
            gtk::prelude::GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);
        }
    }

    fn show_progress(&self) {
        let (done, total) = (self.done.get(), self.total.get());
        if total == 0 {
            self.progress.pulse();
            return;
        }
        self.progress.set_fraction(done as f64 / total as f64);
        let mut amount = format!("{} / {}", glib::format_size(done), glib::format_size(total));
        let (since, from) = self.rate_from.get();
        let elapsed = since.elapsed().as_secs_f64();
        if elapsed > 1.0 && done > from {
            let rest = (total - done) as f64 / ((done - from) as f64 / elapsed);
            amount.push_str(" · ");
            amount.push_str(&rest_label(rest));
        }
        self.amount.set_label(&amount);
    }

    fn ask_conflict(&self, folder: &str, existing: &Side, incoming: &Side, more: bool) {
        let both_dirs = existing.is_dir && incoming.is_dir;
        let kind = if existing.is_dir { "檔案夾" } else { "檔案" };
        self.conflict_title.set_label(&format!(
            "「{folder}」裡已有名為「{}」的{kind}",
            display_name(&existing.path)
        ));
        let newer = incoming_is_newer(existing.modified, incoming.modified);
        let came = match self.verb {
            Verb::Copy => "拷貝來的",
            Verb::Move => "移來的",
        };
        fill_side(&self.existing, existing, "原有", newer == Some(false));
        fill_side(&self.incoming, incoming, came, newer == Some(true));
        self.merge_hint.set_visible(both_dirs);
        self.replace.set_label(if both_dirs { "合併" } else { "取代" });
        self.apply_all.set_active(false);
        self.apply_all.set_visible(more);
        self.conflict_is_dir.set(both_dirs);
        self.pages.set_visible_child_name("conflict");
        self.show();
    }

    fn answer(&self, answer: Answer) {
        let all =
            self.pages.visible_child_name().as_deref() == Some("conflict") && self.apply_all.is_active();
        let _ = self.answers.send((answer, all));
        self.pages.set_visible_child_name("working");
        self.failure.set_visible(false);
        self.file.remove_css_class("error");
        self.file.add_css_class("dim-label");
        self.amount.set_visible(true);
        self.rate_from.set((Instant::now(), self.done.get()));
    }
}

/// A box with the window's padding on every side.
fn padded(orientation: gtk::Orientation) -> gtk::Box {
    gtk::Box::builder()
        .orientation(orientation)
        .spacing(EDGE)
        .margin_top(EDGE)
        .margin_bottom(EDGE)
        .margin_start(EDGE)
        .margin_end(EDGE)
        .build()
}

/// A file's thumbnail when Files or tumbler made one, else its type's icon.
fn file_image(path: &Path, size: i32) -> gtk::Widget {
    let info = gio::File::for_path(path).query_info(
        "standard::icon,thumbnail::path",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    );
    let Ok(info) = info else {
        return gtk::Image::builder()
            .icon_name("text-x-generic")
            .pixel_size(size)
            .build()
            .upcast();
    };
    if let Some(thumbnail) = info.attribute_byte_string("thumbnail::path") {
        let picture = gtk::Picture::for_filename(thumbnail.as_str());
        picture.set_size_request(size, size);
        picture.set_content_fit(gtk::ContentFit::Contain);
        return picture.upcast();
    }
    let image = gtk::Image::builder().pixel_size(size).build();
    if let Some(icon) = info.icon() {
        image.set_from_gicon(&icon);
    }
    image.upcast()
}

fn fill_side(card: &gtk::Box, side: &Side, tag: &str, newer: bool) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    card.add_css_class("card");
    let inner = padded(gtk::Orientation::Vertical);
    inner.set_spacing(2);
    let image = file_image(&side.path, 64);
    image.set_margin_bottom(GAP);
    image.set_halign(gtk::Align::Center);
    inner.append(&image);
    let label = |text: &str, classes: &[&str]| gtk::Label::builder().label(text).css_classes(classes).build();
    inner.append(&label(tag, &["dim-label", "caption"]));
    let amount = if side.is_dir {
        format!("{} 個項目", side.amount)
    } else {
        glib::format_size(side.amount).to_string()
    };
    inner.append(&label(&amount, &["dim-label", "numeric"]));
    if let Some(when) = side.modified.and_then(date_time) {
        inner.append(&label(
            &modified_label(&when),
            if newer {
                &["heading", "numeric"]
            } else {
                &["dim-label", "numeric"]
            },
        ));
    }
    card.append(&inner);
}

/// Which side's date to mark, only where the dates as shown differ: one of two equal
/// dates in bold reads as a mistake.
fn incoming_is_newer(existing: Option<SystemTime>, incoming: Option<SystemTime>) -> Option<bool> {
    let shown = |time: SystemTime| date_time(time).map(|when| modified_label(&when));
    match (existing, incoming) {
        (Some(a), Some(b)) if shown(a) != shown(b) => Some(b > a),
        _ => None,
    }
}

fn date_time(time: SystemTime) -> Option<glib::DateTime> {
    let secs = time.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    glib::DateTime::from_unix_local(secs as i64).ok()
}

/// With the time always: two versions of a file are often from the same day.
fn modified_label(when: &glib::DateTime) -> String {
    let time = format!("{:02}:{:02}", when.hour(), when.minute());
    let Ok(now) = glib::DateTime::now_local() else {
        return time;
    };
    if when.ymd() == now.ymd() {
        format!("今天 {time}")
    } else if when.year() == now.year() {
        format!("{}月{}日 {time}", when.month(), when.day_of_month())
    } else {
        format!(
            "{}年{}月{}日 {time}",
            when.year(),
            when.month(),
            when.day_of_month()
        )
    }
}

fn rest_label(seconds: f64) -> String {
    let seconds = seconds.ceil() as u64;
    if seconds < 60 {
        format!("剩 {seconds} 秒")
    } else if seconds < 3600 {
        format!("剩 {} 分鐘", seconds.div_ceil(60))
    } else {
        format!("剩 {} 小時", seconds.div_ceil(3600))
    }
}

fn display_name(path: &Path) -> String {
    glib::filename_display_basename(path).to_string()
}

/// The work itself, on its own thread: the window only hears how it goes.
struct Work {
    verb: Verb,
    updates: mpsc::Sender<Update>,
    answers: mpsc::Receiver<(Answer, bool)>,
    cancellable: gio::Cancellable,
    done: u64,
    /// The answer kept for every later clash, one for files and one for folders.
    all_files: Option<Answer>,
    all_folders: Option<Answer>,
}

/// The work was stopped: by the person, or as they answered.
struct Stopped;

impl Work {
    fn run(mut self, folder: &Path, sources: &[PathBuf]) {
        let total = sources.iter().map(|path| size_of(path)).sum();
        self.send(Update::Started { total });
        for (index, source) in sources.iter().enumerate() {
            let more = index + 1 < sources.len();
            if self.verb == Verb::Move && source.parent() == Some(folder) {
                self.done += size_of(source);
                continue;
            }
            let Some(name) = source.file_name() else { continue };
            let mut to = folder.join(name);
            // A copy into the folder it is in sits beside the original.
            if source.parent() == Some(folder) {
                to = free_name(&to);
            }
            if self.transfer(source, &to, more).is_err() {
                break;
            }
        }
        self.send(Update::Finished);
    }

    fn send(&self, update: Update) {
        // The window is gone only once the work is stopped, which it finds out by itself.
        let _ = self.updates.send(update);
    }

    fn ask(&mut self, update: Update) -> (Answer, bool) {
        self.send(update);
        self.answers.recv().unwrap_or((Answer::Stop, false))
    }

    fn transfer(&mut self, from: &Path, to: &Path, more: bool) -> Result<(), Stopped> {
        if self.cancellable.is_cancelled() {
            return Err(Stopped);
        }
        let from_meta = std::fs::symlink_metadata(from);
        let is_dir = from_meta.as_ref().is_ok_and(|meta| meta.is_dir());
        if is_dir && inside_itself(from, to) {
            let message = format!("無法把「{}」放進它自己裡面", display_name(from));
            return match self.ask(Update::Failed { message }).0 {
                Answer::Stop => Err(Stopped),
                _ => Ok(()),
            };
        }
        let mut to = to.to_path_buf();
        let mut overwrite = false;
        if let Ok(existing) = std::fs::symlink_metadata(&to) {
            let both_dirs = is_dir && existing.is_dir();
            let kept = if both_dirs {
                self.all_folders
            } else {
                self.all_files
            };
            let answer = match kept {
                Some(answer) => answer,
                None => {
                    let folder = to.parent().map(display_name).unwrap_or_default();
                    let (answer, all) = self.ask(Update::Conflict {
                        folder,
                        existing: side(&to),
                        incoming: side(from),
                        more: more || both_dirs,
                    });
                    if all {
                        *(if both_dirs {
                            &mut self.all_folders
                        } else {
                            &mut self.all_files
                        }) = Some(answer);
                    }
                    answer
                }
            };
            match answer {
                Answer::Stop => return Err(Stopped),
                Answer::Skip => {
                    self.done += size_of(from);
                    return Ok(());
                }
                Answer::KeepBoth => to = free_name(&to),
                Answer::Replace if both_dirs => {}
                Answer::Replace if is_dir || existing.is_dir() => {
                    // A file and a folder do not overwrite each other; what gives way goes
                    // to the trash rather than being lost.
                    let target = gio::File::for_path(&to);
                    self.retrying(from, |cancellable| target.trash(Some(cancellable)))?;
                }
                Answer::Replace | Answer::Retry => overwrite = true,
            }
        }
        if is_dir {
            self.transfer_folder(from, &to)
        } else {
            self.transfer_file(from, &to, overwrite)
        }
    }

    fn transfer_folder(&mut self, from: &Path, to: &Path) -> Result<(), Stopped> {
        if self.verb == Verb::Move && !to.exists() && on_one_disk(from, to) {
            return match std::fs::rename(from, to) {
                Ok(()) => {
                    self.done += size_of(to);
                    Ok(())
                }
                Err(err) => {
                    let message = format!("無法移動「{}」：{err}", display_name(from));
                    match self.ask(Update::Failed { message }).0 {
                        Answer::Stop => Err(Stopped),
                        _ => Ok(()),
                    }
                }
            };
        }
        // Copied item by item, into the folder there when the two merge; a move across
        // disks too, then emptied.
        if !to.is_dir() {
            let target = gio::File::for_path(to);
            self.retrying(from, |cancellable| target.make_directory(Some(cancellable)))?;
        }
        let children = match std::fs::read_dir(from) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>(),
            Err(err) => {
                let message = format!("無法讀取「{}」：{err}", display_name(from));
                return match self.ask(Update::Failed { message }).0 {
                    Answer::Stop => Err(Stopped),
                    _ => Ok(()),
                };
            }
        };
        for child in children {
            let Some(name) = child.file_name() else { continue };
            self.transfer(&child, &to.join(name), true)?;
        }
        if self.verb == Verb::Move {
            // Still holds what was skipped, which stays where it was.
            let _ = std::fs::remove_dir(from);
        }
        Ok(())
    }

    fn transfer_file(&mut self, from: &Path, to: &Path, overwrite: bool) -> Result<(), Stopped> {
        let mut flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA;
        if overwrite {
            flags |= gio::FileCopyFlags::OVERWRITE;
        }
        let size = size_of(from);
        let base = self.done;
        let file = display_name(from);
        self.send(Update::Progress {
            done: base,
            file: file.clone(),
        });
        let (source, target) = (gio::File::for_path(from), gio::File::for_path(to));
        let verb = self.verb;
        let updates = self.updates.clone();
        self.retrying(from, |cancellable| {
            let mut progress = |current: i64, _total: i64| {
                let _ = updates.send(Update::Progress {
                    done: base + current.max(0) as u64,
                    file: file.clone(),
                });
            };
            match verb {
                Verb::Copy => source.copy(&target, flags, Some(cancellable), Some(&mut progress)),
                Verb::Move => source.move_(&target, flags, Some(cancellable), Some(&mut progress)),
            }
        })?;
        self.done = base + size;
        self.send(Update::Progress {
            done: self.done,
            file: display_name(from),
        });
        Ok(())
    }

    /// Does one step until it works, the person skips it, or the work is stopped.
    fn retrying(
        &mut self,
        item: &Path,
        mut step: impl FnMut(&gio::Cancellable) -> Result<(), glib::Error>,
    ) -> Result<(), Stopped> {
        loop {
            let cancellable = self.cancellable.clone();
            let err = match step(&cancellable) {
                Ok(()) => return Ok(()),
                Err(_) if cancellable.is_cancelled() => return Err(Stopped),
                Err(err) => err,
            };
            let message = format!(
                "無法{}「{}」：{}",
                self.verb.label(),
                display_name(item),
                err.message()
            );
            match self.ask(Update::Failed { message }).0 {
                Answer::Retry => continue,
                Answer::Stop => return Err(Stopped),
                _ => {
                    self.done += size_of(item);
                    return Ok(());
                }
            }
        }
    }
}

fn side(path: &Path) -> Side {
    let meta = std::fs::symlink_metadata(path).ok();
    let is_dir = meta.as_ref().is_some_and(|meta| meta.is_dir());
    let amount = if is_dir {
        std::fs::read_dir(path)
            .map(|entries| entries.count() as u64)
            .unwrap_or(0)
    } else {
        meta.as_ref().map_or(0, |meta| meta.len())
    };
    Side {
        path: path.to_path_buf(),
        is_dir,
        amount,
        modified: meta.and_then(|meta| meta.modified().ok()),
    }
}

/// Bytes in a file, or in everything in a folder; links are not followed.
fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| size_of(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

/// Whether `to` is in the folder `from`, through links too: a folder dropped on a link to
/// itself would otherwise be copied into itself without end.
fn inside_itself(from: &Path, to: &Path) -> bool {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    to.parent()
        .is_some_and(|parent| real(parent).starts_with(real(from)))
}

/// Whether `to`'s folder is on the disk `from` is on, where a move is a rename.
fn on_one_disk(from: &Path, to: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let disk = |path: &Path| std::fs::metadata(path).map(|meta| meta.dev()).ok();
    to.parent()
        .is_some_and(|parent| disk(from).is_some() && disk(from) == disk(parent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_name_counts_up_before_the_extension() {
        let dir = std::env::temp_dir().join(format!("nekoland-fileop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("季報.pdf"), "").unwrap();
        std::fs::write(dir.join("季報 2.pdf"), "").unwrap();
        assert_eq!(free_name(&dir.join("季報.pdf")), dir.join("季報 3.pdf"));
        assert_eq!(free_name(&dir.join("2025 季報")), dir.join("2025 季報 2"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_is_not_put_into_itself_through_a_link() {
        let dir = std::env::temp_dir().join(format!("nekoland-fileop-link-{}", std::process::id()));
        let folder = dir.join("A");
        std::fs::create_dir_all(&folder).unwrap();
        std::os::unix::fs::symlink(&folder, dir.join("A 的連結")).unwrap();
        assert!(inside_itself(&folder, &dir.join("A 的連結").join("A")));
        assert!(inside_itself(&folder, &folder.join("A")));
        assert!(!inside_itself(&folder, &dir.join("B")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_date_shown_differently_is_marked_newer() {
        // A whole minute, so the seconds below stay within it in any whole-minute time zone.
        let minute = SystemTime::UNIX_EPOCH + Duration::from_secs(1_699_999_980);
        let at = |secs| Some(minute + Duration::from_secs(secs));
        assert_eq!(incoming_is_newer(at(5), at(40)), None);
        assert_eq!(incoming_is_newer(at(5), at(125)), Some(true));
        assert_eq!(incoming_is_newer(at(125), at(5)), Some(false));
        assert_eq!(incoming_is_newer(None, at(5)), None);
    }
}
