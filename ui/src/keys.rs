//! Settings' 鍵盤快速鍵 page: whose shortcuts Nekoland takes, macOS's, Windows' or Linux's,
//! and every action's keys in them, each of which can be given others, taken away or put
//! back, as the shortcuts crate keeps them.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::{gdk, glib};
use shortcuts::{Action, AppKeys, Combo, Custom, Scheme};

use crate::{group, set, unset, Config};

/// The scheme and the configuration's own keys, as this page last wrote them.
struct Shortcuts {
    scheme: Scheme,
    custom: Custom,
}

impl Shortcuts {
    fn read(config: &Config) -> Self {
        let value = |key: &str| config.0.get(key).cloned();
        Self {
            scheme: value("shortcut_scheme")
                .and_then(|value| value.try_into().ok())
                .unwrap_or_default(),
            custom: value("shortcuts")
                .and_then(|value| {
                    value
                        .try_into()
                        .inspect_err(|err| eprintln!("shortcuts: {err}"))
                        .ok()
                })
                .unwrap_or_default(),
        }
    }

    /// Gives `id` these keys in the scheme, or the scheme's for None.
    fn give(&mut self, id: &str, keys: Option<Vec<Combo>>) {
        let own = self.custom.entry(self.scheme).or_default();
        match keys {
            Some(keys) => {
                own.insert(id.to_string(), keys.iter().map(Combo::to_string).collect());
            }
            None => {
                own.remove(id);
            }
        }
        self.custom.retain(|_, own| !own.is_empty());
    }

    fn write(&self) {
        if self.custom.is_empty() {
            unset("shortcuts");
            return;
        }
        match toml::Value::try_from(&self.custom) {
            Ok(value) => set(&format!("shortcuts = {value}")),
            Err(err) => eprintln!("shortcuts: {err}"),
        }
    }

    fn keys(&self, id: &str) -> Vec<Combo> {
        shortcuts::keys(self.scheme, &self.custom, id)
    }

    /// The action other than `id` that has `combo` now.
    fn holder(&self, combo: &Combo, id: &str) -> Option<&'static Action> {
        shortcuts::bindings(self.scheme, &self.custom)
            .into_iter()
            .find(|(action, keys)| action.id != id && keys.contains(combo))
            .map(|(action, _)| action)
    }
}

/// What a row shows for an action's keys: an application action without keys of the
/// scheme's still has the keys applications take by themselves.
fn keys_label(shortcuts: &Shortcuts, action: &Action) -> Option<String> {
    let scheme = shortcuts.scheme;
    let keys = shortcuts.keys(action.id);
    if keys.is_empty() {
        let native = shortcuts::native(action, "", &AppKeys::new());
        return match native.as_slice() {
            [only] => Some(only.label(scheme)),
            _ => None,
        };
    }
    Some(
        keys.iter()
            .map(|combo| combo.label(scheme))
            .collect::<Vec<_>>()
            .join("、"),
    )
}

/// The page, and what the row that opens it says: the scheme's name.
pub fn shortcuts_page(config: &Config) -> (adw::PreferencesPage, impl Fn() -> String + 'static) {
    let state = Rc::new(RefCell::new(Shortcuts::read(config)));
    let page = adw::PreferencesPage::new();

    let schemes = group("");
    let scheme_row = adw::ComboRow::builder()
        .title("快速鍵配置")
        .model(&gtk::StringList::new(&Scheme::ALL.map(Scheme::title)))
        .selected(
            Scheme::ALL
                .iter()
                .position(|scheme| *scheme == state.borrow().scheme)
                .unwrap_or(0) as u32,
        )
        .build();
    schemes.add(&scheme_row);
    page.add(&schemes);

    let mut rows = Vec::new();
    for shown in shortcuts::Group::ALL {
        let listed = group(shown.title());
        for action in shortcuts::actions().iter().filter(|action| action.group == shown) {
            let row = adw::ActionRow::builder()
                .title(action.title)
                .activatable(true)
                .build();
            let keys = gtk::Label::builder().css_classes(["dim-label"]).build();
            let reset = gtk::Button::builder()
                .icon_name("edit-undo-symbolic")
                .tooltip_text("還原預設值")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            row.add_suffix(&keys);
            row.add_suffix(&reset);
            listed.add(&row);
            rows.push((action, row, keys, reset));
        }
        page.add(&listed);
    }

    let restore = group("");
    let restore_row = adw::ActionRow::builder()
        .title("還原所有快速鍵")
        .activatable(true)
        .css_classes(["error"])
        .build();
    restore.add(&restore_row);
    page.add(&restore);

    let rows = Rc::new(rows);
    let refresh: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[strong]
        state,
        #[strong]
        rows,
        #[weak]
        restore,
        move || {
            let shortcuts = state.borrow();
            for (action, _, keys, reset) in rows.iter() {
                keys.set_label(&keys_label(&shortcuts, action).unwrap_or_else(|| "未設定".into()));
                reset.set_visible(shortcuts::is_custom(
                    shortcuts.scheme,
                    &shortcuts.custom,
                    action.id,
                ));
            }
            restore.set_visible(shortcuts.custom.contains_key(&shortcuts.scheme));
        }
    ));
    refresh();

    scheme_row.connect_selected_notify(glib::clone!(
        #[strong]
        state,
        #[strong]
        refresh,
        move |row| {
            let Some(scheme) = Scheme::ALL.get(row.selected() as usize).copied() else {
                return;
            };
            state.borrow_mut().scheme = scheme;
            set(&format!("shortcut_scheme = {:?}", scheme.name()));
            refresh();
        }
    ));
    for (action, row, _, reset) in rows.iter() {
        let action: &'static Action = action;
        row.connect_activated(glib::clone!(
            #[strong]
            state,
            #[strong]
            refresh,
            move |row| record(row, action, state.clone(), refresh.clone())
        ));
        reset.connect_clicked(glib::clone!(
            #[strong]
            state,
            #[strong]
            refresh,
            move |_| {
                let mut shortcuts = state.borrow_mut();
                shortcuts.give(action.id, None);
                shortcuts.write();
                drop(shortcuts);
                refresh();
            }
        ));
    }
    restore_row.connect_activated(glib::clone!(
        #[strong]
        state,
        #[strong]
        refresh,
        move |row| {
            let scheme = state.borrow().scheme;
            let alert = adw::AlertDialog::new(
                Some("還原所有快速鍵？"),
                Some(&format!("{} 自訂的快速鍵會全部移除。", scheme.title())),
            );
            alert.add_responses(&[("cancel", "取消"), ("restore", "還原")]);
            alert.set_response_appearance("restore", adw::ResponseAppearance::Destructive);
            alert.set_close_response("cancel");
            alert.connect_response(
                Some("restore"),
                glib::clone!(
                    #[strong]
                    state,
                    #[strong]
                    refresh,
                    move |_, _| {
                        let mut shortcuts = state.borrow_mut();
                        let scheme = shortcuts.scheme;
                        shortcuts.custom.remove(&scheme);
                        shortcuts.write();
                        drop(shortcuts);
                        refresh();
                    }
                ),
            );
            alert.present(Some(row));
        }
    ));

    let summary = move || state.borrow().scheme.title().to_string();
    (page, summary)
}

/// Whether a key is a modifier, which a combination is made with rather than of.
fn is_modifier(key: gdk::Key) -> bool {
    key.name().is_some_and(|name| {
        [
            "Shift_",
            "Control_",
            "Alt_",
            "Super_",
            "Meta_",
            "Hyper_",
            "ISO_Level",
        ]
        .iter()
        .any(|start| name.starts_with(start))
    })
}

/// Whether keys pressed alone may be a shortcut: those that type nothing.
fn may_be_alone(key: &str) -> bool {
    key.starts_with('F') && key[1..].parse::<u8>().is_ok() || ["Print", "Pause", "Insert"].contains(&key)
}

/// Takes the keys pressed next for `action`, in a dialog that has every key, Nekoland's
/// shortcuts too, while it is open.
fn record(
    row: &adw::ActionRow,
    action: &'static Action,
    state: Rc<RefCell<Shortcuts>>,
    refresh: Rc<dyn Fn()>,
) {
    let alert = adw::AlertDialog::new(Some(action.title), Some("按下新的組合鍵"));
    alert.add_response("cancel", "取消");
    alert.set_close_response("cancel");
    let (custom, bound) = {
        let shortcuts = state.borrow();
        (
            shortcuts::is_custom(shortcuts.scheme, &shortcuts.custom, action.id),
            !shortcuts.keys(action.id).is_empty(),
        )
    };
    if bound {
        alert.add_response("clear", "清除");
    }
    if custom {
        alert.add_response("default", "還原預設值");
    }

    // The keys taken, while they wait for 取代 because another action has them.
    let pending: Rc<RefCell<Option<(Combo, &'static Action)>>> = Rc::default();
    let apply = glib::clone!(
        #[strong]
        state,
        #[strong]
        refresh,
        move |combo: Combo, holder: Option<&'static Action>| {
            let mut shortcuts = state.borrow_mut();
            if let Some(holder) = holder {
                let rest: Vec<Combo> = shortcuts
                    .keys(holder.id)
                    .into_iter()
                    .filter(|keys| *keys != combo)
                    .collect();
                shortcuts.give(holder.id, Some(rest));
            }
            shortcuts.give(action.id, Some(vec![combo]));
            shortcuts.write();
            drop(shortcuts);
            refresh();
        }
    );
    let apply = Rc::new(apply);

    let taken = glib::clone!(
        #[weak]
        alert,
        #[strong]
        state,
        #[strong]
        pending,
        #[strong]
        apply,
        move |combo: Combo| {
            let holder = state.borrow().holder(&combo, action.id);
            match holder {
                None => {
                    apply(combo, None);
                    alert.close();
                }
                Some(holder) => {
                    let scheme = state.borrow().scheme;
                    alert.set_body(&format!("{} 已用於「{}」", combo.label(scheme), holder.title));
                    if !alert.has_response("replace") {
                        alert.add_response("replace", "取代");
                        alert.set_response_appearance("replace", adw::ResponseAppearance::Suggested);
                    }
                    *pending.borrow_mut() = Some((combo, holder));
                }
            }
        }
    );
    let taken = Rc::new(taken);

    // Super pressed and let go with nothing between is a shortcut of its own.
    let super_alone = Rc::new(std::cell::Cell::new(false));
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        alert,
        #[strong]
        taken,
        #[strong]
        super_alone,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, code, modifiers| {
            super_alone.set(matches!(key, gdk::Key::Super_L | gdk::Key::Super_R));
            if is_modifier(key) {
                return glib::Propagation::Stop;
            }
            // The dialog's own Escape cancels it, whatever modifiers seem held.
            if key == gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
            let logo = modifiers.contains(gdk::ModifierType::SUPER_MASK);
            // The key as it is with no modifier, so that Shift with 3 is 3.
            let plain = gdk::Display::default()
                .and_then(|display| display.map_keycode(code))
                .and_then(|keys| {
                    keys.into_iter()
                        .find(|(at, _)| at.group() == 0 && at.level() == 0)
                })
                .map(|(_, key)| key)
                .unwrap_or(key);
            let Some(name) = plain.name() else {
                return glib::Propagation::Stop;
            };
            let combo = Combo {
                ctrl,
                alt,
                shift: modifiers.contains(gdk::ModifierType::SHIFT_MASK),
                logo,
                key: name.to_string(),
            };
            if !(ctrl || alt || logo) && !may_be_alone(&combo.key) {
                alert.set_body("需搭配 Ctrl、Alt 或 Super");
                return glib::Propagation::Stop;
            }
            taken(combo);
            glib::Propagation::Stop
        }
    ));
    keys.connect_key_released(glib::clone!(
        #[strong]
        taken,
        #[strong]
        super_alone,
        move |_, key, _, _| {
            if super_alone.take() && matches!(key, gdk::Key::Super_L | gdk::Key::Super_R) {
                taken(Combo::parse(shortcuts::SUPER_ALONE).expect("Super alone is a combination"));
            }
        }
    ));
    alert.add_controller(keys);

    alert.connect_response(
        None,
        glib::clone!(
            #[strong]
            state,
            #[strong]
            refresh,
            #[strong]
            pending,
            #[strong]
            apply,
            move |_, response| match response {
                "replace" => {
                    if let Some((combo, holder)) = pending.take() {
                        apply(combo, Some(holder));
                    }
                }
                "clear" | "default" => {
                    let mut shortcuts = state.borrow_mut();
                    shortcuts.give(action.id, (response == "clear").then(Vec::new));
                    shortcuts.write();
                    drop(shortcuts);
                    refresh();
                }
                _ => {}
            }
        ),
    );

    // Nekoland's own shortcuts go to the dialog while it is open, as a key of theirs is
    // what it may be given.
    let toplevel = row
        .root()
        .and_then(|root| root.surface())
        .and_downcast::<gdk::Toplevel>();
    if let Some(toplevel) = &toplevel {
        toplevel.inhibit_system_shortcuts(None::<&gdk::Event>);
    }
    alert.connect_closed(move |_| {
        if let Some(toplevel) = &toplevel {
            toplevel.restore_system_shortcuts();
        }
    });
    alert.present(Some(row));
}
