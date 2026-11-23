//! Mounting Hamasen's servers: SFTP, FTP, SMB, WebDAV and NFS through GVfs, cloud drives
//! and S3 through rclone on a folder of their own. Files and the file chooser both mount them.

use std::{
    cell::RefCell,
    collections::HashSet,
    ffi::OsStr,
    future::Future,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::{
    browse::{self, Kind, Server},
    secrets,
};

pub const KEYRING_SCHEMA: &str = "app.hamasen.Hamasen";
pub const KEYRING_LABEL: &str = "Hamasen";

/// A server's password in the keyring, under Hamasen's libsecret schema, found by its id.
pub fn keyring_item(id: &str) -> secrets::Attributes {
    secrets::Attributes::from([
        ("xdg:schema".to_owned(), KEYRING_SCHEMA.to_owned()),
        ("server".to_owned(), id.to_owned()),
    ])
}
thread_local! {
    /// The servers being mounted now, by id.
    static CONNECTING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

pub fn connecting(id: &str) -> bool {
    CONNECTING.with_borrow(|c| c.contains(id))
}
pub fn is_mounted(server: &Server) -> bool {
    if server.gvfs {
        return server.location.find_enclosing_mount(gio::Cancellable::NONE).is_ok();
    }
    server.location.path().is_some_and(|path| is_mount_point(&path))
}

fn is_mount_point(path: &Path) -> bool {
    match (std::fs::metadata(path), std::fs::metadata(path.join(".."))) {
        (Ok(here), Ok(up)) => here.dev() != up.dev() || here.ino() == up.ino(),
        _ => false,
    }
}

pub fn rclone() -> PathBuf {
    glib::find_program_in_path("rclone")
        .unwrap_or_else(|| "/home/linuxbrew/.linuxbrew/bin/rclone".into())
}

/// Where rclone answers about the folder it serves, as Hamasen names it.
pub fn rc_socket(id: &str) -> PathBuf {
    glib::user_runtime_dir().join(format!("hamasen-{}.sock", &id[..id.len().min(8)]))
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// What rclone mounts: a remote signed in to, or an S3 connection string.
fn rclone_remote(server: &Server) -> String {
    if server.protocol().is_some_and(|p| p.kind == Kind::Oauth) {
        return format!("{}:", server.field("remote"));
    }
    let host = server.field("host");
    let mut options = vec![format!("provider={}", if host.is_empty() { "AWS" } else { "Other" })];
    if !host.is_empty() {
        options.push(format!("endpoint={}", quoted(host)));
    }
    if !server.field("region").is_empty() {
        options.push(format!("region={}", quoted(server.field("region"))));
    }
    format!(":s3,{}:{}", options.join(","), server.field("path").trim_matches('/'))
}

pub fn spawn(argv: &[&OsStr], env: &[(&str, String)]) -> Result<gio::Subprocess, glib::Error> {
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::STDERR_PIPE);
    for (name, value) in env {
        launcher.setenv(name, value, true);
    }
    launcher.spawn(argv)
}

/// Waits for a command; what it said on stderr when it failed.
pub async fn finish(process: &gio::Subprocess) -> Result<(), String> {
    let (_, said) = process
        .communicate_utf8_future(None)
        .await
        .map_err(|e| e.message().to_owned())?;
    if process.is_successful() {
        Ok(())
    } else {
        Err(said.map(|s| s.trim().to_owned()).unwrap_or_default())
    }
}

pub async fn run(argv: &[&OsStr], env: &[(&str, String)]) -> Result<(), String> {
    finish(&spawn(argv, env).map_err(|e| e.message().to_owned())?).await
}

/// Mounts the server, asking for a password in GTK's own way when the kept one does not
/// do. Cancelled, dismissed or mounted already is no failure; a failure says why.
pub fn mount(server: &Server, parent: Option<gtk::Window>) -> impl Future<Output = Result<(), String>> {
    let fresh = CONNECTING.with_borrow_mut(|c| c.insert(server.id.clone()));
    let server = server.clone();
    async move {
        if !fresh {
            return Ok(());
        }
        let mounted = if server.gvfs {
            mount_gvfs(&server, parent.as_ref()).await
        } else {
            mount_rclone(&server).await
        };
        CONNECTING.with_borrow_mut(|c| c.remove(&server.id));
        mounted
    }
}

async fn mount_gvfs(server: &Server, parent: Option<&gtk::Window>) -> Result<(), String> {
    let kept = secrets::lookup(&keyring_item(&server.id)).await.unwrap_or_else(|err| {
        eprintln!("nekoland-files: no password for {} from the keyring: {err}", server.name);
        None
    });
    let operation = gtk::MountOperation::new(parent);
    if let Some(password) = kept {
        let saved = RefCell::new(Some(password));
        let user = server.field("user").to_owned();
        operation.connect_ask_password(move |op, _, default_user, _, flags| {
            // Once: a password the server turns down falls through to GTK's dialog.
            let Some(password) = saved.take() else { return };
            if flags.contains(gio::AskPasswordFlags::NEED_USERNAME) {
                op.set_username(Some(if user.is_empty() { default_user } else { &user }));
            }
            op.set_password(Some(&password));
            op.reply(gio::MountOperationResult::Handled);
            op.stop_signal_emission_by_name("ask-password");
        });
    }
    match server
        .location
        .mount_enclosing_volume_future(gio::MountMountFlags::NONE, Some(&operation))
        .await
    {
        Ok(()) => Ok(()),
        Err(err) if err.matches(gio::IOErrorEnum::Cancelled) || err.matches(gio::IOErrorEnum::FailedHandled) => Ok(()),
        Err(err) if err.matches(gio::IOErrorEnum::AlreadyMounted) && is_mounted(server) => Ok(()),
        Err(err) => {
            let hint = if err.matches(gio::IOErrorEnum::AlreadyMounted) && server.field("scheme") == "smb" {
                "請於「路徑」欄位指定共用資料夾名稱。\n"
            } else if err.matches(gio::IOErrorEnum::TimedOut) {
                "主機未回應。請確認主機、連接埠與網路連線。\n"
            } else {
                ""
            };
            Err(format!("{hint}{}", err.message()))
        }
    }
}

async fn mount_rclone(server: &Server) -> Result<(), String> {
    let path = server.location.path().expect("rclone mounts on a folder");
    std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // S3's keys go in the environment, not on the command line or in rclone.conf.
    let env = if server.protocol().is_some_and(|p| p.kind == Kind::S3) {
        let secret = secrets::lookup(&keyring_item(&server.id)).await.map_err(|e| e.message().to_owned())?;
        vec![
            ("RCLONE_S3_ACCESS_KEY_ID", server.field("user").to_owned()),
            ("RCLONE_S3_SECRET_ACCESS_KEY", secret.unwrap_or_default()),
        ]
    } else {
        Vec::new()
    };
    let rc = format!("--rc-addr=unix://{}", rc_socket(&server.id).display());
    let (rclone, remote) = (rclone(), rclone_remote(server));
    // --daemon returns once the folder is mounted.
    let argv: Vec<&OsStr> = [
        rclone.as_os_str(),
        "mount".as_ref(),
        "--daemon".as_ref(),
        "--vfs-cache-mode".as_ref(),
        "writes".as_ref(),
        "--dir-cache-time".as_ref(),
        "1h".as_ref(),
        "--attr-timeout".as_ref(),
        "1h".as_ref(),
        "--rc".as_ref(),
        rc.as_ref(),
        remote.as_ref(),
        path.as_os_str(),
    ]
    .to_vec();
    run(&argv, &env).await?;
    set_bookmark(server, true);
    Ok(())
}

/// A bookmark is what puts a plain folder in other file managers' sidebars.
pub fn set_bookmark(server: &Server, present: bool) {
    let path = browse::bookmarks_path();
    let line = format!("{} {}", server.location.uri(), server.name);
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|kept| *kept != line)
        .map(str::to_owned)
        .collect();
    if present {
        lines.push(line);
    }
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, text));
    if let Err(err) = written {
        eprintln!("nekoland-files: cannot save {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_mounts_by_connection_string() {
        let record = serde_json::json!({"id": "a", "name": "B", "scheme": "s3", "host": "minio:9000",
            "region": "", "user": "k", "path": "/bucket/"});
        let server = browse::hamasen_server(record.as_object().unwrap().clone()).unwrap();
        assert_eq!(rclone_remote(&server), ":s3,provider=Other,endpoint=\"minio:9000\":bucket");
        let drive = serde_json::json!({"id": "c", "name": "D", "scheme": "gdrive", "remote": "hamasen-c"});
        let drive = browse::hamasen_server(drive.as_object().unwrap().clone()).unwrap();
        assert_eq!(rclone_remote(&drive), "hamasen-c:");
    }
}
