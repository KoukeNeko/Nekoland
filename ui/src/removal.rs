//! Removing an installed app, as Market and Settings both ask it: whether its settings and
//! data go too, what the system keeps through the package manager, and what it left in the
//! home to the trash.

use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::alert;
use crate::sources::{self, App, Kind, Offer, Plan};

/// The program an installed offer gave the system, to open it with.
pub fn launcher(app: &App, offer: &Offer) -> Option<gio::AppInfo> {
    let wanted: Vec<String> = match offer.kind {
        Some(Kind::Flatpak) => vec![format!("{}.desktop", offer.id)],
        Some(Kind::Snap) => vec![
            format!("{}_{}.desktop", offer.id, offer.id),
            format!("{}_", offer.id),
        ],
        _ => vec![format!("{}.desktop", app.key), format!("{}.desktop", offer.id)],
    };
    let named = gio::AppInfo::all().into_iter().find(|info| {
        let id = info.id().map(|id| id.to_lowercase()).unwrap_or_default();
        wanted.iter().any(|want| {
            if want.ends_with('_') {
                id.starts_with(want)
            } else {
                id == want.to_lowercase()
            }
        })
    });
    // A package's desktop file is often named for its AppStream ID, not the package: the
    // one it installed is asked of dpkg.
    named.or_else(|| {
        if offer.kind != Some(Kind::Apt) {
            return None;
        }
        let listed = std::process::Command::new("dpkg-query").args(["-L", &offer.id]).output().ok()?;
        String::from_utf8_lossy(&listed.stdout)
            .lines()
            .filter(|path| path.ends_with(".desktop") && path.contains("/applications/"))
            .filter_map(gio::DesktopAppInfo::from_filename)
            .find(|info| info.should_show())
            .map(|info| info.upcast())
    })
}

/// Asks before removing `offer` of `app`, and whether its settings and data go with it; `then`
/// gets the commands that remove it and what to put in the trash once they have.
pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    app: &App,
    offer: &Offer,
    then: impl FnOnce(Plan, Vec<PathBuf>) + 'static,
) {
    let home = glib::home_dir();
    let leftovers = sources::leftovers(offer, &leftover_names(app, offer), &home);

    let everything = gtk::CheckButton::with_label("一併移除設定與資料");
    let extra = gtk::Box::new(gtk::Orientation::Vertical, 6);
    extra.append(&everything);
    if !leftovers.is_empty() {
        let shown: Vec<String> = leftovers
            .iter()
            .map(|path| match path.strip_prefix(&home) {
                Ok(inside) => format!("~/{}", inside.display()),
                Err(_) => path.display().to_string(),
            })
            .collect();
        let list = gtk::Label::builder()
            .label(shown.join("\n"))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label", "caption"])
            .visible(false)
            .build();
        everything.bind_property("active", &list, "visible").build();
        extra.append(&list);
    }
    let alert = adw::AlertDialog::new(Some(&format!("移除 {}？", app.name)), None);
    alert.set_extra_child(Some(&extra));
    alert.add_responses(&[("cancel", "取消"), ("do", "移除")]);
    alert.set_response_appearance("do", adw::ResponseAppearance::Destructive);
    alert.set_default_response(Some("cancel"));
    alert.set_close_response("cancel");
    let offer = offer.clone();
    alert.choose(parent, gio::Cancellable::NONE, move |response| {
        if response != "do" {
            return;
        }
        let everything = everything.is_active();
        then(sources::remove_plan(&offer, everything), if everything { leftovers } else { Vec::new() });
    });
}

/// The names an app's folders in the home can have: its package, its AppStream ID, and its
/// desktop file's ID and window class.
fn leftover_names(app: &App, offer: &Offer) -> Vec<String> {
    let mut names = vec![offer.id.clone(), app.key.trim_end_matches(".desktop").to_string()];
    if let Some(info) = launcher(app, offer) {
        if let Some(id) = info.id() {
            names.push(id.trim_end_matches(".desktop").to_string());
        }
        if let Some(class) = info.downcast_ref::<gio::DesktopAppInfo>().and_then(|info| info.startup_wm_class()) {
            names.push(class.to_string());
        }
    }
    names.retain(|name| !name.is_empty());
    names.sort();
    names.dedup();
    names
}

/// Moves what an app left behind to the trash, and says what could not be.
pub fn trash_leftovers(parent: &impl IsA<gtk::Widget>, leftovers: &[PathBuf]) {
    let failed: Vec<String> = leftovers
        .iter()
        // The package manager may have taken it already.
        .filter(|path| std::fs::symlink_metadata(path).is_ok())
        .filter_map(|path| {
            gio::File::for_path(path)
                .trash(gio::Cancellable::NONE)
                .err()
                .map(|err| format!("{}：{}", path.display(), err.message()))
        })
        .collect();
    if !failed.is_empty() {
        alert::notice(parent, "有些設定與資料無法移到垃圾桶", Some(&failed.join("\n")));
    }
}
