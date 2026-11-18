//! Settings' 備份 page, after the Mac's Time Machine: the home folder copied to a folder
//! elsewhere, usually an external disk, once a day by a systemd timer or when asked. Each
//! backup is a folder named by its time; files unchanged since the one before are hard links
//! to it, so every backup is whole and only what changed takes room.

use std::{
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{alert, group};

/// `nekoland-settings --backup` backs up and ends, as the timer runs it.
pub const BACKUP_FLAG: &str = "--backup";
/// The newest backup, a link beside them.
const LATEST: &str = "latest";
const UNIT: &str = "nekoland-backup";
/// What is not worth copying: caches, the trash, and files being downloaded.
const LEFT_OUT: [&str; 3] = ["/.cache/", "/.local/share/Trash/", "*.part"];

/// Where backups go, the one place it is kept; the timer's service reads it as it runs.
fn destination_file() -> PathBuf {
    glib::user_config_dir().join("nekoland/backup-to")
}

fn destination() -> Option<PathBuf> {
    let saved = std::fs::read_to_string(destination_file()).ok()?;
    Some(PathBuf::from(saved.trim_end_matches('\n'))).filter(|path| !path.as_os_str().is_empty())
}

/// A backup's folder name, from the time it was made.
fn folder_name(time: &glib::DateTime) -> String {
    time.format("%Y-%m-%d-%H%M%S").map(String::from).unwrap_or_default()
}

/// "2026-11-17-210000" as it is shown: "2026-11-17 21:00".
fn shown_time(name: &str) -> Option<String> {
    let (date, time) = name.rsplit_once('-')?;
    (date.len() == 10 && time.len() == 6 && time.bytes().all(|b| b.is_ascii_digit()))
        .then(|| format!("{date} {}:{}", &time[..2], &time[2..4]))
}

/// Copies `home` into a new backup in `to`.
// shortcut: backups are never thinned out as Time Machine's are; delete old ones by hand,
// or thin them here once disks fill.
pub fn back_up(home: &Path, to: &Path) -> Result<(), String> {
    // A disk that is not plugged in leaves its mount point missing: nothing is made in its place.
    if !to.is_dir() {
        return Err(format!("{} is not there", to.display()));
    }
    if to.starts_with(home) {
        return Err(format!("{} is inside the home folder it would copy", to.display()));
    }
    let name = folder_name(&glib::DateTime::now_local().map_err(|err| err.to_string())?);
    let partial = to.join(format!("{name}.partial"));
    let latest = to.join(LATEST);
    let mut rsync = Command::new("rsync");
    rsync.args(["--archive", "--delete"]);
    rsync.args(LEFT_OUT.map(|pattern| format!("--exclude={pattern}")));
    if latest.is_dir() {
        rsync.arg(format!("--link-dest={}", latest.display()));
    }
    let status = rsync.arg(format!("{}/", home.display())).arg(&partial).status();
    match status {
        // 24: files vanished while they were copied, as a program's own files do.
        Ok(status) if status.success() || status.code() == Some(24) => {}
        Ok(status) => return Err(format!("rsync: {status}")),
        Err(err) => return Err(format!("rsync: {err}")),
    }
    std::fs::rename(&partial, to.join(&name)).map_err(|err| err.to_string())?;
    match std::fs::remove_file(&latest) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.to_string()),
        _ => {}
    }
    std::os::unix::fs::symlink(&name, &latest).map_err(|err| err.to_string())
}

/// Runs a backup to where Settings was told, for the timer.
pub fn back_up_now() -> glib::ExitCode {
    let Some(to) = destination() else {
        eprintln!("backup: no destination in {}", destination_file().display());
        return glib::ExitCode::FAILURE;
    };
    match back_up(&glib::home_dir(), &to) {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("backup: {err}");
            glib::ExitCode::FAILURE
        }
    }
}

/// The service and the daily timer that start it, for systemd to run as the person.
fn write_units() -> std::io::Result<()> {
    let units = glib::user_config_dir().join("systemd/user");
    std::fs::create_dir_all(&units)?;
    let program = std::env::current_exe()?;
    // Quoted, and % doubled, as systemd reads a command line.
    let program = program.display().to_string().replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%");
    std::fs::write(
        units.join(format!("{UNIT}.service")),
        format!(
            "[Unit]\nDescription=Nekoland backup\n\n[Service]\nType=oneshot\nNice=19\nIOSchedulingClass=idle\nExecStart=\"{program}\" {BACKUP_FLAG}\n"
        ),
    )?;
    std::fs::write(
        units.join(format!("{UNIT}.timer")),
        "[Unit]\nDescription=Nekoland backup, daily\n\n[Timer]\nOnCalendar=daily\nPersistent=true\nRandomizedDelaySec=15min\n\n[Install]\nWantedBy=timers.target\n",
    )
}

fn systemctl(args: &[&str]) -> bool {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .inspect_err(|err| eprintln!("systemctl: {err}"))
        .is_ok_and(|status| status.success())
}

pub fn backup_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let rows = group("");
    let place = adw::ActionRow::builder().title("備份位置").use_markup(false).build();
    let choose = gtk::Button::builder().label("選擇⋯").valign(gtk::Align::Center).build();
    place.add_suffix(&choose);
    let automatic = adw::SwitchRow::builder()
        .title("自動備份")
        .active(systemctl(&["--quiet", "is-enabled", &format!("{UNIT}.timer")]))
        .build();
    let last = adw::ActionRow::builder().title("上次備份").build();
    let open = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("打開備份")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let now = gtk::Button::builder().label("立即備份").valign(gtk::Align::Center).build();
    last.add_suffix(&open);
    last.add_suffix(&now);
    rows.add(&place);
    rows.add(&automatic);
    rows.add(&last);
    page.add(&rows);

    let show: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        place,
        #[weak]
        automatic,
        #[weak]
        last,
        #[weak]
        open,
        #[weak]
        now,
        move || {
            let to = destination();
            place.set_subtitle(&to.as_ref().map_or("未設定".into(), |to| to.display().to_string()));
            let newest = to.as_ref().and_then(|to| std::fs::read_link(to.join(LATEST)).ok());
            let newest = newest.as_ref().and_then(|name| shown_time(&name.to_string_lossy()));
            last.set_subtitle(newest.as_deref().unwrap_or("沒有備份"));
            automatic.set_sensitive(to.is_some());
            now.set_sensitive(to.is_some());
            open.set_visible(to.as_ref().is_some_and(|to| to.is_dir()));
        }
    ));
    show();

    choose.connect_clicked(glib::clone!(
        #[strong]
        show,
        move |button| {
            let window = button.root().and_downcast::<gtk::Window>();
            let dialog = gtk::FileDialog::builder().title("選擇備份位置").build();
            let (button, show) = (button.clone(), show.clone());
            dialog.select_folder(window.as_ref(), gio::Cancellable::NONE, move |chosen| {
                let path = match chosen.map(|folder| folder.path()) {
                    Ok(Some(path)) => path,
                    Ok(None) => return alert::notice(&button, "無法使用這個位置", Some("請選擇本機或已掛載的磁碟上的資料夾。")),
                    Err(err) if err.matches(gtk::DialogError::Dismissed) => return,
                    Err(err) => return eprintln!("backup folder: {err}"),
                };
                if path.starts_with(glib::home_dir()) {
                    return alert::notice(&button, "無法備份到家目錄中", Some("請選擇家目錄以外的位置，例如外接磁碟。"));
                }
                let saved = std::fs::create_dir_all(destination_file().parent().unwrap_or(Path::new("/")))
                    .and_then(|()| std::fs::write(destination_file(), format!("{}\n", path.display())));
                if let Err(err) = saved {
                    eprintln!("{}: {err}", destination_file().display());
                }
                show();
            });
        }
    ));

    automatic.connect_active_notify(|row| {
        let timer = format!("{UNIT}.timer");
        let worked = if row.is_active() {
            write_units().inspect_err(|err| eprintln!("backup units: {err}")).is_ok()
                && systemctl(&["daemon-reload"])
                && systemctl(&["enable", "--now", &timer])
        } else {
            systemctl(&["disable", "--now", &timer])
        };
        if !worked {
            alert::notice(row, "無法變更自動備份", None);
        }
    });

    open.connect_clicked(|button| {
        let Some(to) = destination() else { return };
        let uri = gio::File::for_path(to).uri();
        if let Err(err) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
            alert::notice(button, "無法打開備份", Some(&err.to_string()));
        }
    });

    now.connect_clicked(glib::clone!(
        #[weak]
        last,
        move |button| {
            let program = match std::env::current_exe() {
                Ok(program) => program,
                Err(err) => return eprintln!("backup: {err}"),
            };
            let running = gio::Subprocess::newv(
                &[program.as_os_str(), BACKUP_FLAG.as_ref()],
                gio::SubprocessFlags::STDERR_PIPE,
            );
            let running = match running {
                Ok(running) => running,
                Err(err) => return alert::notice(button, "無法備份", Some(&err.to_string())),
            };
            button.set_sensitive(false);
            last.set_subtitle("正在備份");
            let (button, show, finished) = (button.clone(), show.clone(), running.clone());
            running.communicate_utf8_async(None, gio::Cancellable::NONE, move |result| {
                let failed = match &result {
                    Ok(_) if finished.is_successful() => None,
                    Ok((_, said)) => Some(said.as_deref().unwrap_or_default().trim().to_string()),
                    Err(err) => Some(err.to_string()),
                };
                show();
                if let Some(said) = failed {
                    alert::notice(&button, "備份未完成", Some(&said));
                }
            });
        }
    ));
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups_are_named_by_time() {
        let time = glib::DateTime::from_local(2026, 11, 17, 21, 5, 9.0).unwrap();
        assert_eq!(folder_name(&time), "2026-11-17-210509");
        assert_eq!(shown_time("2026-11-17-210509").as_deref(), Some("2026-11-17 21:05"));
        assert_eq!(shown_time("2026-11-17-210509.partial"), None);
    }

    #[test]
    fn a_backup_links_what_did_not_change() {
        let scratch = std::env::temp_dir().join(format!("nekoland-backup-test-{}", std::process::id()));
        let (home, to) = (scratch.join("home"), scratch.join("disk"));
        std::fs::create_dir_all(home.join(".cache")).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(home.join("kept.txt"), "kept").unwrap();
        std::fs::write(home.join(".cache/skipped"), "skipped").unwrap();
        std::fs::create_dir_all(home.join("inside")).unwrap();
        assert!(back_up(&home, &home.join("inside")).is_err());
        assert!(back_up(&home, &scratch.join("unplugged")).is_err());
        back_up(&home, &to).unwrap();
        let first = std::fs::read_link(to.join(LATEST)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        back_up(&home, &to).unwrap();
        let second = std::fs::read_link(to.join(LATEST)).unwrap();
        assert_ne!(first, second);
        use std::os::unix::fs::MetadataExt;
        let inode = |name: &PathBuf| std::fs::metadata(to.join(name).join("kept.txt")).unwrap().ino();
        assert_eq!(inode(&first), inode(&second));
        assert!(!to.join(&second).join(".cache/skipped").exists());
        std::fs::remove_dir_all(&scratch).unwrap();
    }
}
