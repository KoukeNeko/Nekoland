//! Settings' Windows App page: Windows programs, two ways. Wine runs them beside Linux's own;
//! what Wine cannot run runs in Windows 11 Enterprise's 90-day evaluation, in a virtual machine
//! that dockur/windows downloads and installs by itself in a container of Podman's, run by the
//! person rather than root, its programs each in windows of their own through FreeRDP's
//! RemoteApp.

use std::{
    cell::{Cell, RefCell},
    ffi::OsStr,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    time::{Duration, Instant},
};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{alert, apt, as_root, group, output};

/// `nekoland-settings --windows-app [ENTRY]` opens the Windows program a launcher this page
/// made names, or the whole Windows desktop without one.
pub const OPEN_FLAG: &str = "--windows-app";
/// What Wine is installed with: Wine, and winetricks, which puts DXVK in its prefix; Ubuntu's
/// own DXVK package's `dxvk-setup` looks for Wine where Ubuntu's Wine is not.
const WINE_PACKAGES: [&str; 2] = ["wine", "winetricks"];
/// The Direct3D libraries DXVK stands in for, which Wine is told to load from the prefix.
const DXVK_DLLS: [&str; 5] = ["d3d8", "d3d9", "d3d10core", "d3d11", "dxgi"];
/// What Windows programs often want that Wine does not come with, as winetricks names them,
/// and as they are listed.
const COMPONENTS: [(&str, &str); 7] = [
    ("corefonts", "Microsoft 核心字型"),
    ("cjkfonts", "中日韓字型"),
    ("vcrun2022", "Visual C++ 2015–2022"),
    ("dotnet48", ".NET Framework 4.8"),
    ("d3dx9", "DirectX 9"),
    ("d3dcompiler_47", "Direct3D 著色器編譯器"),
    ("mf", "Media Foundation"),
];
/// The Windows a bottle says it is, as `winecfg -v` names them, and as they are listed.
const WINDOWS_VERSIONS: [(&str, &str); 5] = [
    ("win11", "Windows 11"),
    ("win10", "Windows 10"),
    ("win81", "Windows 8.1"),
    ("win7", "Windows 7"),
    ("winxp64", "Windows XP"),
];
/// Where the newest of Kron4ek's builds of Wine is listed, to have a Wine other than
/// Ubuntu's in a bottle.
const RUNNER_RELEASES: &str = "https://api.github.com/repos/Kron4ek/Wine-Builds/releases/latest";
/// The file in a bottle that names the Wine it runs with, when that is not Ubuntu's.
const RUNNER_FILE: &str = "nekoland-runner";
/// `nekoland-settings --wine FILE` opens a Windows program from Files in Wine's own bottle,
/// which is given Nekoland's look first.
pub const WINE_FLAG: &str = "--wine";
/// What Files asks for what Windows' programs add to a file's right-click menu, and has one
/// of them done with.
pub const MENU_FLAG: &str = "--windows-menu";
/// The Windows program that asks a bottle's programs for their right-click items, next to
/// this one.
const SHELL_MENU: &str = "nekoland-shellmenu.exe";
/// Said after it by the process that lists, to another that has Windows list anew.
const MENU_AGAIN: &str = "--again";
/// The files in a bottle that say which look it was last given, and that keep the colours it
/// had before a dark one.
const LOOK_FILE: &str = "nekoland-look";
const COLORS_FILE: &str = "nekoland-colors.reg";
const COLORS_KEY: &str = "HKEY_CURRENT_USER\\Control Panel\\Colors";
/// The dark colours of shell/gtk/nekoland.css and of libadwaita, as Windows writes them:
/// paper, sheet, fill, line, ink, faint ink, what is chosen, and ink on it.
const DARK_NEKOLAND: [&str; 8] = [
    "27 27 30",
    "34 34 38",
    "44 44 49",
    "50 50 55",
    "241 239 234",
    "147 145 151",
    "241 239 234",
    "23 23 26",
];
const DARK_ADWAITA: [&str; 8] = [
    "34 34 38",
    "29 29 32",
    "46 46 50",
    "54 54 58",
    "255 255 255",
    "153 153 156",
    "53 132 228",
    "255 255 255",
];
/// Wine's own programs for looking after a bottle, as they are listed.
const TOOLS: [(&str, &str); 4] = [
    ("Wine 設定", "winecfg"),
    ("登錄編輯程式", "regedit"),
    ("工作管理員", "taskmgr"),
    ("解除安裝程式", "uninstaller"),
];
/// The programs the virtual machine is run and shown with, and their packages.
const MACHINE_PROGRAMS: [(&str, &str); 3] = [
    ("podman", "podman"),
    ("xfreerdp3", "freerdp3-x11"),
    // Gives each of FreeRDP's windows the identity of its program.
    ("xdotool", "xdotool"),
];
const CONTAINER: &str = "nekoland-windows";
// shortcut: pinned to the release tried here; try a newer one before moving the pin.
const IMAGE: &str = "docker.io/dockurr/windows:6.06";
/// Windows 11 Enterprise's evaluation, as dockur/windows names it, and how long it lasts.
const EDITION: &str = "11e";
const EVALUATION_DAYS: i32 = 90;
/// On this computer alone: RDP's port, not 3389, which xrdp takes when 共享 turns 遠端桌面
/// on, and the port of dockur/windows' view of the screen, for watching the installation.
const RDP_PORT: u16 = 3390;
const VIEWER_PORT: u16 = 8006;
/// The room the installation needs: the download, the image made from it, and Windows.
const NEEDED_GB: u64 = 40;
const GB: u64 = 1 << 30;
/// How long a launcher waits for Windows to start and let it in.
const START_TIMEOUT: Duration = Duration::from_secs(300);
/// How the launchers' desktop entries are named, and the key keeping the program they open,
/// apart from Exec so that a Windows path needs no quoting for the shell.
const ENTRY_PREFIX: &str = "nekoland-windows-";
const PROGRAM_KEY: &str = "X-Nekoland-Windows-Program";
const ENTRY_GROUP: &str = "Desktop Entry";
/// Nekoland's agent in Windows, and the name of the folder shared with it, as it reads it
/// under `\\tsclient`.
const AGENT: &str = include_str!("windows/agent.ps1");
const SHARE: &str = "nekoland";
const AGENT_PATH: &str = r"\\tsclient\nekoland\agent.ps1";
/// The whole of this computer's files, as Windows sees them under `\\tsclient`, for a
/// program to open any of them.
const FILES: &str = "root";
/// What the windows of a program without a launcher are known by.
const UNNAMED_APP: &str = "nekoland-windows";
/// The name of a window of Windows', before how much of its top is a title bar of Windows'
/// and how large Windows has it; the compositor's `frame` reads it.
const WINDOW: &str = "nekoland-window-";
/// How long the connection is kept with no window of Windows' shown and nothing asked.
const IDLE: Duration = Duration::from_secs(30);
/// The files in the shared folder that the connection of the programs and the one of the
/// desktop each hold while they are there, and the one that asks whichever is to end.
const APPS_LOCK: &str = "lock";
const DESKTOP_LOCK: &str = "desktop.lock";
const RELEASE: &str = "release";
/// How long a connection is given to end once asked.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(15);
/// A connection that ends sooner than this was refused, Windows not having started yet.
const REFUSED_WITHIN: Duration = Duration::from_secs(10);
/// How long the agent is given to say what Windows shows, which takes it a few seconds.
const AGENT_START: Duration = Duration::from_secs(25);
/// `nekoland-settings --windows-app --agent` keeps the connection without opening anything.
const KEEP_FLAG: &str = "--agent";

pub(crate) fn windows_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.add(&wine_group());
    let machine = MachineView::new();
    page.add(&machine.group);
    page.add(&machine.apps);
    page.add(&machine.found);
    page.add(&machine.removal);
    machine.list_apps();
    machine.refresh();
    // The machine changes by itself as it installs, and when a launcher starts it. Looking at
    // it keeps the view, which its buttons hold only weakly, as long as the page.
    let shown = page.downgrade();
    glib::timeout_add_seconds_local(3, move || {
        if shown.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        if machine.state.is_mapped() {
            machine.refresh();
        }
        glib::ControlFlow::Continue
    });
    page
}

fn button(label: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .valign(gtk::Align::Center)
        .build()
}

/// apt-get installing `packages`, without the status lines Market reads for its progress:
/// they are the last two of its words.
fn apt_install(packages: &[&'static str]) -> Vec<&'static str> {
    let quiet = &apt::APT_GET[..apt::APT_GET.len() - 2];
    quiet
        .iter()
        .copied()
        .chain(["install"])
        .chain(packages.iter().copied())
        .collect()
}

// Wine.

/// The Wine installed, as `wine --version` says it, "wine-10.0 (Debian …)": "10.0".
fn wine_version() -> Option<String> {
    glib::find_program_in_path("wine")?;
    let said = output("wine", &["--version"])?;
    said.split_whitespace()
        .next()
        .map(|word| word.trim_start_matches("wine-").to_string())
}

/// Wine's own bottle: the prefix it uses when it is not told another.
fn default_prefix() -> PathBuf {
    std::env::var_os("WINEPREFIX").map_or_else(|| glib::home_dir().join(".wine"), PathBuf::from)
}

/// Where the bottles made here are kept, each a prefix of Wine's in a folder of its name:
/// Windows' folders, registry and programs of its own, apart from the others'.
fn bottles_dir() -> PathBuf {
    glib::user_data_dir().join("nekoland/wine")
}

/// The bottles by name: Wine's own first, then the ones made here.
fn bottles() -> Vec<(String, PathBuf)> {
    let mut made: Vec<(String, PathBuf)> = std::fs::read_dir(bottles_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .collect();
    made.sort();
    made.insert(0, ("預設".to_string(), default_prefix()));
    made
}

/// Whether a bottle can be made by this name: one folder's, not taken.
fn is_bottle_name(name: &str, taken: &[(String, PathBuf)]) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\0'])
        && !taken.iter().any(|(other, _)| other == name)
}

/// Whether a launcher, as Wine writes them, is of a program in the bottle at `prefix`.
fn is_launcher_of(launcher: &str, prefix: &Path) -> bool {
    launcher.contains(&format!("WINEPREFIX=\"{}\"", prefix.display()))
}

/// Removes a bottle made here: its programs are ended, or they would write it back, and
/// the launchers Wine made for them go with it.
fn remove_bottle(prefix: &Path) -> std::io::Result<()> {
    if !prefix.starts_with(bottles_dir()) {
        return Err(std::io::Error::other("not a bottle made here"));
    }
    if let Some(wineserver) = wineserver(prefix) {
        Command::new(wineserver)
            .arg("-k")
            .env("WINEPREFIX", prefix)
            .status()?;
    }
    let mut folders = vec![glib::user_data_dir().join("applications/wine")];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if std::fs::read_to_string(&path).is_ok_and(|launcher| is_launcher_of(&launcher, prefix)) {
                std::fs::remove_file(path)?;
            }
        }
    }
    std::fs::remove_dir_all(prefix)
}

/// Where the Wines downloaded here are kept, each in a folder of its name.
fn runners_dir() -> PathBuf {
    glib::user_data_dir().join("nekoland/wine-runners")
}

/// The `wine` of a Wine downloaded here.
fn runner_wine(name: &str) -> Option<PathBuf> {
    let wine = runners_dir().join(name).join("bin/wine");
    wine.exists().then_some(wine)
}

/// The Wines downloaded here, by name.
fn runners() -> Vec<String> {
    let mut runners: Vec<String> = std::fs::read_dir(runners_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| runner_wine(name).is_some())
        .collect();
    runners.sort();
    runners
}

/// Removes a Wine downloaded here; the bottles that ran with it run with Ubuntu's again.
fn remove_runner(name: &str) -> std::io::Result<()> {
    if !runners().iter().any(|runner| runner == name) {
        return Err(std::io::Error::other("not a Wine downloaded here"));
    }
    std::fs::remove_dir_all(runners_dir().join(name))
}

/// The Wine a bottle runs with, if not Ubuntu's: one downloaded here that is there still.
fn runner_of(prefix: &Path) -> Option<String> {
    let name = std::fs::read_to_string(prefix.join(RUNNER_FILE)).ok()?;
    let name = name.trim();
    runner_wine(name).map(|_| name.to_string())
}

/// Has a bottle made here run with a Wine downloaded here, or with Ubuntu's.
fn set_runner(prefix: &Path, runner: Option<&str>) -> std::io::Result<()> {
    let file = prefix.join(RUNNER_FILE);
    match runner {
        Some(runner) => std::fs::write(file, runner),
        None => match std::fs::remove_file(file) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        },
    }
}

/// The `wine` a bottle runs with.
fn wine_of(prefix: &Path) -> PathBuf {
    runner_of(prefix)
        .and_then(|runner| runner_wine(&runner))
        .unwrap_or_else(|| PathBuf::from("wine"))
}

/// What Wine's registry is to hold for a bottle to be dark or light. Wine has no dark theme
/// of its own, so a dark bottle goes without a theme, in the colours given; programs that
/// ask Windows which they are to be are told as well.
fn look_registry(dark: Option<[&str; 8]>) -> String {
    let light = u8::from(dark.is_none());
    let mut registry = format!(
        "REGEDIT4\n\n\
         [HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize]\n\
         \"AppsUseLightTheme\"=dword:0000000{light}\n\"SystemUsesLightTheme\"=dword:0000000{light}\n\n\
         [HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\ThemeManager]\n\
         \"ThemeActive\"=\"{light}\"\n"
    );
    if let Some([paper, sheet, fill, line, ink, faint, chosen, on_chosen]) = dark {
        registry += &format!("\n[{COLORS_KEY}]\n");
        for (name, color) in [
            ("ActiveBorder", paper),
            ("ActiveTitle", paper),
            ("AppWorkSpace", sheet),
            ("Background", paper),
            ("ButtonAlternateFace", fill),
            ("ButtonDkShadow", line),
            ("ButtonFace", paper),
            ("ButtonHilight", fill),
            ("ButtonLight", fill),
            ("ButtonShadow", line),
            ("ButtonText", ink),
            ("GradientActiveTitle", paper),
            ("GradientInactiveTitle", paper),
            ("GrayText", faint),
            ("Hilight", chosen),
            ("HilightText", on_chosen),
            ("HotTrackingColor", chosen),
            ("InactiveBorder", paper),
            ("InactiveTitle", paper),
            ("InactiveTitleText", faint),
            ("InfoText", ink),
            ("InfoWindow", sheet),
            ("Menu", paper),
            ("MenuBar", paper),
            ("MenuHilight", chosen),
            ("MenuText", ink),
            ("Scrollbar", fill),
            ("TitleText", ink),
            ("Window", sheet),
            ("WindowFrame", line),
            ("WindowText", ink),
        ] {
            registry += &format!("\"{name}\"=\"{color}\"\n");
        }
    }
    registry
}

/// Gives a bottle the look Nekoland has now, dark or light, in Nekoland's colours or
/// libadwaita's, when it is not the look the bottle was last given.
fn match_look(prefix: &Path) -> Result<(), String> {
    if !prefix.join("user.reg").exists() {
        return Ok(());
    }
    let interface = gio::Settings::new("org.gnome.desktop.interface");
    let wanted = match (
        interface.string("color-scheme") == "prefer-dark",
        interface.string("gtk-theme") == "Nekoland",
    ) {
        (false, _) => "light",
        (true, true) => "dark-nekoland",
        (true, false) => "dark-adwaita",
    };
    let had = std::fs::read_to_string(prefix.join(LOOK_FILE)).unwrap_or_else(|_| "light".into());
    if had.trim() == wanted {
        return Ok(());
    }
    let regedit = |args: &[&OsStr]| {
        said(
            Command::new(wine_of(prefix))
                .arg("regedit")
                .args(args)
                .env("WINEPREFIX", prefix),
        )
        .map(drop)
    };
    let failed = |err: std::io::Error| err.to_string();
    let colors = prefix.join(COLORS_FILE);
    let dark = match wanted {
        "light" => None,
        "dark-nekoland" => Some(DARK_NEKOLAND),
        _ => Some(DARK_ADWAITA),
    };
    if had.trim() == "light" {
        // The colours the bottle's theme gave it, to give back when it is light again.
        regedit(&[OsStr::new("/E"), colors.as_os_str(), OsStr::new(COLORS_KEY)])?;
    } else if dark.is_none() {
        regedit(&[OsStr::new("/S"), colors.as_os_str()])?;
    }
    let look = prefix.join("nekoland-look.reg");
    std::fs::write(&look, look_registry(dark)).map_err(failed)?;
    regedit(&[OsStr::new("/S"), look.as_os_str()])?;
    std::fs::remove_file(&look).map_err(failed)?;
    std::fs::write(prefix.join(LOOK_FILE), wanted).map_err(failed)
}

/// `nekoland-settings --wine FILE`: opens a Windows program from Files in Wine's own bottle,
/// in Nekoland's look.
pub fn run(file: Option<&str>) -> glib::ExitCode {
    let prefix = default_prefix();
    if let Err(why) = match_look(&prefix) {
        eprintln!("{WINE_FLAG}: {why}");
    }
    let Some(file) = file else {
        return glib::ExitCode::FAILURE;
    };
    match start_in(
        &prefix,
        &[OsStr::new("start"), OsStr::new("/unix"), OsStr::new(file)],
    ) {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(err) => {
            notify("無法開啟 Windows 程式", err.message());
            glib::ExitCode::FAILURE
        }
    }
}

/// Starts a program in a bottle with the bottle's Wine, left to run by itself.
fn start_in(prefix: &Path, args: &[&OsStr]) -> Result<(), glib::Error> {
    // The program is opened all the same in the look the bottle has.
    if let Err(why) = match_look(prefix) {
        eprintln!("wine: {why}");
    }
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);
    launcher.setenv("WINEPREFIX", prefix, true);
    let wine = wine_of(prefix);
    let args: Vec<&OsStr> = std::iter::once(wine.as_os_str())
        .chain(args.iter().copied())
        .collect();
    launcher.spawn(&args).map(drop)
}

/// Has a bottle's Wine fill it in, or bring it up to itself after another Wine's, without
/// the windows Wine would ask to install Mono and Gecko in, which the program started
/// next would wait behind.
fn update(prefix: &Path) -> Result<(), glib::Error> {
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);
    launcher.setenv("WINEPREFIX", prefix, true);
    launcher.setenv("WINEDLLOVERRIDES", "mscoree,mshtml=", true);
    let wine = wine_of(prefix);
    launcher
        .spawn(&[wine.as_os_str(), OsStr::new("wineboot"), OsStr::new("-u")])
        .map(drop)
}

/// What a program says, or why it could not be run to its end.
fn said(program: &mut Command) -> Result<String, String> {
    let name = program.get_program().to_string_lossy().into_owned();
    let out = program.output().map_err(|err| format!("{name}: {err}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{name}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Which of `WINDOWS_VERSIONS` a bottle says it is. Wine says it among its warnings.
fn windows_version(prefix: &Path) -> Option<usize> {
    let out = Command::new(wine_of(prefix))
        .args(["winecfg", "-v"])
        .env("WINEPREFIX", prefix)
        .output()
        .ok()?;
    let said = [out.stdout, out.stderr].concat();
    String::from_utf8_lossy(&said)
        .lines()
        .rev()
        .find_map(|line| WINDOWS_VERSIONS.iter().position(|(id, _)| *id == line.trim()))
}

/// Whether one of a release's files is Wine as it is, which runs 32-bit Windows programs
/// without 32-bit libraries of Linux's, which Ubuntu does not come with:
/// `wine-11.22-amd64-wow64.tar.xz`, not a staging build.
fn is_plain_build(file: &str) -> bool {
    file.strip_prefix("wine-")
        .and_then(|rest| rest.strip_suffix("-amd64-wow64.tar.xz"))
        .is_some_and(|version| {
            !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
        })
}

/// The build to download among the files of a release, as GitHub lists them.
fn runner_url(release: &str) -> Option<String> {
    let release: serde_json::Value = serde_json::from_str(release).ok()?;
    release["assets"]
        .as_array()?
        .iter()
        .filter_map(|asset| asset["browser_download_url"].as_str())
        .find(|url| url.rsplit('/').next().is_some_and(is_plain_build))
        .map(str::to_string)
}

/// Downloads the newest Wine there is among Kron4ek's builds, checked against the sum the
/// release gives for it, and unpacks it. Returns its name.
fn download_runner() -> Result<String, String> {
    let failed = |err: std::io::Error| err.to_string();
    let release = said(Command::new("curl").args(["-fsSL", RUNNER_RELEASES]))?;
    let url = runner_url(&release).ok_or("the release has no build of Wine")?;
    let (from, file) = url.rsplit_once('/').ok_or("the release has no build of Wine")?;
    let name = file.trim_end_matches(".tar.xz").to_string();
    let dir = runners_dir();
    if runner_wine(&name).is_some() {
        return Ok(name);
    }
    std::fs::create_dir_all(&dir).map_err(failed)?;
    let archive = dir.join(file);
    said(
        Command::new("curl")
            .args(["-fsSL", "--output"])
            .arg(&archive)
            .arg(&url),
    )?;
    let unpacked = (|| {
        let sums = said(Command::new("curl").args(["-fsSL", &format!("{from}/sha256sums.txt")]))?;
        let sum = said(Command::new("sha256sum").arg(&archive))?;
        let is_listed = sums.lines().any(|line| {
            line.split_whitespace()
                .eq([sum.split_whitespace().next().unwrap_or_default(), file])
        });
        if !is_listed {
            return Err(format!("{file}: not the file the release lists"));
        }
        said(Command::new("tar").arg("-xJf").arg(&archive).arg("-C").arg(&dir)).map(drop)
    })();
    std::fs::remove_file(&archive).map_err(failed)?;
    unpacked.map(|()| name)
}

/// Whether Wine loads DXVK's Direct3D from the bottle rather than its own, as winetricks has it.
fn dxvk_installed(prefix: &Path) -> bool {
    std::fs::read_to_string(prefix.join("user.reg"))
        .is_ok_and(|registry| registry.contains("\"*d3d11\"=\"native\""))
}

/// A bottle's wineserver: its own Wine's, or Ubuntu's, on PATH or beside Wine's libraries.
fn wineserver(prefix: &Path) -> Option<PathBuf> {
    if let Some(wine) = runner_of(prefix).and_then(|runner| runner_wine(&runner)) {
        return Some(wine.with_file_name("wineserver"));
    }
    glib::find_program_in_path("wineserver").or_else(|| {
        let dirs = std::fs::read_dir("/usr/lib").ok()?;
        dirs.flatten()
            .map(|dir| dir.path().join("wine/wineserver"))
            .find(|wineserver| wineserver.exists())
    })
}

/// Has winetricks do `verbs` in a bottle, without asking anything.
fn winetricks(prefix: &Path, verbs: &[&str]) -> std::io::Result<std::process::Output> {
    let mut winetricks = Command::new("winetricks");
    winetricks
        .arg("-q")
        .args(verbs)
        .env("WINEPREFIX", prefix)
        .env("WINE", wine_of(prefix));
    let wineserver = wineserver(prefix);
    // winetricks looks for it beside wine, where Ubuntu's is not.
    if let Some(wineserver) = &wineserver {
        winetricks.env("WINESERVER", wineserver);
    }
    let done = winetricks.output()?;
    // The registry reaches user.reg, where this page reads it, as wineserver ends.
    if let Some(wineserver) = wineserver {
        Command::new(wineserver)
            .arg("-w")
            .env("WINEPREFIX", prefix)
            .status()?;
    }
    Ok(done)
}

/// Turns DXVK on or off; `--force`, as winetricks skips DXVK once it is in the bottle, even
/// with Wine told to load its own.
fn set_dxvk(prefix: &Path, on: bool) -> std::io::Result<std::process::Output> {
    if on {
        winetricks(prefix, &["--force", "dxvk"])
    } else {
        let builtin = DXVK_DLLS.map(|dll| format!("{dll}=builtin"));
        winetricks(prefix, &builtin.each_ref().map(String::as_str))
    }
}

/// Whether a bottle's programs open their windows on Wayland itself rather than through XWayland.
fn wayland_on(prefix: &Path) -> bool {
    std::fs::read_to_string(prefix.join("user.reg"))
        .is_ok_and(|registry| registry.contains("\"Graphics\"=\"wayland\""))
}

/// Has the programs a bottle starts from now on open their windows on Wayland, or as Wine
/// would by itself, through XWayland.
fn set_wayland(prefix: &Path, on: bool) -> Result<(), String> {
    let mut reg = Command::new(wine_of(prefix));
    reg.arg("reg")
        .arg(if on { "add" } else { "delete" })
        .args([r"HKCU\Software\Wine\Drivers", "/v", "Graphics", "/f"])
        .env("WINEPREFIX", prefix);
    if on {
        reg.args(["/d", "wayland"]);
    }
    said(&mut reg).map(drop)
}

/// What winetricks has put in a bottle, as it notes them, one a line.
fn installed_components(prefix: &Path) -> Vec<String> {
    std::fs::read_to_string(prefix.join("winetricks.log"))
        .unwrap_or_default()
        .lines()
        .map(|verb| verb.trim().to_string())
        .collect()
}

/// Why winetricks did not do what it was asked, in its own words, or `None` when it did:
/// the command of its that failed, if it names one, or the last thing it said.
fn failure(
    done: Result<std::io::Result<std::process::Output>, Box<dyn std::any::Any + Send>>,
) -> Option<String> {
    match done {
        Ok(Ok(out)) if out.status.success() => None,
        Ok(Ok(out)) => {
            let said = String::from_utf8_lossy(&out.stderr);
            eprintln!("winetricks: {}", said.trim());
            let lines = || said.lines().map(str::trim).filter(|line| !line.is_empty());
            lines()
                .rfind(|line| line.starts_with("warning: Note: command"))
                .or_else(|| lines().last())
                .map(str::to_string)
                .or_else(|| Some(out.status.to_string()))
        }
        Ok(Err(err)) => Some(err.to_string()),
        Err(_) => Some("winetricks ended early".to_string()),
    }
}

fn wine_group() -> adw::PreferencesGroup {
    let wine = group("Wine");
    let state = adw::ActionRow::builder().title("版本").build();
    let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
    let install = button("安裝");
    state.add_suffix(&spinner);
    state.add_suffix(&install);
    let names = gtk::StringList::new(&[]);
    let bottle = adw::ComboRow::builder().title("環境").model(&names).build();
    let add = button("新增");
    let remove = button("刪除");
    bottle.add_suffix(&add);
    bottle.add_suffix(&remove);
    let runner_names = gtk::StringList::new(&[]);
    let runner = adw::ComboRow::builder()
        .title("Wine 版本")
        .model(&runner_names)
        .build();
    let download = button("下載最新版");
    runner.add_suffix(&download);
    let discard = button("移除");
    runner.add_suffix(&discard);
    let version_names: Vec<&str> = WINDOWS_VERSIONS.iter().map(|(_, name)| *name).collect();
    let windows = adw::ComboRow::builder()
        .title("Windows 版本")
        .model(&gtk::StringList::new(&version_names))
        .build();
    let run = adw::ActionRow::builder().title("執行程式").build();
    let choose = button("選擇⋯");
    run.add_suffix(&choose);
    run.set_activatable_widget(Some(&choose));
    let dxvk = adw::SwitchRow::builder()
        .title("DXVK")
        .subtitle("Direct3D 8–11 以 Vulkan 繪製")
        .build();
    let wayland = adw::SwitchRow::builder()
        .title("Wayland")
        .subtitle("視窗不經 XWayland")
        .build();
    let components = adw::ExpanderRow::builder().title("元件").build();
    let tools = adw::ExpanderRow::builder().title("工具").build();
    wine.add(&state);
    wine.add(&bottle);
    wine.add(&runner);
    wine.add(&windows);
    wine.add(&run);
    wine.add(&dxvk);
    wine.add(&wayland);
    wine.add(&components);
    wine.add(&tools);

    // The bottle chosen, which everything under it is of.
    let prefix = Rc::new(glib::clone!(
        #[weak]
        bottle,
        #[upgrade_or_else]
        || default_prefix(),
        move || {
            bottles()
                .into_iter()
                .nth(bottle.selected() as usize)
                .map_or_else(default_prefix, |(_, prefix)| prefix)
        }
    ));
    // Lists the bottles again, with the one named chosen.
    let list = Rc::new(glib::clone!(
        #[weak]
        bottle,
        move |chosen: &str| {
            let bottles = bottles();
            let listed: Vec<&str> = bottles.iter().map(|(name, _)| name.as_str()).collect();
            names.splice(0, names.n_items(), &listed);
            let at = listed.iter().position(|name| *name == chosen).unwrap_or(0);
            bottle.set_selected(at as u32);
        }
    ));
    list("");

    let component_rows: Vec<(&str, adw::ActionRow, gtk::Button)> = COMPONENTS
        .iter()
        .map(|(verb, name)| {
            let row = adw::ActionRow::builder().title(*name).build();
            let install = button("安裝");
            row.add_suffix(&install);
            components.add_row(&row);
            (*verb, row, install)
        })
        .collect();
    let component_rows = Rc::new(component_rows);
    // The component winetricks is installing, and the bottle it is for: one at a time, as
    // two of winetricks in a bottle get in each other's way.
    let installing: Rc<RefCell<Option<(PathBuf, &str)>>> = Rc::default();

    // Set while the switch is put back to what the bottle has.
    let showing = Rc::new(Cell::new(false));
    let show = Rc::new(glib::clone!(
        #[strong]
        installing,
        #[weak]
        state,
        #[weak]
        install,
        #[weak]
        bottle,
        #[weak]
        remove,
        #[weak]
        runner,
        #[weak]
        discard,
        #[weak]
        windows,
        #[weak]
        run,
        #[weak]
        dxvk,
        #[weak]
        wayland,
        #[weak]
        components,
        #[weak]
        tools,
        #[strong]
        showing,
        #[strong]
        prefix,
        #[strong]
        component_rows,
        move || {
            let version = wine_version();
            let has_winetricks = version.is_some() && glib::find_program_in_path("winetricks").is_some();
            state.set_subtitle(version.as_deref().unwrap_or("未安裝"));
            install.set_visible(version.is_none());
            bottle.set_visible(version.is_some());
            run.set_visible(version.is_some());
            tools.set_visible(version.is_some());
            wayland.set_visible(version.is_some());
            dxvk.set_visible(has_winetricks);
            components.set_visible(has_winetricks);
            let prefix = prefix();
            // Wine's own bottle is Ubuntu's Wine's, which opens a program from Files in it.
            let is_made_here = prefix.starts_with(bottles_dir());
            remove.set_visible(is_made_here);
            runner.set_visible(version.is_some() && is_made_here);
            windows.set_visible(version.is_some());
            let runners = runners();
            let listed: Vec<&str> = std::iter::once("系統")
                .chain(runners.iter().map(String::as_str))
                .collect();
            let chosen = runner_of(&prefix)
                .and_then(|runner| runners.iter().position(|other| *other == runner))
                .map_or(0, |at| at + 1);
            showing.set(true);
            runner_names.splice(0, runner_names.n_items(), &listed);
            runner.set_selected(chosen as u32);
            discard.set_visible(chosen > 0);
            // Not to be chosen from until Wine says which it is.
            windows.set_sensitive(false);
            dxvk.set_active(dxvk_installed(&prefix));
            wayland.set_active(wayland_on(&prefix));
            showing.set(false);
            // Wine is asked what Windows the bottle is, which takes it a moment.
            if version.is_some() {
                let (showing, prefix) = (showing.clone(), prefix.clone());
                glib::spawn_future_local(async move {
                    let asked = gio::spawn_blocking(move || windows_version(&prefix)).await;
                    if let Ok(Some(at)) = asked {
                        showing.set(true);
                        windows.set_selected(at as u32);
                        showing.set(false);
                        windows.set_sensitive(true);
                    }
                });
            }
            let installed = installed_components(&prefix);
            let installing = installing.borrow();
            for (verb, row, install) in component_rows.iter() {
                let has = installed.iter().any(|other| other == verb);
                let is_installing = *installing == Some((prefix.clone(), *verb));
                row.set_subtitle(match (has, is_installing) {
                    (true, _) => "已安裝",
                    (false, true) => "正在安裝",
                    (false, false) => "",
                });
                install.set_visible(!has);
                install.set_sensitive(installing.is_none());
            }
        }
    ));
    show();
    bottle.connect_selected_notify(glib::clone!(
        #[strong]
        show,
        move |_| show()
    ));

    runner.connect_selected_notify(glib::clone!(
        #[strong]
        showing,
        #[strong]
        prefix,
        #[weak]
        discard,
        move |row| {
            if showing.get() {
                return;
            }
            discard.set_visible(row.selected() > 0);
            let chosen = row
                .selected_item()
                .and_downcast::<gtk::StringObject>()
                .filter(|_| row.selected() > 0)
                .map(|name| name.string());
            let prefix = prefix();
            let changed = set_runner(&prefix, chosen.as_deref())
                .map_err(|err| err.to_string())
                .and_then(|()| update(&prefix).map_err(|err| err.to_string()));
            if let Err(err) = changed {
                alert::notice(row, "無法變更 Wine 版本", Some(&err));
            }
        }
    ));
    discard.connect_clicked(glib::clone!(
        #[weak]
        runner,
        #[strong]
        show,
        move |discard| {
            let Some(name) = runner
                .selected_item()
                .and_downcast::<gtk::StringObject>()
                .filter(|_| runner.selected() > 0)
                .map(|name| name.string())
            else {
                return;
            };
            let (show, parent) = (show.clone(), discard.clone());
            alert::confirm(
                discard,
                &format!("移除「{name}」？"),
                Some("使用它的環境會改用系統的 Wine。"),
                "移除",
                move || {
                    if let Err(err) = remove_runner(&name) {
                        alert::notice(&parent, "無法移除 Wine", Some(&err.to_string()));
                    }
                    show();
                },
            );
        }
    ));
    download.connect_clicked(glib::clone!(
        #[weak]
        runner,
        #[strong]
        show,
        move |download| {
            download.set_sensitive(false);
            runner.set_subtitle("正在下載");
            let (download, show) = (download.clone(), show.clone());
            glib::spawn_future_local(async move {
                let downloaded = gio::spawn_blocking(download_runner).await;
                download.set_sensitive(true);
                runner.set_subtitle("");
                show();
                match downloaded {
                    Ok(Ok(_)) => {}
                    Ok(Err(err)) => alert::notice(&runner, "無法下載 Wine", Some(&err)),
                    Err(_) => alert::notice(&runner, "無法下載 Wine", None),
                }
            });
        }
    ));
    windows.connect_selected_notify(glib::clone!(
        #[strong]
        showing,
        #[strong]
        prefix,
        move |row| {
            if showing.get() {
                return;
            }
            let Some((version, _)) = WINDOWS_VERSIONS.get(row.selected() as usize) else {
                return;
            };
            let (prefix, row) = (prefix(), row.clone());
            glib::spawn_future_local(async move {
                let set = gio::spawn_blocking(move || {
                    said(
                        Command::new(wine_of(&prefix))
                            .args(["winecfg", "-v", version])
                            .env("WINEPREFIX", &prefix),
                    )
                })
                .await;
                match set {
                    Ok(Ok(_)) => {}
                    Ok(Err(err)) => alert::notice(&row, "無法變更 Windows 版本", Some(&err)),
                    Err(_) => alert::notice(&row, "無法變更 Windows 版本", None),
                }
            });
        }
    ));

    install.connect_clicked(glib::clone!(
        #[weak]
        state,
        #[weak]
        spinner,
        #[strong]
        show,
        move |install| {
            install.set_visible(false);
            spinner.set_visible(true);
            state.set_subtitle("正在安裝");
            let show = show.clone();
            as_root("安裝 Wine", &apt_install(&WINE_PACKAGES), None, move |worked| {
                spinner.set_visible(false);
                show();
                if !worked {
                    alert::notice(&state, "無法安裝 Wine", None);
                }
            });
        }
    ));

    add.connect_clicked(glib::clone!(
        #[strong]
        list,
        move |button| {
            let (list, parent) = (list.clone(), button.clone());
            let name = alert::prompt(button, "新增環境", "", -1, "新增", move |name| {
                if !is_bottle_name(&name, &bottles()) {
                    return alert::notice(&parent, "無法新增環境", Some("名稱已使用，或含有「/」。"));
                }
                let prefix = bottles_dir().join(&name);
                // Wine fills the folder in as its first program starts; this is that program.
                let made = std::fs::create_dir_all(&prefix)
                    .map_err(|err| err.to_string())
                    .and_then(|()| update(&prefix).map_err(|err| err.to_string()));
                list(&name);
                if let Err(err) = made {
                    alert::notice(&parent, "無法新增環境", Some(&err));
                }
            });
            name.set_placeholder_text(Some("名稱"));
        }
    ));

    remove.connect_clicked(glib::clone!(
        #[weak]
        bottle,
        #[strong]
        prefix,
        #[strong]
        list,
        move |button| {
            let name = bottle
                .selected_item()
                .and_downcast::<gtk::StringObject>()
                .map(|name| name.string())
                .unwrap_or_default();
            let (prefix, list, parent) = (prefix(), list.clone(), button.clone());
            alert::confirm(
                button,
                &format!("刪除「{name}」？"),
                Some("其中安裝的程式與檔案都會刪除。"),
                "刪除",
                move || {
                    glib::spawn_future_local(async move {
                        let removed = gio::spawn_blocking(move || remove_bottle(&prefix)).await;
                        list("");
                        match removed {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => alert::notice(&parent, "無法刪除環境", Some(&err.to_string())),
                            Err(_) => alert::notice(&parent, "無法刪除環境", None),
                        }
                    });
                },
            );
        }
    ));

    choose.connect_clicked(glib::clone!(
        #[strong]
        prefix,
        move |button| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Windows 程式"));
            filter.add_suffix("exe");
            filter.add_suffix("msi");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder()
                .title("選擇 Windows 程式")
                .accept_label("執行")
                .filters(&filters)
                .build();
            let window = button.root().and_downcast::<gtk::Window>();
            let (button, prefix) = (button.clone(), prefix.clone());
            dialog.open(window.as_ref(), gio::Cancellable::NONE, move |chosen| {
                let path = match chosen.map(|file| file.path()) {
                    Ok(Some(path)) => path,
                    Ok(None) => return,
                    Err(err) if err.matches(gtk::DialogError::Dismissed) => return,
                    Err(err) => return eprintln!("Windows program: {err}"),
                };
                // `start` opens an installer as Windows would, a .msi with msiexec.
                let args = [OsStr::new("start"), OsStr::new("/unix"), path.as_os_str()];
                if let Err(err) = start_in(&prefix(), &args) {
                    alert::notice(&button, "無法執行程式", Some(&err.to_string()));
                }
            });
        }
    ));

    wayland.connect_active_notify(glib::clone!(
        #[strong]
        showing,
        #[strong]
        show,
        #[strong]
        prefix,
        move |row| {
            if showing.get() {
                return;
            }
            let (on, prefix) = (row.is_active(), prefix());
            row.set_sensitive(false);
            let (row, show) = (row.clone(), show.clone());
            glib::spawn_future_local(async move {
                let set = gio::spawn_blocking(move || set_wayland(&prefix, on)).await;
                row.set_sensitive(true);
                // The switch keeps what was chosen rather than being shown again: while the
                // bottle's programs run, Wine writes user.reg only every 30 seconds or so.
                let why = match set {
                    Ok(Ok(())) => return,
                    Ok(Err(err)) => Some(err),
                    Err(_) => None,
                };
                show();
                alert::notice(&row, "無法變更 Wayland", why.as_deref());
            });
        }
    ));

    dxvk.connect_active_notify(glib::clone!(
        #[strong]
        show,
        #[strong]
        prefix,
        move |row| {
            if showing.get() {
                return;
            }
            let (on, prefix) = (row.is_active(), prefix());
            row.set_sensitive(false);
            let (row, show) = (row.clone(), show.clone());
            glib::spawn_future_local(async move {
                let done = gio::spawn_blocking(move || set_dxvk(&prefix, on)).await;
                row.set_sensitive(true);
                show();
                if let Some(why) = failure(done) {
                    alert::notice(&row, "無法變更 DXVK", Some(&why));
                }
            });
        }
    ));

    for (verb, row, install) in component_rows.iter() {
        let verb = *verb;
        install.connect_clicked(glib::clone!(
            #[weak]
            row,
            #[strong]
            show,
            #[strong]
            prefix,
            #[strong]
            installing,
            move |_| {
                let prefix = prefix();
                installing.replace(Some((prefix.clone(), verb)));
                show();
                let (show, installing) = (show.clone(), installing.clone());
                glib::spawn_future_local(async move {
                    let done = gio::spawn_blocking(move || winetricks(&prefix, &[verb])).await;
                    installing.take();
                    show();
                    if let Some(why) = failure(done) {
                        let title = format!("無法安裝「{}」", row.title());
                        alert::notice(&row, &title, Some(&why));
                    }
                });
            }
        ));
    }

    for (name, tool) in TOOLS {
        let row = adw::ActionRow::builder().title(name).build();
        let open = button("開啟");
        row.add_suffix(&open);
        row.set_activatable_widget(Some(&open));
        tools.add_row(&row);
        open.connect_clicked(glib::clone!(
            #[strong]
            prefix,
            move |button| {
                if let Err(err) = start_in(&prefix(), &[OsStr::new(tool)]) {
                    alert::notice(button, &format!("無法開啟{name}"), Some(&err.to_string()));
                }
            }
        ));
    }
    let files = adw::ActionRow::builder().title("C 槽").build();
    let browse = button("開啟");
    files.add_suffix(&browse);
    files.set_activatable_widget(Some(&browse));
    tools.add_row(&files);
    browse.connect_clicked(glib::clone!(
        #[strong]
        prefix,
        move |button| {
            let drive = gio::File::for_path(prefix().join("drive_c"));
            if let Err(err) = gio::AppInfo::launch_default_for_uri(&drive.uri(), gio::AppLaunchContext::NONE)
            {
                alert::notice(button, "無法開啟 C 槽", Some(&err.to_string()));
            }
        }
    ));
    let programs = adw::ActionRow::builder().title("執行中的程式").build();
    let end = button("全部結束");
    programs.add_suffix(&end);
    tools.add_row(&programs);
    end.connect_clicked(move |button| {
        let prefix = prefix();
        let ended = wineserver(&prefix)
            .ok_or_else(|| std::io::Error::other("wineserver is not installed"))
            .and_then(|wineserver| {
                Command::new(wineserver)
                    .arg("-k")
                    .env("WINEPREFIX", prefix)
                    .status()
            });
        if let Err(err) = ended {
            alert::notice(button, "無法結束程式", Some(&err.to_string()));
        }
    });
    wine
}

// The Windows machine.

/// What the Windows machine is, as Podman and Windows say.
#[derive(Clone, Debug, PartialEq)]
enum Machine {
    /// Without KVM: the computer, or the one it runs in, has no hardware virtualization on.
    Unsupported,
    Missing,
    /// Downloading and installing Windows, with how far it has got, and whether Windows has
    /// started, so that it may be finished.
    Installing {
        stage: String,
        booted: bool,
    },
    Stopping,
    Stopped,
    Running,
}

/// Where the machine is kept: its storage, the account Windows was installed with, and the
/// day the installation finished, which the evaluation counts from.
fn folder() -> PathBuf {
    glib::user_data_dir().join("nekoland/windows")
}

fn storage() -> PathBuf {
    folder().join("storage")
}

fn installed_file() -> PathBuf {
    folder().join("installed")
}

/// The day Windows finished installing, as "2026-11-21".
fn installed_on() -> Option<String> {
    let day = std::fs::read_to_string(installed_file()).ok()?;
    Some(day.trim().to_string()).filter(|day| !day.is_empty())
}

/// The day the evaluation ends, counted from the day it was installed.
fn evaluation_ends(installed: &str) -> Option<String> {
    let mut parts = installed.split('-').map(str::parse::<i32>);
    let (Some(Ok(year)), Some(Ok(month)), Some(Ok(day))) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    let ends = glib::DateTime::from_local(year, month, day, 0, 0, 0.0)
        .ok()?
        .add_days(EVALUATION_DAYS)
        .ok()?;
    ends.format("%Y-%m-%d").ok().map(String::from)
}

fn today() -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|now| now.format("%Y-%m-%d").ok())
        .map(String::from)
        .unwrap_or_default()
}

/// The Windows account's name and password, as the machine was made with them.
fn account() -> Option<(String, String)> {
    let kept = std::fs::read_to_string(folder().join("account")).ok()?;
    let (user, password) = kept.split_once('\n')?;
    Some((user.to_string(), password.trim_end_matches('\n').to_string()))
}

/// A password for the Windows account, which no one types: the launchers give it.
fn new_password() -> std::io::Result<String> {
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut bytes = [0u8; 20];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes
        .iter()
        .map(|byte| LETTERS[*byte as usize % LETTERS.len()] as char)
        .collect())
}

/// Runs podman, saying why it failed in podman's last words.
fn podman(args: &[&str]) -> Result<String, String> {
    let out = Command::new("podman")
        .args(args)
        .output()
        .map_err(|err| format!("podman: {err}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    Err(said
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("podman failed")
        .trim()
        .to_string())
}

/// What the machine is now; this waits on Podman, so it is not for the main thread.
fn machine() -> Machine {
    if !Path::new("/dev/kvm").exists() {
        return Machine::Unsupported;
    }
    let Ok(status) = podman(&["container", "inspect", "--format", "{{.State.Status}}", CONTAINER]) else {
        if setting_up().is_some() {
            return Machine::Installing {
                stage: "正在準備".into(),
                booted: false,
            };
        }
        return Machine::Missing;
    };
    match status.trim() {
        "running" => {}
        "stopping" => return Machine::Stopping,
        _ => return Machine::Stopped,
    }
    if installed_on().is_some() {
        return Machine::Running;
    }
    // What its viewer shows last, where it counts the download's progress.
    let shown = podman(&["exec", CONTAINER, "cat", "/run/shm/msg.html"]).unwrap_or_default();
    let log = podman(&["logs", CONTAINER]).unwrap_or_default();
    let (stage, booted) = install_stage(&format!("{log}\n{shown}"));
    Machine::Installing { stage, booted }
}

/// Whether Windows, once started, has finished installing: it then lets its account in. Until
/// then FreeRDP waits most of a minute for it each time.
fn finish_installing() -> bool {
    let done = account().is_some_and(|(user, password)| lets_in(&user, &password));
    if done {
        if let Err(err) = std::fs::write(installed_file(), format!("{}\n", today())) {
            eprintln!("{}: {err}", installed_file().display());
        }
    }
    done
}

/// Podman making the machine, by its process ID, while it downloads the image of dockur's
/// before there is a container to look at.
fn setting_up() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(folder().join("setup"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    (name.trim() == "podman").then_some(pid)
}

/// How far the installation has got, from what dockur/windows last said, and whether it has
/// started Windows, whose own installation it cannot see. The download's progress is a
/// percentage, as "Downloading Windows 11 Enterprise (26.8%)".
fn install_stage(log: &str) -> (String, bool) {
    for line in log.lines().rev() {
        let line = line.trim();
        if line.contains("Booting") || line.contains("Windows started") {
            return ("正在安裝".into(), true);
        }
        if line.contains("Downloading") {
            let percent = line.split_once('%').and_then(|(before, _)| {
                let digits = before
                    .rfind(|c: char| !c.is_ascii_digit() && c != '.')
                    .map_or(before, |at| &before[at + 1..]);
                digits.parse::<f64>().ok()
            });
            return match percent {
                Some(percent) => (format!("正在下載 {percent:.0}%"), false),
                None => ("正在下載".into(), false),
            };
        }
        if line.contains("Extracting") || line.contains("Adding") || line.contains("Building") {
            return ("正在準備安裝".into(), false);
        }
    }
    ("正在準備".into(), false)
}

/// The words FreeRDP connects to the machine with, as the account; the password it reads
/// from its input, not from words any program on the computer can list, and before it
/// connects, as `+auth-only` wants it.
fn rdp_args(user: &str) -> Vec<String> {
    vec![
        format!("/v:127.0.0.1:{RDP_PORT}"),
        format!("/u:{user}"),
        "/from-stdin:force".into(),
        "/cert:ignore".into(),
    ]
}

/// Runs FreeRDP with the password at its input.
fn freerdp(args: &[String], password: &str, quiet: bool) -> std::io::Result<std::process::ExitStatus> {
    start_freerdp(args, password, quiet)?.wait()
}

fn start_freerdp(args: &[String], password: &str, quiet: bool) -> std::io::Result<std::process::Child> {
    let mut command = Command::new("xfreerdp3");
    command.args(args).stdin(Stdio::piped());
    if quiet {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = command.spawn()?;
    if let Some(mut input) = child.stdin.take() {
        // FreeRDP may leave before it reads, when it cannot connect at all.
        let _ = writeln!(input, "{password}");
    }
    Ok(child)
}

/// Whether Windows takes its account over RDP, as it does once installed and started.
fn lets_in(user: &str, password: &str) -> bool {
    let mut args = rdp_args(user);
    args.extend(["+auth-only".into(), "/timeout:10000".into()]);
    freerdp(&args, password, true).is_ok_and(|status| status.success())
}

fn memory_gb() -> u64 {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb: u64 = crate::field(&meminfo, "MemTotal", ':')
        .and_then(|value| value.split_whitespace().next()?.parse().ok())
        .unwrap_or(0);
    kb / (1 << 20)
}

/// Makes the virtual machine, which goes on to download and install Windows by itself.
fn create_machine() -> Result<(), String> {
    let failed = |err: std::io::Error| err.to_string();
    std::fs::create_dir_all(storage()).map_err(failed)?;
    let user = glib::user_name().to_string_lossy().into_owned();
    let password = new_password().map_err(failed)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(folder().join("account"))
        .and_then(|mut kept| write!(kept, "{user}\n{password}\n"))
        .map_err(failed)?;
    // Half the computer's memory and processors, and Windows 11's least.
    let memory = format!("RAM_SIZE={}G", (memory_gb() / 2).clamp(4, 16));
    let cores = std::thread::available_parallelism().map_or(2, |count| (count.get() / 2).max(2));
    let cores = format!("CPU_CORES={cores}");
    let (version, name) = (format!("VERSION={EDITION}"), format!("USERNAME={user}"));
    let (rdp_tcp, rdp_udp) = (
        format!("127.0.0.1:{RDP_PORT}:3389/tcp"),
        format!("127.0.0.1:{RDP_PORT}:3389/udp"),
    );
    let viewer = format!("127.0.0.1:{VIEWER_PORT}:8006");
    let volume = format!("{}:/storage", storage().display());
    // What it says goes to a file rather than to Settings, so that it goes on when Settings is
    // closed, as the machine's download would otherwise stop with it.
    let said = folder().join("setup.log");
    let log = std::fs::File::create(&said).map_err(failed)?;
    let mut child = Command::new("podman")
        // `--env PASSWORD` takes it from here, out of the words other programs can list.
        .env("PASSWORD", &password)
        .args(["run", "--detach", "--name", CONTAINER])
        .args([
            "--env",
            &version,
            "--env",
            "LANGUAGE=zh-tw",
            // Signed in on its own screen, Windows would ask each RemoteApp whether to take
            // that session over, which none can answer. dockur/windows itself lets RemoteApp
            // run any program.
            "--env",
            "AUTOLOGIN=N",
            "--env",
            &name,
            "--env",
            "PASSWORD",
        ])
        .args(["--env", &memory, "--env", &cores])
        .args([
            "--device",
            "/dev/kvm",
            "--device",
            "/dev/net/tun",
            "--cap-add",
            "NET_ADMIN",
        ])
        .args(["--publish", &rdp_tcp, "--publish", &rdp_udp, "--publish", &viewer])
        .args(["--volume", &volume, "--stop-timeout", "120", IMAGE])
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .map_err(|err| format!("podman: {err}"))?;
    let setup = folder().join("setup");
    std::fs::write(&setup, child.id().to_string()).map_err(failed)?;
    let status = child.wait().map_err(failed)?;
    let _ = std::fs::remove_file(&setup);
    if status.success() {
        return Ok(());
    }
    let said = std::fs::read_to_string(&said).unwrap_or_default();
    Err(said
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("podman failed")
        .trim()
        .to_string())
}

/// Takes the machine away, with Windows, its files, and the launchers made for its programs.
fn remove_machine() -> Result<(), String> {
    // Or Podman would make it after all, once its download ends.
    if let Some(pid) = setting_up() {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
    if let Err(why) = podman(&["rm", "--force", "--time", "10", CONTAINER]) {
        if !why.contains("no such container") {
            return Err(why);
        }
    }
    if storage().exists() {
        // What Windows' container wrote belongs to the users it maps, which only it can remove.
        podman(&["unshare", "rm", "-rf", &storage().to_string_lossy()])?;
    }
    match std::fs::remove_dir_all(folder()) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.to_string()),
        _ => {}
    }
    for (id, ..) in launchers() {
        if let Err(err) = remove_launcher(&id) {
            eprintln!("{id}: {err}");
        }
    }
    // The image is worth keeping for nothing else; another container still using it keeps it.
    if let Err(why) = podman(&["rmi", IMAGE]) {
        eprintln!("podman rmi: {why}");
    }
    Ok(())
}

// Launchers for Windows' programs.

fn entries_dir() -> PathBuf {
    glib::user_data_dir().join("applications")
}

fn entry_path(id: &str) -> PathBuf {
    entries_dir().join(format!("{id}.desktop"))
}

/// The launchers made here, by name: each one's ID, name and Windows program.
fn launchers() -> Vec<(String, String, String)> {
    let Ok(entries) = std::fs::read_dir(entries_dir()) else {
        return Vec::new();
    };
    let mut found: Vec<(String, String, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_str()?.strip_suffix(".desktop")?.to_string();
            id.starts_with(ENTRY_PREFIX).then_some(())?;
            let file = glib::KeyFile::new();
            file.load_from_file(entry.path(), glib::KeyFileFlags::NONE).ok()?;
            let name = file.string(ENTRY_GROUP, "Name").ok()?.to_string();
            let program = file.string(ENTRY_GROUP, PROGRAM_KEY).ok()?.to_string();
            Some((id, name, program))
        })
        .collect();
    found.sort_by(|a, b| a.1.cmp(&b.1));
    found
}

/// Where a launcher's icon is kept, as Windows drew it.
fn icon_path(id: &str) -> PathBuf {
    folder().join("icons").join(format!("{id}.png"))
}

/// The folder shared with the agent: its script, what is asked of it, and what it says.
fn share_dir() -> PathBuf {
    folder().join("share")
}

/// Where Windows lists its programs, with their icons.
fn found_dir() -> PathBuf {
    share_dir().join("found")
}

/// A program Windows has in its Start menu.
#[derive(Debug, PartialEq)]
struct Found {
    name: String,
    program: String,
    /// What its icon's file is named after.
    number: String,
    /// The extensions of the files Windows opens with it, as ".docx".
    extensions: Vec<String>,
}

/// The programs in the list the agent writes, by name.
fn parse_found(list: &str) -> Vec<Found> {
    let mut found: Vec<Found> = list
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim_start_matches('\u{feff}').split('\t');
            let (name, program, number) = (fields.next()?, fields.next()?, fields.next()?);
            is_windows_path(program).then(|| Found {
                name: name.to_string(),
                program: program.to_string(),
                number: number.to_string(),
                extensions: fields
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

fn found_apps() -> Vec<Found> {
    parse_found(&std::fs::read_to_string(found_dir().join("apps.tsv")).unwrap_or_default())
}

/// Asks the agent for something, in a file of its own, named once it is whole.
fn ask(request: &str) -> std::io::Result<()> {
    let requests = share_dir().join("requests");
    std::fs::create_dir_all(&requests)?;
    let name = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
        std::process::id()
    );
    let (written, asked) = (
        requests.join(format!("{name}.new")),
        requests.join(format!("{name}.txt")),
    );
    std::fs::write(&written, format!("{request}\n"))?;
    std::fs::rename(written, asked)
}

/// Has Windows list its programs, through the agent, starting it if it is not there.
fn find_apps() -> Result<(), String> {
    let failed = |err: std::io::Error| err.to_string();
    // Without the last list and its icons, whose numbers the new one gives anew.
    match std::fs::remove_dir_all(found_dir()) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(failed(err)),
        _ => {}
    }
    ask("find").map_err(failed)?;
    let exe = std::env::current_exe().map_err(failed)?;
    let keeper = gio::Subprocess::newv(
        &[exe.as_os_str(), OsStr::new(OPEN_FLAG), OsStr::new(KEEP_FLAG)],
        gio::SubprocessFlags::NONE,
    )
    .map_err(|err| err.to_string())?;
    let list = found_dir().join("apps.tsv");
    let started = Instant::now();
    while !list.exists() {
        if started.elapsed() > START_TIMEOUT {
            return Err("Windows took too long to list its programs".to_string());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    drop(keeper);
    Ok(())
}

/// A window Windows shows, as the agent lists it.
#[derive(Debug, PartialEq)]
struct Shown {
    /// Its handle in Windows, eight hexadecimal digits: FreeRDP's window is `RAIL:` and it.
    handle: String,
    program: String,
    /// How much of its top is a title bar Windows draws.
    title: u32,
    /// How large Windows has it.
    size: (u32, u32),
    /// Its program as the Store's are known, `shell:AppsFolder\…`, when it is one of those:
    /// their files move with each update.
    packaged: String,
}

fn parse_shown(list: &str) -> Vec<Shown> {
    list.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (handle, program, title) = (fields.next()?, fields.next()?, fields.next()?);
            let (width, height) = (
                fields.next()?.trim().parse().ok()?,
                fields.next()?.trim().parse().ok()?,
            );
            (handle.len() == 8 && handle.bytes().all(|digit| digit.is_ascii_hexdigit())).then(|| Shown {
                handle: handle.to_string(),
                program: program.to_string(),
                title: title.trim().parse().unwrap_or(0),
                size: (width, height),
                packaged: fields.next().unwrap_or_default().trim().to_string(),
            })
        })
        .collect()
}

/// What the compositor is to know a window by: the launcher of its program, and the name
/// that has it draw its title bar over Windows' and leave out what FreeRDP's window is
/// larger by.
fn identity(shown: &Shown, launchers: &[(String, String, String)]) -> (String, String) {
    let app = launchers
        .iter()
        .find(|(_, _, program)| {
            program.eq_ignore_ascii_case(&shown.program) || program.eq_ignore_ascii_case(&shown.packaged)
        })
        .map_or(UNNAMED_APP, |(id, ..)| id)
        .to_string();
    let (width, height) = shown.size;
    (format!("{WINDOW}{}-{width}-{height}", shown.title), app)
}

/// FreeRDP's windows for one of Windows', which it names after the handle until they are
/// given an identity.
fn x_windows(handle: &str) -> Vec<String> {
    let found = Command::new("xdotool")
        .args(["search", "--class", &format!("^RAIL:{handle}$")])
        .output();
    found.map_or_else(
        |_| Vec::new(),
        |found| {
            String::from_utf8_lossy(&found.stdout)
                .split_whitespace()
                .map(str::to_string)
                .collect()
        },
    )
}

/// Gives FreeRDP's `windows` the identity; whether there were any and they took it.
fn tag(windows: &[String], (name, app): &(String, String)) -> bool {
    !windows.is_empty()
        && windows.iter().all(|window| {
            Command::new("xdotool")
                .args(["set_window", "--classname", name, "--class", app, window])
                .status()
                .is_ok_and(|status| status.success())
        })
}

/// How Windows names `file`, any of this computer's.
fn windows_path(file: &Path) -> Option<String> {
    let file = std::path::absolute(file).ok()?;
    Some(format!(
        "\\\\tsclient\\{FILES}{}",
        file.to_str()?.replace('/', "\\")
    ))
}

/// Keeps the one connection to Windows: its account has one session, so every program is
/// started in it by the agent, which this starts as the connection's own program. Each
/// window FreeRDP then shows is given its program's launcher to be known by, as the agent
/// tells them apart. Returns at once when another keeps the connection already, and when no
/// window has been shown for a while.
fn keep_connection() -> Result<(), String> {
    let failed = |err: std::io::Error| err.to_string();
    let share = share_dir();
    std::fs::create_dir_all(share.join("requests")).map_err(failed)?;
    let lock = std::fs::File::create(share.join(APPS_LOCK)).map_err(failed)?;
    if lock.try_lock().is_err() {
        return Ok(());
    }
    have_released(DESKTOP_LOCK)?;
    let (user, password) = account().ok_or("Windows is not installed")?;
    let running = podman(&[
        "container",
        "inspect",
        "--format",
        "{{.State.Running}}",
        CONTAINER,
    ])?;
    if running.trim() != "true" {
        podman(&["start", CONTAINER])?;
        // Nothing shows for the minute or so Windows takes to start.
        notify("正在啟動 Windows", "");
    }
    let listed = share.join("windows.tsv");
    let _ = std::fs::remove_file(&listed);
    std::fs::write(share.join("agent.ps1"), AGENT).map_err(failed)?;
    let mut args = rdp_args(&user);
    args.extend([
        "/sound".into(),
        // FreeRDP splits these at commas, but for a part in quotes.
        format!("/drive:{SHARE},\"{}\"", share.display()),
        format!("/drive:{FILES},/"),
        // conhost without a window, or the agent would have a terminal's. The shared folder
        // is there a moment after the program starts, so it waits for its script.
        format!(
            "/app:program:conhost.exe,cmd:--headless powershell.exe -NoProfile -ExecutionPolicy Bypass \
             -Command while (-not (Test-Path {AGENT_PATH})) {{ Start-Sleep -Milliseconds 200 }}; {AGENT_PATH}"
        ),
    ]);
    let connect = || start_freerdp(&args, &password, false).map_err(|err| format!("xfreerdp3: {err}"));
    let mut connection = connect()?;
    // The identity each of Windows' windows was given, and FreeRDP's windows that have it:
    // once given it, they are not named after the handle any more.
    let mut tagged: std::collections::HashMap<String, ((String, String), Vec<String>)> = Default::default();
    let (started, mut connected, mut busy, mut answered) =
        (Instant::now(), Instant::now(), Instant::now(), false);
    let ended = loop {
        std::thread::sleep(Duration::from_millis(300));
        if connection.try_wait().map_err(failed)?.is_some() {
            if answered {
                break Ok(());
            }
            if started.elapsed() > START_TIMEOUT {
                break Err("Windows did not let its account in".to_string());
            }
            // Windows is starting still. Connecting is itself the way to ask: after a
            // connection that only signs in, Windows holds the next one up for a minute.
            std::thread::sleep(Duration::from_secs(2));
            connection = connect()?;
            connected = Instant::now();
            continue;
        }
        let Ok(list) = std::fs::read_to_string(&listed) else {
            // The agent puts each list in the place of the last, which is not there for the
            // moment between.
            if answered {
                continue;
            }
            if started.elapsed() > START_TIMEOUT {
                break Err("the agent did not start in Windows".to_string());
            }
            // Now and then Windows holds a connection up for a minute or more; the next
            // one is let in at once.
            if connected.elapsed() > AGENT_START {
                let _ = connection.kill();
                let _ = connection.wait();
                connection = connect()?;
                connected = Instant::now();
                tagged.clear();
            }
            continue;
        };
        answered = true;
        let shown = parse_shown(&list);
        let launchers = launchers();
        tagged.retain(|handle, _| shown.iter().any(|shown| shown.handle == *handle));
        for shown in &shown {
            let identity = identity(shown, &launchers);
            let windows = match tagged.get(&shown.handle) {
                Some((was, _)) if *was == identity => continue,
                Some((_, windows)) => windows.clone(),
                None => x_windows(&shown.handle),
            };
            if tag(&windows, &identity) {
                tagged.insert(shown.handle.clone(), (identity, windows));
            }
        }
        // The desktop is to be shown, in the session this connection is to.
        if share.join(RELEASE).exists() {
            break Ok(());
        }
        let asked =
            std::fs::read_dir(share.join("requests")).is_ok_and(|mut requests| requests.next().is_some());
        if !shown.is_empty() || asked {
            busy = Instant::now();
        } else if busy.elapsed() > IDLE {
            break Ok(());
        }
    };
    let _ = connection.kill();
    let _ = connection.wait();
    ended
}

/// The types of the files with `extensions`, as a desktop entry names them.
fn mime_types(extensions: &[String]) -> Vec<String> {
    let mut types: Vec<String> = extensions
        .iter()
        .filter_map(|extension| {
            // Without the file's content, GIO calls one whose name it does not know empty.
            let name = format!("file{}", extension.to_lowercase());
            let (kind, uncertain) = gio::content_type_guess(Some(name), &[]);
            let kind = gio::content_type_get_mime_type(&kind)?;
            (!uncertain && !["application/octet-stream", "application/x-zerosize"].contains(&kind.as_str()))
                .then(|| kind.to_string())
        })
        .collect();
    types.sort();
    types.dedup();
    types
}

/// What a program from the Store is started by, before its name there.
const PACKAGED: &str = "shell:AppsFolder\\";

/// Whether `text` is a whole path in Windows, as `C:\Windows\notepad.exe`, or a program
/// from the Store, as `shell:AppsFolder\Microsoft.WindowsStore_8wekyb3d8bbwe!App`.
fn is_windows_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    text.strip_prefix(PACKAGED).is_some_and(|name| name.contains('!'))
        || bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// The program's file name without its extension: `C:\Windows\notepad.exe` is "notepad".
fn program_stem(program: &str) -> &str {
    // One from the Store is named by who made it, a dot, itself, and what tells it apart.
    if let Some(name) = program.strip_prefix(PACKAGED) {
        let name = name.split(['_', '!']).next().unwrap_or(name);
        return name.rsplit('.').next().unwrap_or(name);
    }
    let file = program.rsplit('\\').next().unwrap_or(program);
    file.rsplit_once('.').map_or(file, |(stem, _)| stem)
}

/// A launcher's ID, from its program's name, unlike those taken.
fn launcher_id(program: &str, taken: impl Fn(&str) -> bool) -> String {
    let slug: String = program_stem(program)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = match slug.trim_matches('-') {
        "" => "app",
        slug => slug,
    };
    let mut id = format!("{ENTRY_PREFIX}{slug}");
    let mut count = 2;
    while taken(&id) {
        id = format!("{ENTRY_PREFIX}{slug}-{count}");
        count += 1;
    }
    id
}

/// Writes a desktop entry opening `program` in Windows, so that it is among the applications;
/// its windows take the entry's ID as their class, for the Dock to know them by.
/// It opens a file given to it, and has the icon and the files' types Windows listed for the
/// program, if it did.
fn add_launcher(name: &str, program: &str) -> Result<(), String> {
    let failed = |err: std::io::Error| err.to_string();
    let exe = std::env::current_exe().map_err(failed)?;
    let id = launcher_id(program, |id| entry_path(id).exists());
    let found = found_apps()
        .into_iter()
        .find(|found| found.program.eq_ignore_ascii_case(program));
    let drawn = found
        .as_ref()
        .map(|found| found_dir().join(format!("{}.png", found.number)))
        .filter(|drawn| drawn.exists());
    let icon = match drawn {
        Some(drawn) => {
            let kept = icon_path(&id);
            std::fs::create_dir_all(folder().join("icons")).map_err(failed)?;
            std::fs::copy(drawn, &kept).map_err(failed)?;
            kept.to_string_lossy().into_owned()
        }
        None => "application-x-executable".into(),
    };
    let types = found.map_or_else(Vec::new, |found| mime_types(&found.extensions));
    let file = glib::KeyFile::new();
    file.set_string(ENTRY_GROUP, "Type", "Application");
    file.set_string(ENTRY_GROUP, "Name", name);
    file.set_string(
        ENTRY_GROUP,
        "Exec",
        &format!("\"{}\" {OPEN_FLAG} {id} %f", exe.display()),
    );
    file.set_string(ENTRY_GROUP, "Icon", &icon);
    file.set_string(ENTRY_GROUP, "StartupWMClass", &id);
    file.set_string(ENTRY_GROUP, PROGRAM_KEY, program);
    if !types.is_empty() {
        file.set_string(ENTRY_GROUP, "MimeType", &format!("{};", types.join(";")));
    }
    std::fs::create_dir_all(entries_dir()).map_err(failed)?;
    file.save_to_file(entry_path(&id))
        .map_err(|err| err.to_string())?;
    if !types.is_empty() {
        // The files' types are found through the folder's cache of them.
        if let Err(err) = Command::new("update-desktop-database")
            .arg(entries_dir())
            .status()
        {
            eprintln!("update-desktop-database: {err}");
        }
    }
    Ok(())
}

/// Takes a launcher away, with its icon.
fn remove_launcher(id: &str) -> std::io::Result<()> {
    std::fs::remove_file(entry_path(id))?;
    match std::fs::remove_file(icon_path(id)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Says what a launcher does where it has no window to say it in.
fn notify(summary: &str, body: &str) {
    let said = Command::new("notify-send")
        .args(["--icon", "application-x-executable", summary, body])
        .status();
    if let Err(err) = said {
        eprintln!("notify-send: {err}");
    }
}

/// What the items Windows has for a file are kept under: they go by its extension, and a
/// folder has its own.
fn menu_name(file: &Path) -> String {
    if file.is_dir() {
        return "folder".to_string();
    }
    file.extension().and_then(OsStr::to_str).map_or_else(
        || "file".to_string(),
        |extension| format!(".{}", extension.to_lowercase()),
    )
}

/// A program Wine made a launcher for, as the launcher says it: its name, its bottle, and
/// the Start menu's shortcut to it there. Not what is no program, as a manual is.
fn wine_program(launcher: &str) -> Option<(String, PathBuf, String)> {
    let line = |key: &str| launcher.lines().find_map(|line| line.strip_prefix(key));
    if !line("StartupWMClass=")?.ends_with(".exe") {
        return None;
    }
    let (prefix, command) = line("Exec=env WINEPREFIX=\"")?.split_once("\" ")?;
    // After Wine's own name: the shortcut, a backslash before each space of it, and each
    // of those and of its own written twice.
    let shortcut = command
        .split_once(' ')?
        .1
        .replace("\\\\", "\\")
        .replace("\\ ", " ");
    Some((line("Name=")?.to_string(), PathBuf::from(prefix), shortcut))
}

/// The programs installed in Wine's bottles, by the launchers Wine made for them.
fn wine_programs() -> Vec<(String, PathBuf, String)> {
    let (mut programs, mut folders) = (Vec::new(), vec![glib::user_data_dir().join("applications/wine")]);
    while let Some(folder) = folders.pop() {
        for path in std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
        {
            if path.is_dir() {
                folders.push(path);
            } else if let Some(program) = std::fs::read_to_string(&path)
                .ok()
                .as_deref()
                .and_then(wine_program)
            {
                programs.push(program);
            }
        }
    }
    programs.sort();
    programs
}

/// Has the helper built from `windows/shellmenu.c` say what a bottle's programs add to a
/// file's right-click menu, or do the item named of them. Nothing where the helper was not
/// built or the bottle has not been filled in.
fn shell_menu(prefix: &Path, file: &str, item: &[String]) -> Option<std::process::Output> {
    let helper = std::env::current_exe().ok()?.with_file_name(SHELL_MENU);
    if !helper.exists() || !prefix.join("system.reg").exists() {
        return None;
    }
    // Wine has the whole of Linux's files as its drive Z.
    Command::new(wine_of(prefix))
        .arg(helper)
        .arg(format!("Z:{}", file.replace('/', "\\")))
        .args(item)
        .env("WINEPREFIX", prefix)
        .env("WINEDEBUG", "-all")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
}

/// `nekoland-settings --windows-menu FILE [ITEM…]`: says what Windows' programs add to the
/// file's right-click menu, a line an item, its submenus' names and its own with tabs
/// between; or has Windows do the item named so. The list is the one Windows last gave for
/// files of the kind, there at once; Windows gives it anew while it runs.
pub fn menu(args: &[String]) -> glib::ExitCode {
    let done = match args {
        [again, file] if again == MENU_AGAIN => {
            let listed = windows_path(Path::new(file))
                .ok_or_else(|| format!("{file}: not a file"))
                .and_then(|path| {
                    ask(&format!("menu\t{path}\t{}", menu_name(Path::new(file))))
                        .map_err(|err| err.to_string())
                });
            listed.and_then(|()| keep_connection())
        }
        [file] => {
            let list = share_dir()
                .join("menus")
                .join(format!("{}.tsv", menu_name(Path::new(file))));
            let listed = std::fs::read_to_string(&list);
            print!("{}", listed.as_deref().unwrap_or_default());
            // A file is opened with a program in Wine too; a folder they have no use for.
            if !Path::new(file).is_dir() {
                for (name, ..) in wine_programs() {
                    println!("{name}");
                }
            }
            // shortcut: every bottle's Wine is started at each right-click, cached when
            // that is slow with many bottles.
            for (_, prefix) in bottles() {
                if let Some(said) = shell_menu(&prefix, file, &[]) {
                    print!("{}", String::from_utf8_lossy(&said.stdout));
                }
            }
            // Anew when Windows is connected to already, or has never said: connecting takes
            // a while, which a list that is there is not worth.
            let is_connected =
                std::fs::File::open(share_dir().join(APPS_LOCK)).is_ok_and(|lock| lock.try_lock().is_err());
            if (is_connected || listed.is_err()) && machine() == Machine::Running {
                std::env::current_exe()
                    .and_then(|exe| {
                        Command::new(exe)
                            .args([MENU_FLAG, MENU_AGAIN, file])
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .spawn()
                    })
                    .map(drop)
                    .map_err(|err| err.to_string())
            } else {
                Ok(())
            }
        }
        [file, name] if wine_programs().iter().any(|(program, ..)| program == name) => {
            let programs = wine_programs();
            let (_, prefix, shortcut) = programs
                .iter()
                .find(|(program, ..)| program == name)
                .expect("found above");
            // Wine has the whole of Linux's files as its drive Z.
            let file = format!("Z:{}", file.replace('/', "\\"));
            start_in(
                prefix,
                &[OsStr::new("start"), OsStr::new(shortcut), OsStr::new(&file)],
            )
            .map_err(|err| err.to_string())
        }
        // The helper says so when the item is none of the bottle's.
        [file, item @ ..]
            if !item.is_empty()
                && bottles().iter().any(|(_, prefix)| {
                    shell_menu(prefix, file, item).is_some_and(|done| done.status.success())
                }) =>
        {
            Ok(())
        }
        [file, item @ ..] if !item.is_empty() => windows_path(Path::new(file))
            .ok_or_else(|| format!("{file}: not a file"))
            .and_then(|path| {
                ask(&format!("verb\t{path}\t{}", item.join("\t"))).map_err(|err| err.to_string())
            })
            .and_then(|()| keep_connection()),
        _ => Err("no file".to_string()),
    };
    match done {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{MENU_FLAG}: {why}");
            glib::ExitCode::FAILURE
        }
    }
}

/// Opens a launcher's program, or the desktop without one, from Settings: in a process of its
/// own, which waits for Windows and outlives Settings.
fn spawn_open(button: &gtk::Button, id: Option<&str>) {
    // Through GLib, which waits for each when it ends, so that none is left behind as a zombie.
    let opened = std::env::current_exe()
        .map_err(|err| err.to_string())
        .and_then(|exe| {
            let mut args = vec![exe.into_os_string(), OPEN_FLAG.into()];
            args.extend(id.map(Into::into));
            let args: Vec<&OsStr> = args.iter().map(|arg| arg.as_os_str()).collect();
            gio::Subprocess::newv(&args, gio::SubprocessFlags::NONE).map_err(|err| err.to_string())
        });
    if let Err(err) = opened {
        alert::notice(button, "無法開啟", Some(&err.to_string()));
    }
}

/// `nekoland-settings --windows-app [ENTRY [FILE]]`: starts Windows if it is off, waits for it
/// to let its account in, and shows the entry's program in windows of its own, with the file
/// open in it, or the desktop.
pub fn open(id: Option<&str>, file: Option<&str>) -> glib::ExitCode {
    match open_in_window(id, file) {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{OPEN_FLAG}: {why}");
            notify("無法開啟 Windows App", &why);
            glib::ExitCode::FAILURE
        }
    }
}

fn open_in_window(id: Option<&str>, file: Option<&str>) -> Result<(), String> {
    let Some(id) = id else {
        return show_desktop();
    };
    if id != KEEP_FLAG {
        let entry = glib::KeyFile::new();
        entry
            .load_from_file(entry_path(id), glib::KeyFileFlags::NONE)
            .map_err(|err| format!("{id}: {err}"))?;
        let program = entry
            .string(ENTRY_GROUP, PROGRAM_KEY)
            .map_err(|err| format!("{id}: {err}"))?;
        let file = match file {
            Some(file) => windows_path(Path::new(file)).ok_or_else(|| format!("{file}: not a file"))?,
            None => String::new(),
        };
        ask(&format!("run\t{program}\t{file}")).map_err(|err| err.to_string())?;
    }
    keep_connection()
}

/// Has the connection that holds `lock` end, and waits for it to: Windows asks whether to
/// take the session over from a connection that is there still, and holds the new one up
/// until it is answered.
fn have_released(lock: &str) -> Result<(), String> {
    let failed = |err: std::io::Error| err.to_string();
    let share = share_dir();
    let held = std::fs::File::create(share.join(lock)).map_err(failed)?;
    let asked = Instant::now();
    while held.try_lock().is_err() {
        if asked.elapsed() > RELEASE_TIMEOUT {
            return Err("the other connection to Windows did not end".into());
        }
        std::fs::write(share.join(RELEASE), "").map_err(failed)?;
        std::thread::sleep(Duration::from_millis(200));
    }
    match std::fs::remove_file(share.join(RELEASE)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(failed(err)),
        _ => Ok(()),
    }
}

/// The whole Windows desktop in a window, in a connection of its own, which the programs'
/// gives way to and which gives way to theirs: both are to the one session of the account.
fn show_desktop() -> Result<(), String> {
    let failed = |err: std::io::Error| err.to_string();
    let share = share_dir();
    std::fs::create_dir_all(&share).map_err(failed)?;
    let lock = std::fs::File::create(share.join(DESKTOP_LOCK)).map_err(failed)?;
    if lock.try_lock().is_err() {
        return Ok(());
    }
    let (user, password) = account().ok_or("Windows is not installed")?;
    let running = podman(&[
        "container",
        "inspect",
        "--format",
        "{{.State.Running}}",
        CONTAINER,
    ])?;
    if running.trim() != "true" {
        podman(&["start", CONTAINER])?;
    }
    have_released(APPS_LOCK)?;
    let mut args = rdp_args(&user);
    args.extend([
        "/sound".into(),
        "+home-drive".into(),
        "+dynamic-resolution".into(),
        "/t:Windows".into(),
        // Known as Windows in the Dock and the menu bar, by the launcher of that name.
        format!("/wm-class:{UNNAMED_APP}"),
    ]);
    // Connecting is the way to ask whether Windows has started: after a connection that only
    // signs in, it holds the next one up for a minute. One that ends at once was not let in.
    let connect = || start_freerdp(&args, &password, false).map_err(|err| format!("xfreerdp3: {err}"));
    let (started, mut connected) = (Instant::now(), Instant::now());
    let mut connection = connect()?;
    loop {
        std::thread::sleep(Duration::from_millis(300));
        // A program is to be opened, in the session this connection is to.
        if share.join(RELEASE).exists() {
            let _ = connection.kill();
            let _ = connection.wait();
            return Ok(());
        }
        let Some(status) = connection.try_wait().map_err(failed)? else {
            continue;
        };
        if status.success() || connected.elapsed() > REFUSED_WITHIN {
            return Ok(());
        }
        if started.elapsed() > START_TIMEOUT {
            return Err(format!("xfreerdp3: {status}"));
        }
        std::thread::sleep(Duration::from_secs(2));
        connection = connect()?;
        connected = Instant::now();
    }
}

/// The Windows group, the group of its launchers, and the group that removes it.
struct MachineView {
    group: adw::PreferencesGroup,
    state: adw::ActionRow,
    spinner: gtk::Spinner,
    install: gtk::Button,
    viewer: gtk::Button,
    start: gtk::Button,
    desktop: gtk::Button,
    stop: gtk::Button,
    apps: adw::PreferencesGroup,
    added: adw::EntryRow,
    listed: RefCell<Vec<adw::ActionRow>>,
    /// The programs Windows has that are not yet Apps here, and the button that looks for them.
    found: adw::PreferencesGroup,
    find: gtk::Button,
    removal: adw::PreferencesGroup,
    remove: gtk::Button,
    /// What Settings is doing to the machine itself, shown in place of what it is.
    busy: Cell<Option<&'static str>>,
    /// Set while the machine is looked at, so that looks do not pile up.
    looking: Cell<bool>,
    /// The machine as last looked at, shown again when what Settings does to it changes.
    seen: RefCell<Machine>,
}

impl MachineView {
    fn new() -> Rc<Self> {
        let state = adw::ActionRow::builder().title("Windows 11 企業版評估版").build();
        let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
        let (install, viewer, start, desktop, stop) = (
            button("安裝"),
            button("顯示畫面"),
            button("啟動"),
            button("顯示畫面"),
            button("關機"),
        );
        state.add_suffix(&spinner);
        for suffix in [&install, &viewer, &start, &desktop, &stop] {
            suffix.set_visible(false);
            state.add_suffix(suffix);
        }
        let group = group("Windows 虛擬機");
        group.add(&state);
        // Both shown once the machine has been looked at.
        let apps = self::group("App");
        apps.set_visible(false);
        let added = adw::EntryRow::builder()
            .title("新增 App 的程式路徑")
            .show_apply_button(true)
            .build();
        apps.add(&added);
        let found = self::group("Windows 中的程式");
        found.set_visible(false);
        let find = button("搜尋");
        found.set_header_suffix(Some(&find));
        let removal = self::group("");
        removal.set_visible(false);
        let remove = button("移除");
        remove.add_css_class("destructive-action");
        let removing = adw::ActionRow::builder().title("移除 Windows").build();
        removing.add_suffix(&remove);
        removal.add(&removing);
        let view = Rc::new(Self {
            group,
            state,
            spinner,
            install,
            viewer,
            start,
            desktop,
            stop,
            apps,
            added,
            listed: RefCell::default(),
            found,
            find,
            removal,
            remove,
            busy: Cell::new(None),
            looking: Cell::new(false),
            seen: RefCell::new(Machine::Missing),
        });
        view.connect();
        view
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let on = move |then: fn(&Rc<Self>, &gtk::Button)| {
            let weak = weak.clone();
            move |button: &gtk::Button| {
                if let Some(view) = weak.upgrade() {
                    then(&view, button);
                }
            }
        };
        self.install.connect_clicked(on(|view, _| view.ask_to_install()));
        self.viewer.connect_clicked(on(|_, button| {
            let uri = format!("http://127.0.0.1:{VIEWER_PORT}/");
            if let Err(err) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
                alert::notice(button, "無法顯示安裝畫面", Some(&err.to_string()));
            }
        }));
        self.start.connect_clicked(on(|view, _| {
            view.act("正在啟動", "無法啟動 Windows", || {
                podman(&["start", CONTAINER]).map(drop)
            });
        }));
        self.stop.connect_clicked(on(|view, _| {
            view.act("正在關機", "無法關閉 Windows", || {
                podman(&["stop", "--time", "120", CONTAINER]).map(drop)
            });
        }));
        self.desktop
            .connect_clicked(on(|_, button| spawn_open(button, None)));
        self.find.connect_clicked(on(|view, _| view.find_apps()));
        self.remove.connect_clicked(on(|view, button| {
            let view = view.clone();
            alert::confirm(
                button,
                "移除 Windows？",
                Some("虛擬機、其中的檔案與已新增的 App 都會刪除。"),
                "移除",
                move || {
                    view.act("正在移除", "無法移除 Windows", remove_machine);
                },
            );
        }));
        let weak = Rc::downgrade(self);
        self.added.connect_apply(move |field| {
            let Some(view) = weak.upgrade() else { return };
            let program = field.text().trim().to_string();
            if !is_windows_path(&program) {
                return alert::notice(
                    field,
                    "無法新增 App",
                    Some("請輸入 Windows 中的完整路徑，例如 C:\\Windows\\notepad.exe。"),
                );
            }
            let (field, stem) = (field.clone(), program_stem(&program).to_string());
            alert::prompt(
                &field.clone(),
                "新增 App",
                &stem,
                -1,
                "新增",
                move |name| match add_launcher(&name, &program) {
                    Ok(()) => {
                        // Turned off meanwhile, as emptying the field would otherwise offer to apply it.
                        field.set_show_apply_button(false);
                        field.set_text("");
                        field.set_show_apply_button(true);
                        view.list_apps();
                    }
                    Err(why) => alert::notice(&field, "無法新增 App", Some(&why)),
                },
            );
        });
    }

    fn find_apps(self: &Rc<Self>) {
        self.act("正在搜尋程式", "無法搜尋程式", find_apps);
    }

    /// Asks before downloading Windows, once there is room for it.
    fn ask_to_install(self: &Rc<Self>) {
        let free = gio::File::for_path(glib::home_dir())
            .query_filesystem_info(gio::FILE_ATTRIBUTE_FILESYSTEM_FREE, gio::Cancellable::NONE)
            .map(|info| info.attribute_uint64(gio::FILE_ATTRIBUTE_FILESYSTEM_FREE));
        match free {
            Ok(free) if free < NEEDED_GB * GB => {
                let why = format!(
                    "安裝 Windows 需要 {NEEDED_GB} GB 可用空間，目前剩 {} GB。",
                    free / GB
                );
                return alert::notice(&self.state, "磁碟空間不足", Some(&why));
            }
            Ok(_) => {}
            Err(err) => eprintln!("free space: {err}"),
        }
        let alert = adw::AlertDialog::new(
            Some("安裝 Windows 11 企業版評估版？"),
            Some("從 Microsoft 下載 6.6 GB 並自動安裝，需要 30 分鐘。\n評估版可使用 90 天。"),
        );
        alert.add_responses(&[("cancel", "取消"), ("install", "安裝")]);
        alert.set_response_appearance("install", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("install"));
        alert.set_close_response("cancel");
        let view = self.clone();
        alert.choose(&self.state, gio::Cancellable::NONE, move |response| {
            if response == "install" {
                view.set_up();
            }
        });
    }

    /// Installs what the machine needs, then makes it.
    fn set_up(self: &Rc<Self>) {
        let missing: Vec<&'static str> = MACHINE_PROGRAMS
            .iter()
            .filter(|(program, _)| glib::find_program_in_path(program).is_none())
            .map(|(_, package)| *package)
            .collect();
        let create = |view: &Rc<Self>| view.act("正在下載", "無法建立 Windows 虛擬機", create_machine);
        if missing.is_empty() {
            return create(self);
        }
        self.set_busy(Some("正在安裝所需套件"));
        self.refresh();
        let view = self.clone();
        as_root(
            "安裝 Windows 虛擬機所需的套件",
            &apt_install(&missing),
            None,
            move |worked| {
                view.set_busy(None);
                if worked {
                    create(&view);
                } else {
                    view.refresh();
                    alert::notice(&view.state, "無法安裝所需套件", None);
                }
            },
        );
    }

    /// Does `work` away from the main thread, saying `doing` meanwhile, and `failed` with why
    /// when it fails.
    fn act(
        self: &Rc<Self>,
        doing: &'static str,
        failed: &'static str,
        work: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) {
        self.set_busy(Some(doing));
        self.refresh();
        let view = self.clone();
        glib::spawn_future_local(async move {
            let done = gio::spawn_blocking(work)
                .await
                .unwrap_or_else(|_| Err(String::new()));
            view.set_busy(None);
            view.list_apps();
            view.refresh();
            if let Err(why) = done {
                alert::notice(
                    &view.state,
                    failed,
                    Some(why.as_str()).filter(|why| !why.is_empty()),
                );
            }
        });
    }

    /// Looks at the machine away from the main thread, and shows it.
    fn refresh(self: &Rc<Self>) {
        if self.looking.replace(true) {
            return;
        }
        let view = self.clone();
        glib::spawn_future_local(async move {
            let machine = gio::spawn_blocking(machine).await.unwrap_or(Machine::Missing);
            view.show(&machine);
            // Shown first, as finding out whether Windows is ready takes a while.
            if matches!(machine, Machine::Installing { booted: true, .. })
                && gio::spawn_blocking(finish_installing).await.unwrap_or(false)
            {
                view.show(&Machine::Running);
                view.find_apps();
            }
            view.looking.set(false);
        });
    }

    /// Says what Settings does to the machine at once, while a look at it may still be going on.
    fn set_busy(&self, doing: Option<&'static str>) {
        self.busy.set(doing);
        let seen = self.seen.borrow().clone();
        self.show(&seen);
    }

    fn show(&self, machine: &Machine) {
        *self.seen.borrow_mut() = machine.clone();
        let installed = installed_on();
        let busy = self.busy.get();
        let ends = installed.as_deref().and_then(evaluation_ends);
        let with_end = |state: &str| match &ends {
            Some(ends) if *ends < today() => format!("{state} · 評估期已過"),
            Some(ends) => format!("{state} · 評估期至 {ends}"),
            None => state.to_string(),
        };
        let subtitle = match (busy, machine) {
            (Some(doing), _) => doing.to_string(),
            (None, Machine::Unsupported) => "需要 KVM 硬體虛擬化".into(),
            (None, Machine::Missing) => "未安裝".into(),
            (None, Machine::Installing { stage, .. }) => stage.clone(),
            (None, Machine::Stopping) => "正在關機".into(),
            (None, Machine::Stopped) if installed.is_none() => "安裝未完成".into(),
            (None, Machine::Stopped) => with_end("已停止"),
            (None, Machine::Running) => with_end("執行中"),
        };
        self.state.set_subtitle(&subtitle);
        self.state.set_sensitive(*machine != Machine::Unsupported);
        let idle = busy.is_none();
        let installing = matches!(machine, Machine::Installing { .. });
        self.spinner
            .set_visible(!idle || installing || *machine == Machine::Stopping);
        self.install.set_visible(idle && *machine == Machine::Missing);
        self.viewer.set_visible(idle && installing);
        self.start.set_visible(idle && *machine == Machine::Stopped);
        self.desktop.set_visible(idle && *machine == Machine::Running);
        self.stop.set_visible(idle && *machine == Machine::Running);
        self.apps.set_visible(installed.is_some());
        self.found.set_visible(installed.is_some());
        self.find.set_sensitive(idle && *machine == Machine::Running);
        self.removal
            .set_visible(!matches!(machine, Machine::Missing | Machine::Unsupported));
        self.remove.set_sensitive(idle);
    }

    /// Lists the launchers above the field for another one, and the programs Windows has
    /// besides them, again after each change.
    fn list_apps(self: &Rc<Self>) {
        for row in self.listed.borrow_mut().drain(..) {
            // A row is in the group it was added to.
            if row.is_ancestor(&self.apps) {
                self.apps.remove(&row);
            } else {
                self.found.remove(&row);
            }
        }
        self.apps.remove(&self.added);
        let app_row = |name: &str, program: &str, icon: PathBuf| {
            let row = adw::ActionRow::builder()
                .title(name)
                .subtitle(program)
                .use_markup(false)
                .build();
            if icon.exists() {
                row.add_prefix(
                    &gtk::Image::builder()
                        .file(icon.to_string_lossy())
                        .pixel_size(32)
                        .build(),
                );
            }
            row
        };
        let launchers = launchers();
        for found in found_apps() {
            if launchers
                .iter()
                .any(|(_, _, program)| program.eq_ignore_ascii_case(&found.program))
            {
                continue;
            }
            let row = app_row(
                &found.name,
                &found.program,
                found_dir().join(format!("{}.png", found.number)),
            );
            let add = button("新增");
            let weak = Rc::downgrade(self);
            add.connect_clicked(move |button| {
                if let Err(why) = add_launcher(&found.name, &found.program) {
                    return alert::notice(button, "無法新增 App", Some(&why));
                }
                if let Some(view) = weak.upgrade() {
                    view.list_apps();
                }
            });
            row.add_suffix(&add);
            self.found.add(&row);
            self.listed.borrow_mut().push(row);
        }
        for (id, name, program) in launchers {
            let row = app_row(&name, &program, icon_path(&id));
            let open = button("開啟");
            let opened = id.clone();
            open.connect_clicked(move |button| spawn_open(button, Some(&opened)));
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("移除")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |button| {
                if let Err(err) = remove_launcher(&id) {
                    return alert::notice(button, "無法移除 App", Some(&err.to_string()));
                }
                if let Some(view) = weak.upgrade() {
                    view.list_apps();
                }
            });
            row.add_suffix(&open);
            row.add_suffix(&remove);
            self.apps.add(&row);
            self.listed.borrow_mut().push(row);
        }
        self.apps.add(&self.added);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_evaluation_ends_ninety_days_after_installing() {
        assert_eq!(evaluation_ends("2026-11-21").as_deref(), Some("2027-02-19"));
        assert_eq!(evaluation_ends("not a day"), None);
    }

    #[test]
    fn launchers_are_named_after_their_programs() {
        let none_taken = |_: &str| false;
        assert_eq!(
            launcher_id(r"C:\Windows\notepad.exe", none_taken),
            "nekoland-windows-notepad"
        );
        assert_eq!(
            launcher_id(r"C:\Program Files\Microsoft Office\WINWORD.EXE", none_taken),
            "nekoland-windows-winword"
        );
        assert_eq!(
            launcher_id(r"C:\Apps\記事本.exe", none_taken),
            "nekoland-windows-app"
        );
        let first_taken = |id: &str| id == "nekoland-windows-notepad";
        assert_eq!(
            launcher_id(r"D:\notepad.exe", first_taken),
            "nekoland-windows-notepad-2"
        );
    }

    #[test]
    fn windows_programs_are_listed_by_name() {
        let list = "\u{feff}WordPad\tC:\\Program Files\\wordpad.exe\t2\t.rtf .wri\nEdge\tC:\\Edge\\msedge.exe\t1\t\nnot a line\n";
        let found = parse_found(list);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "Edge");
        assert!(found[0].extensions.is_empty());
        assert_eq!(found[1].program, r"C:\Program Files\wordpad.exe");
        assert_eq!(found[1].extensions, [".rtf", ".wri"]);
    }

    #[test]
    fn a_bottle_is_named_as_one_folder_and_owns_its_launchers() {
        let taken = vec![("預設".to_string(), PathBuf::from("/home/neko/.wine"))];
        assert!(is_bottle_name("遊戲", &taken));
        for name in ["", "預設", "a/b", "..", ".hidden"] {
            assert!(!is_bottle_name(name, &taken), "{name}");
        }
        let launcher =
            "Exec=env WINEPREFIX=\"/home/neko/.local/share/nekoland/wine/遊戲\" wine C:\\\\a.lnk\n";
        assert!(is_launcher_of(
            launcher,
            Path::new("/home/neko/.local/share/nekoland/wine/遊戲")
        ));
        assert!(!is_launcher_of(
            launcher,
            Path::new("/home/neko/.local/share/nekoland/wine/遊")
        ));
    }

    #[test]
    fn the_wine_downloaded_needs_no_32_bit_libraries() {
        let release = r#"{"assets": [
            {"browser_download_url": "https://example.org/d/11.22/sha256sums.txt"},
            {"browser_download_url": "https://example.org/d/11.22/wine-11.22-amd64-wow64.tar.xz"},
            {"browser_download_url": "https://example.org/d/11.22/wine-11.22-staging-amd64.tar.xz"},
            {"browser_download_url": "https://example.org/d/11.22/wine-11.22-amd64.tar.xz"},
            {"browser_download_url": "https://example.org/d/11.22/wine-11.22-x86.tar.xz"}
        ]}"#;
        assert_eq!(
            runner_url(release).as_deref(),
            Some("https://example.org/d/11.22/wine-11.22-amd64-wow64.tar.xz")
        );
        assert_eq!(runner_url(r#"{"assets": []}"#), None);
    }

    #[test]
    fn a_file_has_a_name_in_windows() {
        assert_eq!(
            windows_path(Path::new("/home/neko/文件/a b, c.docx")).as_deref(),
            Some(r"\\tsclient\root\home\neko\文件\a b, c.docx")
        );
    }

    #[test]
    fn a_dark_bottle_goes_without_a_theme_in_the_colours_given() {
        let dark = look_registry(Some(DARK_NEKOLAND));
        assert!(dark.contains("\"AppsUseLightTheme\"=dword:00000000\n"));
        assert!(dark.contains("\"ThemeActive\"=\"0\"\n"));
        assert!(dark.contains("[HKEY_CURRENT_USER\\Control Panel\\Colors]\n"));
        assert!(dark.contains("\"Window\"=\"34 34 38\"\n"));
        let light = look_registry(None);
        assert!(light.contains("\"ThemeActive\"=\"1\"\n"));
        assert!(!light.contains("Colors"));
    }

    #[test]
    fn a_wine_launcher_names_its_program_bottle_and_shortcut() {
        let launcher = "[Desktop Entry]\nName=7-Zip File Manager\nExec=env WINEPREFIX=\"/home/neko/.wine\" wine-stable C:\\\\ProgramData\\\\Start\\ Menu\\\\7-Zip\\ File\\ Manager.lnk\nStartupWMClass=7zfm.exe\n";
        assert_eq!(
            wine_program(launcher),
            Some((
                "7-Zip File Manager".to_string(),
                PathBuf::from("/home/neko/.wine"),
                r"C:\ProgramData\Start Menu\7-Zip File Manager.lnk".to_string()
            ))
        );
        assert_eq!(wine_program(&launcher.replace("7zfm.exe", "7-zip.chm")), None);
    }

    #[test]
    fn a_files_items_go_by_its_extension() {
        assert_eq!(menu_name(Path::new("/nowhere/Report.DOCX")), ".docx");
        assert_eq!(menu_name(Path::new("/nowhere/Makefile")), "file");
        assert_eq!(menu_name(Path::new("/")), "folder");
    }

    #[test]
    fn a_window_is_known_by_its_programs_launcher() {
        let shown = parse_shown(
            "0003012A\tC:\\Windows\\regedit.exe\t31\t800\t600\n000A0001\tC:\\Apps\\Other.exe\t0\t461\t259\nnot a line\n",
        );
        assert_eq!(shown.len(), 2);
        let launchers = vec![(
            "nekoland-windows-regedit".to_string(),
            "登錄編輯程式".to_string(),
            r"c:\windows\REGEDIT.EXE".to_string(),
        )];
        assert_eq!(
            identity(&shown[0], &launchers),
            (
                "nekoland-window-31-800-600".to_string(),
                "nekoland-windows-regedit".to_string()
            )
        );
        assert_eq!(
            identity(&shown[1], &launchers),
            (
                "nekoland-window-0-461-259".to_string(),
                "nekoland-windows".to_string()
            )
        );
    }

    #[test]
    fn only_whole_windows_paths_make_launchers() {
        assert!(is_windows_path(r"C:\Windows\notepad.exe"));
        assert!(!is_windows_path("notepad.exe"));
        assert!(!is_windows_path("/home/neko/a.exe"));
        let store = r"shell:AppsFolder\Microsoft.WindowsStore_8wekyb3d8bbwe!App";
        assert!(is_windows_path(store));
        assert_eq!(program_stem(store), "WindowsStore");
    }

    #[test]
    fn the_installation_says_how_far_it_has_got() {
        assert_eq!(install_stage(""), ("正在準備".into(), false));
        assert_eq!(
            install_stage(
                "❯ Downloading Windows 11 Enterprise...\n<p class=\"loading\">Downloading Windows 11 Enterprise (26.8%)</p>"
            ),
            ("正在下載 27%".into(), false)
        );
        assert_eq!(
            install_stage("❯ Downloading Windows 11 Enterprise...\n❯ Booting Windows using QEMU v11.0.0..."),
            ("正在安裝".into(), true)
        );
    }
}
