//! Sunshine, the host Moonlight streams a desktop from: installed from its own releases,
//! as Ubuntu has no package of it, and run as one of two user units: one streams the desktop
//! at the computer, the other a Nekoland session of its own, headless, that runs whether or
//! not anyone is logged in at the computer. Its web interface's API pairs and forgets the
//! devices, with the password it keeps, which Settings keeps in the keyring.

use super::*;

pub(crate) const PROGRAM: &str = "/usr/bin/sunshine";
const RELEASES: &str = "https://api.github.com/repos/LizardByte/Sunshine/releases/latest";
/// The user units; the compositor knows them by these names too (src/headless.rs).
const DESKTOP_UNIT: &str = "nekoland-sunshine.service";
const SESSION_UNIT: &str = "nekoland-remote-session.service";
const UDEV_RULE: &str = "/etc/udev/rules.d/61-nekoland-sunshine.rules";
/// What Sunshine's package copies to /etc as it is set up, which removing it leaves behind.
/// Sunshine's `port`, which its other ports are counted from.
const DEFAULT_PORT: u32 = 47989;
const PACKAGE_COPIES: [&str; 2] = ["/etc/udev/rules.d/60-sunshine.rules", "/etc/modules-load.d/60-sunshine.conf"];
const WEB_UI: &str = "https://localhost:47990";
const KEYRING_SCHEMA: &str = "org.nekoland.Sunshine";
/// How Nekoland is drawn for Sunshine: wlr-screencopy, which both of its sessions have.
const CAPTURE: (&str, &str) = ("capture", "wlr");
/// What Sunshine runs as each client connects and leaves, `do` and `undo`.
const PREP_COMMANDS: &str = "global_prep_cmd";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    /// The desktop at the computer, as it is.
    Desktop,
    /// A session of its own.
    Session,
}

impl Mode {
    const ALL: [Mode; 2] = [Mode::Desktop, Mode::Session];

    fn unit(self) -> &'static str {
        match self {
            Mode::Desktop => DESKTOP_UNIT,
            Mode::Session => SESSION_UNIT,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Desktop => "目前的桌面",
            Mode::Session => "新的工作階段",
        }
    }
}

fn installed() -> bool {
    Path::new(PROGRAM).exists()
}

/// Which session Sunshine streams, or None while it is off.
fn mode() -> Option<Mode> {
    Mode::ALL.into_iter().find(|mode| {
        output("systemctl", &["--user", "is-enabled", mode.unit()]).is_some_and(|state| state.trim() == "enabled")
    })
}

fn sunshine_dir() -> PathBuf {
    glib::user_config_dir().join("sunshine")
}

fn conf_path() -> PathBuf {
    sunshine_dir().join("sunshine.conf")
}

// Installing.

/// The release's package for this system, as `ID`, `VERSION_ID` and `VERSION_CODENAME` of
/// os-release name it ("+ubuntu26.10", "+debiantrixie"), and its sha256.
fn package_url(release: &str, os_release: &str, arch: &str) -> Option<(String, String)> {
    let release: serde_json::Value = serde_json::from_str(release).ok()?;
    let value = |key: &str| field(os_release, key, '=').map(|value| value.trim_matches('"').to_string());
    let id = value("ID")?;
    let suffixes: Vec<String> = [value("VERSION_ID"), value("VERSION_CODENAME")]
        .into_iter()
        .flatten()
        .map(|version| format!("+{id}{version}_{arch}.deb"))
        .collect();
    release["assets"].as_array()?.iter().find_map(|asset| {
        let name = asset["name"].as_str()?;
        suffixes.iter().any(|suffix| name.ends_with(suffix.as_str())).then(|| {
            let sum = asset["digest"].as_str().and_then(|digest| digest.strip_prefix("sha256:"));
            Some((asset["browser_download_url"].as_str()?.to_string(), sum?.to_string()))
        })?
    })
}

/// Downloads the newest Sunshine for this system, checked against the sum GitHub gives for
/// it. Returns where it is.
fn download_package() -> Result<PathBuf, String> {
    // GitHub answers an address that asked too often this hour with 403 or 429.
    let release = said(Command::new("curl").args(["-fsSL", RELEASES])).map_err(|err| {
        if err.ends_with("error: 403") || err.ends_with("error: 429") { "GitHub 暫時拒絕查詢版本，請稍後再試。".to_string() } else { err }
    })?;
    let os_release = std::fs::read_to_string("/etc/os-release").map_err(|err| err.to_string())?;
    let arch = said(Command::new("dpkg").arg("--print-architecture"))?;
    let (url, sum) = package_url(&release, &os_release, arch.trim()).ok_or("Sunshine 沒有這個系統版本的套件")?;
    let dir = glib::user_cache_dir().join("nekoland");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let file = dir.join("sunshine.deb");
    said(Command::new("curl").args(["-fsSL", "--output"]).arg(&file).arg(&url))?;
    let got = said(Command::new("sha256sum").arg(&file))?;
    if got.split_whitespace().next() != Some(sum.as_str()) {
        let _ = std::fs::remove_file(&file);
        return Err(format!("{url}: 與發行版列出的檔案不符"));
    }
    Ok(file)
}

/// What `command` printed, or what it said when it failed.
fn said(command: &mut Command) -> Result<String, String> {
    let out = command.output().map_err(|err| err.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Installs the package at `$2` for the user `$1`. A session of Sunshine's own may run with
/// no one at the computer, where uaccess gives no one the devices Sunshine makes its input
/// with, nor the GPU: they are given to `$1` instead.
fn install_script() -> String {
    let apt = apt::APT_GET[..apt::APT_GET.len() - 2].join(" ");
    format!(
        r#"set -e
{apt} install "$2"
cat > {UDEV_RULE} <<EOF
# Written by Nekoland's Settings, for Sunshine's session of its own.
KERNEL=="uinput", SUBSYSTEM=="misc", OWNER="$1"
KERNEL=="uhid", OWNER="$1"
SUBSYSTEM=="input", KERNEL=="event*", ATTRS{{name}}=="*libvirtualhid*", OWNER="$1"
SUBSYSTEM=="input", KERNEL=="event*", ATTRS{{phys}}=="libvirtualhid/*", OWNER="$1"
SUBSYSTEM=="hidraw", KERNEL=="hidraw*", ENV{{HID_PHYS}}=="libvirtualhid/*", OWNER="$1"
EOF
getent group render >/dev/null && usermod -aG render "$1"
udevadm control --reload-rules
udevadm trigger --property-match=DEVNAME=/dev/uinput --property-match=DEVNAME=/dev/uhid
{firewall}"#,
        firewall = sharing::firewall_sync()
    )
}

/// The ports Sunshine listens on, as its documentation has them: offsets from its `port`
/// setting, 47989 unless set.
pub(crate) fn firewall_ports(conf: &str) -> Vec<String> {
    let base = conf_entries(conf)
        .into_iter()
        .find_map(|(key, value)| (key == "port").then(|| value.parse::<u32>().ok()).flatten())
        .unwrap_or(DEFAULT_PORT);
    vec![
        format!("{},{},{},{}/tcp", base - 5, base, base + 1, base + 21),
        format!("{}:{},{},{}/udp", base + 9, base + 11, base + 13, base + 21),
    ]
}

/// `purge` takes its system-wide configuration too.
fn remove_script(purge: bool) -> String {
    let apt = apt::APT_GET[..apt::APT_GET.len() - 2].join(" ");
    let verb = if purge { "purge" } else { "remove" };
    let copies = PACKAGE_COPIES.join(" ");
    let firewall = sharing::firewall_sync();
    format!("set -e\n{apt} {verb} sunshine\nrm -f {UDEV_RULE} {copies}\nudevadm control --reload-rules\n{firewall}")
}

/// What Sunshine keeps for this person: its settings and paired devices, the units Settings
/// wrote for it and its login in the keyring.
async fn remove_own_data() {
    LOGIN.set(None);
    if let Some(user) = web_user() {
        if let Err(err) = secrets::clear(&keyring_item(&user)).await {
            eprintln!("keyring: {err}");
        }
    }
    let units = glib::user_config_dir().join("systemd/user");
    let removed = [
        (units.join(DESKTOP_UNIT), std::fs::remove_file(units.join(DESKTOP_UNIT))),
        (units.join(SESSION_UNIT), std::fs::remove_file(units.join(SESSION_UNIT))),
        (sunshine_dir(), std::fs::remove_dir_all(sunshine_dir())),
    ];
    for (path, removed) in removed {
        match removed {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => eprintln!("{}: {err}", path.display()),
            _ => {}
        }
    }
    run("systemctl", &["--user", "daemon-reload"]);
}

// Running.

/// The user units, written again each time, as the compositor's program may have moved.
fn write_units() -> std::io::Result<()> {
    let dir = glib::user_config_dir().join("systemd/user");
    std::fs::create_dir_all(&dir)?;
    let unit = |description: &str, command: &str| {
        format!(
            "# Written by Nekoland's Settings.\n[Unit]\nDescription={description}\n\n[Service]\nExecStart={command}\nRestart=on-failure\nRestartSec=5s\n\n[Install]\nWantedBy=default.target\n"
        )
    };
    // Started with the user's services, before the desktop; the compositor restarts it once
    // it has told them where its display is.
    write_atomic(&dir.join(DESKTOP_UNIT), &unit("Sunshine, streaming the desktop at the computer", PROGRAM))?;
    let nekoland = nekoland_program();
    let session = format!("\"{}\" --headless -c {PROGRAM}", nekoland.display());
    write_atomic(&dir.join(SESSION_UNIT), &unit("Sunshine, streaming a Nekoland session of its own", &session))
}

/// Turns Sunshine to stream `mode`, or off.
fn apply(mode: Option<Mode>) -> Result<(), String> {
    write_units().map_err(|err| err.to_string())?;
    let systemctl = |args: &[&str]| said(Command::new("systemctl").arg("--user").args(args)).map(drop);
    systemctl(&["daemon-reload"])?;
    systemctl(&["disable", "--now", DESKTOP_UNIT, SESSION_UNIT])?;
    let Some(mode) = mode else {
        return Ok(());
    };
    // Unless it was told another way to draw, or other commands to run for its clients.
    let path = conf_path();
    let read = std::fs::read_to_string(&path).unwrap_or_default();
    let mut conf = read.clone();
    let has = |conf: &str, key: &str| conf_entries(conf).iter().any(|(found, _)| found == key);
    if !has(&conf, CAPTURE.0) {
        conf = with_conf_value(&conf, CAPTURE.0, Some(CAPTURE.1));
    }
    if !has(&conf, PREP_COMMANDS) {
        conf = with_conf_value(&conf, PREP_COMMANDS, Some(&stream_mode_commands(&nekoland_program())));
    }
    if conf != read {
        std::fs::create_dir_all(sunshine_dir()).map_err(|err| err.to_string())?;
        write_atomic(&path, &conf).map_err(|err| err.to_string())?;
    }
    if mode == Mode::Session {
        // The user's services, this session among them, then start as the computer does.
        said(Command::new("loginctl").arg("enable-linger"))?;
    }
    systemctl(&["enable", "--now", mode.unit()])
}

/// Sunshine's prep commands that give each client a screen of its own size and rate: Nekoland
/// is told them as the client connects, and to go back as it leaves.
fn stream_mode_commands(nekoland: &Path) -> String {
    let program = format!("'{}'", nekoland.display().to_string().replace('\'', r"'\''"));
    let prep = serde_json::json!([{
        "do": format!(
            "sh -c \"{program} msg 'stream-mode '${{SUNSHINE_CLIENT_WIDTH}}x${{SUNSHINE_CLIENT_HEIGHT}}@${{SUNSHINE_CLIENT_FPS}}\""
        ),
        "undo": format!("sh -c \"{program} msg stream-mode\""),
        "elevated": false,
    }]);
    prep.to_string()
}

/// Restarts Sunshine where it runs, for a setting changed.
fn restart() {
    run("systemctl", &["--user", "try-restart", DESKTOP_UNIT, SESSION_UNIT]);
}

// sunshine.conf: `key = value` lines.

fn conf_entries(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_string(), value.trim().to_string())).filter(|(key, _)| !key.is_empty())
        })
        .collect()
}

/// `text` with `key` set to `value`, in the place it had, or removed for None.
fn with_conf_value(text: &str, key: &str, value: Option<&str>) -> String {
    let is_key = |line: &str| line.split_once('=').is_some_and(|(found, _)| found.trim() == key) && !line.trim_start().starts_with('#');
    let mut lines: Vec<String> = Vec::new();
    let mut placed = false;
    for line in text.lines() {
        if !is_key(line) {
            lines.push(line.to_string());
        } else if let Some(value) = value.filter(|_| !placed) {
            lines.push(format!("{key} = {value}"));
            placed = true;
        }
    }
    if let Some(value) = value.filter(|_| !placed) {
        lines.push(format!("{key} = {value}"));
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn set_conf_value(key: &str, value: Option<&str>) -> std::io::Result<()> {
    let path = conf_path();
    let conf = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::create_dir_all(sunshine_dir())?;
    write_atomic(&path, &with_conf_value(&conf, key, value))?;
    restart();
    Ok(())
}

// The web interface's API.

#[derive(Debug)]
enum ApiError {
    /// The password was not the one Sunshine keeps.
    Refused,
    Failed(String),
}

type Login = (String, String);

thread_local! {
    /// The login once given, for as long as Settings runs: without the keyring it would be
    /// asked for at each step.
    static LOGIN: RefCell<Option<Login>> = const { RefCell::new(None) };
}

/// How long Sunshine is waited for when it does not answer: it may be starting, as it is
/// after its password was set.
const API_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Asks Sunshine's API at `path`, posting `body` if there is one, as `login`.
fn api(path: &str, body: Option<&str>, login: &Login) -> Result<serde_json::Value, ApiError> {
    let since = std::time::Instant::now();
    loop {
        match api_once(path, body, login) {
            Err(ApiError::Failed(err)) if err == NOT_RUNNING && since.elapsed() < API_WAIT => {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            answer => return answer,
        }
    }
}

const NOT_RUNNING: &str = "Sunshine 未執行";

fn api_once(path: &str, body: Option<&str>, (user, password): &Login) -> Result<serde_json::Value, ApiError> {
    let mut command = Command::new("curl");
    // The login goes in on stdin, where other programs cannot read it as they can arguments.
    command.args(["-sk", "--config", "-", "-w", "\n%{http_code}"]);
    if let Some(body) = body {
        command.args(["-H", "Content-Type: application/json", "--data-binary", body]);
    }
    command.arg(format!("{WEB_UI}{path}"));
    let quoted = |text: &str| text.replace('\\', "\\\\").replace('"', "\\\"");
    let login = format!("user = \"{}:{}\"\n", quoted(user), quoted(password));
    let child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let out = child
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().map_or(Ok(()), |mut stdin| stdin.write_all(login.as_bytes()))?;
            child.wait_with_output()
        })
        .map_err(|err| ApiError::Failed(err.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    match code.trim() {
        "401" => Err(ApiError::Refused),
        "000" => Err(ApiError::Failed(NOT_RUNNING.into())),
        code if code.starts_with('2') => serde_json::from_str(body).map_err(|err| ApiError::Failed(err.to_string())),
        code => Err(ApiError::Failed(format!("HTTP {code}"))),
    }
}

/// The name Sunshine's own login is under, once one was made.
fn web_user() -> Option<String> {
    let state = std::fs::read_to_string(sunshine_dir().join("sunshine_state.json")).ok()?;
    let state: serde_json::Value = serde_json::from_str(&state).ok()?;
    state["username"].as_str().filter(|user| !user.is_empty()).map(str::to_string)
}

fn keyring_item(user: &str) -> secrets::Attributes {
    secrets::Attributes::from([
        ("xdg:schema".to_string(), KEYRING_SCHEMA.to_string()),
        ("user".to_string(), user.to_string()),
    ])
}

/// Gets Sunshine's login for `then`: from the keyring, else asked for, or made, when
/// Sunshine has none yet. `then` gets None when there is none to be had.
fn with_login(page: &adw::PreferencesPage, then: impl FnOnce(Option<Login>) + 'static) {
    let page = page.clone();
    if let Some(login) = LOGIN.with_borrow(Clone::clone) {
        return then(Some(login));
    }
    glib::spawn_future_local(async move {
        let Some(user) = web_user() else {
            let user = std::env::var("USER").unwrap_or_default();
            // Whichever of the two ways the dialog ends takes it.
            let then: Rc<RefCell<Option<Box<dyn FnOnce(Option<Login>)>>>> = Rc::new(RefCell::new(Some(Box::new(then))));
            let answer = then.clone();
            let cancelled: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(then) = answer.borrow_mut().take() {
                    then(None);
                }
            });
            return users::ask_password(&page.clone(), "Sunshine 密碼", "設定", cancelled, move |password| {
                let Some(then) = then.borrow_mut().take() else {
                    return;
                };
                glib::spawn_future_local(async move {
                    let login = (user.clone(), password.clone());
                    let made = gio::spawn_blocking(move || said(Command::new(PROGRAM).args(["--creds", &user, &password]))).await;
                    match made {
                        Ok(Ok(_)) => {
                            remember(&login).await;
                            restart();
                            return then(Some(login));
                        }
                        Ok(Err(err)) => alert::notice(&page, "無法設定 Sunshine 密碼", Some(&err)),
                        Err(_) => alert::notice(&page, "無法設定 Sunshine 密碼", None),
                    }
                    then(None);
                });
            });
        };
        match secrets::lookup(&keyring_item(&user)).await {
            Ok(Some(password)) => {
                let login = (user, password);
                LOGIN.set(Some(login.clone()));
                return then(Some(login));
            }
            Ok(None) => {}
            Err(err) => eprintln!("keyring: {err}"),
        }
        let entry = adw::PasswordEntryRow::builder().title("密碼").build();
        let alert = adw::AlertDialog::new(Some("Sunshine 密碼"), Some(&user));
        alert.set_extra_child(Some(&users::form(&[entry.upcast_ref()])));
        alert.add_responses(&[("cancel", "取消"), ("do", "繼續")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("do"));
        alert.set_close_response("cancel");
        if alert.choose_future(&page).await != "do" || entry.text().is_empty() {
            return then(None);
        }
        let login = (user, entry.text().to_string());
        remember(&login).await;
        then(Some(login));
    });
}

async fn remember(login: &Login) {
    LOGIN.set(Some(login.clone()));
    let (user, password) = login;
    if let Err(err) = secrets::store(&keyring_item(user), &format!("Sunshine: {user}"), password).await {
        eprintln!("keyring: {err}");
    }
}

/// Asks the API as `with_login` gets the login; a password refused is forgotten, to be
/// asked for again next time. `then` gets None when it failed, which is told, or no
/// login was given.
fn ask_api(
    page: &adw::PreferencesPage,
    failure: &'static str,
    path: &'static str,
    body: Option<String>,
    then: impl FnOnce(Option<serde_json::Value>) + 'static,
) {
    let parent = page.clone();
    with_login(page, move |login| {
        let Some(login) = login else {
            return then(None);
        };
        glib::spawn_future_local(async move {
            let user = login.0.clone();
            let asked = gio::spawn_blocking(move || api(path, body.as_deref(), &login)).await;
            match asked {
                Ok(Ok(answer)) => return then(Some(answer)),
                Ok(Err(ApiError::Refused)) => {
                    LOGIN.set(None);
                    if let Err(err) = secrets::clear(&keyring_item(&user)).await {
                        eprintln!("keyring: {err}");
                    }
                    alert::notice(&parent, failure, Some("密碼錯誤"));
                }
                Ok(Err(ApiError::Failed(err))) => alert::notice(&parent, failure, Some(&err)),
                Err(_) => alert::notice(&parent, failure, None),
            }
            then(None);
        });
    });
}

// The page.

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}

pub(crate) fn text_button(label: &str) -> gtk::Button {
    gtk::Button::builder().label(label).valign(gtk::Align::Center).build()
}

/// Sunshine's rows, for the 遠端登入 group of the Sharing page.
pub(crate) fn add_sunshine_rows(page: &adw::PreferencesPage, rows: &adw::PreferencesGroup) {
    // Until it is installed, a row to install it from: a switch row held off would hold its
    // button off with it.
    let missing = adw::ActionRow::builder().title("Sunshine").build();
    let install = text_button("安裝");
    missing.add_suffix(&install);
    let service = adw::SwitchRow::builder().title("Sunshine").build();
    let remove = text_button("移除");
    service.add_suffix(&remove);
    let labels: Vec<&str> = Mode::ALL.iter().map(|mode| mode.label()).collect();
    let session = adw::ComboRow::builder()
        .title("工作階段")
        .model(&gtk::StringList::new(&labels))
        .build();
    let devices = subpage_row("裝置", String::new, devices_page(page));
    let settings = subpage_row("設定", String::new, conf_page());
    let web = adw::ActionRow::builder().title("網頁管理").build();
    let open = text_button("開啟");
    web.add_suffix(&open);
    web.set_activatable_widget(Some(&open));
    for row in [missing.upcast_ref::<gtk::Widget>(), service.upcast_ref(), session.upcast_ref(), devices.upcast_ref(), settings.upcast_ref(), web.upcast_ref()] {
        rows.add(row);
    }

    // Set while the rows are made to show Sunshine as it is, not changed by the person.
    let showing = Rc::new(Cell::new(false));
    // Set while it is being installed, removed or turned.
    let busy = Rc::new(Cell::new(false));
    let show = Rc::new(glib::clone!(
        #[weak]
        missing,
        #[weak]
        service,
        #[weak]
        install,
        #[weak]
        remove,
        #[weak]
        session,
        #[weak]
        devices,
        #[weak]
        settings,
        #[weak]
        web,
        #[strong]
        showing,
        #[strong]
        busy,
        move || {
            let (installed, mode) = (installed(), mode());
            let idle = !busy.get();
            // While it is being changed, the rows keep what the person chose: put back then,
            // the switch would be turned again.
            if idle {
                showing.set(true);
                service.set_active(mode.is_some());
                if let Some(mode) = mode {
                    session.set_selected(Mode::ALL.iter().position(|each| *each == mode).unwrap_or(0) as u32);
                }
                showing.set(false);
            }
            service.set_sensitive(idle);
            install.set_sensitive(idle);
            remove.set_sensitive(idle);
            session.set_sensitive(idle);
            if idle {
                missing.set_subtitle("未安裝");
                service.set_subtitle(&mode.map(|_| remote_address()).unwrap_or_default());
            }
            missing.set_visible(!installed);
            for row in [service.upcast_ref::<gtk::Widget>(), session.upcast_ref(), devices.upcast_ref(), settings.upcast_ref()] {
                row.set_visible(installed);
            }
            web.set_visible(mode.is_some());
        }
    ));
    show();

    // Runs `work` off the main thread, with the rows held still and `doing` said meanwhile.
    let run_busy = Rc::new(glib::clone!(
        #[weak]
        service,
        #[strong]
        busy,
        #[strong]
        show,
        move |doing: &str, failure: &'static str, work: Box<dyn FnOnce() -> Result<(), String> + Send>| {
            busy.set(true);
            show();
            service.set_subtitle(doing);
            let (busy, show, service) = (busy.clone(), show.clone(), service.clone());
            glib::spawn_future_local(async move {
                let done = gio::spawn_blocking(work).await;
                busy.set(false);
                show();
                match done {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => alert::notice(&service, failure, Some(&err)),
                    Err(_) => alert::notice(&service, failure, None),
                }
            });
        }
    ));

    let chosen = glib::clone!(
        #[weak]
        session,
        #[upgrade_or]
        Mode::Desktop,
        move || Mode::ALL.get(session.selected() as usize).copied().unwrap_or(Mode::Desktop)
    );
    service.connect_active_notify(glib::clone!(
        #[strong]
        showing,
        #[strong]
        run_busy,
        #[strong]
        chosen,
        move |row| {
            if showing.get() {
                return;
            }
            let mode = row.is_active().then(&chosen);
            let (doing, failure) = if mode.is_some() { ("正在開啟", "無法開啟 Sunshine") } else { ("正在關閉", "無法關閉 Sunshine") };
            run_busy(doing, failure, Box::new(move || apply(mode)));
        }
    ));
    session.connect_selected_notify(glib::clone!(
        #[strong]
        showing,
        #[strong]
        run_busy,
        #[weak]
        service,
        move |_| {
            if showing.get() || !service.is_active() {
                return;
            }
            let mode = chosen();
            run_busy("正在切換", "無法切換工作階段", Box::new(move || apply(Some(mode))));
        }
    ));

    install.connect_clicked(glib::clone!(
        #[weak(rename_to = service)]
        missing,
        #[strong]
        busy,
        #[strong]
        show,
        move |_| {
            busy.set(true);
            show();
            service.set_subtitle("正在下載");
            let (busy, show, service) = (busy.clone(), show.clone(), service.clone());
            glib::spawn_future_local(async move {
                let downloaded = gio::spawn_blocking(download_package).await;
                let row = service.clone();
                let finish = move |failure: Option<String>| {
                    busy.set(false);
                    show();
                    if let Some(err) = failure {
                        alert::notice(&row, "無法安裝 Sunshine", Some(&err).filter(|err| !err.is_empty()).map(String::as_str));
                    }
                };
                let package = match downloaded {
                    Ok(Ok(package)) => package,
                    Ok(Err(err)) => return finish(Some(err)),
                    Err(_) => return finish(Some(String::new())),
                };
                service.set_subtitle("正在安裝");
                let user = std::env::var("USER").unwrap_or_default();
                let script = install_script();
                let package_path = package.to_string_lossy().into_owned();
                let args = ["sh", "-c", script.as_str(), "sh", user.as_str(), package_path.as_str()];
                as_root("安裝 Sunshine", &args, None, move |worked| {
                    if let Err(err) = std::fs::remove_file(&package) {
                        eprintln!("{}: {err}", package.display());
                    }
                    finish((!worked).then(String::new));
                });
            });
        }
    ));
    remove.connect_clicked(glib::clone!(
        #[weak]
        service,
        #[strong]
        busy,
        #[strong]
        show,
        move |remove| {
            let (service, busy, show) = (service.clone(), busy.clone(), show.clone());
            alert::confirm_removal(remove, "移除 Sunshine？", "一併移除設定與已配對的裝置", move |purge| {
                busy.set(true);
                show();
                service.set_subtitle("正在移除");
                glib::spawn_future_local(async move {
                    let stopped = gio::spawn_blocking(|| apply(None)).await;
                    if let Ok(Err(err)) = stopped {
                        eprintln!("Sunshine: {err}");
                    }
                    let script = remove_script(purge);
                    as_root("移除 Sunshine", &["sh", "-c", &script], None, move |worked| {
                        glib::spawn_future_local(async move {
                            if worked && purge {
                                remove_own_data().await;
                            }
                            busy.set(false);
                            show();
                            if !worked {
                                alert::notice(&service, "無法移除 Sunshine", None);
                            }
                        });
                    });
                });
            });
        }
    ));
    open.connect_clicked(|button| {
        let window = button.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(WEB_UI).launch(window.as_ref(), gio::Cancellable::NONE, |launched| {
            if let Err(err) = launched {
                eprintln!("{WEB_UI}: {err}");
            }
        });
    });
}

/// 裝置: the devices paired with Sunshine, and pairing one more with the PIN Moonlight shows.
fn devices_page(sharing: &adw::PreferencesPage) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let paired = group("");
    let pair = text_button("配對⋯");
    paired.set_header_suffix(Some(&pair));
    page.add(&paired);
    let shown: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    // Set while the list is asked for: the page is mapped more than once as it opens, and
    // each would ask for the password.
    let listing = Rc::new(Cell::new(false));

    let list: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        paired,
        #[weak]
        sharing,
        #[strong]
        shown,
        #[strong]
        listing,
        move || {
            if listing.replace(true) {
                return;
            }
            let (shown, page) = (shown.clone(), sharing.clone());
            let done = listing.clone();
            ask_api(&sharing, "無法列出裝置", "/api/clients/list", None, move |answer| {
                done.set(false);
                let Some(answer) = answer else {
                    return;
                };
                for row in shown.borrow_mut().drain(..) {
                    paired.remove(&row);
                }
                let clients = answer["named_certs"].as_array().cloned().unwrap_or_default();
                if clients.is_empty() {
                    let row = adw::ActionRow::builder().title("沒有配對的裝置").build();
                    paired.add(&row);
                    shown.borrow_mut().push(row);
                }
                for client in clients {
                    let (Some(name), Some(uuid)) = (client["name"].as_str(), client["uuid"].as_str()) else {
                        continue;
                    };
                    let row = adw::ActionRow::builder().title(glib::markup_escape_text(name)).build();
                    let forget = icon_button("user-trash-symbolic", "移除");
                    row.add_suffix(&forget);
                    let (name, uuid) = (name.to_string(), uuid.to_string());
                    forget.connect_clicked(glib::clone!(
                        #[weak]
                        page,
                        #[weak]
                        paired,
                        #[weak]
                        row,
                        move |forget| {
                            let body = serde_json::json!({ "uuid": uuid }).to_string();
                            alert::confirm(forget, &format!("移除「{name}」？"), None, "移除", move || {
                                ask_api(&page, "無法移除裝置", "/api/clients/unpair", Some(body), move |answer| {
                                    if answer.is_some() {
                                        paired.remove(&row);
                                    }
                                });
                            });
                        }
                    ));
                    paired.add(&row);
                    shown.borrow_mut().push(row);
                }
            });
        }
    ));
    // Listed as the page is opened, which may ask for the password.
    page.connect_map(glib::clone!(
        #[strong]
        list,
        move |_| list()
    ));
    pair.connect_clicked(glib::clone!(
        #[weak]
        sharing,
        #[strong]
        list,
        move |_| {
            let list = list.clone();
            // The devices that asked to pair, each for the PIN it shows.
            ask_api(&sharing, "無法配對", "/api/pin", None, glib::clone!(
                #[weak]
                sharing,
                move |answer| {
                    let Some(answer) = answer else {
                        return;
                    };
                    let waiting = pending_pairings(&answer);
                    if waiting.is_empty() {
                        return alert::notice(&sharing, "沒有等候配對的裝置", Some("在 Moonlight 新增這部電腦後，再輸入它顯示的 PIN 碼。"));
                    }
                    ask_pin(&sharing, waiting, list);
                }
            ));
        }
    ));
    page
}

/// A device waiting to be paired: its request's id, and its name and address as shown.
#[derive(Debug, PartialEq)]
struct Pending {
    id: String,
    name: String,
    address: String,
}

/// The pairing requests `GET /api/pin` lists.
fn pending_pairings(answer: &serde_json::Value) -> Vec<Pending> {
    let text = |pairing: &serde_json::Value, key: &str| pairing[key].as_str().unwrap_or_default().to_string();
    answer["pairings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pairing| {
            let id = pairing["id"].as_str()?.to_string();
            Some(Pending { id, name: text(pairing, "name"), address: text(pairing, "address") })
        })
        .collect()
}

/// Asks which of the waiting devices to pair, by the PIN it shows, and under what name.
fn ask_pin(sharing: &adw::PreferencesPage, waiting: Vec<Pending>, then: Rc<dyn Fn()>) {
    let labels: Vec<String> = waiting
        .iter()
        .map(|pending| {
            let name = if pending.name.is_empty() { "Moonlight" } else { &pending.name };
            if pending.address.is_empty() { name.to_string() } else { format!("{name}（{}）", pending.address) }
        })
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let device = adw::ComboRow::builder().title("裝置").model(&gtk::StringList::new(&labels)).build();
    let pin = adw::EntryRow::builder().title("PIN 碼").input_purpose(gtk::InputPurpose::Digits).build();
    let name = adw::EntryRow::builder().title("名稱").text(&waiting[0].name).build();
    device.connect_selected_notify(glib::clone!(
        #[weak]
        name,
        #[strong(rename_to = names)]
        waiting.iter().map(|pending| pending.name.clone()).collect::<Vec<_>>(),
        move |device| name.set_text(names.get(device.selected() as usize).map_or("", String::as_str))
    ));
    let alert = adw::AlertDialog::new(Some("配對裝置"), None);
    alert.set_extra_child(Some(&users::form(&[device.upcast_ref(), pin.upcast_ref(), name.upcast_ref()])));
    alert.add_responses(&[("cancel", "取消"), ("do", "配對")]);
    alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("do"));
    alert.set_close_response("cancel");
    let sharing = sharing.clone();
    alert.choose(&sharing.clone(), gio::Cancellable::NONE, move |response| {
        let pin = pin.text().trim().to_string();
        let Some(pending) = waiting.get(device.selected() as usize).filter(|_| response == "do" && !pin.is_empty()) else {
            return;
        };
        let name = Some(name.text().trim().to_string()).filter(|name| !name.is_empty()).unwrap_or_else(|| "Moonlight".into());
        let body = serde_json::json!({ "pairing_id": pending.id, "pin": pin, "name": name }).to_string();
        let parent = sharing.clone();
        ask_api(&sharing, "無法配對", "/api/pin", Some(body), move |answer| {
            let Some(answer) = answer else {
                return;
            };
            if answer["status"].as_bool() == Some(true) {
                then();
            } else {
                alert::notice(&parent, "無法配對", Some("PIN 碼不符"));
            }
        });
    });
}

/// 設定: sunshine.conf's settings, each changed in place, and new ones added by name; what
/// each one is, its web interface tells.
fn conf_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let entries = group("sunshine.conf");
    let adding = group("");
    let key = adw::EntryRow::builder().title("名稱").build();
    let value = adw::EntryRow::builder().title("值").build();
    let add = text_button("新增");
    value.add_suffix(&add);
    adding.add(&key);
    adding.add(&value);
    page.add(&entries);
    page.add(&adding);
    let shown: Rc<RefCell<Vec<adw::EntryRow>>> = Rc::default();

    let list: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        entries,
        #[strong]
        shown,
        move || {
            for row in shown.borrow_mut().drain(..) {
                entries.remove(&row);
            }
            let conf = std::fs::read_to_string(conf_path()).unwrap_or_default();
            for (name, text) in conf_entries(&conf) {
                let row = adw::EntryRow::builder().title(&name).text(&text).show_apply_button(true).build();
                let forget = icon_button("user-trash-symbolic", "移除");
                row.add_suffix(&forget);
                let applied = name.clone();
                row.connect_apply(move |row| {
                    if let Err(err) = set_conf_value(&applied, Some(row.text().trim())) {
                        alert::notice(row, "無法儲存設定", Some(&err.to_string()));
                    }
                });
                forget.connect_clicked(glib::clone!(
                    #[weak]
                    entries,
                    #[weak]
                    row,
                    move |_| match set_conf_value(&name, None) {
                        Ok(()) => entries.remove(&row),
                        Err(err) => alert::notice(&row, "無法儲存設定", Some(&err.to_string())),
                    }
                ));
                entries.add(&row);
                shown.borrow_mut().push(row);
            }
        }
    ));
    page.connect_map(glib::clone!(
        #[strong]
        list,
        move |_| list()
    ));
    add.connect_clicked(glib::clone!(
        #[weak]
        key,
        #[weak]
        value,
        move |add| {
            let name = key.text().trim().to_string();
            if name.is_empty() || name.contains(['=', '#', '\n']) {
                return;
            }
            match set_conf_value(&name, Some(value.text().trim())) {
                Ok(()) => {
                    key.set_text("");
                    value.set_text("");
                    list();
                }
                Err(err) => alert::notice(add, "無法儲存設定", Some(&err.to_string())),
            }
        }
    ));
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_is_the_one_for_this_system() {
        let release = r#"{"assets": [
            {"name": "sunshine_1-1+ubuntu24.04_amd64.deb", "browser_download_url": "https://x/24", "digest": "sha256:aa"},
            {"name": "sunshine_1-1+ubuntu26.10_arm64.deb", "browser_download_url": "https://x/arm", "digest": "sha256:bb"},
            {"name": "sunshine_1-1+ubuntu26.10_amd64.deb", "browser_download_url": "https://x/26", "digest": "sha256:cc"},
            {"name": "sunshine_1-1+debiantrixie_amd64.deb", "browser_download_url": "https://x/trixie", "digest": "sha256:dd"}
        ]}"#;
        let ubuntu = "ID=ubuntu\nVERSION_ID=\"26.10\"\nVERSION_CODENAME=resolute\n";
        assert_eq!(package_url(release, ubuntu, "amd64"), Some(("https://x/26".into(), "cc".into())));
        let debian = "ID=debian\nVERSION_ID=\"13\"\nVERSION_CODENAME=trixie\n";
        assert_eq!(package_url(release, debian, "amd64"), Some(("https://x/trixie".into(), "dd".into())));
        assert_eq!(package_url(release, "ID=fedora\nVERSION_ID=44\n", "amd64"), None);
    }

    #[test]
    fn clients_tell_nekoland_their_size_as_they_connect_and_leave() {
        let commands = stream_mode_commands(Path::new("/usr/local/bin/nekoland"));
        let prep: serde_json::Value = serde_json::from_str(&commands).unwrap();
        assert_eq!(
            prep[0]["do"],
            "sh -c \"'/usr/local/bin/nekoland' msg 'stream-mode '${SUNSHINE_CLIENT_WIDTH}x${SUNSHINE_CLIENT_HEIGHT}@${SUNSHINE_CLIENT_FPS}\""
        );
        assert_eq!(prep[0]["undo"], "sh -c \"'/usr/local/bin/nekoland' msg stream-mode\"");
        assert!(!commands.contains('\n'), "one line of sunshine.conf");
    }

    #[test]
    fn conf_values_change_in_place() {
        let conf = "# comment = kept\nport = 47989\nsunshine_name = desk\n";
        assert_eq!(
            conf_entries(conf),
            vec![("port".to_string(), "47989".to_string()), ("sunshine_name".to_string(), "desk".to_string())]
        );
        assert_eq!(
            with_conf_value(conf, "port", Some("48000")),
            "# comment = kept\nport = 48000\nsunshine_name = desk\n"
        );
        assert_eq!(with_conf_value(conf, "port", None), "# comment = kept\nsunshine_name = desk\n");
        assert_eq!(with_conf_value("", "capture", Some("wlr")), "capture = wlr\n");
    }

    #[test]
    fn sunshines_ports_follow_its_port_setting() {
        assert_eq!(firewall_ports(""), ["47984,47989,47990,48010/tcp", "47998:48000,48002,48010/udp"]);
        assert_eq!(firewall_ports("port = 50000\n"), ["49995,50000,50001,50021/tcp", "50009:50011,50013,50021/udp"]);
    }

    #[test]
    fn pending_pairings_are_read_with_what_they_lack() {
        let answer = serde_json::json!({ "pairings": [
            { "id": "0123456789abcdef0123456789abcdef", "name": "Steam Deck", "address": "192.168.1.20" },
            { "id": "fedcba9876543210fedcba9876543210" },
            { "name": "no id" }
        ]});
        assert_eq!(
            pending_pairings(&answer),
            vec![
                Pending { id: "0123456789abcdef0123456789abcdef".into(), name: "Steam Deck".into(), address: "192.168.1.20".into() },
                Pending { id: "fedcba9876543210fedcba9876543210".into(), name: String::new(), address: String::new() },
            ]
        );
        assert!(pending_pairings(&serde_json::json!({ "pairings": [] })).is_empty());
    }
}
