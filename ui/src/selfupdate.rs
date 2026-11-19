//! Nekoland's own updates, on its one channel, Dev: each commit to main. install.sh records
//! the commit it installed and the checkout it came from; updating pulls main into that
//! checkout and runs its install.sh again, which takes CI's build of the commit where there is one.

use std::process::{Command, Stdio};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::alert;

/// The channel as shown.
pub const CHANNEL: &str = "Dev";
const REMOTE: &str = "https://github.com/KoukeNeko/Nekoland.git";
const BRANCH: &str = "main";
/// Written by install.sh.
const INSTALLED: &str = "/usr/local/share/nekoland/COMMIT";
const SOURCE: &str = "/usr/local/share/nekoland/SOURCE";

#[derive(Clone, Debug)]
pub struct Status {
    /// None when what is installed did not record it.
    pub installed: Option<String>,
    pub latest: String,
}

impl Status {
    pub fn is_current(&self) -> bool {
        self.installed.as_deref() == Some(self.latest.as_str())
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

/// The installed commit and main's latest; blocks on the network.
pub fn check() -> Result<Status, String> {
    let installed = std::fs::read_to_string(INSTALLED)
        .ok()
        .map(|text| text.trim().to_string());
    let listed = Command::new("git")
        .args(["ls-remote", REMOTE, &format!("refs/heads/{BRANCH}")])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("git: {err}"))?;
    let latest = String::from_utf8_lossy(&listed.stdout)
        .split_whitespace()
        .next()
        .filter(|_| listed.status.success())
        .map(str::to_string)
        .ok_or_else(|| "無法連線到 GitHub。".to_string())?;
    Ok(Status { installed, latest })
}

/// Installs main's latest commit; blocks for minutes, longer where CI has not built it.
fn update() -> Result<(), String> {
    let source = std::fs::read_to_string(SOURCE)
        .map(|text| text.trim().to_string())
        .map_err(|_| "找不到安裝 Nekoland 的原始碼。請在原始碼中執行 install.sh。".to_string())?;
    run(Command::new("git").args(["-C", &source, "pull", "--ff-only", "-q", REMOTE, BRANCH]))?;
    let askpass = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("nekoland-askpass")))
        .filter(|path| path.exists())
        .unwrap_or_else(|| "/usr/local/bin/nekoland-askpass".into());
    // install.sh and the scripts it runs call plain sudo: a sudo first on PATH asks with askpass,
    // and the terminal `script` gives them all lets sudo remember the password across them.
    // In the person's own runtime folder, made new and theirs alone: a sudo someone else put first
    // on PATH would be given the password.
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let wrapper = glib::user_runtime_dir().join(format!("nekoland-update-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&wrapper)
        .map_err(|err| err.to_string())?;
    let sudo = wrapper.join("sudo");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&sudo)
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, b"#!/bin/sh\nexec /usr/bin/sudo -A \"$@\"\n")
        })
        .map_err(|err| err.to_string())?;
    let path = format!(
        "{}:{}",
        wrapper.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let installed = run(Command::new("script")
        .args([
            "-qec",
            &format!("sh {}", shell_quote(&format!("{source}/install.sh"))),
            "/dev/null",
        ])
        .env("PATH", path)
        .env("SUDO_ASKPASS", askpass)
        .env("NEKOLAND_ASKPASS_REASON", "更新 Nekoland"));
    if let Err(err) = std::fs::remove_dir_all(&wrapper) {
        eprintln!("selfupdate: {}: {err}", wrapper.display());
    }
    installed
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Runs a command to its end; on a failure, the last line it said (under `script`, its output and
/// errors are one).
fn run(command: &mut Command) -> Result<(), String> {
    let out = command
        .stdin(Stdio::null())
        .output()
        .map_err(|err| err.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr) + String::from_utf8_lossy(&out.stdout);
    eprintln!("selfupdate: {}", said.trim());
    Err(said
        .lines()
        .map(|line| line.trim_matches(|c: char| c.is_whitespace() || c.is_control()))
        .rfind(|line| !line.is_empty())
        .unwrap_or("")
        .to_string())
}

fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    glib::spawn_future_local(async move {
        if let Ok(value) = gio::spawn_blocking(work).await {
            done(value);
        }
    });
}

/// Nekoland's row: its channel and commit, and 更新 when main has a newer one. Given no
/// status, it checks for itself.
pub fn row(status: Option<Status>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title("Nekoland")
        .subtitle(format!("{CHANNEL} · 正在檢查"))
        .build();
    let button = gtk::Button::builder()
        .label("更新")
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    row.add_suffix(&button);
    let show = glib::clone!(
        #[weak]
        row,
        #[weak]
        button,
        move |checked: Result<Status, String>| match checked {
            Ok(status) => {
                let installed = status.installed.as_deref().map_or("未記錄版本", short);
                row.set_subtitle(&if status.is_current() {
                    format!("{CHANNEL} · {installed} · 已是最新版本")
                } else {
                    format!("{CHANNEL} · {installed} → {}", short(&status.latest))
                });
                button.set_visible(!status.is_current());
            }
            Err(err) => {
                eprintln!("selfupdate: {err}");
                row.set_subtitle(&format!("{CHANNEL} · 無法檢查"));
            }
        }
    );
    match status {
        Some(status) => show(Ok(status)),
        None => background(check, show),
    }
    button.connect_clicked(glib::clone!(
        #[weak]
        row,
        move |button| {
            button.set_sensitive(false);
            row.set_subtitle(&format!("{CHANNEL} · 正在更新"));
            background(
                update,
                glib::clone!(
                    #[weak]
                    row,
                    #[weak]
                    button,
                    move |result: Result<(), String>| {
                        button.set_sensitive(true);
                        match result {
                            Ok(()) => {
                                button.set_visible(false);
                                row.set_subtitle(&format!("{CHANNEL} · 重新登入後生效"));
                            }
                            Err(err) => {
                                row.set_subtitle(&format!("{CHANNEL} · 無法更新"));
                                alert::notice(
                                    &row,
                                    "無法更新 Nekoland",
                                    Some(&err).filter(|err| !err.is_empty()).map(String::as_str),
                                );
                            }
                        }
                    }
                ),
            );
        }
    ));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_only_when_the_installed_commit_is_the_latest() {
        let status = |installed: Option<&str>| Status {
            installed: installed.map(str::to_string),
            latest: "abc".into(),
        };
        assert!(status(Some("abc")).is_current());
        assert!(!status(Some("def")).is_current());
        assert!(!status(None).is_current());
    }
}
