//! Recording the screen, or an area of it, through wf-recorder, which keeps on after 截圖
//! has gone: its process is written down, for the same key to stop it.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

use gtk::glib;

use super::output;

/// The process recording and the file it writes, one to a line.
fn pid_file() -> PathBuf {
    glib::user_runtime_dir().join("nekoland-record")
}

/// The recording going on: its process and its file.
fn running() -> Option<(i32, PathBuf)> {
    let text = std::fs::read_to_string(pid_file()).ok()?;
    let (pid, path) = text.split_once('\n')?;
    let pid: i32 = pid.trim().parse().ok()?;
    // A recording that ended some other way leaves its line behind.
    std::path::Path::new(&format!("/proc/{pid}")).exists().then(|| (pid, PathBuf::from(path.trim())))
}

pub fn is_recording() -> bool {
    running().is_some()
}

/// Starts recording `area`, in points, or every display.
pub fn start(area: Option<(f64, f64, f64, f64)>) {
    let folder = glib::user_special_dir(glib::UserDirectory::Videos).unwrap_or_else(glib::home_dir);
    let stamp = glib::DateTime::now_local()
        .ok()
        .and_then(|now| now.format("%Y-%m-%d %H.%M.%S").ok())
        .map(|stamp| stamp.to_string())
        .unwrap_or_default();
    let path = folder.join(format!("螢幕錄影 {stamp}.mp4"));
    if let Err(err) = std::fs::create_dir_all(&folder) {
        return output::notify("無法錄影", &err.to_string(), None, 5000, None);
    }
    let mut command = Command::new("wf-recorder");
    command.arg("-f").arg(&path);
    match area {
        Some((x, y, w, h)) => {
            command.arg("-g").arg(format!("{},{} {}x{}", x.round(), y.round(), w.round(), h.round()));
        }
        // wf-recorder records one display, and with more than one asks which on its input;
        // the first the compositor lists is the main one.
        None => {
            let displays = super::ipc::msg("displays").unwrap_or_default();
            if let Some(name) = displays.lines().find(|line| line.ends_with("\ton")).and_then(|line| line.split('\t').next()) {
                command.arg("-o").arg(name);
            }
        }
    }
    let child = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match child {
        Ok(child) => {
            let kept = std::fs::write(pid_file(), format!("{}\n{}\n", child.id(), path.display()));
            if let Err(err) = kept {
                eprintln!("nekoland-shot: {err}");
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            output::notify("無法錄影", "需要 wf-recorder", None, 5000, None)
        }
        Err(err) => output::notify("無法錄影", &err.to_string(), None, 5000, None),
    }
}

/// Stops the recording, waits for wf-recorder to finish the file, and says where it is.
pub fn stop() {
    let Some((pid, path)) = running() else {
        // A recording that ended by itself leaves its line, and the button with it.
        let _ = std::fs::remove_file(pid_file());
        return;
    };
    // SAFETY: a signal to a process; nothing of this one's memory is involved.
    unsafe {
        libc::kill(pid, libc::SIGINT);
    }
    let _ = std::fs::remove_file(pid_file());
    for _ in 0..50 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    let uri = glib::filename_to_uri(&path, None).ok().map(|uri| uri.to_string());
    output::notify(
        "螢幕錄影已儲存",
        &name,
        None,
        8000,
        uri.map(|uri| {
            Box::new(move || {
                if let Err(err) = gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>) {
                    eprintln!("nekoland-shot: {err}");
                }
            }) as Box<dyn Fn()>
        }),
    );
}
