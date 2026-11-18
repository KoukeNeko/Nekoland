//! What the pointer is on in a pane: a path to an image shows the image beside it, as Nekominal's previews; a
//! `file:line` opens in the editor of the settings when it is clicked with Ctrl held.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

use gtk::{gdk, gio, glib, prelude::*};
use vte4::prelude::*;

use super::{config, pane::Pane, profiles};

/// PCRE2 flags: any case, and `^` and `$` at each line, which VTE asks of a match.
const PCRE2_CASELESS: u32 = 0x0000_0008;
const PCRE2_MULTILINE: u32 = 0x0000_0400;
const IMAGE_PATTERN: &str = r"(?:~|\.{1,2})?/?[\w.@%+~-]+(?:/[\w.@%+~-]+)*\.(?:png|jpe?g|gif|webp|bmp|svg)\b";
const FILE_LINE_PATTERN: &str = r"(?:~|\.{1,2})?/?[\w.@%+~-]+(?:/[\w.@%+~-]+)*\.\w+:\d+(?::\d+)?";
const POINTER_CURSOR: &str = "pointer";

/// What a pane's pointer is on, and the image shown for it.
struct Watch {
    pane: Weak<Pane>,
    image_tag: i32,
    file_tag: i32,
    shown: RefCell<Option<PathBuf>>,
    popover: gtk::Popover,
    picture: gtk::Picture,
}

fn add_match(screen: &vte4::Terminal, pattern: &str) -> Option<i32> {
    match vte4::Regex::for_match(pattern, PCRE2_CASELESS | PCRE2_MULTILINE) {
        Ok(regex) => {
            let tag = screen.match_add_regex(&regex, 0);
            screen.match_set_cursor_name(tag, POINTER_CURSOR);
            Some(tag)
        }
        Err(error) => {
            eprintln!("nekoland-terminal: could not watch for {pattern}: {error}");
            None
        }
    }
}

/// The file a path written in the pane names: from home with `~`, else from where the shell is.
pub fn resolve(text: &str, directory: Option<&Path>) -> PathBuf {
    match text.strip_prefix('~') {
        Some(rest) => glib::home_dir().join(rest.trim_start_matches('/')),
        None if text.starts_with('/') => PathBuf::from(text),
        None => directory.map_or_else(|| PathBuf::from(text), |directory| directory.join(text)),
    }
}

/// A `file:line[:column]` split into the file and its line.
pub fn split_file_line(text: &str) -> Option<(&str, u32)> {
    let (rest, last) = text.rsplit_once(':')?;
    let last = last.parse::<u32>().ok()?;
    match rest.rsplit_once(':') {
        Some((file, line)) => match line.parse::<u32>() {
            Ok(line) => Some((file, line)),
            Err(_) => Some((rest, last)),
        },
        None => Some((rest, last)),
    }
}

/// The editor's command for a file and line, `{file}` and `{line}` in it standing for them.
pub fn editor_command(editor: &str, file: &Path, line: u32) -> Vec<String> {
    profiles::split_args(editor)
        .into_iter()
        .map(|arg| arg.replace("{file}", &file.display().to_string()).replace("{line}", &line.to_string()))
        .collect()
}

pub fn attach(pane: &Rc<Pane>) {
    let screen: &vte4::Terminal = pane.screen.upcast_ref();
    let (Some(image_tag), Some(file_tag)) = (add_match(screen, IMAGE_PATTERN), add_match(screen, FILE_LINE_PATTERN)) else {
        return;
    };
    let picture = gtk::Picture::builder().can_shrink(true).build();
    let popover = gtk::Popover::builder()
        .child(&picture)
        .autohide(false)
        .can_target(false)
        .has_arrow(false)
        .build();
    popover.set_parent(&pane.screen);
    pane.screen.connect_destroy(glib::clone!(
        #[weak]
        popover,
        move |_| popover.unparent()
    ));
    let watch = Rc::new(Watch {
        pane: Rc::downgrade(pane),
        image_tag,
        file_tag,
        shown: RefCell::default(),
        popover,
        picture,
    });
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion(glib::clone!(
        #[strong]
        watch,
        move |_, x, y| watch.pointer_at(x, y)
    ));
    motion.connect_leave(glib::clone!(
        #[strong]
        watch,
        move |_| watch.hide()
    ));
    pane.screen.add_controller(motion);
    let click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_PRIMARY)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    click.connect_pressed(glib::clone!(
        #[strong]
        watch,
        move |gesture, _, x, y| {
            let ctrl = gesture.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
            if ctrl && watch.open_file_line(x, y) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    ));
    pane.screen.add_controller(click);
}

/// Opens a file in the editor of the settings at `line`, or in the desktop's own app for it when none is set.
pub fn open_file(widget: &impl IsA<gtk::Widget>, path: &Path, line: Option<u32>) {
    let editor = config::current().editor;
    if editor.trim().is_empty() {
        let window = widget.root().and_downcast::<gtk::Window>();
        gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(window.as_ref(), gio::Cancellable::NONE, |result| {
            if let Err(error) = result {
                eprintln!("nekoland-terminal: could not open the file: {error}");
            }
        });
        return;
    }
    let command = editor_command(&editor, path, line.unwrap_or(1));
    let Some((program, args)) = command.split_first() else {
        return;
    };
    if let Err(error) = std::process::Command::new(program).args(args).spawn() {
        eprintln!("nekoland-terminal: could not start {program}: {error}");
    }
}

impl Watch {
    fn matched(&self, x: f64, y: f64) -> Option<(String, i32)> {
        let pane = self.pane.upgrade()?;
        let (text, tag) = pane.screen.check_match_at(x, y);
        Some((text?.to_string(), tag))
    }

    fn pointer_at(&self, x: f64, y: f64) {
        let config = config::current();
        let Some(pane) = self.pane.upgrade() else {
            return;
        };
        let image = self
            .matched(x, y)
            .filter(|(_, tag)| config.preview_images && *tag == self.image_tag)
            .map(|(text, _)| resolve(&text, pane.directory().as_deref()))
            .filter(|path| path.is_file());
        let Some(path) = image else {
            self.hide();
            return;
        };
        if self.shown.borrow().as_ref() != Some(&path) {
            if !self.show(&path, config.preview_size) {
                return;
            }
            self.shown.replace(Some(path));
        }
        let at = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
        self.popover.set_pointing_to(Some(&at));
    }

    /// Shows the image at most `size` on its longest side; false when it cannot be read.
    fn show(&self, path: &Path, size: u32) -> bool {
        let texture = match gdk::Texture::from_file(&gio::File::for_path(path)) {
            Ok(texture) => texture,
            Err(error) => {
                eprintln!("nekoland-terminal: could not read {}: {error}", path.display());
                return false;
            }
        };
        let (width, height) = (texture.width().max(1) as f64, texture.height().max(1) as f64);
        let scale = (f64::from(size) / width.max(height)).min(1.0);
        self.picture
            .set_size_request((width * scale) as i32, (height * scale) as i32);
        self.picture.set_paintable(Some(&texture));
        self.popover.popup();
        true
    }

    fn hide(&self) {
        if self.shown.take().is_some() {
            self.popover.popdown();
            self.picture.set_paintable(gdk::Paintable::NONE);
        }
    }

    /// Opens a `file:line` under the pointer; false when there is none.
    fn open_file_line(&self, x: f64, y: f64) -> bool {
        let Some(pane) = self.pane.upgrade() else {
            return false;
        };
        let Some((text, tag)) = self.matched(x, y) else {
            return false;
        };
        if tag != self.file_tag {
            return false;
        }
        let Some((file, line)) = split_file_line(&text) else {
            return false;
        };
        let path = resolve(file, pane.directory().as_deref());
        open_file(&pane.screen, &path, Some(line));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_line_gives_its_file_and_line() {
        assert_eq!(split_file_line("src/main.rs:42"), Some(("src/main.rs", 42)));
        assert_eq!(split_file_line("src/main.rs:42:7"), Some(("src/main.rs", 42)));
        assert_eq!(split_file_line("main.rs"), None);
    }

    #[test]
    fn the_editor_command_has_the_file_and_line_in_it() {
        let command = editor_command("code --goto {file}:{line}", Path::new("/a b/c.rs"), 3);
        assert_eq!(command, vec!["code", "--goto", "/a b/c.rs:3"]);
    }

    #[test]
    fn a_relative_path_is_read_from_the_shells_folder() {
        assert_eq!(resolve("x.png", Some(Path::new("/tmp"))), PathBuf::from("/tmp/x.png"));
        assert_eq!(resolve("/etc/x.png", Some(Path::new("/tmp"))), PathBuf::from("/etc/x.png"));
    }
}
