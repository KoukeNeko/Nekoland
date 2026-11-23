//! The Sharing page: remote desktop, remote login over SSH and Sunshine, and file sharing.

use super::*;
use super::sunshine::text_button;

/// Where Settings keeps the `~/.xsession` there was before it gave xrdp sessions Nekoland.
pub(crate) const SAVED_XSESSION_FILE: &str = "nekoland/xsession";
/// How an `~/.xsession` Settings wrote begins, to tell it from one written by hand.
pub(crate) const XSESSION_HEAD: &str = "# Written by Nekoland's Settings, for xrdp sessions.\n";
pub(crate) const XRDP_INI: &str = "/etc/xrdp/xrdp.ini";
pub(crate) const RDP_PORT: u16 = 3389;
/// Where the firewall lets a shared service in from, as macOS does: the private networks a
/// LAN uses, never the internet.
const LAN: [&str; 3] = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"];
const LAN6: [&str; 2] = ["fe80::/10", "fc00::/7"];
/// The comments ufw keeps on the rules made for each service, to find them again by.
pub(crate) const SSH_RULES: &str = "nekoland-ssh";
const XRDP_RULES: &str = "nekoland-xrdp";
const SAMBA_RULES: &str = "nekoland-samba";
const SAMBA_PORTS: [&str; 2] = ["137,138/udp", "139,445/tcp"];

/// Shell that takes the rules made under `tag` out of ufw; nothing without ufw.
pub(crate) fn firewall_deny(tag: &str) -> String {
    format!(
        r#"if command -v ufw >/dev/null; then
    ufw show added | sed -n "s/^ufw \(.*\) comment '{tag}'$/\1/p" | while read -r rule; do ufw delete $rule >/dev/null; done
fi
"#
    )
}

/// Shell that lets `ports`, as ufw writes them ("3389/tcp", "47998:48000,48002/udp"), in
/// through ufw from the LAN under `tag`, in place of what it let in before. ufw keeps them
/// while it is off, for when it is turned on.
pub(crate) fn firewall_allow(tag: &str, ports: &[String]) -> String {
    let mut script = firewall_deny(tag);
    script += &format!(
        "if command -v ufw >/dev/null; then\n    nets=\"{}\"\n    if grep -q '^IPV6=yes' /etc/default/ufw; then nets=\"$nets {}\"; fi\n    for net in $nets; do\n",
        LAN.join(" "),
        LAN6.join(" ")
    );
    for (port, proto) in ports.iter().filter_map(|port| port.split_once('/')) {
        script += &format!("        ufw allow from \"$net\" to any port {port} proto {proto} comment '{tag}' >/dev/null\n");
    }
    script + "    done\nfi\n"
}

/// xrdp, the X server it starts sessions in, and its sound through PipeWire.
const XRDP_PACKAGES: [&str; 3] = ["xrdp", "xorgxrdp", "pipewire-module-xrdp"];

/// What other computers may reach here: the desktop over RDP, through xrdp, with the session
/// they get, and a shell over SSH.
pub(crate) fn sharing_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let rows = group("");
    // Until it is installed, a row to install it from: a switch row held off would hold its
    // button off with it.
    let missing = adw::ActionRow::builder().title("遠端桌面").subtitle("未安裝").build();
    let install = text_button("安裝");
    missing.add_suffix(&install);
    let service = adw::SwitchRow::builder().title("遠端桌面").build();
    let remove = text_button("移除");
    service.add_suffix(&remove);
    let address = fact_row("位址", &remote_address());
    let port = std::fs::read_to_string(XRDP_INI).map_or(RDP_PORT, |ini| xrdp_port(&ini));
    let port = fact_row("連接埠", &port.to_string());
    let session = adw::ComboRow::builder()
        .title("遠端工作階段")
        .model(&gtk::StringList::new(&["Nekoland", "系統預設"]))
        .selected(if xsession_is_nekoland() { 0 } else { 1 })
        .build();
    rows.add(&missing);
    rows.add(&service);
    rows.add(&address);
    rows.add(&port);
    rows.add(&session);
    page.add(&rows);

    // Set while the switch is made to show xrdp as it is, not turned by the person.
    let showing = Rc::new(Cell::new(false));
    let show = Rc::new(glib::clone!(
        #[weak]
        missing,
        #[weak]
        install,
        #[weak]
        service,
        #[weak]
        address,
        #[weak]
        port,
        #[weak]
        session,
        #[strong]
        showing,
        move || {
            let running = xrdp_running();
            showing.set(true);
            service.set_active(running == Some(true));
            showing.set(false);
            // Turned off or removed from an xrdp session, it would end the connection it is
            // changed over.
            let remote = std::env::var_os("XRDP_SESSION").is_some();
            service.set_sensitive(!remote);
            service.set_subtitle(if remote { "遠端連線中" } else { "" });
            missing.set_subtitle("未安裝");
            install.set_sensitive(true);
            missing.set_visible(running.is_none());
            service.set_visible(running.is_some());
            session.set_visible(running.is_some());
            address.set_visible(running == Some(true));
            port.set_visible(running == Some(true));
        }
    ));
    show();
    install.connect_clicked(glib::clone!(
        #[weak]
        missing,
        #[strong]
        show,
        move |install| {
            install.set_sensitive(false);
            missing.set_subtitle("正在安裝");
            // Installed off, as Sunshine is: the switch is what lets others in.
            let script = format!("set -e\n{} install {}\nsystemctl disable --now xrdp\n", apt::APT_GET[..apt::APT_GET.len() - 2].join(" "), XRDP_PACKAGES.join(" "));
            let show = show.clone();
            as_root("安裝 xrdp", &["sh", "-c", &script], None, move |worked| {
                show();
                if !worked {
                    alert::notice(&missing, "無法安裝 xrdp", None);
                }
            });
        }
    ));
    remove.connect_clicked(glib::clone!(
        #[weak]
        service,
        #[strong]
        show,
        move |remove| {
            let show = show.clone();
            alert::confirm_removal(remove, "移除 xrdp？", "一併移除設定", move |purge| {
                service.set_sensitive(false);
                service.set_subtitle("正在移除");
                let verb = if purge { "purge" } else { "remove" };
                let script = format!(
                    "set -e\n{} {verb} {}\n{}",
                    apt::APT_GET[..apt::APT_GET.len() - 2].join(" "),
                    XRDP_PACKAGES.join(" "),
                    firewall_deny(XRDP_RULES)
                );
                let show = show.clone();
                as_root("移除 xrdp", &["sh", "-c", &script], None, move |worked| {
                    // Its sessions' Nekoland, and the ~/.xsession there was before it.
                    if worked && purge {
                        if let Err(err) = set_xsession(false) {
                            eprintln!("~/.xsession: {err}");
                        }
                    }
                    show();
                    if !worked {
                        alert::notice(&service, "無法移除 xrdp", None);
                    }
                });
            });
        }
    ));
    service.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        let (verb, reason, firewall) = if row.is_active() {
            // shortcut: the port as xrdp.ini has it now; changed there later, the rule follows
            // the next time xrdp is turned on here.
            let port = std::fs::read_to_string(XRDP_INI).map_or(RDP_PORT, |ini| xrdp_port(&ini));
            ("enable", "開啟遠端桌面", firewall_allow(XRDP_RULES, &[format!("{port}/tcp")]))
        } else {
            ("disable", "關閉遠端桌面", firewall_deny(XRDP_RULES))
        };
        row.set_sensitive(false);
        let show = show.clone();
        let script = format!("set -e\nsystemctl {verb} --now xrdp\n{firewall}");
        match sudo(reason, &["sh", "-c", &script], gio::SubprocessFlags::NONE) {
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
    let login = group("遠端登入");
    if let Some(ssh) = ssh_units() {
        login.add(&ssh_row(ssh));
    }
    sunshine::add_sunshine_rows(&page, &login);
    page.add(&login);
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

/// Undoes SHARE_FOLDER's share of `$2` for `$1` and their Samba password, before Samba is
/// purged: either may never have been made.
const UNSHARE_FOLDER: &str = r#"set -e
runuser -u "$1" -g sambashare -- net usershare delete "$2" 2>/dev/null || true
smbpasswd -x "$1" >/dev/null 2>&1 || true
"#;
/// And after it, the rest of what SHARE_FOLDER set up; smb.conf is samba-common's, which
/// other programs' SMB libraries keep.
const UNSHARE_SETUP: &str = r#"gpasswd -d "$1" sambashare >/dev/null 2>&1 || true
if [ -f /etc/samba/smb.conf ]; then
    sed -i '/^   usershare max shares = 100$/d' /etc/samba/smb.conf
fi
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
    let missing = adw::ActionRow::builder().title("檔案共享").subtitle("未安裝").build();
    let install = text_button("安裝");
    missing.add_suffix(&install);
    let service = adw::SwitchRow::builder().title("檔案共享").build();
    let remove = text_button("移除");
    service.add_suffix(&remove);
    let shared = fact_row("共享資料夾", &folder.display().to_string());
    sharing.add(&missing);
    sharing.add(&service);
    sharing.add(&shared);
    // Set while the switch is put back to what Samba is.
    let showing = Rc::new(Cell::new(false));
    let show = Rc::new(glib::clone!(
        #[weak]
        missing,
        #[weak]
        install,
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
            service.set_sensitive(true);
            service.set_subtitle(&match running {
                Some(true) => format!("smb://{}/{share}", remote_address()),
                _ => String::new(),
            });
            missing.set_subtitle("未安裝");
            install.set_sensitive(true);
            missing.set_visible(running.is_none());
            service.set_visible(running.is_some());
            shared.set_visible(running == Some(true));
        }
    ));
    show();
    install.connect_clicked(glib::clone!(
        #[weak]
        missing,
        #[strong]
        show,
        move |install| {
            install.set_sensitive(false);
            missing.set_subtitle("正在安裝");
            // Installing starts it; sharing is what the switch turns on.
            let apt = apt::APT_GET[..apt::APT_GET.len() - 2].join(" ");
            let script = format!("set -e\n{apt} install samba\nsystemctl disable --now smbd\n");
            let show = show.clone();
            as_root("安裝 Samba", &["sh", "-c", &script], None, move |worked| {
                show();
                if !worked {
                    alert::notice(&missing, "無法安裝 Samba", None);
                }
            });
        }
    ));
    remove.connect_clicked(glib::clone!(
        #[weak]
        service,
        #[strong]
        show,
        #[strong]
        user,
        #[strong]
        share,
        move |remove| {
            let (show, user, share) = (show.clone(), user.clone(), share.clone());
            alert::confirm_removal(remove, "移除 Samba？", "一併移除設定", move |purge| {
                service.set_sensitive(false);
                service.set_subtitle("正在移除");
                let apt = apt::APT_GET[..apt::APT_GET.len() - 2].join(" ");
                let firewall = firewall_deny(SAMBA_RULES);
                let script = if purge {
                    format!("{UNSHARE_FOLDER}{apt} purge samba\n{UNSHARE_SETUP}{firewall}")
                } else {
                    format!("set -e\n{apt} remove samba\n{firewall}")
                };
                let show = show.clone();
                let args = ["sh", "-c", script.as_str(), "sh", user.as_str(), share.as_str()];
                as_root("移除 Samba", &args, None, move |worked| {
                    show();
                    if !worked {
                        alert::notice(&service, "無法移除 Samba", None);
                    }
                });
            });
        }
    ));
    let page = page.clone();
    service.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        row.set_sensitive(false);
        let show = show.clone();
        if !row.is_active() {
            let script = format!("set -e\nsystemctl disable --now smbd\n{}", firewall_deny(SAMBA_RULES));
            return as_root("關閉檔案共享", &["sh", "-c", &script], None, move |_| show());
        }
        if let Err(err) = std::fs::create_dir_all(&folder) {
            eprintln!("{}: {err}", folder.display());
            return show();
        }
        let (user, folder, share, page) = (user.clone(), folder.display().to_string(), share.clone(), page.clone());
        let cancelled: Rc<dyn Fn()> = show.clone();
        users::ask_password(&page.clone(), "檔案共享密碼", "開啟", cancelled, move |typed| {
            let ports = SAMBA_PORTS.map(String::from);
            let script = format!("{SHARE_FOLDER}{}", firewall_allow(SAMBA_RULES, &ports));
            let args = ["sh", "-c", script.as_str(), "sh", user.as_str(), folder.as_str(), share.as_str()];
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

/// Letting others in over SSH, which the Mac calls 遠端登入, as the group it is in here is.
pub(crate) fn ssh_row(unit: &'static str) -> adw::SwitchRow {
    // `systemctl is-active` fails when any is not, so its lines are read rather than its status.
    let running = || {
        let states = Command::new("systemctl").args(["is-active", "ssh.socket", "ssh.service"]).output();
        states.is_ok_and(|states| String::from_utf8_lossy(&states.stdout).lines().any(|state| state == "active"))
    };
    let service = adw::SwitchRow::builder().title("SSH").active(running()).build();
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
        // shortcut: port 22, as Ubuntu's sshd listens on; one set otherwise in sshd_config is
        // not let through the firewall.
        let script = if row.is_active() {
            format!("set -e\nsystemctl enable --now {unit}\n{}", firewall_allow(SSH_RULES, &["22/tcp".into()]))
        } else {
            format!("set -e\nsystemctl disable --now ssh.socket ssh.service\n{}", firewall_deny(SSH_RULES))
        };
        let args = ["sh", "-c", script.as_str()];
        let reason = if row.is_active() { "開啟遠端登入" } else { "關閉遠端登入" };
        row.set_sensitive(false);
        let (row, showing) = (row.clone(), showing.clone());
        as_root(reason, &args, None, move |_| {
            showing.set(true);
            row.set_active(running());
            showing.set(false);
            row.set_sensitive(true);
        });
    });
    service
}
