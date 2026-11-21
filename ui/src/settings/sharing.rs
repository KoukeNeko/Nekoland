//! The Sharing page: remote desktop, file sharing and remote login.

use super::*;

/// Where Settings keeps the `~/.xsession` there was before it gave xrdp sessions Nekoland.
pub(crate) const SAVED_XSESSION_FILE: &str = "nekoland/xsession";
/// How an `~/.xsession` Settings wrote begins, to tell it from one written by hand.
pub(crate) const XSESSION_HEAD: &str = "# Written by Nekoland's Settings, for xrdp sessions.\n";
pub(crate) const XRDP_INI: &str = "/etc/xrdp/xrdp.ini";
pub(crate) const RDP_PORT: u16 = 3389;

/// What other computers may reach here: the desktop over RDP, through xrdp, with the session
/// they get, and a shell over SSH.
pub(crate) fn sharing_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let rows = group("");
    let service = adw::SwitchRow::builder().title("遠端桌面").build();
    let address = fact_row("位址", &remote_address());
    let port = std::fs::read_to_string(XRDP_INI).map_or(RDP_PORT, |ini| xrdp_port(&ini));
    let port = fact_row("連接埠", &port.to_string());
    let session = adw::ComboRow::builder()
        .title("遠端工作階段")
        .model(&gtk::StringList::new(&["Nekoland", "系統預設"]))
        .selected(if xsession_is_nekoland() { 0 } else { 1 })
        .build();
    rows.add(&service);
    rows.add(&address);
    rows.add(&port);
    rows.add(&session);
    page.add(&rows);

    // Set while the switch is made to show xrdp as it is, not turned by the person.
    let showing = Rc::new(Cell::new(false));
    let show = Rc::new(glib::clone!(
        #[weak]
        service,
        #[weak]
        address,
        #[weak]
        port,
        #[strong]
        showing,
        move || {
            let running = xrdp_running();
            showing.set(true);
            service.set_active(running == Some(true));
            showing.set(false);
            // Turned off from an xrdp session, it would end the connection it is changed over.
            let remote = std::env::var_os("XRDP_SESSION").is_some();
            service.set_sensitive(running.is_some() && !remote);
            service.set_subtitle(match running {
                None => "未安裝 xrdp",
                Some(_) if remote => "遠端連線中",
                Some(_) => "",
            });
            address.set_visible(running == Some(true));
            port.set_visible(running == Some(true));
        }
    ));
    show();
    service.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        let (verb, reason) = if row.is_active() {
            ("enable", "開啟遠端桌面")
        } else {
            ("disable", "關閉遠端桌面")
        };
        row.set_sensitive(false);
        let show = show.clone();
        match sudo(reason, &["systemctl", verb, "--now", "xrdp"], gio::SubprocessFlags::NONE) {
            // Turned down or failed, the switch goes back to what xrdp is.
            Ok(sudo) => sudo.wait_check_async(gio::Cancellable::NONE, move |result| {
                if let Err(err) = result {
                    eprintln!("systemctl {verb} xrdp: {err}");
                }
                show();
            }),
            Err(err) => {
                eprintln!("sudo: {err}");
                show();
            }
        }
    });
    session.connect_selected_notify(|row| {
        if let Err(err) = set_xsession(row.selected() == 0) {
            eprintln!("~/.xsession: {err}");
        }
    });
    if let Some(ssh) = ssh_units() {
        page.add(&remote_login_group(ssh));
    }
    page.add(&file_sharing_group(&page));
    page
}

/// Turns on Samba and shares the Public folder as `$3`, with the account's own
/// Samba password: Samba keeps its own, so it is asked for as sharing is turned on.
pub(crate) const SHARE_FOLDER: &str = r#"set -e
usermod -aG sambashare "$1"
smbpasswd -a -s "$1"
# Ubuntu's smb.conf leaves the users' own shares off, its line for them commented out.
grep -Eq '^[[:space:]]*usershare max shares' /etc/samba/smb.conf ||
    sed -i '/^\[global\]/a\   usershare max shares = 100' /etc/samba/smb.conf
systemctl enable --now smbd
runuser -u "$1" -g sambashare -- net usershare add "$3" "$2" "" Everyone:F guest_ok=n
"#;

/// 檔案共享: the Public folder over SMB, as the Mac and Windows share it.
pub(crate) fn file_sharing_group(page: &adw::PreferencesPage) -> adw::PreferencesGroup {
    let sharing = group("");
    let user = std::env::var("USER").unwrap_or_default();
    // Installing Samba starts it; sharing is on once the folder was shared, which put the
    // person in `sambashare`, whose share list only its members can read.
    let state = glib::clone!(
        #[strong]
        user,
        move || {
            let shown = output("systemctl", &["show", "smbd", "--property=LoadState,ActiveState"])?;
            let value = |key: &str| field(&shown, key, '=');
            let member = output("getent", &["group", "sambashare"])
                .is_some_and(|line| line.trim().rsplit(':').next().is_some_and(|members| members.split(',').any(|m| m == user)));
            (value("LoadState").as_deref() == Some("loaded"))
                .then(|| member && value("ActiveState").as_deref() == Some("active"))
        }
    );
    let folder = glib::user_special_dir(glib::UserDirectory::PublicShare)
        .unwrap_or_else(|| glib::home_dir().join("Public"));
    // Samba takes no share named as a user.
    let share = format!("{}-{user}", folder.file_name().unwrap_or_default().to_string_lossy());
    let service = adw::SwitchRow::builder().title("檔案共享").build();
    let shared = fact_row("共享資料夾", &folder.display().to_string());
    sharing.add(&service);
    sharing.add(&shared);
    // Set while the switch is put back to what Samba is.
    let showing = Rc::new(Cell::new(false));
    let show = Rc::new(glib::clone!(
        #[weak]
        service,
        #[weak]
        shared,
        #[strong]
        showing,
        #[strong]
        share,
        move || {
            let running = state();
            showing.set(true);
            service.set_active(running == Some(true));
            showing.set(false);
            service.set_sensitive(running.is_some());
            service.set_subtitle(&match running {
                None => "未安裝 Samba".to_string(),
                Some(true) => format!("smb://{}/{share}", remote_address()),
                Some(false) => String::new(),
            });
            shared.set_visible(running == Some(true));
        }
    ));
    show();
    let page = page.clone();
    service.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        row.set_sensitive(false);
        let show = show.clone();
        if !row.is_active() {
            return as_root("關閉檔案共享", &["systemctl", "disable", "--now", "smbd"], None, move |_| show());
        }
        if let Err(err) = std::fs::create_dir_all(&folder) {
            eprintln!("{}: {err}", folder.display());
            return show();
        }
        let (user, folder, share, page) = (user.clone(), folder.display().to_string(), share.clone(), page.clone());
        let cancelled: Rc<dyn Fn()> = show.clone();
        users::ask_password(&page.clone(), "檔案共享密碼", "開啟", cancelled, move |typed| {
            let args = ["sh", "-c", SHARE_FOLDER, "sh", user.as_str(), folder.as_str(), share.as_str()];
            as_root("開啟檔案共享", &args, Some(format!("{typed}\n{typed}\n")), move |worked| {
                if !worked {
                    alert::notice(&page, "無法開啟檔案共享", None);
                }
                show();
            });
        });
    });
    sharing
}

/// The units that start OpenSSH's server: a socket where it is started on demand, as Ubuntu
/// has it, else the service; None without it.
pub(crate) fn ssh_units() -> Option<&'static str> {
    let loaded = |unit: &str| {
        output("systemctl", &["show", unit, "--property=LoadState"])
            .is_some_and(|shown| field(&shown, "LoadState", '=').as_deref() == Some("loaded"))
    };
    if loaded("ssh.socket") {
        Some("ssh.socket")
    } else {
        loaded("ssh.service").then_some("ssh.service")
    }
}

/// 遠端登入, the Mac's name for letting others in over SSH.
pub(crate) fn remote_login_group(unit: &'static str) -> adw::PreferencesGroup {
    let login = group("");
    // `systemctl is-active` fails when any is not, so its lines are read rather than its status.
    let running = || {
        let states = Command::new("systemctl").args(["is-active", "ssh.socket", "ssh.service"]).output();
        states.is_ok_and(|states| String::from_utf8_lossy(&states.stdout).lines().any(|state| state == "active"))
    };
    let service = adw::SwitchRow::builder().title("遠端登入").active(running()).build();
    // What to type on the other computer, while it can be typed.
    let user = std::env::var("USER").unwrap_or_default();
    service
        .bind_property("active", &service, "subtitle")
        .transform_to(move |_, on: bool| Some(if on { format!("ssh {user}@{}", remote_address()) } else { String::new() }))
        .sync_create()
        .build();
    // Set while the switch is put back to what the service is.
    let showing = Rc::new(Cell::new(false));
    service.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        let args: &[&str] = if row.is_active() {
            &["systemctl", "enable", "--now", unit]
        } else {
            &["systemctl", "disable", "--now", "ssh.socket", "ssh.service"]
        };
        let reason = if row.is_active() { "開啟遠端登入" } else { "關閉遠端登入" };
        row.set_sensitive(false);
        let (row, showing) = (row.clone(), showing.clone());
        as_root(reason, args, None, move |_| {
            showing.set(true);
            row.set_active(running());
            showing.set(false);
            row.set_sensitive(true);
        });
    });
    login.add(&service);
    login
}
