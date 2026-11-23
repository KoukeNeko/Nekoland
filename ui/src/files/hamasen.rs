//! Hamasen's servers, set up and mounted by Files itself: SFTP, FTP, SMB, WebDAV and NFS
//! through GVfs, cloud drives and S3 through rclone on a folder of their own. The list
//! carries on from Hamasen's servers.json and the passwords are its keyring items.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ffi::OsStr,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use adw::prelude::*;
use gtk::{gio, glib};
use serde_json::{Map, Value};

use crate::{
    alert,
    browse::{self, hamasen_records, hamasen_servers, protocol, Kind, Server, EDGE, PROTOCOLS},
    ops, secrets, Window,
};
pub use crate::mount::{cancel, connecting, is_mounted, mount};
use crate::mount::{finish, keyring_item, rc_socket, rclone, run, set_bookmark, spawn, KEYRING_LABEL};


const GUIDE_URL: &str = "https://console.cloud.google.com/apis/credentials";
/// At login the network, a VPN and the keyring may not be up yet, so it is tried a while.
const LOGIN_ATTEMPTS: u32 = 20;
const LOGIN_RETRY: u32 = 15;
const WATCH_EVERY: u32 = 3;

thread_local! {
    /// The servers being unmounted from Files, whose going is no lost connection.
    static RELEASING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// The files rclone still has to upload, by the folder it serves.
    static UPLOADS: RefCell<HashMap<PathBuf, u64>> = RefCell::new(HashMap::new());
}

/// Says the server is being unmounted, or with `false` that it stayed.
pub fn releasing(id: &str, going: bool) {
    RELEASING.with_borrow_mut(|r| {
        if going {
            r.insert(id.to_owned());
        } else {
            r.remove(id);
        }
    });
}

/// The files rclone still has to upload into the folder it serves.
pub fn uploads_into(root: &Path) -> u64 {
    UPLOADS.with_borrow(|u| u.get(root).copied().unwrap_or(0))
}

fn mounted_ids(servers: &[Server]) -> HashSet<String> {
    servers.iter().filter(|s| is_mounted(s)).map(|s| s.id.clone()).collect()
}

fn notify(app: &adw::Application, key: &str, title: &str) {
    app.send_notification(Some(key), &gio::Notification::new(title));
}

/// Says when a server's connection is lost, or rclone has sent on all it took; Files keeps
/// running for it while a server is mounted, with no window open.
pub fn watch(app: &adw::Application) {
    let app = app.clone();
    let mut mounted = mounted_ids(&hamasen_servers());
    let mut hold = None;
    let asking: Rc<RefCell<HashSet<String>>> = Rc::default();
    glib::timeout_add_seconds_local(WATCH_EVERY, move || {
        let servers = hamasen_servers();
        let now = mounted_ids(&servers);
        RELEASING.with_borrow_mut(|releasing| {
            for server in servers.iter().filter(|s| mounted.contains(&s.id) && !now.contains(&s.id)) {
                if !releasing.contains(&server.id) {
                    notify(&app, &format!("lost-{}", server.id), &format!("「{}」連線中斷", server.name));
                }
            }
            releasing.retain(|id| now.contains(id));
        });
        mounted = now;
        hold = (!mounted.is_empty()).then(|| hold.take().unwrap_or_else(|| app.hold()));

        let served: Vec<&Server> = servers.iter().filter(|s| !s.gvfs && mounted.contains(&s.id)).collect();
        let roots: HashSet<PathBuf> = served.iter().filter_map(|s| s.location.path()).collect();
        let gone = UPLOADS.with_borrow_mut(|u| {
            let before = u.len();
            u.retain(|root, _| roots.contains(root));
            u.len() != before
        });
        if gone {
            ops::transfers_changed();
        }
        // rclone answers slowly while it is busy listing, so off the main loop, once at a time.
        for server in served {
            let Some(root) = server.location.path() else { continue };
            if !asking.borrow_mut().insert(server.id.clone()) {
                continue;
            }
            let (app, asking, server) = (app.clone(), asking.clone(), server.clone());
            glib::spawn_future_local(async move {
                let socket = rc_socket(&server.id);
                let count = gio::spawn_blocking(move || pending_uploads(&socket)).await.unwrap_or(0);
                asking.borrow_mut().remove(&server.id);
                let before = UPLOADS.with_borrow_mut(|u| u.insert(root, count)).unwrap_or(0);
                if before == count {
                    return;
                }
                if count == 0 {
                    notify(&app, &format!("uploaded-{}", server.id), &format!("「{}」上傳完成", server.name));
                }
                ops::transfers_changed();
            });
        }
        glib::ControlFlow::Continue
    });
}


fn save_records(records: &[Map<String, Value>]) -> Result<(), String> {
    let path = browse::hamasen_servers_path();
    let text = serde_json::to_string_pretty(records).map_err(|e| e.to_string())?;
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, text))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Puts the record in the list, in its place when it is there already.
fn put(record: Map<String, Value>) -> Result<(), String> {
    let mut records = hamasen_records();
    let id = record.get("id").cloned();
    match records.iter_mut().find(|r| r.get("id") == id.as_ref()) {
        Some(kept) => *kept = record,
        None => records.push(record),
    }
    save_records(&records)
}

/// Unmounts a folder rclone serves, asking first while it still has files to upload.
pub fn unmount(win: &Rc<Window>, server: Server) {
    let weak = Rc::downgrade(win);
    glib::spawn_future_local(async move {
        let socket = rc_socket(&server.id);
        let pending = gio::spawn_blocking(move || pending_uploads(&socket)).await.unwrap_or(0);
        let Some(win) = weak.upgrade() else { return };
        if pending == 0 {
            eject(&win, server);
            return;
        }
        let title = format!("卸載「{}」？", server.name);
        let body = format!("{pending} 個檔案尚未上傳。");
        win.confirm(&title, &body, "仍要卸載", move |win| eject(win, server));
    });
}

fn eject(win: &Rc<Window>, server: Server) {
    let weak = Rc::downgrade(win);
    releasing(&server.id, true);
    glib::spawn_future_local(async move {
        let path = server.location.path().expect("rclone mounts on a folder");
        match run(&["fusermount3".as_ref(), "-u".as_ref(), path.as_os_str()], &[]).await {
            Ok(()) => set_bookmark(&server, false),
            Err(why) => {
                releasing(&server.id, false);
                if let Some(win) = weak.upgrade() {
                    win.alert(&format!("無法退出「{}」", server.name), Some(&why));
                }
            }
        }
    });
}

/// The files rclone has taken but not sent on yet; 0 when it cannot be asked.
fn pending_uploads(socket: &Path) -> u64 {
    let ask = || -> std::io::Result<String> {
        let mut stream = UnixStream::connect(socket)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(
            b"POST /vfs/stats HTTP/1.0\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
        )?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        Ok(reply)
    };
    ask().ok().and_then(|reply| uploads_in(&reply)).unwrap_or(0)
}

fn uploads_in(reply: &str) -> Option<u64> {
    let (_, body) = reply.split_once("\r\n\r\n")?;
    let stats: Value = serde_json::from_str(body).ok()?;
    let cache = stats.get("diskCache")?;
    Some(cache.get("uploadsQueued")?.as_u64()? + cache.get("uploadsInProgress")?.as_u64()?)
}

/// The concrete `Host` aliases of an SSH config. They are mounted by alias, so ssh itself
/// applies that entry's HostName, User, Port and IdentityFile.
fn ssh_config_hosts(text: &str) -> Vec<String> {
    let host = regex::Regex::new(r"(?i)^\s*host\s*[=\s]\s*(.+)").expect("a valid pattern");
    let mut hosts: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(found) = host.captures(line) {
            for name in found[1].split_whitespace() {
                if !name.contains(['*', '?', '!']) && !hosts.iter().any(|h| h == name) {
                    hosts.push(name.to_owned());
                }
            }
        }
    }
    hosts
}

pub fn import_ssh(win: &Rc<Window>) {
    let text = std::fs::read_to_string(browse::home().join(".ssh/config")).unwrap_or_default();
    let mut records = hamasen_records();
    let known: HashSet<String> = records
        .iter()
        .filter(|r| r.get("scheme").and_then(Value::as_str) == Some("sftp"))
        .filter_map(|r| r.get("host").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let fresh: Vec<String> = ssh_config_hosts(&text)
        .into_iter()
        .filter(|h| !known.contains(h))
        .collect();
    if fresh.is_empty() {
        win.alert("已匯入 0 台伺服器", None);
        return;
    }
    for host in fresh {
        let record = serde_json::json!({"id": glib::uuid_string_random().as_str(), "name": host,
            "scheme": "sftp", "host": host, "port": null, "user": "", "path": ""});
        records.extend(record.as_object().cloned());
    }
    if let Err(why) = save_records(&records) {
        win.alert("無法儲存伺服器", Some(&why));
    }
}

pub fn delete(win: &Rc<Window>, server: Server) {
    let title = format!("刪除「{}」？", server.name);
    let weak = Rc::downgrade(win);
    alert::confirm(&win.window, &title, None, "刪除", move || {
        let Some(win) = weak.upgrade() else { return };
        let mut records = hamasen_records();
        records.retain(|r| r.get("id").and_then(Value::as_str) != Some(server.id.as_str()));
        if let Err(why) = save_records(&records) {
            return win.alert("無法刪除伺服器", Some(&why));
        }
        let id = server.id.clone();
        glib::spawn_future_local(async move {
            if let Err(err) = secrets::clear(&keyring_item(&id)).await {
                eprintln!("nekoland-files: the keyring keeps the password of {id}: {err}");
            }
        });
        if server.gvfs {
            return;
        }
        let remote = server.field("remote").to_owned();
        if is_mounted(&server) {
            unmount(&win, server);
        }
        if !remote.is_empty() {
            glib::spawn_future_local(async move {
                let argv = [rclone().into_os_string(), "config".into(), "delete".into(), remote.into()];
                let argv: Vec<&OsStr> = argv.iter().map(|a| a.as_os_str()).collect();
                if let Err(why) = run(&argv, &[]).await {
                    eprintln!("nekoland-files: rclone keeps its remote: {why}");
                }
            });
        }
    });
}

/// Mounts the servers set to mount at login, trying again for a few minutes; a server that
/// still fails at the end is said in a notification, as there is no window to say it in.
pub fn mount_at_login(app: &adw::Application) {
    let hold = Rc::new(app.hold());
    let app = app.clone();
    glib::spawn_future_local(async move {
        for attempt in 1..=LOGIN_ATTEMPTS {
            glib::timeout_future_seconds(if attempt == 1 { 2 } else { LOGIN_RETRY }).await;
            let last = attempt == LOGIN_ATTEMPTS;
            let waiting: Vec<Server> = hamasen_servers()
                .into_iter()
                .filter(|s| s.record.get("auto_mount").and_then(Value::as_bool) == Some(true))
                .filter(|s| !is_mounted(s))
                .collect();
            if waiting.is_empty() {
                break;
            }
            for server in waiting.into_iter().filter(|s| !connecting(&s.id)) {
                let (hold, app) = (hold.clone(), app.clone());
                glib::spawn_future_local(async move {
                    let _held = hold;
                    let Err(why) = mount(&server, None).await else { return };
                    eprintln!("nekoland-files: cannot mount {} yet: {why}", server.name);
                    if last {
                        let notice = gio::Notification::new(&format!("無法掛載「{}」", server.name));
                        notice.set_body(Some(&why));
                        app.send_notification(Some(&format!("mount-{}", server.id)), &notice);
                    }
                });
            }
        }
        drop(hold);
    });
}

struct Form {
    alert: adw::AlertDialog,
    name: adw::EntryRow,
    protocol: adw::ComboRow,
    host: adw::EntryRow,
    port: adw::EntryRow,
    region: adw::EntryRow,
    user: adw::EntryRow,
    password: adw::PasswordEntryRow,
    path: adw::EntryRow,
    auto_mount: adw::SwitchRow,
    drive: gtk::Box,
    own_client: adw::SwitchRow,
    client_id: adw::EntryRow,
    client_secret: adw::PasswordEntryRow,
    guide: gtk::Box,
    record: RefCell<Map<String, Value>>,
}

/// Where a server GVfs mounts is, as it is saved.
struct Address {
    scheme: &'static str,
    host: String,
    port: String,
    user: String,
    path: String,
}

fn entry_row(title: &str, text: &str) -> adw::EntryRow {
    adw::EntryRow::builder().title(title).text(text).build()
}

/// Sets up a new server, or changes one.
pub fn edit(win: &Rc<Window>, server: Option<&Server>) {
    let record = server.map(|s| s.record.clone()).unwrap_or_default();
    let text = |key: &str| record.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    let port = record.get("port").and_then(Value::as_u64).map(|p| p.to_string()).unwrap_or_default();
    let labels: Vec<&str> = PROTOCOLS.iter().map(|p| p.label).collect();
    let scheme = text("scheme");
    let selected = PROTOCOLS.iter().position(|p| p.scheme == scheme).unwrap_or(0);
    let [name, host, region, user, path] = ["name", "host", "region", "user", "path"].map(text);
    let auto_mount = record.get("auto_mount").and_then(Value::as_bool).unwrap_or(false);

    let guide = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    guide.append(
        &gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .selectable(true)
            .label(
                "1. 於 Google Cloud Console 建立專案。\n\
                 2. 於「API 和服務」→「程式庫」啟用 Google Drive API。\n\
                 3. 設定「OAuth 同意畫面」，並將所使用的帳戶加入測試使用者。\n\
                 4. 於「憑證」建立「OAuth 用戶端 ID」，應用程式類型選擇「電腦版應用程式」。\n\
                 5. 將用戶端 ID 與密鑰填入上方欄位。",
            )
            .build(),
    );
    guide.append(
        &gtk::LinkButton::builder()
            .uri(GUIDE_URL)
            .label("開啟 Google Cloud Console")
            .halign(gtk::Align::Start)
            .build(),
    );
    let own_client = adw::SwitchRow::builder().title("使用自己的用戶端 ID").build();
    let client_id = entry_row("用戶端 ID", "");
    let client_secret = adw::PasswordEntryRow::builder().title("用戶端密鑰").build();
    let drive = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(EDGE).build();
    drive.append(&alert::form(&[own_client.upcast_ref(), client_id.upcast_ref(), client_secret.upcast_ref()]));
    drive.append(&guide);

    let form = Rc::new(Form {
        alert: adw::AlertDialog::new(Some(if server.is_some() { "伺服器" } else { "新增伺服器" }), None),
        name: entry_row("名稱", &name),
        protocol: adw::ComboRow::builder()
            .title("通訊協定")
            .model(&gtk::StringList::new(&labels))
            .selected(selected as u32)
            .build(),
        host: entry_row("主機", &host),
        port: adw::EntryRow::builder()
            .title("連接埠")
            .text(&port)
            .input_purpose(gtk::InputPurpose::Digits)
            .build(),
        region: entry_row("區域", &region),
        user: entry_row("使用者", &user),
        password: adw::PasswordEntryRow::builder().title("密碼").build(),
        path: entry_row("路徑", &path),
        auto_mount: adw::SwitchRow::builder()
            .title("登入時掛載")
            .active(auto_mount)
            .build(),
        drive,
        own_client,
        client_id,
        client_secret,
        guide,
        record: RefCell::new(record),
    });
    let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(EDGE).build();
    let rows = form.rows();
    content.append(&alert::form(&rows.iter().collect::<Vec<_>>()));
    content.append(&form.drive);
    form.alert.set_extra_child(Some(&content));
    form.alert.add_responses(&[("cancel", "取消"), ("save", "儲存")]);
    form.alert.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    form.alert.set_default_response(Some("save"));
    form.alert.set_close_response("cancel");
    form.show_rows();
    form.protocol.connect_selected_notify(on!(&form, |f, _r| f.show_rows()));
    form.own_client.connect_active_notify(on!(&form, |f, _r| f.show_rows()));
    let typed: [&adw::EntryRow; 6] = [
        &form.host,
        &form.user,
        form.password.upcast_ref(),
        &form.path,
        &form.client_id,
        form.client_secret.upcast_ref(),
    ];
    for row in typed {
        row.connect_changed(on!(&form, |f, _r| f.show_rows()));
    }

    if let Some(server) = server {
        let (id, weak) = (server.id.clone(), Rc::downgrade(&form));
        glib::spawn_future_local(async move {
            match secrets::lookup(&keyring_item(&id)).await {
                Ok(Some(password)) => {
                    if let Some(form) = weak.upgrade() {
                        form.password.set_text(&password);
                    }
                }
                Ok(None) => {}
                Err(err) => eprintln!("nekoland-files: no password for {id} from the keyring: {err}"),
            }
        });
    }
    // The answer holds the form until it comes.
    let (answered, weak) = (form.clone(), Rc::downgrade(win));
    form.alert.clone().choose(&win.window, gio::Cancellable::NONE, move |response| {
        if let (true, Some(win)) = (response == "save", weak.upgrade()) {
            answered.save(&win);
        }
    });
}

impl Form {
    fn rows(&self) -> Vec<gtk::Widget> {
        vec![
            self.name.clone().upcast(),
            self.protocol.clone().upcast(),
            self.host.clone().upcast(),
            self.port.clone().upcast(),
            self.region.clone().upcast(),
            self.user.clone().upcast(),
            self.password.clone().upcast(),
            self.path.clone().upcast(),
            self.auto_mount.clone().upcast(),
        ]
    }

    fn chosen(&self) -> &'static browse::Protocol {
        &PROTOCOLS[(self.protocol.selected() as usize).min(PROTOCOLS.len() - 1)]
    }

    /// The rows the protocol uses, each named as it is there, and 儲存 once they are enough.
    fn show_rows(&self) {
        let p = self.chosen();
        let (host, user, password, path) = match p.kind {
            Kind::S3 => ("端點（選填）", "存取金鑰 ID", "私密存取金鑰", "儲存貯體"),
            _ if p.scheme == "nfs" => ("主機", "使用者", "密碼", "匯出路徑"),
            // GVfs mounts the shares of an SMB host, not the host itself.
            _ if p.scheme == "smb" => ("主機", "使用者", "密碼", "共用資料夾"),
            _ => ("主機", "使用者", "密碼", "路徑"),
        };
        self.host.set_title(host);
        self.user.set_title(user);
        self.password.set_title(password);
        self.path.set_title(path);
        let shown: &[&gtk::Widget] = match p.kind {
            Kind::Oauth => &[],
            Kind::S3 => &[
                self.host.upcast_ref(),
                self.region.upcast_ref(),
                self.user.upcast_ref(),
                self.password.upcast_ref(),
                self.path.upcast_ref(),
            ],
            Kind::Gvfs if p.scheme == "nfs" => &[self.host.upcast_ref(), self.path.upcast_ref()],
            Kind::Gvfs => &[
                self.host.upcast_ref(),
                self.port.upcast_ref(),
                self.user.upcast_ref(),
                self.password.upcast_ref(),
                self.path.upcast_ref(),
            ],
        };
        let remote: [&gtk::Widget; 6] = [
            self.host.upcast_ref(),
            self.port.upcast_ref(),
            self.region.upcast_ref(),
            self.user.upcast_ref(),
            self.password.upcast_ref(),
            self.path.upcast_ref(),
        ];
        for row in remote {
            row.set_visible(shown.contains(&row));
        }
        let signed_in = self.record.borrow().get("remote").is_some();
        self.drive.set_visible(p.scheme == "gdrive" && !signed_in);
        let own = self.own_client.is_active();
        self.client_id.set_visible(own);
        self.client_secret.set_visible(own);
        self.guide.set_visible(own);
        self.alert.set_response_enabled("save", self.ready());
    }

    /// Whether the fields say enough to save the server.
    fn ready(&self) -> bool {
        let filled = |row: &adw::EntryRow| !row.text().trim().is_empty();
        match self.chosen().kind {
            Kind::Oauth => {
                !self.drive.is_visible()
                    || !self.own_client.is_active()
                    || (filled(&self.client_id) && filled(self.client_secret.upcast_ref()))
            }
            Kind::S3 => filled(&self.user) && !self.password.text().is_empty(),
            Kind::Gvfs => self.address().is_some(),
        }
    }

    /// Where the fields say a GVfs server is, a pasted address filling in the rest; none
    /// while that is not enough to mount it.
    fn address(&self) -> Option<Address> {
        let mut host = self.host.text().trim().to_owned();
        let mut scheme = self.chosen().scheme;
        let mut port = self.port.text().trim().to_owned();
        let (mut user, mut path) = (self.user.text().trim().to_owned(), self.path.text().trim().to_owned());
        if host.contains("://") {
            let uri = glib::Uri::parse(&host, glib::UriFlags::NONE).ok()?;
            if let Some(known) = protocol(&uri.scheme()).filter(|q| q.kind == Kind::Gvfs) {
                scheme = known.scheme;
            }
            host = uri.host().map(|h| h.to_string()).unwrap_or_default();
            if port.is_empty() && uri.port() > 0 {
                port = uri.port().to_string();
            }
            if user.is_empty() {
                user = uri.user().map(|u| unescape(&u)).unwrap_or_default();
            }
            if path.is_empty() {
                path = unescape(&uri.path());
            }
        }
        let share = scheme != "smb" || !path.trim_matches('/').is_empty();
        (!host.is_empty() && share).then_some(Address { scheme, host, port, user, path })
    }

    /// The record's id, given one when it is new.
    fn id(&self) -> String {
        let mut record = self.record.borrow_mut();
        let id = record
            .entry("id")
            .or_insert_with(|| glib::uuid_string_random().as_str().into());
        id.as_str().unwrap_or_default().to_owned()
    }

    fn commit(&self, win: &Rc<Window>, fields: Vec<(&str, Value)>) {
        self.id();
        let mut record = self.record.borrow().clone();
        record.insert("auto_mount".into(), self.auto_mount.is_active().into());
        for (key, value) in fields {
            record.insert(key.into(), value);
        }
        if let Err(why) = put(record) {
            win.alert("無法儲存伺服器", Some(&why));
        }
    }

    fn keep_password(&self, win: &Rc<Window>, password: String) {
        let (id, weak) = (self.id(), Rc::downgrade(win));
        glib::spawn_future_local(async move {
            if let Err(err) = secrets::store(&keyring_item(&id), KEYRING_LABEL, &password).await {
                if !err.matches(gio::IOErrorEnum::FailedHandled) {
                    if let Some(win) = weak.upgrade() {
                        win.alert("無法儲存密碼", Some(err.message()));
                    }
                }
            }
        });
    }

    fn save(self: &Rc<Self>, win: &Rc<Window>) {
        let p = self.chosen();
        match p.kind {
            Kind::Oauth => self.save_oauth(win, p),
            Kind::S3 => self.save_s3(win, p),
            Kind::Gvfs => self.save_gvfs(win),
        }
    }

    fn save_oauth(self: &Rc<Self>, win: &Rc<Window>, p: &'static browse::Protocol) {
        let name = Some(self.name.text().trim().to_owned()).filter(|n| !n.is_empty()).unwrap_or(p.label.into());
        if self.record.borrow().get("remote").is_some() {
            return self.commit(win, vec![("name", name.into()), ("scheme", p.scheme.into())]);
        }
        let id = self.id();
        let remote = format!("hamasen-{}", &id[..id.len().min(8)]);
        let mut argv = vec![rclone().into_os_string(), "config".into(), "create".into(), remote.clone().into(), p.rclone_type.into()];
        // Google's shared rclone client ID is being retired, so a personal one is offered.
        if p.scheme == "gdrive" && self.own_client.is_active() {
            argv.extend([
                "client_id".into(),
                self.client_id.text().trim().into(),
                "client_secret".into(),
                self.client_secret.text().trim().into(),
            ]);
        }
        let argv: Vec<&OsStr> = argv.iter().map(|a| a.as_os_str()).collect();
        let failed = format!("無法授權 {}", p.label);
        let process = match spawn(&argv, &[]) {
            Ok(process) => process,
            Err(err) => return win.alert(&failed, Some(err.message())),
        };
        // It opens the browser to sign in and waits for it, until it is stopped.
        let waiting = adw::AlertDialog::new(Some(&format!("{} 授權中", p.label)), Some("請在瀏覽器中登入。"));
        waiting.add_response("cancel", "取消");
        let stopped = Rc::new(Cell::new(false));
        let (stopping, stop) = (stopped.clone(), process.clone());
        waiting.connect_response(None, move |_, _| {
            stopping.set(true);
            stop.force_exit();
        });
        waiting.present(Some(&win.window));
        let (form, win) = (self.clone(), Rc::downgrade(win));
        glib::spawn_future_local(async move {
            let signed_in = finish(&process).await;
            if stopped.get() {
                return;
            }
            waiting.force_close();
            let Some(win) = win.upgrade() else { return };
            match signed_in {
                Ok(()) => form.commit(&win, vec![("name", name.into()), ("scheme", p.scheme.into()), ("remote", remote.into())]),
                Err(why) => win.alert(&failed, Some(&why)),
            }
        });
    }

    fn save_s3(&self, win: &Rc<Window>, p: &'static browse::Protocol) {
        let bucket = self.path.text().trim().to_owned();
        let name = [self.name.text().trim().to_owned(), bucket.clone(), p.label.to_owned()]
            .into_iter()
            .find(|n| !n.is_empty())
            .unwrap_or_default();
        self.keep_password(win, self.password.text().to_string());
        self.commit(
            win,
            vec![
                ("name", name.into()),
                ("scheme", p.scheme.into()),
                ("host", self.host.text().trim().into()),
                ("region", self.region.text().trim().into()),
                ("user", self.user.text().trim().into()),
                ("path", bucket.into()),
            ],
        );
    }

    fn save_gvfs(&self, win: &Rc<Window>) {
        // 儲存 is only offered once there is an address.
        let Some(address) = self.address() else { return };
        self.keep_password(win, self.password.text().to_string());
        let name = Some(self.name.text().trim().to_owned()).filter(|n| !n.is_empty()).unwrap_or(address.host.clone());
        let port: Value = address.port.parse::<u64>().map_or(Value::Null, Value::from);
        self.commit(
            win,
            vec![
                ("name", name.into()),
                ("scheme", address.scheme.into()),
                ("host", address.host.into()),
                ("port", port),
                ("user", address.user.into()),
                ("path", address.path.into()),
            ],
        );
    }
}

fn unescape(text: &str) -> String {
    glib::Uri::unescape_string(text, None::<&str>)
        .map(|t| t.to_string())
        .unwrap_or_else(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_hosts_are_the_concrete_aliases_once() {
        let config = "Host nas  nas-backup\n  HostName 10.0.0.2\nhost=*.lan\nHost !bad web\nHost nas\nMatch host x\n";
        assert_eq!(ssh_config_hosts(config), ["nas", "nas-backup", "web"]);
    }

    #[test]
    fn rclone_says_how_many_files_wait_to_upload() {
        let reply = "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n\
                     {\"diskCache\": {\"uploadsQueued\": 2, \"uploadsInProgress\": 1}}";
        assert_eq!(uploads_in(reply), Some(3));
        assert_eq!(uploads_in("HTTP/1.0 404 Not Found\r\n\r\n{}"), None);
    }
}
