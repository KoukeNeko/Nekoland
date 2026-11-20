//! The rows of the phone notifications on the Terminal page, as Nekominal's: where they go, the token, which events,
//! and a test that says what it came to.

use std::{cell::Cell, rc::Rc, time::Instant};

use adw::prelude::*;
use gtk::glib;

use super::{
    lang::{tr, trf},
    config,
    push::{self, PushEvent},
    settings::{change, field, mark_refused},
};

pub const HEADING: &str = "手機通知";
pub const URL: &str = "傳送到";
pub const KIND: &str = "格式";
pub const WHEN: &str = "何時傳送";
pub const TOKEN: &str = "存取權杖";
pub const EVENTS: &str = "傳送哪些事件";
pub const TEST: &str = "測試";
const URL_NOTE: &str = "ntfy 主題，或接收 POST 的 https 網址";
const TOKEN_NOTE: &str = "選填，以 Bearer 傳送，不寫進記錄";
const SECONDS_IN_MINUTE: u64 = 60;

/// The address the notifications go to; one the push would refuse is marked and kept out.
pub fn url_row() -> adw::ActionRow {
    let field = field(&config::current().push.url, |field, text| {
        let url = text.trim().to_owned();
        if !push::is_valid_url(&url) {
            mark_refused(field, Some(push::BAD_URL));
            return;
        }
        mark_refused(field, None);
        change(|config| config.push.url = url);
    });
    field.set_hexpand(true);
    field.set_placeholder_text(Some("https://ntfy.sh/…"));
    let row = adw::ActionRow::builder().title(tr(URL)).subtitle(tr(URL_NOTE)).build();
    row.add_suffix(&field);
    row
}

/// The token, hidden as it is typed and shown only on asking.
pub fn token_row() -> adw::ActionRow {
    let entry = gtk::PasswordEntry::builder()
        .text(config::current().push.token)
        .show_peek_icon(true)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let take = |entry: &gtk::PasswordEntry| {
        let token = entry.text().trim().to_owned();
        if config::current().push.token != token {
            change(|config| config.push.token = token);
        }
    };
    entry.connect_activate(take);
    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave(glib::clone!(
        #[weak]
        entry,
        move |_| take(&entry)
    ));
    entry.add_controller(focus);
    let row = adw::ActionRow::builder().title(tr(TOKEN)).subtitle(tr(TOKEN_NOTE)).build();
    row.add_suffix(&entry);
    row
}

/// The events pushed, any of them on, each a switch under the row.
pub fn events_row() -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder().title(tr(EVENTS)).build();
    let chosen = config::current().push.events;
    for event in PushEvent::ALL {
        let switch = adw::SwitchRow::builder().title(event.label()).active(chosen.contains(&event)).build();
        switch.connect_active_notify(move |switch| {
            let on = switch.is_active();
            change(|config| set_event(&mut config.push.events, event, on));
        });
        row.add_row(&switch);
    }
    row
}

/// `event` in or out of `events`, kept in the order of [`PushEvent::ALL`].
fn set_event(events: &mut Vec<PushEvent>, event: PushEvent, on: bool) {
    events.retain(|listed| *listed != event);
    if on {
        events.push(event);
        events.sort();
    }
}

/// A button that sends a test with the settings as they are, and beside it what it came to.
pub fn test_row() -> adw::ActionRow {
    let status = gtk::Label::builder().css_classes(["caption"]).valign(gtk::Align::Center).build();
    let button = gtk::Button::builder().label(tr("傳送測試通知")).valign(gtk::Align::Center).build();
    let row = adw::ActionRow::builder().title(tr(TEST)).build();
    row.add_suffix(&status);
    row.add_suffix(&button);
    let sent_at: Rc<Cell<Option<Instant>>> = Rc::default();
    button.connect_clicked(glib::clone!(
        #[weak]
        row,
        #[weak]
        status,
        move |button| {
            let push = config::current().push;
            if push.url.is_empty() {
                show_status(&status, tr(push::BAD_URL), "error");
                return;
            }
            sent_at.set(Some(Instant::now()));
            button.set_sensitive(false);
            show_status(&status, tr("傳送中"), "dim-label");
            let button = button.clone();
            let sent_at = sent_at.clone();
            push::send_test(push, move |result| {
                button.set_sensitive(true);
                let ago = sent_at.get().map_or_else(|| tr("剛才").to_owned(), minutes_ago);
                match result {
                    Ok(()) => {
                        show_status(&status, tr("● 已送達"), "success");
                        row.set_subtitle(&trf("上次傳送：{0}，手機已收到", &[&ago]));
                    }
                    Err(why) => {
                        show_status(&status, &why, "error");
                        row.set_subtitle(&trf("上次傳送：{0}", &[&ago]));
                    }
                }
            });
        }
    ));
    row
}

fn show_status(status: &gtk::Label, text: &str, class: &str) {
    status.set_text(text);
    status.set_css_classes(&["caption", class]);
}

fn minutes_ago(since: Instant) -> String {
    match since.elapsed().as_secs() / SECONDS_IN_MINUTE {
        0 => tr("剛才").to_owned(),
        minutes => trf("{0} 分鐘前", &[&minutes]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_stay_in_their_order() {
        let mut events = vec![PushEvent::Done];
        set_event(&mut events, PushEvent::Waiting, true);
        set_event(&mut events, PushEvent::Message, true);
        assert_eq!(events, [PushEvent::Waiting, PushEvent::Done, PushEvent::Message]);
        set_event(&mut events, PushEvent::Done, false);
        assert_eq!(events, [PushEvent::Waiting, PushEvent::Message]);
    }
}
