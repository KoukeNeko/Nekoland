//! Settings' 使用者 page: the person logged in, with their name, picture and password and
//! whether the first console logs them in by itself, and the computer's other people, added,
//! made administrators and removed, through the system's own tools as root.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::{alert, as_root, group};

/// Where systemd takes the first console's login from, as install.sh starts Nekoland there.
const AUTOLOGIN: &str = "/etc/systemd/system/getty@tty1.service.d/nekoland-autologin.conf";
/// The group whose members sudo lets administer the computer, as Ubuntu has it.
const ADMINS: &str = "sudo";

/// Someone who can log in, as /etc/passwd and /etc/group have them.
#[derive(Clone, Debug, PartialEq)]
struct Person {
    login: String,
    name: String,
    admin: bool,
}

#[derive(Default)]
struct UsersPage {
    groups: RefCell<Vec<adw::PreferencesGroup>>,
}

/// The people of `passwd`: the accounts people get, from uid 1000, that have a shell to log
/// in with; the rest are the system's own.
fn people_in(passwd: &str, groups: &str) -> Vec<Person> {
    let admins: Vec<&str> = groups
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{ADMINS}:")))
        .and_then(|rest| rest.splitn(3, ':').nth(2))
        .map(|members| members.split(',').filter(|member| !member.is_empty()).collect())
        .unwrap_or_default();
    passwd
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            let [login, _, uid, _, gecos, _, shell] = fields[..] else {
                return None;
            };
            let uid: u32 = uid.parse().ok()?;
            let can_log_in = !shell.ends_with("nologin") && !shell.ends_with("false");
            ((1000..60000).contains(&uid) && can_log_in).then(|| Person {
                login: login.to_string(),
                // GECOS holds the full name first, then the room and telephones nobody fills in.
                name: Some(gecos.split(',').next().unwrap_or_default())
                    .filter(|name| !name.is_empty())
                    .unwrap_or(login)
                    .to_string(),
                admin: admins.contains(&login),
            })
        })
        .collect()
}

fn people() -> Vec<Person> {
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
    people_in(&read("/etc/passwd"), &read("/etc/group"))
}

/// A login name useradd takes and other programs agree on.
fn is_login_name(login: &str) -> bool {
    let mut chars = login.chars();
    login.len() <= 32
        && chars.next().is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn picture() -> PathBuf {
    glib::home_dir().join(".face")
}

fn logs_in_by_itself(login: &str) -> bool {
    std::fs::read_to_string(AUTOLOGIN).is_ok_and(|text| text.contains(&format!("--autologin {login} ")))
}

pub fn users_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let shown = Rc::new(UsersPage::default());
    fill(&page, &shown);
    page
}

fn fill(page: &adw::PreferencesPage, shown: &Rc<UsersPage>) {
    for old in shown.groups.take() {
        page.remove(&old);
    }
    let me = glib::user_name().to_string_lossy().into_owned();
    let everyone = people();
    let refresh = {
        let (page, shown) = (page.downgrade(), shown.clone());
        Rc::new(move || {
            if let Some(page) = page.upgrade() {
                fill(&page, &shown);
            }
        })
    };
    let failed = |page: &adw::PreferencesPage, title: &str| alert::notice(page, title, Some("請確認密碼正確後再試一次。"));

    let mine = group("");
    let person = everyone.iter().find(|person| person.login == me).cloned().unwrap_or(Person {
        login: me.clone(),
        name: me.clone(),
        admin: false,
    });
    let avatar = adw::Avatar::new(64, Some(&person.name), true);
    if let Ok(texture) = gdk::Texture::from_filename(picture()) {
        avatar.set_custom_image(Some(&texture));
    }
    let header = adw::ActionRow::builder()
        .title(person.name.as_str())
        .subtitle(if person.admin { format!("{} · 管理員", person.login) } else { person.login.clone() })
        .use_markup(false)
        .build();
    header.add_prefix(&avatar);
    let choose = gtk::Button::builder()
        .label("選擇圖片⋯")
        .valign(gtk::Align::Center)
        .build();
    choose.connect_clicked(glib::clone!(
        #[strong]
        refresh,
        move |button| choose_picture(button, refresh.clone())
    ));
    header.add_suffix(&choose);
    mine.add(&header);

    let name = adw::EntryRow::builder().title("全名").text(person.name.as_str()).show_apply_button(true).build();
    name.connect_apply(glib::clone!(
        #[strong]
        refresh,
        #[strong]
        me,
        #[weak]
        page,
        move |row| {
            let typed = row.text().trim().to_string();
            // A colon or a comma would end the field in /etc/passwd.
            if typed.is_empty() || typed.contains([':', ',', '\n']) {
                return alert::notice(&page, "無法使用這個名稱", Some("名稱不能是空白，也不能有冒號或逗號。"));
            }
            let refresh = refresh.clone();
            as_root("變更全名", &["usermod", "-c", &typed, &me], None, move |worked| {
                if !worked {
                    failed(&page, "無法變更全名");
                }
                refresh();
            });
        }
    ));
    mine.add(&name);

    let password = adw::ActionRow::builder().title("密碼").build();
    let change = gtk::Button::builder().label("變更⋯").valign(gtk::Align::Center).build();
    change.connect_clicked(glib::clone!(
        #[strong]
        me,
        #[weak]
        page,
        move |_| {
            let me = me.clone();
            let parent = page.clone();
            ask_password(&parent, "變更密碼", "變更", Rc::new(|| {}), move |typed| {
                as_root("變更密碼", &["chpasswd"], Some(format!("{me}:{typed}\n")), move |worked| {
                    if !worked {
                        failed(&page, "無法變更密碼");
                    }
                });
            });
        }
    ));
    password.add_suffix(&change);
    mine.add(&password);
    mine.add(&pin_row(&page, refresh.clone()));

    let automatic = adw::SwitchRow::builder()
        .title("自動登入")
        .active(logs_in_by_itself(&me))
        .build();
    automatic.connect_active_notify(glib::clone!(
        #[strong]
        refresh,
        #[strong]
        me,
        #[weak]
        page,
        move |row| {
            row.set_sensitive(false);
            let refresh = refresh.clone();
            let then = move |worked: bool| {
                if !worked {
                    failed(&page, "無法變更自動登入");
                }
                refresh();
            };
            if row.is_active() {
                // The login name is handed to sh as $1, never put into the script.
                let script = format!(
                    "mkdir -p \"$(dirname {AUTOLOGIN})\" && printf '[Service]\\nExecStart=\\nExecStart=-/sbin/agetty --autologin %s --noclear %%I $TERM\\n' \"$1\" > {AUTOLOGIN} && systemctl daemon-reload"
                );
                as_root("開啟自動登入", &["sh", "-c", &script, "sh", &me], None, then);
            } else {
                let script = format!("rm -f {AUTOLOGIN} && systemctl daemon-reload");
                as_root("關閉自動登入", &["sh", "-c", &script], None, then);
            }
        }
    ));
    mine.add(&automatic);
    page.add(&mine);

    let others = group("其他使用者");
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("新增使用者")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    add.connect_clicked(glib::clone!(
        #[strong]
        refresh,
        #[weak]
        page,
        move |_| add_person(&page, refresh.clone())
    ));
    others.set_header_suffix(Some(&add));
    let rest: Vec<&Person> = everyone.iter().filter(|person| person.login != me).collect();
    if rest.is_empty() {
        others.add(&adw::ActionRow::builder().title("沒有其他使用者").css_classes(["dim-label"]).build());
    }
    for person in rest {
        others.add(&other_row(&page, person, refresh.clone()));
    }
    page.add(&others);
    shown.groups.replace(vec![mine, others]);
}

fn other_row(page: &adw::PreferencesPage, person: &Person, refresh: Rc<dyn Fn()>) -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder()
        .title(person.name.as_str())
        .subtitle(person.login.as_str())
        .use_markup(false)
        .build();
    row.add_prefix(&adw::Avatar::new(32, Some(&person.name), true));
    let admin = adw::SwitchRow::builder().title("管理員").active(person.admin).build();
    let login = person.login.clone();
    admin.connect_active_notify(glib::clone!(
        #[strong]
        refresh,
        #[strong]
        login,
        #[weak]
        page,
        move |row| {
            row.set_sensitive(false);
            let verb = if row.is_active() { "-a" } else { "-d" };
            let refresh = refresh.clone();
            as_root("變更管理員", &["gpasswd", verb, &login, ADMINS], None, move |worked| {
                if !worked {
                    alert::notice(&page, "無法變更管理員", Some("請確認密碼正確後再試一次。"));
                }
                refresh();
            });
        }
    ));
    row.add_row(&admin);
    let remove = adw::ActionRow::builder().title("刪除使用者").activatable(true).build();
    remove.add_css_class("error");
    let name = person.name.clone();
    remove.connect_activated(glib::clone!(
        #[weak]
        page,
        move |_| {
            let (login, refresh) = (login.clone(), refresh.clone());
            let parent = page.clone();
            alert::confirm(
                &parent,
                &format!("刪除「{name}」？"),
                Some("其個人專屬檔案夾會留在 /home。"),
                "刪除",
                move || {
                    as_root("刪除使用者", &["userdel", &login], None, move |worked| {
                        if !worked {
                            alert::notice(&page, "無法刪除使用者", Some("使用者登入中時無法刪除。"));
                        }
                        refresh();
                    });
                },
            );
        }
    ));
    row.add_row(&remove);
    row
}

/// The rows a dialog asks with, in the list look of the rest of Settings.
fn form(rows: &[&gtk::Widget]) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .width_request(alert::FIELD_WIDTH)
        .css_classes(["boxed-list"])
        .build();
    for row in rows {
        list.append(*row);
    }
    list
}

/// The PIN that unlocks the lock screen, kept by `nekoland-lock` hashed in the data folder.
fn pin_is_set() -> bool {
    glib::user_data_dir().join("nekoland/pin").exists()
}

/// Runs `nekoland-lock` with a command that sets or takes away the PIN, the PIN on its
/// standard input; `then` gets whether it worked.
fn lock_pin(command: &'static str, pin: Option<String>, then: impl FnOnce(bool) + 'static) {
    let mut flags = gio::SubprocessFlags::STDERR_PIPE;
    if pin.is_some() {
        flags |= gio::SubprocessFlags::STDIN_PIPE;
    }
    let child = match gio::Subprocess::newv(&["nekoland-lock".as_ref(), std::ffi::OsStr::new(command)], flags) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("nekoland-lock {command}: {err}");
            return then(false);
        }
    };
    let waited = child.clone();
    child.communicate_utf8_async(pin.map(|pin| format!("{pin}\n")), gio::Cancellable::NONE, move |result| {
        let worked = matches!(&result, Ok(_)) && waited.is_successful();
        if let Ok((_, Some(said))) = &result {
            if !said.is_empty() {
                eprintln!("nekoland-lock {command}: {}", said.trim());
            }
        }
        then(worked);
    });
}

/// PIN: digits, six at least, that unlock the lock screen as the password does; five wrong
/// ones in a lock leave only the password.
fn pin_row(page: &adw::PreferencesPage, refresh: Rc<dyn Fn()>) -> adw::ActionRow {
    let set = pin_is_set();
    let row = adw::ActionRow::builder().title("PIN").subtitle(if set { "已設定" } else { "未設定" }).build();
    let change = gtk::Button::builder()
        .label(if set { "變更⋯" } else { "設定⋯" })
        .valign(gtk::Align::Center)
        .build();
    let (parent, done) = (page.clone(), refresh.clone());
    change.connect_clicked(move |_| {
        let first = adw::PasswordEntryRow::builder().title("新 PIN").build();
        let again = adw::PasswordEntryRow::builder().title("確認 PIN").build();
        let dialog = adw::AlertDialog::new(Some(if set { "變更 PIN" } else { "設定 PIN" }), None);
        dialog.set_extra_child(Some(&form(&[first.upcast_ref(), again.upcast_ref()])));
        dialog.add_responses(&[("cancel", "取消"), ("do", if set { "變更" } else { "設定" })]);
        dialog.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("do"));
        dialog.set_close_response("cancel");
        let (page, done) = (parent.clone(), done.clone());
        dialog.choose(&parent, gio::Cancellable::NONE, move |response| {
            if response != "do" {
                return;
            }
            let typed = first.text().to_string();
            if typed != again.text() {
                return alert::notice(&page, "PIN 不相符", Some("兩次輸入的 PIN 必須相同。"));
            }
            if typed.len() < 6 || !typed.bytes().all(|byte| byte.is_ascii_digit()) {
                return alert::notice(&page, "無法使用這個 PIN", Some("PIN 至少 6 位數字。"));
            }
            lock_pin("--set-pin", Some(typed), move |worked| {
                if worked {
                    done();
                } else {
                    alert::notice(&page, "無法設定 PIN", None);
                }
            });
        });
    });
    row.add_suffix(&change);
    if set {
        let remove = gtk::Button::builder().label("移除").valign(gtk::Align::Center).css_classes(["destructive-action"]).build();
        let (page, done) = (page.clone(), refresh);
        remove.connect_clicked(move |_| {
            let (page, done) = (page.clone(), done.clone());
            lock_pin("--clear-pin", None, move |worked| {
                if worked {
                    done();
                } else {
                    alert::notice(&page, "無法移除 PIN", None);
                }
            });
        });
        row.add_suffix(&remove);
    }
    row
}

/// Asks for a new password twice, done with `action`; `then` gets it when both are the same,
/// and `cancelled` runs when it is not given.
pub fn ask_password(
    page: &adw::PreferencesPage,
    title: &str,
    action: &str,
    cancelled: Rc<dyn Fn()>,
    then: impl FnOnce(String) + 'static,
) {
    let first = adw::PasswordEntryRow::builder().title("新密碼").build();
    let again = adw::PasswordEntryRow::builder().title("確認密碼").build();
    let alert = adw::AlertDialog::new(Some(title), None);
    alert.set_extra_child(Some(&form(&[first.upcast_ref(), again.upcast_ref()])));
    alert.add_responses(&[("cancel", "取消"), ("do", action)]);
    alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("do"));
    alert.set_close_response("cancel");
    let page = page.clone();
    alert.choose(&page.clone(), gio::Cancellable::NONE, move |response| {
        if response != "do" {
            return cancelled();
        }
        let typed = first.text().to_string();
        if typed.is_empty() || typed != again.text() || typed.contains('\n') {
            cancelled();
            return alert::notice(&page, "密碼不相符", Some("兩次輸入的密碼必須相同，且不能是空白。"));
        }
        then(typed);
    });
}

fn add_person(page: &adw::PreferencesPage, refresh: Rc<dyn Fn()>) {
    let name = adw::EntryRow::builder().title("全名").build();
    let login = adw::EntryRow::builder().title("帳號名稱").build();
    let first = adw::PasswordEntryRow::builder().title("密碼").build();
    let again = adw::PasswordEntryRow::builder().title("確認密碼").build();
    let admin = adw::SwitchRow::builder().title("管理員").build();
    // The login name follows the full name until it is typed into itself.
    let typed_login = Rc::new(std::cell::Cell::new(false));
    name.connect_changed(glib::clone!(
        #[strong]
        typed_login,
        #[weak]
        login,
        move |name| {
            if !typed_login.get() {
                let suggested: String = name
                    .text()
                    .to_lowercase()
                    .chars()
                    .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                    .collect();
                login.set_text(&suggested);
            }
        }
    ));
    login.connect_changed(glib::clone!(
        #[strong]
        typed_login,
        #[weak]
        name,
        move |login| {
            let suggested: String = name.text().to_lowercase().chars().filter(char::is_ascii_alphanumeric).collect();
            typed_login.set(login.text() != suggested);
        }
    ));
    let alert = adw::AlertDialog::new(Some("新增使用者"), None);
    alert.set_extra_child(Some(&form(&[
        name.upcast_ref(),
        login.upcast_ref(),
        first.upcast_ref(),
        again.upcast_ref(),
        admin.upcast_ref(),
    ])));
    alert.add_responses(&[("cancel", "取消"), ("do", "新增")]);
    alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
    alert.set_close_response("cancel");
    let page = page.clone();
    alert.choose(&page.clone(), gio::Cancellable::NONE, move |response| {
        if response != "do" {
            return;
        }
        let (full, account, typed) = (name.text().trim().to_string(), login.text().to_string(), first.text().to_string());
        if !is_login_name(&account) {
            return alert::notice(&page, "無法使用這個帳號名稱", Some("請以小寫英文字母開頭，只用小寫英文字母、數字、- 和 _。"));
        }
        if full.contains([':', ',', '\n']) {
            return alert::notice(&page, "無法使用這個名稱", Some("名稱不能有冒號或逗號。"));
        }
        if typed.is_empty() || typed != again.text() || typed.contains('\n') {
            return alert::notice(&page, "密碼不相符", Some("兩次輸入的密碼必須相同，且不能是空白。"));
        }
        // One sudo for all of it; the password comes on stdin, never on a command line.
        let script = "useradd -m -s /bin/bash -c \"$1\" \"$2\" && IFS= read -r password && printf '%s:%s\\n' \"$2\" \"$password\" | chpasswd && { [ \"$3\" = no ] || usermod -aG sudo \"$2\"; }";
        let made_admin = if admin.is_active() { "yes" } else { "no" };
        let refresh = refresh.clone();
        let page = page.clone();
        as_root(
            "新增使用者",
            &["sh", "-c", script, "sh", &full, &account, made_admin],
            Some(format!("{typed}\n")),
            move |worked| {
                if !worked {
                    alert::notice(&page, "無法新增使用者", Some("這個帳號名稱可能已有人使用。"));
                }
                refresh();
            },
        );
    });
}

fn choose_picture(button: &gtk::Button, refresh: Rc<dyn Fn()>) {
    let images = gtk::FileFilter::new();
    images.set_name(Some("圖片"));
    images.add_pixbuf_formats();
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&images);
    let dialog = gtk::FileDialog::builder().title("選擇圖片").filters(&filters).build();
    let window = button.root().and_downcast::<gtk::Window>();
    let button = button.clone();
    dialog.open(window.as_ref(), gio::Cancellable::NONE, move |chosen| {
        let path = match chosen.map(|file| file.path()) {
            Ok(Some(path)) => path,
            Ok(None) => return eprintln!("picture: the file chosen has no local path"),
            Err(err) if err.matches(gtk::DialogError::Dismissed) => return,
            Err(err) => return eprintln!("picture: {err}"),
        };
        // A square of it, as small as an avatar is ever drawn, so ~/.face stays light.
        let saved = gtk::gdk_pixbuf::Pixbuf::from_file(&path).map_err(|err| err.to_string()).and_then(|image| {
            let side = image.width().min(image.height());
            let square = image.new_subpixbuf((image.width() - side) / 2, (image.height() - side) / 2, side, side);
            let small = square
                .scale_simple(256, 256, gtk::gdk_pixbuf::InterpType::Bilinear)
                .ok_or("could not scale it")?;
            small.savev(picture(), "png", &[]).map_err(|err| err.to_string())
        });
        if let Err(err) = saved {
            eprintln!("picture {}: {err}", path.display());
            return alert::notice(&button, "無法使用這張圖片", None);
        }
        refresh();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_are_those_who_log_in() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
            syslog:x:104:110::/home/syslog:/usr/sbin/nologin\n\
            neko:x:1000:1000:Neko Cat,,,:/home/neko:/bin/bash\n\
            guest:x:1001:1001::/home/guest:/bin/bash\n\
            nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n";
        let groups = "sudo:x:27:neko\nneko:x:1000:\n";
        assert_eq!(
            people_in(passwd, groups),
            [
                Person { login: "neko".into(), name: "Neko Cat".into(), admin: true },
                Person { login: "guest".into(), name: "guest".into(), admin: false },
            ]
        );
    }

    #[test]
    fn login_names_are_checked() {
        assert!(is_login_name("neko"));
        assert!(is_login_name("_svc-2"));
        assert!(!is_login_name("Neko"));
        assert!(!is_login_name("2neko"));
        assert!(!is_login_name("ne ko"));
        assert!(!is_login_name(""));
    }
}
