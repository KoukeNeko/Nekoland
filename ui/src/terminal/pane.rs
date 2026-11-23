//! One shell on screen: its session, what it has been told, and where it is. What it writes passes the triggers on its
//! way to the screen; the bell, a long command's end and the triggers' notifications are told from here.

use std::{
    cell::{Cell, RefCell},
    io,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use vte4::prelude::*;

use super::{
    lang::{tr, trf},
    push::Happening,
    agent::{PaneState, Status},
    api,
    carry::{self, CarriedPane},
    config::{self, BellSound, Osc52Policy, RightClick},
    graphics_stream::GraphicsFilter,
    hint_mode::HintLayer,
    images::{self, Cells, Images},
    jobs::{self, Repository},
    key_input::{Action, KeyInputState},
    kitty_keyboard,
    launcher,
    osc::{self, Signal, Terminator},
    preview, profiles,
    screen::Screen,
    session::{Event, Launch, Session},
    ligatures,
    sticky_header::StickyHeader,
    subagents::{self, Subagents},
    theme, title,
    triggers::{Fired, Matcher},
    usage::{self, Stamp, Usage},
};

/// The size a pane starts at, before it is laid out.
const START_ROWS: u16 = 24;
const START_COLUMNS: u16 = 80;
const UNTITLED: &str = "終端機";
const BELL_CLASS: &str = "term-bell";
const BELL_FLASH: Duration = Duration::from_millis(150);
const KIB: usize = 1024;
/// How many lines of a paste its confirmation shows.
const PASTE_PREVIEW_LINES: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneEvent {
    Changed,
    Exited,
    /// Something to tell the user in a notification: a trigger's, or a long command's end.
    Notify { title: String, body: String, happening: Happening },
}

type EventHandler = Rc<dyn Fn(&Rc<Pane>, PaneEvent)>;

pub struct Pane {
    pub widget: gtk::Box,
    pub screen: Screen,
    session: Session,
    state: RefCell<PaneState>,
    directory: RefCell<Option<PathBuf>>,
    repository: RefCell<Option<Repository>>,
    /// The profile it was started from.
    profile: String,
    /// The triggers, built again when the settings change.
    matcher: RefCell<(u64, Matcher)>,
    /// When the command running started, for a notification when a long one ends.
    command_since: Cell<Option<Instant>>,
    /// The status last told, so that a notification goes out only when it changes.
    pub reported: Cell<Status>,
    /// The name the commands Claude Code runs in it know it by, and what they last said.
    session_name: String,
    usage: RefCell<Option<Usage>>,
    usage_stamp: Cell<Option<Stamp>>,
    /// The subagents its Claude Code session runs, and when their folder last changed.
    subagents: RefCell<Subagents>,
    subagents_stamp: Cell<Option<std::time::SystemTime>>,
    /// The labels of hint mode, over the screen.
    hints: Rc<HintLayer>,
    /// The prompt pinned over the top row while the view is scrolled up.
    sticky: Rc<StickyHeader>,
    /// The kitty keyboard protocol the program in it asked for, and the keys held.
    keys: Rc<KeyInputState>,
    /// The pictures programs show, taken out of what they write on its way to the screen.
    graphics: RefCell<GraphicsFilter>,
    images: Rc<RefCell<Images>>,
}

impl Drop for Pane {
    fn drop(&mut self) {
        usage::forget(&self.session_name);
    }
}

pub struct PaneOptions<'a> {
    pub directory: Option<&'a Path>,
    pub bash_integration: Option<&'a Path>,
    /// The profile to start; None for the default one.
    pub profile: Option<&'a str>,
    /// A program and its arguments to run instead of the profile's shell; empty for the shell.
    pub command: &'a [String],
}

/// What tells a program the automation socket and the pane it runs in, for `nekoland-terminal cli`.
fn api_environment(session_name: &str) -> [(String, String); 2] {
    [
        (api::SOCKET_VAR.to_string(), api::socket_path().display().to_string()),
        (api::PANE_VAR.to_string(), session_name.to_string()),
    ]
}

/// What tells the commands Claude Code runs which pane they run in, and while the status bar is on, the settings that
/// make them this program.
fn usage_environment(session_name: &str, status_bar: bool) -> Vec<(String, String)> {
    let mut variables = vec![(usage::SESSION_VAR.to_string(), session_name.to_string())];
    if let Some(file) = status_bar.then(usage::claude_settings_file).flatten() {
        variables.push((usage::CLAUDE_SETTINGS_VAR.to_string(), file.display().to_string()));
    }
    variables
}

/// An action bound to the key, with its target, that can be carried out now: a disabled one leaves the key to the
/// program.
fn shortcut_action(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<(String, Option<glib::Variant>)> {
    let application = gio::Application::default().and_downcast::<gtk::Application>()?;
    let accelerator = gtk::accelerator_name(key, modifiers & gtk::accelerator_get_default_mod_mask());
    application
        .actions_for_accel(&accelerator)
        .iter()
        .filter_map(|detailed_name| gio::Action::parse_detailed_name(detailed_name).ok())
        .find(|(name, _)| is_action_enabled(&application, name))
        .map(|(name, target)| (name.to_string(), target))
}

/// Whether an action, `win.copy` or `app.quit`, is enabled.
fn is_action_enabled(application: &gtk::Application, name: &str) -> bool {
    let action = match name.split_once('.') {
        Some(("app", action)) => application.lookup_action(action),
        Some(("win", action)) => application
            .active_window()
            .and_downcast::<gtk::ApplicationWindow>()
            .and_then(|window| window.lookup_action(action)),
        _ => None,
    };
    action.is_some_and(|action| action.is_enabled())
}

/// A profile's folder with `~` at its start read as home; None when it names none.
fn profile_directory(cwd: &str) -> Option<PathBuf> {
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return None;
    }
    Some(match cwd.strip_prefix('~') {
        Some(rest) => glib::home_dir().join(rest.trim_start_matches('/')),
        None => PathBuf::from(cwd),
    })
}

impl Pane {
    pub fn spawn(
        options: &PaneOptions,
        on_event: impl Fn(&Rc<Pane>, PaneEvent) + 'static,
    ) -> io::Result<Rc<Self>> {
        let config = config::current();
        let profile = profiles::named(options.profile);
        let directory = profile_directory(&profile.cwd).or_else(|| options.directory.map(Path::to_path_buf));
        let profile_args = profiles::split_args(&profile.args);
        let (program, args, bash_integration) = match options.command.split_first() {
            Some((program, args)) => (program.as_str(), args, None),
            None => (profile.program.as_str(), profile_args.as_slice(), options.bash_integration),
        };
        let (clean, mut environment) = profiles::environment(&profile);
        let session_name = usage::new_session();
        environment.extend(usage_environment(&session_name, config.status_bar));
        environment.extend(api_environment(&session_name));
        let (session, events) = Session::spawn(&Launch {
            program,
            args,
            environment: &environment,
            clean,
            directory: directory.as_deref(),
            bash_integration: bash_integration.filter(|_| config.shell_integration),
            rows: START_ROWS,
            columns: START_COLUMNS,
        })?;
        Ok(Self::assemble((session, events), &profile, directory, session_name, Rc::new(on_event)))
    }

    /// Takes over a pane the copy of this program before carried over, with what it showed.
    pub fn adopt(carried: &CarriedPane, on_event: impl Fn(&Rc<Pane>, PaneEvent) + 'static) -> io::Result<Rc<Self>> {
        let profile = profiles::named(Some(&carried.profile));
        let session = Session::adopt(carried.master, carried.shell_pid)?;
        usage::continue_after(&carried.id);
        let pane =
            Self::assemble(session, &profile, carried.directory.clone(), carried.id.clone(), Rc::new(on_event));
        pane.screen.feed(carry::screen_text(carried).as_bytes());
        if carried.busy {
            pane.redraw_program();
        }
        Ok(pane)
    }

    /// What is carried over of it to the copy of this program that takes over; its pty stays open for that one.
    pub fn carry(&self, text: String) -> io::Result<CarriedPane> {
        Ok(CarriedPane {
            master: self.session.hand_over()?,
            shell_pid: self.session.shell_pid,
            id: self.session_name.clone(),
            profile: self.profile.clone(),
            directory: self.directory(),
            text,
            busy: self.running_program().is_some(),
            title: self.screen.window_title().map(|title| title.to_string()),
        })
    }

    /// Takes back what `carry` handed over, when the restart did not happen.
    pub fn keep(&self) {
        if let Err(error) = self.session.keep() {
            eprintln!("nekoland-terminal: a pane's pty could not be kept from the shells: {error}");
        }
    }

    /// Asks the program in front to draw its screen again, as it does when the window changes size: the screen it drew
    /// on is new.
    fn redraw_program(&self) {
        let Some(group) = self.session.foreground_group() else {
            return;
        };
        // SAFETY: a negative pid names the process group; no memory is involved.
        if unsafe { libc::kill(-group, libc::SIGWINCH) } < 0 {
            eprintln!("nekoland-terminal: could not ask the program to redraw: {}", io::Error::last_os_error());
        }
    }

    fn assemble(
        (session, events): (Session, async_channel::Receiver<Event>),
        profile: &config::Profile,
        directory: Option<PathBuf>,
        session_name: String,
        on_event: EventHandler,
    ) -> Rc<Self> {
        let config = config::current();
        let screen = Screen::default();
        screen.set_hexpand(true);
        screen.set_vexpand(true);
        theme::set_up(&screen, profile);
        screen.set_context_menu_model(Some(&context_menu()));

        let hints = HintLayer::new();
        let sticky = StickyHeader::new(&screen, profile);
        let images = Rc::new(RefCell::new(Images::default()));
        screen.add_painter(images::painter(images.clone()));
        screen.add_painter(ligatures::painter(profile));
        let keys = KeyInputState::new(&screen);
        let layers = gtk::Overlay::builder().child(&screen).hexpand(true).vexpand(true).build();
        layers.add_overlay(sticky.widget());
        layers.add_overlay(keys.preedit_widget());
        layers.add_overlay(hints.widget());
        let widget = gtk::Box::builder().css_classes(["term-pane"]).build();
        widget.append(&layers);

        let pane = Rc::new(Self {
            widget,
            screen,
            session,
            state: RefCell::default(),
            repository: RefCell::new(directory.as_deref().and_then(jobs::repository_of)),
            directory: RefCell::new(directory),
            profile: profile.name.clone(),
            matcher: RefCell::new((config::generation(), Matcher::new(&config.triggers))),
            command_since: Cell::default(),
            reported: Cell::new(Status::Idle),
            session_name,
            usage: RefCell::default(),
            usage_stamp: Cell::default(),
            subagents: RefCell::default(),
            subagents_stamp: Cell::default(),
            hints,
            sticky,
            keys,
            graphics: RefCell::default(),
            images,
        });
        pane.hints.attach(&pane);
        pane.connect(on_event, events);
        preview::attach(&pane);
        pane
    }

    fn connect(self: &Rc<Self>, on_event: EventHandler, events: async_channel::Receiver<Event>) {
        let weak = Rc::downgrade(self);
        self.screen.connect_commit(glib::clone!(
            #[strong]
            weak,
            move |_, text, _| {
                if let Some(pane) = weak.upgrade() {
                    let answer = images::with_sixel_attribute(text);
                    pane.session.write(answer.as_deref().unwrap_or(text).as_bytes());
                }
            }
        ));
        self.screen.connect_resized(glib::clone!(
            #[strong]
            weak,
            move |rows, columns| {
                if let Some(pane) = weak.upgrade() {
                    pane.resize_session(rows, columns);
                }
            }
        ));
        self.screen.connect_char_size_changed(glib::clone!(
            #[strong]
            weak,
            move |screen, _, _| {
                if let Some(pane) = weak.upgrade() {
                    let (rows, columns) = screen.downcast_ref::<Screen>().map_or((0, 0), Screen::rows_and_columns);
                    pane.resize_session(rows, columns);
                }
            }
        ));
        self.screen.connect_window_title_changed(glib::clone!(
            #[strong]
            weak,
            #[strong]
            on_event,
            move |_| {
                if let Some(pane) = weak.upgrade() {
                    on_event(&pane, PaneEvent::Changed);
                }
            }
        ));
        // A right click focuses the pane, so that the menu's commands act on it; set to copy or paste, it does that
        // instead of opening the menu.
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        click.connect_pressed(glib::clone!(
            #[strong]
            weak,
            move |gesture, _, _, _| {
                let Some(pane) = weak.upgrade() else {
                    return;
                };
                pane.screen.grab_focus();
                if config::current().right_click == RightClick::Smart {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    pane.copy_or_paste();
                }
            }
        ));
        self.screen.add_controller(click);
        let keys = gtk::EventControllerKey::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
        keys.connect_key_pressed(glib::clone!(
            #[strong]
            weak,
            move |controller, key, keycode, modifiers| match weak.upgrade() {
                Some(pane) => pane.press_key(controller, key, keycode, modifiers),
                None => glib::Propagation::Proceed,
            }
        ));
        keys.connect_key_released(glib::clone!(
            #[strong]
            weak,
            move |controller, key, keycode, modifiers| {
                if let Some(pane) = weak.upgrade() {
                    pane.release_key(controller, key, keycode, modifiers);
                }
            }
        ));
        self.screen.add_controller(keys);
        let typed = weak.clone();
        self.keys.set_text_handler(move |text| {
            if let Some(pane) = typed.upgrade() {
                pane.session.write(text.as_bytes());
            }
        });
        self.screen.connect_selection_changed(|screen| {
            if config::current().copy_on_select && screen.has_selection() {
                screen.copy_clipboard_format(vte4::Format::Text);
            }
        });
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(pane) = weak.upgrade() else {
                    return;
                };
                match event {
                    Event::Output { bytes, signals } => pane.take_output(&bytes, &signals, &on_event),
                    Event::Exited => {
                        pane.state.borrow_mut().end();
                        on_event(&pane, PaneEvent::Exited);
                        return;
                    }
                }
            }
        });
    }

    /// What the shell wrote: through the triggers to the screen, and what it signalled into the pane's state.
    fn take_output(self: &Rc<Self>, bytes: &[u8], signals: &[Signal], on_event: &EventHandler) {
        // As Nekominal's: the event says the pane printed; `get-text` reads what.
        if api::wants(api::Topic::Output) {
            api::publish(api::Topic::Output, self.id(), serde_json::json!({}));
        }
        let shown = self.take_pictures(bytes);
        let fired = self.feed_through_triggers(&shown);
        let prompt = osc::prompt_text(bytes).filter(|_| signals.contains(&Signal::PromptStart));
        let new_prompt = prompt.is_some();
        if let Some(prompt) = prompt {
            self.sticky.remember_prompt(prompt);
        }
        self.sticky.refresh_after_output(new_prompt);
        let mut events = Vec::new();
        for signal in signals {
            match signal {
                Signal::Bell => self.ring(),
                Signal::CommandStart => self.command_since.set(Some(Instant::now())),
                Signal::CommandEnd(status) => {
                    api::publish(api::Topic::CommandFinished, self.id(), serde_json::json!({ "exit": status }));
                    events.extend(self.long_command_ended(*status));
                }
                Signal::ClipboardWrite(text) => self.copy_for_program(text),
                Signal::ClipboardRead(terminator) => self.answer_clipboard(*terminator),
                Signal::Keyboard(request) => self.take_keyboard_request(*request),
                Signal::AlternateScreen(on) => self.keys.set_alternate(*on),
                // A program that pushed and did not pop is gone once the shell prompts again.
                Signal::PromptStart => self.keys.clear(),
                _ => {}
            }
        }
        let mut state = self.state.borrow_mut();
        signals.iter().for_each(|signal| state.apply(signal));
        drop(state);
        for fired in &fired {
            events.extend(self.carry_out(fired));
        }
        if !signals.is_empty() || !fired.is_empty() {
            on_event(self, PaneEvent::Changed);
        }
        for event in events {
            on_event(self, event);
        }
    }

    /// The pictures in what a program wrote, kept and put on the screen as their cells; what it asked is answered.
    fn take_pictures(&self, bytes: &[u8]) -> Vec<u8> {
        let pieces = self.graphics.borrow_mut().split(bytes);
        let mut images = self.images.borrow_mut();
        let mut shown = Vec::with_capacity(bytes.len());
        for piece in pieces {
            let output = images.take(piece, self.screen.upcast_ref());
            shown.extend(output.screen);
            if !output.reply.is_empty() {
                self.session.write(&output.reply);
            }
        }
        shown
    }

    /// The pty told the screen's rows and columns, and how large it is in pixels, which programs size pictures by.
    fn resize_session(&self, rows: u16, columns: u16) {
        let cells = Cells::of(self.screen.upcast_ref());
        let pixels = |count: u16, cell: u32| u16::try_from(u32::from(count) * cell).unwrap_or(u16::MAX);
        self.session.resize(rows, columns, (pixels(columns, cells.width), pixels(rows, cells.height)));
    }

    /// A kitty keyboard request, while the settings let programs ask; a query is answered.
    fn take_keyboard_request(&self, request: kitty_keyboard::Request) {
        if !config::current().kitty_keyboard {
            return;
        }
        if let Some(answer) = self.keys.request(request) {
            self.session.write(&answer);
        }
    }

    /// A key pressed, as the kitty keyboard protocol has it when the program asked for it; the terminal's shortcuts and
    /// hint mode's keys are theirs.
    fn press_key(
        &self,
        controller: &gtk::EventControllerKey,
        key: gdk::Key,
        keycode: u32,
        modifiers: gdk::ModifierType,
    ) -> glib::Propagation {
        if self.hints.is_on() {
            return glib::Propagation::Proceed;
        }
        // Carried out here rather than left to the window, which would see the key only after VTE had sent it.
        if let Some((name, target)) = shortcut_action(key, modifiers) {
            if let Err(error) = self.screen.activate_action(&name, target.as_ref()) {
                eprintln!("nekoland-terminal: could not carry out {name}: {error}");
            }
            return glib::Propagation::Stop;
        }
        let action = self.keys.press(controller, key, keycode, modifiers);
        self.carry_out_key(action)
    }

    /// A key let go: not reported when its press was the terminal's, as the program never saw that.
    fn release_key(
        &self,
        controller: &gtk::EventControllerKey,
        key: gdk::Key,
        keycode: u32,
        modifiers: gdk::ModifierType,
    ) {
        if self.hints.is_on() || shortcut_action(key, modifiers).is_some() {
            self.keys.forget(keycode);
            return;
        }
        let action = self.keys.release(controller, key, keycode, modifiers);
        self.carry_out_key(action);
    }

    fn carry_out_key(&self, action: Action) -> glib::Propagation {
        match action {
            Action::Send(bytes) => {
                // VTE scrolls down on a key it sends itself; this one it does not see.
                if let Some(view) = self.screen.vadjustment() {
                    view.set_value(view.upper() - view.page_size());
                }
                self.session.write(&bytes);
                glib::Propagation::Stop
            }
            Action::Swallow => glib::Propagation::Stop,
            Action::Pass => glib::Propagation::Proceed,
        }
    }

    fn feed_through_triggers(&self, bytes: &[u8]) -> Vec<Fired> {
        let mut matcher = self.matcher.borrow_mut();
        if matcher.0 != config::generation() {
            *matcher = (config::generation(), Matcher::new(&config::current().triggers));
        }
        if matcher.1.is_empty() {
            self.screen.feed(bytes);
            return Vec::new();
        }
        let program = self.running_program();
        let (screen, fired) = matcher.1.process(bytes, program.as_deref());
        self.screen.feed(&screen);
        fired
    }

    fn carry_out(&self, fired: &Fired) -> Option<PaneEvent> {
        match fired {
            Fired::Notify(text) => {
                return Some(PaneEvent::Notify {
                    title: self.label(),
                    body: text.clone(),
                    happening: Happening::Message,
                });
            }
            Fired::Badge => self.state.borrow_mut().apply(&Signal::Bell),
            Fired::Send(text) => self.session.write(text.as_bytes()),
            Fired::Bell => {
                self.ring();
                self.state.borrow_mut().apply(&Signal::Bell);
            }
        }
        None
    }

    fn in_background(&self) -> bool {
        self.widget
            .root()
            .and_downcast::<gtk::Window>()
            .is_none_or(|window| !window.is_active())
    }

    /// The bell: a flash of the pane, the Dock icon asking for attention and a sound, as the settings say.
    fn ring(&self) {
        let config = config::current();
        self.ask_attention_away();
        if config.bell_visual {
            self.widget.add_css_class(BELL_CLASS);
            let widget = self.widget.clone();
            glib::timeout_add_local_once(BELL_FLASH, move || widget.remove_css_class(BELL_CLASS));
        }
        let sound = match config.bell_sound {
            BellSound::None => false,
            BellSound::Background => self.in_background(),
            BellSound::Always => true,
        };
        if sound {
            if let Some(display) = gdk::Display::default() {
                display.beep();
            }
        }
    }

    /// The Dock icon asks for attention while the window is not active, when the settings say so.
    fn ask_attention_away(&self) {
        if !config::current().bell_taskbar || !self.in_background() {
            return;
        }
        if let Some(application) = gio::Application::default() {
            launcher::ask_attention(&application);
        }
    }

    /// A command that ran at least as long as the settings say ended while no one looked: worth a notification.
    fn long_command_ended(&self, status: Option<i32>) -> Option<PaneEvent> {
        let since = self.command_since.take()?;
        let threshold = config::current().long_command;
        let seconds = since.elapsed().as_secs();
        if threshold == 0 || seconds < u64::from(threshold) || !self.in_background() {
            return None;
        }
        self.ask_attention_away();
        let (body, happening) = match status.filter(|code| *code != 0) {
            Some(code) => (trf("指令失敗，結束代碼 {0}，耗時 {1} 秒", &[&code, &seconds]), Happening::CommandFailed),
            None => (trf("指令完成，耗時 {0} 秒", &[&seconds]), Happening::Command),
        };
        Some(PaneEvent::Notify { title: self.label(), body, happening })
    }

    /// Reads the foreground job, where the shell is and the branch there; true when any changed.
    pub fn poll(&self) -> bool {
        let job = self
            .session
            .foreground_group()
            .and_then(|group| jobs::foreground_job(group, self.session.shell_pid));
        let mut state = self.state.borrow_mut();
        let job_changed = state.job != job;
        state.job = job;
        drop(state);
        let directory = self.session.directory().or_else(|| self.directory());
        let repository = directory.as_deref().and_then(jobs::repository_of);
        let place_changed =
            *self.directory.borrow() != directory || *self.repository.borrow() != repository;
        if place_changed {
            self.directory.replace(directory);
            self.repository.replace(repository);
        }
        job_changed || place_changed | self.poll_usage() | self.poll_subagents()
    }

    /// Reads again what Claude Code said in it, when its files were written since; true when they were.
    fn poll_usage(&self) -> bool {
        let stamp = usage::stamp(&self.session_name);
        if self.usage_stamp.replace(stamp) == stamp {
            return false;
        }
        self.usage.replace(stamp.and_then(|_| usage::read_usage(&self.session_name)));
        true
    }

    /// Reads the session's subagents again when their folder changed, and every time while one runs: its end only
    /// rewrites a file in it. True when they changed.
    fn poll_subagents(&self) -> bool {
        let folder = self.usage.borrow().as_ref().and_then(|usage| usage.transcript.as_deref().map(subagents::folder));
        let stamp = folder.as_deref().and_then(subagents::stamp);
        let running = self.subagents.borrow().running() > 0;
        if self.subagents_stamp.replace(stamp) == stamp && !running {
            return false;
        }
        let read = match (&folder, stamp) {
            (Some(folder), Some(_)) => subagents::read(folder, &self.subagents.borrow()),
            _ => Subagents::default(),
        };
        // While one runs its time goes on, so the bar is drawn again each time.
        let changed = read != *self.subagents.borrow() || running;
        self.subagents.replace(read);
        changed
    }

    /// The subagents of its Claude Code session that run, with those that ran alongside them.
    pub fn subagents(&self) -> Subagents {
        self.subagents.borrow().clone()
    }

    /// The folder Claude Code keeps its session's subagents in.
    pub fn subagents_folder(&self) -> Option<PathBuf> {
        self.usage.borrow().as_ref()?.transcript.as_deref().map(subagents::folder)
    }

    /// What Claude Code last said of its session in it.
    pub fn usage(&self) -> Option<Usage> {
        self.usage.borrow().clone()
    }

    pub fn status(&self) -> Status {
        self.state.borrow().status()
    }

    /// A program's copy through OSC 52, as the settings allow it.
    fn copy_for_program(&self, text: &str) {
        if config::current().osc52 != Osc52Policy::Off {
            self.screen.clipboard().set_text(text);
        }
    }

    /// Tells a program what the clipboard holds, when the settings let it read it.
    fn answer_clipboard(self: &Rc<Self>, terminator: Terminator) {
        if config::current().osc52 != Osc52Policy::ReadWrite {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Some(pane) = weak.upgrade() else {
                return;
            };
            let text = match pane.screen.clipboard().read_text_future().await {
                Ok(text) => text.map(|text| text.to_string()).unwrap_or_default(),
                Err(error) => {
                    eprintln!("nekoland-terminal: could not read the clipboard for a program: {error}");
                    String::new()
                }
            };
            pane.session.write(&osc::clipboard_answer(&text, terminator));
        });
    }

    /// What a right click does when it is not a menu: copies what is selected, else pastes.
    fn copy_or_paste(self: &Rc<Self>) {
        if self.screen.has_selection() {
            self.screen.copy_clipboard_format(vte4::Format::Text);
            self.screen.unselect_all();
        } else {
            self.paste();
        }
    }

    /// Pastes the clipboard, asking first when it is several lines or large, as the settings say.
    /// Labels the URLs, paths and hashes on the screen to pick by the keyboard, or stops when it is on.
    pub fn toggle_hints(&self) {
        self.hints.toggle(self);
    }

    /// Types `text` into what runs in the pane, as a snippet does.
    pub fn send(&self, text: &str) {
        self.session.write(text.as_bytes());
    }

    pub fn paste(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.screen
            .clipboard()
            .read_text_async(gio::Cancellable::NONE, move |result| {
                let Some(pane) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(Some(text)) => {
                        pane.paste_checked(text.to_string());
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("nekoland-terminal: could not read the clipboard: {error}"),
                }
            });
    }

    /// Pastes `text`, asking first as the settings say; false when it asks.
    pub fn paste_checked(self: &Rc<Self>, text: String) -> bool {
        let Some(heading) = paste_question(&text, &config::current()) else {
            self.screen.paste_text(&text);
            return true;
        };
        let preview: Vec<&str> = text.lines().take(PASTE_PREVIEW_LINES).collect();
        let dialog = adw::AlertDialog::new(Some(&heading), Some(&preview.join("\n")));
        dialog.add_responses(&[("cancel", tr("取消")), ("paste", tr("貼上"))]);
        dialog.set_response_appearance("paste", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("paste"));
        dialog.set_close_response("cancel");
        let pane = self.clone();
        dialog.connect_response(Some("paste"), move |_, _| pane.screen.paste_text(&text));
        dialog.present(Some(&self.screen));
        false
    }

    /// The id the automation API knows it by, the one its programs get as `NEKOLAND_TERMINAL_PANE`.
    pub fn id(&self) -> &str {
        &self.session_name
    }

    /// Hangs up on what runs in it; the pane goes once its shell exits.
    pub fn hang_up(&self) {
        self.session.hang_up();
    }

    /// The last part of the folder the shell is in, or `~` at home, as the bar names it.
    pub fn folder_label(&self) -> Option<String> {
        let directory = self.directory()?;
        if directory == glib::home_dir() {
            return Some("~".to_string());
        }
        let name = directory.file_name()?.to_string_lossy().into_owned();
        Some(name)
    }

    pub fn acknowledge(&self) {
        self.state.borrow_mut().acknowledge();
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn directory(&self) -> Option<PathBuf> {
        self.directory.borrow().clone()
    }

    pub fn repository(&self) -> Option<Repository> {
        self.repository.borrow().clone()
    }

    /// The shell the pane started; what runs in it is under it.
    pub fn shell_pid(&self) -> i32 {
        self.session.shell_pid
    }

    /// The program running in it, for a confirmation before it is closed.
    pub fn running_program(&self) -> Option<String> {
        self.state.borrow().job.as_ref().map(|job| job.program.clone())
    }

    pub fn agent_name(&self) -> Option<String> {
        self.state.borrow().agent_name()
    }

    /// What a part of the title format holds for this pane.
    fn title_part(&self, part: title::Part) -> Option<String> {
        match part {
            title::Part::Title => self
                .state
                .borrow()
                .reported_title()
                .or_else(|| self.screen.window_title().map(|title| title.to_string())),
            title::Part::Folder => self.folder_label(),
            title::Part::Profile => Some(self.profile.clone()),
            title::Part::Shell => self.running_program().or_else(|| Some(self.profile.clone())),
            title::Part::Agent => self.agent_name(),
        }
    }

    /// What the tab is called when no one named it, as the title format says.
    pub fn label(&self) -> String {
        let label = title::compose(&config::current().tab_title, |part| self.title_part(part));
        if label.is_empty() {
            tr(UNTITLED).to_string()
        } else {
            label
        }
    }

    /// Where it is, as Nekominal says it: its folder and the branch there.
    pub fn place(&self) -> String {
        let branch = self
            .repository
            .borrow()
            .as_ref()
            .and_then(|repository| repository.branch.clone());
        match (self.folder_label(), branch) {
            (Some(folder), Some(branch)) => format!("{folder} · {branch}"),
            (Some(folder), None) => folder,
            (None, Some(branch)) => branch,
            (None, None) => String::new(),
        }
    }

    /// The agent's message, the notification, or why the command failed.
    pub fn message(&self) -> Option<String> {
        self.state.borrow().message()
    }
}

/// The question a paste asks before it goes in, as Nekominal's: its lines or its size; None when it goes in at once.
pub fn paste_question(text: &str, config: &config::Config) -> Option<String> {
    let lines = text.trim_end_matches(['\r', '\n']).lines().count();
    if config.warn_multiline && lines > 1 {
        return Some(trf("貼上 {0} 行？", &[&lines]));
    }
    let limit = config.warn_large_kib as usize * KIB;
    (limit > 0 && text.len() >= limit).then(|| trf("貼上 {0}？", &[&glib::format_size(text.len() as u64)]))
}

impl Pane {
    /// Names the menu again in the language now chosen.
    pub fn relabel(&self) {
        self.screen.set_context_menu_model(Some(&context_menu()));
    }
}

fn context_menu() -> gio::Menu {
    let edit = gio::Menu::new();
    edit.append(Some(tr("複製")), Some("win.copy"));
    edit.append(Some(tr("貼上")), Some("win.paste"));
    edit.append(Some(tr("尋找")), Some("win.find"));
    let split = gio::Menu::new();
    split.append(Some(tr("向右分割")), Some("win.split-right"));
    split.append(Some(tr("向下分割")), Some("win.split-down"));
    let menu = gio::Menu::new();
    menu.append_section(None, &edit);
    menu.append_section(None, &split);
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paste_asks_when_it_is_several_lines_or_large() {
        let config = config::Config::default();
        assert_eq!(paste_question("ls\n", &config), None);
        assert_eq!(paste_question("a\nb\n", &config).as_deref(), Some("貼上 2 行？"));
        assert!(paste_question(&"x".repeat(6 * KIB), &config).is_some());
        let quiet = config::Config { warn_multiline: false, warn_large_kib: 0, ..config };
        assert_eq!(paste_question("a\nb", &quiet), None);
    }
}
