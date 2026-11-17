//! The web browsers Market offers on its 瀏覽器 page and in the choice asked at login, each
//! from the places it can be installed from on this computer, in the order Market prefers.
//! A computer from install.sh has no Flatpak, so there they come from the Snap Store and apt.

use std::path::Path;

use gtk::{gio, prelude::*};

use super::sources::{self, App, Kind, Offer, Repository};

/// One place a browser comes from.
enum Source {
    /// A Flathub application ID.
    Flathub(&'static str),
    /// A snap's name.
    Snap(&'static str),
    /// A package of Ubuntu's archive, and the desktop entry it installs.
    Ubuntu(&'static str, &'static str),
    /// A vendor repository's file name, its package, and the desktop entry it installs.
    Vendor(&'static str, &'static str, &'static str),
}

struct Browser {
    /// Its AppStream ID, which Market's other pages know it by.
    key: &'static str,
    name: &'static str,
    developer: &'static str,
    summary: &'static str,
    icon_name: &'static str,
    sources: &'static [Source],
}

const BROWSERS: [Browser; 8] = [
    Browser {
        key: "org.mozilla.firefox",
        name: "Firefox",
        developer: "Mozilla",
        summary: "非營利組織維護的開源瀏覽器",
        icon_name: "firefox",
        sources: &[Source::Flathub("org.mozilla.firefox"), Source::Snap("firefox")],
    },
    Browser {
        key: "com.google.chrome",
        name: "Google Chrome",
        developer: "Google",
        summary: "同步 Google 帳號的書籤與密碼",
        icon_name: "google-chrome",
        sources: &[
            Source::Flathub("com.google.Chrome"),
            Source::Vendor("google-chrome", "google-chrome-stable", "google-chrome.desktop"),
        ],
    },
    Browser {
        key: "com.brave.browser",
        name: "Brave",
        developer: "Brave Software",
        summary: "預設阻擋廣告與追蹤器",
        icon_name: "brave-browser",
        sources: &[Source::Flathub("com.brave.Browser"), Source::Snap("brave")],
    },
    Browser {
        key: "com.vivaldi.vivaldi",
        name: "Vivaldi",
        developer: "Vivaldi Technologies",
        summary: "分頁、面板與快捷鍵都能自訂",
        icon_name: "vivaldi",
        sources: &[Source::Flathub("com.vivaldi.Vivaldi"), Source::Snap("vivaldi")],
    },
    Browser {
        key: "org.chromium.chromium",
        name: "Chromium",
        developer: "Chromium 專案",
        summary: "Chrome 的開源基礎，不含 Google 服務",
        icon_name: "chromium",
        sources: &[Source::Flathub("org.chromium.Chromium"), Source::Snap("chromium")],
    },
    Browser {
        key: "com.microsoft.edge",
        name: "Microsoft Edge",
        developer: "Microsoft",
        summary: "同步 Microsoft 帳號",
        icon_name: "microsoft-edge",
        sources: &[Source::Flathub("com.microsoft.Edge")],
    },
    Browser {
        key: "io.gitlab.librewolf-community",
        name: "LibreWolf",
        developer: "LibreWolf 社群",
        summary: "加強隱私設定、移除遙測的 Firefox",
        icon_name: "librewolf",
        sources: &[Source::Flathub("io.gitlab.librewolf-community")],
    },
    Browser {
        key: "org.gnome.epiphany",
        name: "GNOME Web",
        developer: "GNOME",
        summary: "輕巧，和 GNOME app 外觀一致",
        icon_name: "org.gnome.Epiphany",
        sources: &[
            Source::Flathub("org.gnome.Epiphany"),
            Source::Ubuntu("epiphany-browser", "org.gnome.Epiphany.desktop"),
        ],
    },
];

impl Source {
    fn kind(&self) -> Kind {
        match self {
            Source::Flathub(_) => Kind::Flatpak,
            Source::Snap(_) => Kind::Snap,
            Source::Ubuntu(..) | Source::Vendor(..) => Kind::Apt,
        }
    }

    fn id(&self) -> &'static str {
        match self {
            Source::Flathub(id) | Source::Snap(id) | Source::Ubuntu(id, _) | Source::Vendor(_, id, _) => id,
        }
    }

    fn vendor(&self) -> Option<&'static sources::Vendor> {
        let Source::Vendor(file, ..) = self else {
            return None;
        };
        sources::VENDORS.iter().find(|vendor| vendor.file == *file)
    }

    /// Whether this computer has what installing from here takes.
    fn reachable(&self) -> bool {
        match self {
            Source::Flathub(_) => has_program("flatpak"),
            Source::Snap(_) => Path::new(sources::SNAPD_SOCKET).exists(),
            Source::Ubuntu(..) => true,
            Source::Vendor(..) => self.vendor().is_some(),
        }
    }

    fn offer(&self, developer: &str, repos: &[Repository]) -> Option<Offer> {
        let offer = match self {
            Source::Flathub(id) => Offer {
                kind: Some(Kind::Flatpak),
                id: id.to_string(),
                origin: "Flathub".into(),
                via: "flatpak".into(),
                publisher: developer.to_string(),
                verified: true,
                ..Default::default()
            },
            Source::Snap(name) => Offer {
                kind: Some(Kind::Snap),
                id: name.to_string(),
                origin: "Snap Store".into(),
                via: "snap".into(),
                admin: true,
                ..Default::default()
            },
            Source::Ubuntu(package, _) => Offer {
                kind: Some(Kind::Apt),
                id: package.to_string(),
                origin: sources::UBUNTU.into(),
                via: "apt".into(),
                publisher: sources::UBUNTU.into(),
                admin: true,
                ..Default::default()
            },
            Source::Vendor(_, package, _) => {
                let vendor = self.vendor()?;
                sources::vendor_offer(vendor, package, !sources::vendor_added(vendor, repos))
            }
        };
        Some(offer)
    }

    /// The desktop entry installing from here gave, once it is installed.
    fn entry(&self) -> Option<gio::AppInfo> {
        match self {
            Source::Flathub(id) => desktop_entry(&format!("{id}.desktop")),
            Source::Ubuntu(_, desktop) | Source::Vendor(_, _, desktop) => desktop_entry(desktop),
            // A snap's entries are named after the snap and each of its apps.
            Source::Snap(name) => {
                let prefix = format!("{name}_");
                gio::AppInfo::all_for_type(crate::web::WEB_TYPES[0])
                    .into_iter()
                    .find(|app| app.id().is_some_and(|id| id.starts_with(&prefix)))
            }
        }
    }
}

fn desktop_entry(id: &str) -> Option<gio::AppInfo> {
    gio::DesktopAppInfo::new(id).map(|app| app.upcast())
}

fn has_program(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

/// The browsers this computer can install, each with the places it can come from here and
/// the one it came from marked installed.
pub fn available() -> Vec<App> {
    let repos = sources::repositories();
    BROWSERS
        .iter()
        .filter_map(|browser| {
            let offers: Vec<Offer> = browser
                .sources
                .iter()
                .filter(|source| source.reachable())
                .filter_map(|source| {
                    let mut offer = source.offer(browser.developer, &repos)?;
                    if source.entry().is_some() {
                        offer.installed = Some(String::new());
                        offer.repo = None;
                    }
                    Some(offer)
                })
                .collect();
            (!offers.is_empty()).then(|| App {
                key: sources::key_of(browser.key),
                name: browser.name.into(),
                summary: browser.summary.into(),
                developer: browser.developer.into(),
                icon_name: browser.icon_name.into(),
                icon_url: flathub_icon_url(browser),
                offers,
                ..Default::default()
            })
        })
        .collect()
}

/// Flathub's icon for a browser it has, which it serves to anyone without Flatpak.
fn flathub_icon_url(browser: &Browser) -> String {
    browser
        .sources
        .iter()
        .find_map(|source| match source {
            Source::Flathub(id) => Some(format!(
                "https://dl.flathub.org/repo/appstream/{}/icons/128x128/{id}.png",
                std::env::consts::ARCH
            )),
            _ => None,
        })
        .unwrap_or_default()
}

/// The desktop entry of a browser installed from `offer`, or None when the offer is not one
/// of the browsers here or is not installed yet.
pub fn entry_of(offer: &Offer) -> Option<gio::AppInfo> {
    source_of(offer).and_then(Source::entry)
}

/// Whether `offer` installs one of the browsers here.
pub fn is_browser(offer: &Offer) -> bool {
    source_of(offer).is_some()
}

fn source_of(offer: &Offer) -> Option<&'static Source> {
    BROWSERS
        .iter()
        .flat_map(|browser| browser.sources)
        .find(|source| Some(source.kind()) == offer.kind && source.id() == offer.id)
}
