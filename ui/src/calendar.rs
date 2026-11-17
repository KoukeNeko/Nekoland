//! `nekoland-calendar`: a small calendar, of GTK's and libadwaita's own widgets with no
//! styling of its own, so that it looks as the system's GTK style says: it is how
//! Nekoland's look for GTK applications is tried on an application that knows nothing of
//! it. A month to pick a day from, the day's events, kept in a file, and a dialog to add one.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use adw::prelude::*;
use gtk::{gio, glib};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Calendar";
const CALENDARS: [&str; 3] = ["個人", "工作", "節日"];
const WEEKDAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];

#[derive(Clone, PartialEq, Debug)]
struct Event {
    /// The day, as YYYY-MM-DD.
    date: String,
    /// The hour and minute; none for one that takes the whole day.
    time: Option<(u32, u32)>,
    calendar: usize,
    title: String,
}

impl Event {
    /// A line of the file events are kept in: day, time, calendar and title between tabs.
    fn line(&self) -> String {
        let time = self
            .time
            .map_or("-".to_string(), |(hour, minute)| format!("{hour:02}:{minute:02}"));
        format!(
            "{}\t{time}\t{}\t{}",
            self.date,
            self.calendar,
            self.title.replace(['\t', '\n'], " ")
        )
    }

    fn parse(line: &str) -> Option<Self> {
        let mut parts = line.splitn(4, '\t');
        let (date, time, calendar, title) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let time = match time.split_once(':') {
            Some((hour, minute)) => Some((hour.parse().ok()?, minute.parse().ok()?)),
            None => None,
        };
        Some(Self {
            date: date.to_string(),
            time,
            calendar: calendar.parse::<usize>().ok()?.min(CALENDARS.len() - 1),
            title: title.to_string(),
        })
    }

    fn when(&self) -> String {
        self.time.map_or("整天".to_string(), |(hour, minute)| {
            format!("{hour:02}:{minute:02}")
        })
    }
}

fn events_file() -> PathBuf {
    glib::user_data_dir().join("nekoland/calendar.tsv")
}

fn load() -> Vec<Event> {
    std::fs::read_to_string(events_file())
        .map(|text| text.lines().filter_map(Event::parse).collect())
        .unwrap_or_default()
}

fn save(events: &[Event]) {
    let path = events_file();
    let text: String = events.iter().map(|event| event.line() + "\n").collect();
    if let Err(err) =
        std::fs::create_dir_all(path.parent().unwrap_or(&path)).and_then(|()| std::fs::write(&path, text))
    {
        eprintln!("{}: {err}", path.display());
    }
}

/// What the window shows and changes.
struct State {
    events: RefCell<Vec<Event>>,
    /// Which calendars' events are listed.
    shown: RefCell<[bool; CALENDARS.len()]>,
    query: RefCell<String>,
    month: gtk::Calendar,
    list: gtk::ListBox,
    empty: adw::StatusPage,
    day: adw::NavigationPage,
    toasts: adw::ToastOverlay,
}

impl State {
    fn date(&self) -> String {
        self.month
            .date()
            .format("%Y-%m-%d")
            .map(String::from)
            .unwrap_or_default()
    }

    /// Shows the chosen day's events, earliest first, and marks the month's days that have any.
    fn refresh(self: &Rc<Self>) {
        let chosen = self.month.date();
        let weekday = WEEKDAYS[(chosen.day_of_week().clamp(1, 7) - 1) as usize];
        self.day.set_title(&format!(
            "{}月{}日 週{weekday}",
            chosen.month(),
            chosen.day_of_month()
        ));

        self.month.clear_marks();
        let month = format!("{:04}-{:02}-", chosen.year(), chosen.month());
        for event in self.events.borrow().iter() {
            if let Some(day) = event.date.strip_prefix(&month).and_then(|day| day.parse().ok()) {
                self.month.mark_day(day);
            }
        }

        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let date = self.date();
        let query = self.query.borrow().to_lowercase();
        let shown = *self.shown.borrow();
        let mut today: Vec<(usize, Event)> = self
            .events
            .borrow()
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, event)| {
                shown[event.calendar]
                    && if query.is_empty() {
                        event.date == date
                    } else {
                        event.title.to_lowercase().contains(&query)
                    }
            })
            .collect();
        today.sort_by(|(_, a), (_, b)| (&a.date, a.time).cmp(&(&b.date, b.time)));
        for (index, event) in &today {
            let found = if query.is_empty() {
                String::new()
            } else {
                format!("{} · ", event.date)
            };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&event.title))
                .subtitle(format!("{found}{} · {}", event.when(), CALENDARS[event.calendar]))
                .build();
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("刪除")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let (state, index) = (self.clone(), *index);
            remove.connect_clicked(move |_| state.remove(index));
            row.add_suffix(&remove);
            self.list.append(&row);
        }
        self.list.set_visible(!today.is_empty());
        self.empty.set_visible(today.is_empty());
        self.empty.set_title(if query.is_empty() {
            "沒有行程"
        } else {
            "找不到行程"
        });
    }

    fn add(self: &Rc<Self>, event: Event) {
        self.events.borrow_mut().push(event);
        save(&self.events.borrow());
        self.refresh();
    }

    /// Takes an event away, and offers to put it back.
    fn remove(self: &Rc<Self>, index: usize) {
        let event = self.events.borrow_mut().remove(index);
        save(&self.events.borrow());
        self.refresh();
        let toast = adw::Toast::builder()
            .title(format!("已刪除「{}」", event.title))
            .button_label("復原")
            .build();
        let state = self.clone();
        toast.connect_button_clicked(move |_| state.add(event.clone()));
        self.toasts.add_toast(toast);
    }
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build);
    app.run()
}

fn build(app: &adw::Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    let state = Rc::new(State {
        events: RefCell::new(load()),
        shown: RefCell::new([true; CALENDARS.len()]),
        query: RefCell::default(),
        month: gtk::Calendar::builder().margin_start(12).margin_end(12).build(),
        list: gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .valign(gtk::Align::Start)
            .build(),
        empty: adw::StatusPage::builder()
            .icon_name("x-office-calendar-symbolic")
            .vexpand(true)
            .build(),
        day: adw::NavigationPage::builder().title("行事曆").build(),
        toasts: adw::ToastOverlay::new(),
    });

    // The side: the month, and which calendars are shown.
    let calendars = adw::PreferencesGroup::builder()
        .title("行事曆")
        .margin_top(18)
        .margin_start(12)
        .margin_end(12)
        .build();
    for (index, name) in CALENDARS.iter().enumerate() {
        let check = gtk::CheckButton::builder()
            .active(true)
            .valign(gtk::Align::Center)
            .build();
        let row = adw::ActionRow::builder()
            .title(*name)
            .activatable_widget(&check)
            .build();
        row.add_prefix(&check);
        check.connect_toggled(glib::clone!(
            #[strong]
            state,
            move |check| {
                state.shown.borrow_mut()[index] = check.is_active();
                state.refresh();
            }
        ));
        calendars.add(&row);
    }
    let side = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    side.append(&state.month);
    side.append(&calendars);
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("新增行程")
        .build();
    let side_bar = adw::HeaderBar::new();
    side_bar.pack_start(&add);
    let side_view = adw::ToolbarView::new();
    side_view.add_top_bar(&side_bar);
    side_view.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&side)
            .build(),
    ));

    // The day: its events, a search over all of them, and a menu.
    let search = gtk::SearchEntry::builder().placeholder_text("搜尋行程").build();
    let search_bar = gtk::SearchBar::builder()
        .child(&adw::Clamp::builder().maximum_size(400).child(&search).build())
        .build();
    search_bar.connect_entry(&search);
    let find = gtk::ToggleButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("搜尋")
        .build();
    find.bind_property("active", &search_bar, "search-mode-enabled")
        .bidirectional()
        .build();
    search.connect_search_changed(glib::clone!(
        #[strong]
        state,
        move |search| {
            *state.query.borrow_mut() = search.text().to_string();
            state.refresh();
        }
    ));
    let menu = gio::Menu::new();
    menu.append(Some("今天"), Some("app.today"));
    menu.append(Some("刪除所有行程⋯"), Some("app.clear"));
    let day_bar = adw::HeaderBar::new();
    day_bar.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("選單")
            .menu_model(&menu)
            .build(),
    );
    day_bar.pack_end(&find);
    let events = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    events.append(&state.list);
    events.append(&state.empty);
    let page = adw::Clamp::builder()
        .maximum_size(640)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .child(&events)
        .build();
    state.toasts.set_child(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&page)
            .build(),
    ));
    let day_view = adw::ToolbarView::new();
    day_view.add_top_bar(&day_bar);
    day_view.add_top_bar(&search_bar);
    day_view.set_content(Some(&state.toasts));
    state.day.set_child(Some(&day_view));

    let split = adw::NavigationSplitView::builder()
        .sidebar(
            &adw::NavigationPage::builder()
                .title("行事曆")
                .child(&side_view)
                .build(),
        )
        .content(&state.day)
        .min_sidebar_width(300.0)
        .build();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("行事曆")
        .default_width(920)
        .default_height(640)
        .content(&split)
        .build();

    state.month.connect_day_selected(glib::clone!(
        #[strong]
        state,
        move |_| state.refresh()
    ));
    for signal in ["next-month", "prev-month", "next-year", "prev-year"] {
        state.month.connect_local(
            signal,
            false,
            glib::clone!(
                #[strong]
                state,
                move |_| {
                    state.refresh();
                    None
                }
            ),
        );
    }
    add.connect_clicked(glib::clone!(
        #[strong]
        state,
        #[weak]
        window,
        move |_| ask_event(&state, &window)
    ));

    let today = gio::SimpleAction::new("today", None);
    today.connect_activate(glib::clone!(
        #[strong]
        state,
        move |_, _| {
            if let Ok(now) = glib::DateTime::now_local() {
                state.month.select_day(&now);
            }
        }
    ));
    app.add_action(&today);
    let clear = gio::SimpleAction::new("clear", None);
    clear.connect_activate(glib::clone!(
        #[strong]
        state,
        #[weak]
        window,
        move |_, _| {
            let dialog =
                adw::AlertDialog::new(Some("刪除所有行程？"), Some("所有日期的行程都會刪除，無法復原。"));
            dialog.add_responses(&[("cancel", "取消"), ("delete", "刪除")]);
            dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            dialog.connect_response(
                Some("delete"),
                glib::clone!(
                    #[strong]
                    state,
                    move |_, _| {
                        state.events.borrow_mut().clear();
                        save(&state.events.borrow());
                        state.refresh();
                    }
                ),
            );
            dialog.present(Some(&window));
        }
    ));
    app.add_action(&clear);

    state.refresh();
    window.present();
}

/// Asks for an event on the chosen day: its title, whether it takes the whole day or when
/// it starts, and its calendar.
fn ask_event(state: &Rc<State>, window: &adw::ApplicationWindow) {
    // Return in the title adds it, as the button does.
    let title = adw::EntryRow::builder()
        .title("標題")
        .activates_default(true)
        .build();
    let all_day = adw::SwitchRow::builder().title("整天").build();
    let hour = adw::SpinRow::with_range(0.0, 23.0, 1.0);
    hour.set_title("時");
    hour.set_value(9.0);
    let minute = adw::SpinRow::with_range(0.0, 55.0, 5.0);
    minute.set_title("分");
    for row in [&hour, &minute] {
        all_day
            .bind_property("active", row, "sensitive")
            .invert_boolean()
            .sync_create()
            .build();
    }
    let calendar = adw::ComboRow::builder()
        .title("行事曆")
        .model(&gtk::StringList::new(&CALENDARS))
        .build();
    let group = adw::PreferencesGroup::new();
    for row in [
        title.upcast_ref::<gtk::Widget>(),
        all_day.upcast_ref(),
        hour.upcast_ref(),
        minute.upcast_ref(),
        calendar.upcast_ref(),
    ] {
        group.add(row);
    }

    // An alert is as wide as what it holds.
    group.set_width_request(372);
    let alert = adw::AlertDialog::new(Some("新增行程"), None);
    alert.set_extra_child(Some(&group));
    alert.add_responses(&[("cancel", "取消"), ("add", "新增")]);
    alert.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    alert.set_response_enabled("add", false);
    alert.set_default_response(Some("add"));
    alert.set_close_response("cancel");
    title.connect_changed(glib::clone!(
        #[weak]
        alert,
        move |title| alert.set_response_enabled("add", !title.text().trim().is_empty())
    ));
    alert.connect_response(
        None,
        glib::clone!(
            #[strong]
            state,
            #[strong]
            title,
            move |_, response| {
                if response != "add" {
                    return;
                }
                state.add(Event {
                    date: state.date(),
                    time: (!all_day.is_active()).then(|| (hour.value() as u32, minute.value() as u32)),
                    calendar: calendar.selected() as usize,
                    title: title.text().trim().to_string(),
                });
            }
        ),
    );
    // Typing goes to the title at once.
    alert.set_focus(Some(&title));
    alert.present(Some(window));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_read_back_as_it_was_written() {
        let timed = Event {
            date: "2026-11-16".into(),
            time: Some((9, 5)),
            calendar: 1,
            title: "週會\t討論".into(),
        };
        assert_eq!(timed.line(), "2026-11-16\t09:05\t1\t週會 討論");
        assert_eq!(
            Event::parse(&timed.line()).map(|event| event.when()),
            Some("09:05".into())
        );
        let whole = Event {
            time: None,
            title: "生日".into(),
            ..timed
        };
        assert_eq!(Event::parse(&whole.line()), Some(whole));
        assert_eq!(Event::parse("not an event"), None);
    }
}
