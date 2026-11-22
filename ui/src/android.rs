//! Settings' Android Apps page: Android's apps through Waydroid, which runs LineageOS in a
//! container, with Google Play. The page installs and sets Waydroid up, registers it with
//! Google Play, which signs in on a registered device only, and lists the apps installed,
//! which Waydroid itself gives launchers. Apple Music, which has neither a Linux program nor a
//! Mac one Darling runs, is got from there.

use std::{cell::RefCell, ffi::OsStr, rc::Rc};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{
    alert, as_root, group, output, sudo,
    windows::{apt_install, button, entries, entry_path},
};

/// What Waydroid names an app's launcher: this, then the app's package. It names the app's
/// windows the same.
const APP_PREFIX: &str = "waydroid.";
const APPLE_MUSIC: &str = "com.apple.android.music";
/// There once Waydroid is set up; what it holds is read by anyone.
const WAYDROID_CONFIG: &str = "/var/lib/waydroid/waydroid.cfg";
/// What `SETUP` adds to Android as it starts.
const ANDROID_INIT: &str = "/var/lib/waydroid/overlay/system/etc/init/nekoland.rc";
/// Android's properties for an app to be as Nekoland's own: in a window of its own, in
/// Chinese, and running with no window open, as a program does, for what it plays and
/// notifies; Waydroid otherwise stops all of Android then. Android keeps a language chosen in
/// it over this one.
const PROPERTIES: [&str; 3] = [
    "persist.waydroid.multi_windows = true",
    "persist.sys.locale = zh-TW",
    "persist.waydroid.suspend = false",
];
/// Waydroid shares the clipboard through pyclip, which Ubuntu has no package of; this one
/// does it with wl-clipboard.
const PYCLIP: &str = include_str!("android/pyclip.py");
/// Sets Waydroid up, as root, with Google Play and `PROPERTIES`, and with pyclip from its
/// input. The properties are a list that one said twice breaks; Android takes them as it
/// starts again. As it starts, Android is also to leave window shadows to its compositor,
/// or each window's surface is larger by room for one, which Waydroid's has no part in; to
/// leave windows' opening and closing to it too, as what Android animates them by is no
/// part of any; to show no on-screen keyboard while there is a keyboard, which Waydroid's
/// always is, and an input method only what it shows by the text; and its dark mode is to
/// follow the property Nekoland sets, which needs no root.
const SETUP: &str = r#"set -e
config=/var/lib/waydroid/waydroid.cfg
[ -e "$config" ] || waydroid init -s GAPPS
for property in "$@"; do
    grep -q "^${property%% *} " "$config" || sed -i "/^\[properties\]/a $property" "$config"
done
mkdir -p /var/lib/waydroid/overlay/system/etc/init
cat > /var/lib/waydroid/overlay/system/etc/init/nekoland.rc <<'END'
on property:sys.boot_completed=1
    exec_background - root root -- /system/bin/settings put global render_shadows_in_compositor 1
    exec_background - root root -- /system/bin/settings put global window_animation_scale 0
    exec_background - root root -- /system/bin/settings put global transition_animation_scale 0
    exec_background - root root -- /system/bin/settings put secure show_ime_with_hard_keyboard 0

on property:sys.boot_completed=1 && property:persist.nekoland.night=*
    exec_background - root root -- /system/bin/cmd uimode night ${persist.nekoland.night}

on property:persist.nekoland.night=*
    exec_background - root root -- /system/bin/cmd uimode night ${persist.nekoland.night}
END
site=$(/usr/bin/python3 -c 'import site; print(site.getsitepackages()[0])')
mkdir -p "$site"
cat > "$site/pyclip.py"
waydroid upgrade --offline"#;
/// Where a device Google Play did not come with is registered, for Play to be signed in on.
const PLAY_REGISTRATION: &str = "https://www.google.com/android/uncertified";
/// Prints the ID Google's services in Waydroid go by, from their own database.
const PLAY_DEVICE_ID: &str = "import sqlite3, sys\n\
    found = sqlite3.connect('file:' + sys.argv[1] + '?mode=ro', uri=True)\n\
    print(found.execute(\"select value from main where name = 'android_id'\").fetchone()[0])";

pub(crate) fn android_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let apps = group("App");
    page.add(&waydroid_group(&apps));
    page.add(&apps);
    page
}

/// What `SETUP` writes to `ANDROID_INIT`.
fn android_init() -> &'static str {
    let start = SETUP.find("<<'END'\n").map_or(0, |at| at + "<<'END'\n".len());
    let end = SETUP[start..]
        .find("\nEND\n")
        .map_or(SETUP.len(), |at| start + at + 1);
    &SETUP[start..end]
}

/// Whether Waydroid is set up as `SETUP` sets it up.
fn is_set_up() -> bool {
    let Ok(config) = std::fs::read_to_string(WAYDROID_CONFIG) else {
        return false;
    };
    let has = |property: &str| {
        let key = property.split(' ').next().unwrap_or(property);
        config.lines().any(|line| line.starts_with(&format!("{key} ")))
    };
    PROPERTIES.iter().all(|property| has(property))
        // Set up by an earlier Nekoland, it is set up again with what this one adds.
        && std::fs::read_to_string(ANDROID_INIT).is_ok_and(|init| init == android_init())
        && output("/usr/bin/python3", &["-c", "import pyclip"]).is_some()
}

/// Starts what Waydroid is asked, left to run: the first it is asked starts Android, and
/// keeps it.
fn start_waydroid(args: &[&str]) -> Result<(), String> {
    let argv: Vec<&OsStr> = std::iter::once("waydroid")
        .chain(args.iter().copied())
        .map(OsStr::new)
        .collect();
    gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE)
        .spawn(&argv)
        .map(drop)
        .map_err(|err| err.to_string())
}

/// Reads the ID Google Play knows Waydroid's Android by, as root, its database being Android's.
fn read_play_device_id(then: impl FnOnce(Result<String, String>) + 'static) {
    let database =
        glib::user_data_dir().join("waydroid/data/data/com.google.android.gsf/databases/gservices.db");
    let database = database.to_string_lossy();
    let args = ["/usr/bin/python3", "-I", "-c", PLAY_DEVICE_ID, &database];
    let flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE;
    let child = match sudo("註冊裝置", &args, flags) {
        Ok(child) => child,
        Err(err) => return then(Err(err.to_string())),
    };
    let waited = child.clone();
    child.communicate_utf8_async(None, gio::Cancellable::NONE, move |result| {
        then(match result {
            Ok((said, _)) if waited.is_successful() => Ok(said.unwrap_or_default().trim().to_string()),
            Ok((_, why)) => Err(why
                .unwrap_or_default()
                .lines()
                .last()
                .unwrap_or_default()
                .to_string()),
            Err(err) => Err(err.to_string()),
        });
    });
}

/// The apps installed in Android that are among the applications, by name.
fn installed_apps() -> Vec<gio::DesktopAppInfo> {
    entries(APP_PREFIX, "Exec")
        .into_iter()
        .filter_map(|(id, _, _)| gio::DesktopAppInfo::from_filename(entry_path(&id)))
        .filter(|app| app.should_show())
        .collect()
}

fn app_row(app: &gio::DesktopAppInfo) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(app.name())
        .use_markup(false)
        .build();
    if let Some(icon) = app.icon() {
        row.add_prefix(&gtk::Image::builder().gicon(&icon).pixel_size(32).build());
    }
    let open = button("開啟");
    let app = app.clone();
    open.connect_clicked(move |open| {
        if let Err(err) = app.launch(&[], gio::AppLaunchContext::NONE) {
            alert::notice(open, "無法執行程式", Some(&err.to_string()));
        }
    });
    row.add_suffix(&open);
    row
}

/// The Apple Music row while it is not installed: it opens its page in Google Play.
fn apple_music_row() -> adw::ActionRow {
    let row = adw::ActionRow::builder().title("Apple Music").build();
    let get = button("安裝");
    get.connect_clicked(|get| {
        let page = format!("market://details?id={APPLE_MUSIC}");
        if let Err(why) = start_waydroid(&["app", "intent", "android.intent.action.VIEW", &page]) {
            alert::notice(get, "無法執行程式", Some(&why));
        }
    });
    row.add_suffix(&get);
    row
}

/// Waydroid's state and Google Play's registration, above `apps`, which it fills while
/// Waydroid is set up.
fn waydroid_group(apps: &adw::PreferencesGroup) -> adw::PreferencesGroup {
    let waydroid = group("Waydroid");
    let state = adw::ActionRow::builder().title("版本").build();
    let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
    let install = button("安裝");
    state.add_suffix(&spinner);
    state.add_suffix(&install);
    let play = adw::ActionRow::builder().title("Google Play").build();
    let register = button("註冊");
    play.add_suffix(&register);
    waydroid.add(&state);
    waydroid.add(&play);

    let listed: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    let show = glib::clone!(
        #[weak]
        state,
        #[weak]
        install,
        #[weak]
        play,
        #[weak]
        apps,
        move || {
            let version = glib::find_program_in_path("waydroid")
                .and_then(|_| output("waydroid", &["--version"]))
                .map(|said| said.trim().to_string());
            let set_up = version.is_some() && is_set_up();
            state.set_subtitle(match (&version, set_up) {
                (None, _) => "未安裝",
                (Some(_), false) => "未設定",
                (Some(version), true) => version,
            });
            install.set_label(if version.is_some() { "設定" } else { "安裝" });
            install.set_visible(!set_up);
            play.set_visible(set_up);
            apps.set_visible(set_up);
            for row in listed.borrow_mut().drain(..) {
                apps.remove(&row);
            }
            if !set_up {
                return;
            }
            let installed = installed_apps();
            let rows = installed.iter().map(app_row);
            let has_music = installed.iter().any(|app| {
                app.id()
                    .is_some_and(|id| id == format!("{APP_PREFIX}{APPLE_MUSIC}.desktop"))
            });
            let music = (!has_music).then(apple_music_row);
            for row in music.into_iter().chain(rows) {
                apps.add(&row);
                listed.borrow_mut().push(row);
            }
        }
    );
    show();
    // Apps come and go in Android, which the page has no word of: it looks again when shown.
    waydroid.connect_map(glib::clone!(
        #[strong]
        show,
        move |_| show()
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
            let done = move |worked: bool| {
                spinner.set_visible(false);
                show();
                if !worked {
                    alert::notice(&state, "無法安裝 Waydroid", None);
                }
            };
            let set_up = {
                let done = done.clone();
                move || {
                    let args: Vec<&str> = ["sh", "-c", SETUP, "sh"].into_iter().chain(PROPERTIES).collect();
                    as_root("安裝 Waydroid", &args, Some(PYCLIP.to_string()), done)
                }
            };
            if glib::find_program_in_path("waydroid").is_some() {
                set_up();
            } else {
                as_root(
                    "安裝 Waydroid",
                    &apt_install(&["waydroid"]),
                    None,
                    move |worked| {
                        if worked {
                            set_up()
                        } else {
                            done(false)
                        }
                    },
                );
            }
        }
    ));
    // Its ID goes on the clipboard for the page that registers it.
    register.connect_clicked(glib::clone!(
        #[weak]
        play,
        move |register| {
            register.set_sensitive(false);
            let register = register.clone();
            read_play_device_id(move |id| {
                register.set_sensitive(true);
                match id {
                    Ok(id) => {
                        play.set_subtitle(&id);
                        play.clipboard().set_text(&id);
                        if let Err(err) = gio::AppInfo::launch_default_for_uri(
                            PLAY_REGISTRATION,
                            gio::AppLaunchContext::NONE,
                        ) {
                            alert::notice(&play, "無法開啟", Some(&err.to_string()));
                        }
                    }
                    Err(why) => alert::notice(&play, "無法讀取裝置 ID", Some(&why)),
                }
            });
        }
    ));
    waydroid
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the setup writes to Android's init is what an install set up is checked by.
    #[test]
    fn android_init_is_the_setups() {
        assert!(android_init().starts_with("on property:sys.boot_completed=1\n"));
        assert!(android_init().ends_with("${persist.nekoland.night}\n"));
        assert!(SETUP.contains(&format!("<<'END'\n{}END\n", android_init())));
    }

    /// The setup adds each property once, however often it runs, and leaves the rest be.
    #[test]
    fn setup_says_each_property_once() {
        let folder = std::env::temp_dir().join(format!("nekoland-waydroid-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let config = folder.join("waydroid.cfg");
        std::fs::write(
            &config,
            "[waydroid]\narch = x86_64\n\n[properties]\npersist.sys.locale = en-US\n",
        )
        .unwrap();
        // Only the part that writes properties, against the file here.
        let script = SETUP
            .lines()
            .take_while(|line| !line.starts_with("mkdir"))
            .filter(|line| !line.contains("waydroid "))
            .collect::<Vec<_>>()
            .join("\n")
            .replace("/var/lib/waydroid/waydroid.cfg", &config.to_string_lossy());
        for _ in 0..2 {
            let ran = std::process::Command::new("sh")
                .args(["-c", &script, "sh"])
                .args(PROPERTIES)
                .status()
                .unwrap();
            assert!(ran.success());
        }
        let written = std::fs::read_to_string(&config).unwrap();
        assert_eq!(
            written.matches("persist.waydroid.multi_windows = true").count(),
            1
        );
        assert_eq!(written.matches("persist.sys.locale").count(), 1);
        assert!(written.contains("persist.sys.locale = en-US"));
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
