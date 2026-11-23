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
/// The comment ufw keeps on the rules made here, followed by the service's name.
const RULES: &str = "nekoland-";
const SAMBA_PORTS: &str = "137,138/udp 139,445/tcp";

/// The SSH and xrdp ports last put in effect, to restart them by when their configuration
/// names others; in /run, as each starts with its own at boot.
const PORTS_SEEN: &str = "/run/nekoland-sharing-ports";
/// `nekoland-settings --firewall-sync USER`, run as root: see `sync_firewall`.
pub(crate) const FIREWALL_FLAG: &str = "--firewall-sync";
/// What, changed, brings the rules up to date: the ports' configurations, the firewall
/// turned on, and services turned on or off, by hand as much as here.
const FIREWALL_WATCHED: [&str; 6] = [
    "/etc/ssh/sshd_config",
    "/etc/ssh/sshd_config.d",
    XRDP_INI,
    "/etc/ufw/ufw.conf",
    "/etc/systemd/system/multi-user.target.wants",
    "/etc/systemd/system/sockets.target.wants",
];

/// The command, for a script run as root, that brings the firewall up to what is shared
/// here, and keeps it so from then on.
pub(crate) fn firewall_sync() -> String {
    let program = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("nekoland-settings"));
    let program = program.to_string_lossy().replace('\'', "'\\''");
    let user = std::env::var("USER").unwrap_or_default();
    format!("'{program}' {FIREWALL_FLAG} '{user}'\n")
}

/// As root: makes ufw's rules for what `user` shares what they are now (see
/// `firewall_script`), and has systemd do it again whenever any of it changes, and at boot.
pub(crate) fn sync_firewall(user: Option<&str>) -> glib::ExitCode {
    let Some((user, home)) = user.and_then(|user| {
        let entry = output("getent", &["passwd", user])?;
        Some((user, PathBuf::from(entry.trim().split(':').nth(5)?)))
    }) else {
        eprintln!("{FIREWALL_FLAG}: no such user");
        return glib::ExitCode::FAILURE;
    };
    if let Err(err) = watch_firewall(user, &home) {
        eprintln!("{FIREWALL_FLAG}: {err}");
    }
    match Command::new("sh").arg("-c").arg(firewall_script(&home, Path::new(PORTS_SEEN))).status() {
        Ok(status) if status.success() => glib::ExitCode::SUCCESS,
        done => {
            eprintln!("{FIREWALL_FLAG}: {done:?}");
            glib::ExitCode::FAILURE
        }
    }
}

/// The system units that run this again for `user`: a path unit on what is watched, and
/// the service it starts, at boot too. Written, and started, only when they change.
fn watch_firewall(user: &str, home: &Path) -> std::io::Result<()> {
    let name = format!("nekoland-firewall-{user}");
    let program = std::env::current_exe()?;
    let sunshine = home.join(".config/sunshine/sunshine.conf");
    let watched: String = FIREWALL_WATCHED
        .iter()
        .map(|path| format!("PathChanged={path}\n"))
        .chain([format!("PathChanged={}\n", sunshine.display())])
        .collect();
    let units = [
        (
            format!("{name}.service"),
            format!(
                "# Written by Nekoland's Settings.\n[Unit]\nDescription=Let what {user} shares through the firewall, from the LAN\n\n[Service]\nType=oneshot\nExecStart=\"{}\" {FIREWALL_FLAG} {user}\n\n[Install]\nWantedBy=multi-user.target\n",
                program.display()
            ),
        ),
        (
            format!("{name}.path"),
            format!("# Written by Nekoland's Settings.\n[Unit]\nDescription=Watch what {user} shares, for the firewall\n\n[Path]\n{watched}\n[Install]\nWantedBy=multi-user.target\n"),
        ),
    ];
    let dir = Path::new("/etc/systemd/system");
    let mut changed = false;
    for (unit, text) in &units {
        let path = dir.join(unit);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            write_atomic(&path, text)?;
            changed = true;
        }
    }
    if changed {
        let systemctl = |args: &[&str]| Command::new("systemctl").args(args).status().map(drop);
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", &units[0].0])?;
        systemctl(&["enable", "--now", &units[1].0])?;
    }
    Ok(())
}

/// Shell that makes ufw's rules for the shared services what they are now: each one that
/// is on (Sunshine: installed, as it turns on without root) let in from the LAN on the
/// ports its configuration has now, and the others not. ufw keeps them while it is off, for
/// when it is turned on; nothing without ufw. `home` is whose Sunshine it is.
fn firewall_script(home: &Path, seen: &Path) -> String {
    let xrdp = std::fs::read_to_string(XRDP_INI).map_or(RDP_PORT, |ini| xrdp_port(&ini));
    let conf = std::fs::read_to_string(home.join(".config/sunshine/sunshine.conf")).unwrap_or_default();
    let sunshine = super::sunshine::firewall_ports(&conf).join(" ");
    format!(
        r#"ssh_ports=$(sshd -T 2>/dev/null | awk 'tolower($1) == "port" {{ print $2 }}' | sort -nu | paste -sd, -)
ssh_ports=${{ssh_ports:-22}}
# A port changed in a configuration since it was last seen here is put in effect: the
# service listens on it once restarted. Not at boot, when each starts with its own.
seen='{seen}'
if [ -f "$seen" ]; then
    if [ "$(sed -n 's/^ssh //p' "$seen")" != "$ssh_ports" ]; then
        if systemctl -q is-active ssh.socket; then
            # Its ports are generated from sshd_config as systemd reloads.
            systemctl daemon-reload
            systemctl restart ssh.socket
        else
            systemctl try-restart ssh.service
        fi
    fi
    if [ "$(sed -n 's/^xrdp //p' "$seen")" != {xrdp} ]; then systemctl try-restart xrdp; fi
fi
printf 'ssh %s\nxrdp %s\n' "$ssh_ports" {xrdp} >"$seen"
if command -v ufw >/dev/null; then
    nets="{lan}"
    if grep -q '^IPV6=yes' /etc/default/ufw; then nets="$nets {lan6}"; fi
    wanted=$(mktemp)
    allow() {{
        service=$1; shift
        for net in $nets; do for spec in "$@"; do
            echo "ufw allow from $net to any port ${{spec%/*}} proto ${{spec#*/}} comment '{RULES}$service'" >>"$wanted"
        done; done
    }}
    if systemctl -q is-enabled ssh.socket || systemctl -q is-enabled ssh.service; then
        allow ssh "$ssh_ports/tcp"
    fi
    if systemctl -q is-enabled xrdp; then allow xrdp {xrdp}/tcp; fi
    if systemctl -q is-enabled smbd; then allow samba {SAMBA_PORTS}; fi
    if [ -x {sunshine_program} ]; then allow sunshine {sunshine}; fi
    ufw show added | grep " comment '{RULES}" >"$wanted.had" || true
    grep -vxF -f "$wanted" "$wanted.had" | sed "s/^ufw \(.*\) comment .*$/\1/" | while read -r rule; do
        ufw delete $rule >/dev/null
    done
    grep -vxF -f "$wanted.had" "$wanted" | while read -r rule; do eval "$rule" >/dev/null; done
    rm -f "$wanted" "$wanted.had"
fi
"#,
        lan = LAN.join(" "),
        lan6 = LAN6.join(" "),
        sunshine_program = super::sunshine::PROGRAM,
        seen = seen.display(),
    )
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
    let port = adw::ActionRow::builder().title("連接埠").build();
    let port_value = fact_label("");
    port.add_suffix(&port_value);
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
        port_value,
        #[weak]
        session,
        #[strong]
        showing,
        move || {
            let running = xrdp_running();
            // As xrdp.ini has it now, changed there since too.
            port_value.set_label(&std::fs::read_to_string(XRDP_INI).map_or(RDP_PORT, |ini| xrdp_port(&ini)).to_string());
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
    // Shown again, what changed meanwhile, by hand too.
    page.connect_map(glib::clone!(
        #[strong]
        show,
        move |_| show()
    ));
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
                    firewall_sync()
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
        let (verb, reason) = if row.is_active() {
            ("enable", "開啟遠端桌面")
        } else {
            ("disable", "關閉遠端桌面")
        };
        row.set_sensitive(false);
        let show = show.clone();
        let script = format!("set -e\nsystemctl {verb} --now xrdp\n{}", firewall_sync());
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
                let firewall = firewall_sync();
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
            let script = format!("set -e\nsystemctl disable --now smbd\n{}", firewall_sync());
            return as_root("關閉檔案共享", &["sh", "-c", &script], None, move |_| show());
        }
        if let Err(err) = std::fs::create_dir_all(&folder) {
            eprintln!("{}: {err}", folder.display());
            return show();
        }
        let (user, folder, share, page) = (user.clone(), folder.display().to_string(), share.clone(), page.clone());
        let cancelled: Rc<dyn Fn()> = show.clone();
        users::ask_password(&page.clone(), "檔案共享密碼", "開啟", cancelled, move |typed| {
            let script = format!("{SHARE_FOLDER}{}", firewall_sync());
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

/// The ports sshd listens on, as its configuration names them: the files in sshd_config.d,
/// which Ubuntu's sshd_config includes first, then sshd_config itself, up to a `Match`
/// block. They are readable by anyone, unlike what `sshd -T` needs.
fn ssh_ports() -> Vec<u16> {
    let mut files: Vec<PathBuf> = std::fs::read_dir("/etc/ssh/sshd_config.d")
        .map(|dir| dir.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|ext| ext == "conf")).collect())
        .unwrap_or_default();
    files.sort();
    files.push(PathBuf::from("/etc/ssh/sshd_config"));
    files.iter().flat_map(|file| sshd_config_ports(&std::fs::read_to_string(file).unwrap_or_default())).collect()
}

fn sshd_config_ports(text: &str) -> Vec<u16> {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .take_while(|words| !words.first().is_some_and(|word| word.eq_ignore_ascii_case("match")))
        .filter_map(|words| match words.as_slice() {
            [key, port] if key.eq_ignore_ascii_case("port") => port.parse().ok(),
            _ => None,
        })
        .collect()
}

/// What to type on the other computer: `-p` only for a port other than SSH's own.
fn ssh_command(user: &str, ports: &[u16]) -> String {
    match ports.first() {
        Some(port) if *port != 22 => format!("ssh -p {port} {user}@{}", remote_address()),
        _ => format!("ssh {user}@{}", remote_address()),
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
        .transform_to(move |_, on: bool| Some(if on { ssh_command(&user, &ssh_ports()) } else { String::new() }))
        .sync_create()
        .build();
    // Shown again, with the port sshd_config has now.
    service.connect_map(|row| row.notify("active"));
    // Set while the switch is put back to what the service is.
    let showing = Rc::new(Cell::new(false));
    service.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        let script = if row.is_active() {
            format!("set -e\nsystemctl enable --now {unit}\n{}", firewall_sync())
        } else {
            format!("set -e\nsystemctl disable --now ssh.socket ssh.service\n{}", firewall_sync())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn sshds_ports_stop_at_a_match_block() {
        assert_eq!(sshd_config_ports("# Port 2200\nPort 2222\nport 22\nMatch User x\n    Port 23\n"), [2222, 22]);
        assert!(sshd_config_ports("#Port 22\n").is_empty());
    }

    #[test]
    fn the_firewall_is_brought_to_what_is_shared() {
        let dir = std::env::temp_dir().join(format!("nekoland-firewall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("ufw.log");
        let stub = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // SSH on its own port and Samba on; xrdp off, with a rule left from when it was on;
        // one SSH rule already there, and one rule made by hand.
        stub("systemctl", &format!(r#"case "$1" in -q) ;; *) echo "systemctl $*" >>{};; esac
case "$3" in ssh.socket|smbd) exit 0;; *) exit 3;; esac"#, log.display()));
        stub("sshd", "echo 'Port 2222'");
        stub(
            "ufw",
            &format!(
                r#"if [ "$1" = show ]; then
    echo "Added user rules (see 'ufw status' for running firewall):"
    echo "ufw allow from 10.0.0.0/8 to any port 2222 proto tcp comment 'nekoland-ssh'"
    echo "ufw allow from 10.0.0.0/8 to any port 3389 proto tcp comment 'nekoland-xrdp'"
    echo "ufw allow 8080/tcp"
else
    echo "$*" >>{}
fi"#,
                log.display()
            ),
        );
        let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap_or_default());
        // SSH's port was another the last time.
        let seen = dir.join("seen");
        std::fs::write(&seen, "ssh 22\nxrdp 3389\n").unwrap();
        let ran = Command::new("sh").arg("-c").arg(firewall_script(&dir, &seen)).env("PATH", path).status().unwrap();
        let seen = std::fs::read_to_string(&seen).unwrap();
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(ran.success());
        assert_eq!(seen, "ssh 2222\nxrdp 3389\n");
        assert!(calls.contains("systemctl restart ssh.socket"), "{calls}");
        assert!(!calls.contains("xrdp"), "{calls}");
        let calls: Vec<&str> = calls.lines().collect();
        assert!(calls.contains(&"delete allow from 10.0.0.0/8 to any port 3389 proto tcp"), "{calls:?}");
        assert!(calls.contains(&"allow from 192.168.0.0/16 to any port 2222 proto tcp comment nekoland-ssh"), "{calls:?}");
        assert!(calls.contains(&"allow from 10.0.0.0/8 to any port 139,445 proto tcp comment nekoland-samba"), "{calls:?}");
        assert!(calls.contains(&"allow from 172.16.0.0/12 to any port 137,138 proto udp comment nekoland-samba"), "{calls:?}");
        // What is there already stays, and what was made by hand is not touched.
        assert!(!calls.iter().any(|call| call.contains("10.0.0.0/8 to any port 2222")), "{calls:?}");
        assert!(!calls.iter().any(|call| call.contains("8080")), "{calls:?}");
    }
}
