//! The input method for what Nekoland types into itself: the applications' search field and
//! a folder's name. It talks to Fcitx 5 over D-Bus, as the GTK and Qt input method modules
//! do: keys go to an input context, and what it commits, the text being composed and its
//! candidates come back, the candidates to be drawn by Nekoland. Keys it does not take come
//! back too, to be handled as typed.

use std::sync::mpsc;

use smithay::reexports::calloop::channel::Sender;
use zbus::{blocking::Connection, blocking::MessageIterator, zvariant::OwnedObjectPath, MatchRule};

use crate::dbus::Event;

const FCITX: &str = "org.fcitx.Fcitx5";
const INPUT_METHOD: &str = "org.fcitx.Fcitx.InputMethod1";
const INPUT_CONTEXT: &str = "org.fcitx.Fcitx.InputContext1";
/// The text being composed, formatted; and the candidates drawn by the client, not Fcitx.
const CAPABILITIES: u64 = (1 << 1) | (1 << 4) | (1 << 39);

/// A key as Fcitx takes it: its symbol, its X key code, and the X modifier mask.
#[derive(Clone, Copy)]
pub struct Key {
    pub keysym: u32,
    pub keycode: u32,
    pub state: u32,
    pub release: bool,
    pub time: u32,
}

pub enum Request {
    /// A view that takes text opened: what one before it left composing is dropped.
    Focus,
    Key(Key),
}

/// What to choose from, the one lit, and a note such as the page.
pub type Candidates = (Vec<String>, Option<usize>, String);

/// What the input method says.
#[derive(Debug, Clone, PartialEq)]
pub enum ImeEvent {
    /// Text to put in, as typed.
    Commit(String),
    /// The text being composed, empty when there is none.
    Preedit(String),
    /// What to choose from; nothing when the list is empty.
    Candidates(Option<Candidates>),
    /// A key the input method did not take, to be handled as it would be without one.
    Key(u32),
}

pub struct Ime {
    requests: mpsc::Sender<Request>,
}

impl Ime {
    /// False when the input method has gone, as when Fcitx restarts: another is needed.
    pub fn send(&self, request: Request) -> bool {
        self.requests.send(request).is_ok()
    }
}

/// Connects to Fcitx and makes an input context. None when Fcitx is not running.
pub fn start(events: Sender<Event>) -> Option<Ime> {
    let connection = Connection::session()
        .inspect_err(|err| tracing::warn!("input method: {err}"))
        .ok()?;
    let reply = connection
        .call_method(
            Some(FCITX),
            "/org/freedesktop/portal/inputmethod",
            Some(INPUT_METHOD),
            "CreateInputContext",
            &(vec![("program", "nekoland")],),
        )
        .inspect_err(|err| tracing::warn!("input method: {err}"))
        .ok()?;
    let (path, _uuid): (OwnedObjectPath, Vec<u8>) = reply.body().deserialize().ok()?;
    if let Err(err) = connection.call_method(
        Some(FCITX),
        path.as_str(),
        Some(INPUT_CONTEXT),
        "SetCapability",
        &(CAPABILITIES,),
    ) {
        tracing::warn!("input method: {err}");
    }

    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(INPUT_CONTEXT)
        .ok()?
        .path(path.clone())
        .ok()?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &connection, Some(64)).ok()?;
    let signal_events = events.clone();
    std::thread::Builder::new()
        .name("nekoland-ime-signals".into())
        .spawn(move || {
            for message in signals.flatten() {
                let member = message
                    .header()
                    .member()
                    .map(|member| member.to_string())
                    .unwrap_or_default();
                let body = message.body();
                let event = match member.as_str() {
                    "CommitString" => body.deserialize::<String>().ok().map(ImeEvent::Commit),
                    "UpdateFormattedPreedit" => {
                        body.deserialize::<(Vec<(String, i32)>, i32)>()
                            .ok()
                            .map(|(parts, _)| {
                                ImeEvent::Preedit(parts.into_iter().map(|(text, _)| text).collect())
                            })
                    }
                    "UpdateClientSideUI" => body
                        .deserialize::<(
                            Vec<(String, i32)>,
                            i32,
                            Vec<(String, i32)>,
                            Vec<(String, i32)>,
                            Vec<(String, String)>,
                            i32,
                            i32,
                            bool,
                            bool,
                        )>()
                        .ok()
                        .map(|(_, _, aux_up, aux_down, candidates, index, ..)| {
                            let note = aux_up.into_iter().chain(aux_down).map(|(text, _)| text).collect();
                            let candidates: Vec<String> =
                                candidates.into_iter().map(|(_, text)| text).collect();
                            ImeEvent::Candidates(
                                (!candidates.is_empty())
                                    .then(|| (candidates, usize::try_from(index).ok(), note)),
                            )
                        }),
                    // Fcitx gives a key back that it handled only in part.
                    "ForwardKey" => body
                        .deserialize::<(u32, u32, bool)>()
                        .ok()
                        .filter(|(_, _, release)| !release)
                        .map(|(keysym, ..)| ImeEvent::Key(keysym)),
                    _ => None,
                };
                if let Some(event) = event {
                    let _ = signal_events.send(Event::Ime(event));
                }
            }
        })
        .ok()?;

    let (requests, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("nekoland-ime".into())
        .spawn(move || {
            let context = |method: &str| {
                connection.call_method(Some(FCITX), path.as_str(), Some(INPUT_CONTEXT), method, &())
            };
            for request in receiver {
                let key = match request {
                    Request::Key(key) => Some(key),
                    Request::Focus => None,
                };
                let result = match request {
                    Request::Focus => context("Reset").and_then(|_| context("FocusIn")).map(drop),
                    Request::Key(key) => connection
                        .call_method(
                            Some(FCITX),
                            path.as_str(),
                            Some(INPUT_CONTEXT),
                            "ProcessKeyEvent",
                            &(key.keysym, key.keycode, key.state, key.release, key.time),
                        )
                        .and_then(|reply| reply.body().deserialize::<bool>())
                        .map(|handled| {
                            if !handled && !key.release {
                                let _ = events.send(Event::Ime(ImeEvent::Key(key.keysym)));
                            }
                        }),
                };
                // The context is gone with Fcitx; the key is typed as it would be without it,
                // and the next one starts another.
                if let Err(err) = result {
                    tracing::warn!("input method: {err}");
                    if let Some(key) = key.filter(|key| !key.release) {
                        let _ = events.send(Event::Ime(ImeEvent::Key(key.keysym)));
                    }
                    break;
                }
            }
        })
        .ok()?;
    Some(Ime { requests })
}

impl crate::state::Nekoland {
    /// Hands a key to the input method while the applications' text field takes keys; a
    /// letter typed in the overview opens the applications first, as typing there does.
    /// Returns whether the key went to it.
    pub fn ime_key(
        &mut self,
        modifiers: &smithay::input::keyboard::ModifiersState,
        keysym: &smithay::input::keyboard::KeysymHandle<'_>,
        pressed: bool,
        time: u32,
    ) -> bool {
        use smithay::input::keyboard::Keysym;
        let sym = keysym.modified_sym();
        // Ctrl+Space switches the input method; other shortcuts stay the compositor's.
        let typing = self.overview.is_some()
            && self.menu_bar.open.is_none()
            && !(modifiers.alt || modifiers.logo)
            && (!modifiers.ctrl || sym == Keysym::space);
        if !typing {
            return false;
        }
        if !self.apps_shown() {
            if !(pressed && sym.key_char().is_some_and(|character| !character.is_control())) {
                return false;
            }
            self.show_apps("");
        }
        if self.ime.is_none() {
            self.ime = self.events.clone().and_then(start);
        }
        let (Some(ime), Some(apps)) = (
            &self.ime,
            self.overview.as_mut().and_then(|overview| overview.apps.as_mut()),
        ) else {
            return false;
        };
        let state = u32::from(modifiers.shift)
            | u32::from(modifiers.caps_lock) << 1
            | u32::from(modifiers.ctrl) << 2
            | u32::from(modifiers.logo) << 6;
        let sent = (apps.ime_focused || ime.send(Request::Focus))
            && ime.send(Request::Key(Key {
                keysym: sym.raw(),
                keycode: keysym.raw_code().raw(),
                state,
                release: !pressed,
                time,
            }));
        apps.ime_focused = sent;
        if !sent {
            self.ime = None;
        }
        sent
    }

    /// What the input method says goes into the applications' text field.
    pub fn ime_event(&mut self, event: ImeEvent) {
        use smithay::input::keyboard::Keysym;
        match event {
            ImeEvent::Commit(text) => {
                for character in text.chars() {
                    self.apps_key(Keysym::NoSymbol, Some(character));
                }
            }
            ImeEvent::Key(raw) => {
                let sym = Keysym::from(raw);
                self.apps_key(sym, sym.key_char());
            }
            ImeEvent::Preedit(text) => {
                if let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) {
                    apps.preedit = text;
                }
            }
            ImeEvent::Candidates(candidates) => {
                if let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) {
                    apps.candidates = candidates;
                }
            }
        }
    }
}
