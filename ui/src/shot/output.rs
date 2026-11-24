//! Where a screenshot goes: the clipboard, a file named by the template, the history it can
//! be opened again from, a notice with its picture, an S3 bucket, and what text and QR codes
//! are read out of it. The preferences it follows are in prefs.rs.

use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use gtk::{gio, glib, prelude::*};
use serde::{Deserialize, Serialize};

use super::img::Img;
use super::marks::{Backdrop, Doc, Mark};

pub use super::prefs::{After, Prefs, S3};

/// A template's fields filled in with the time now: {年} {月} {日} {時} {分} {秒}.
pub fn file_name(template: &str, format: &str) -> String {
    let now = glib::DateTime::now_local().expect("the local time");
    let fields = [
        ("{年}", now.format("%Y")),
        ("{月}", now.format("%m")),
        ("{日}", now.format("%d")),
        ("{時}", now.format("%H")),
        ("{分}", now.format("%M")),
        ("{秒}", now.format("%S")),
    ];
    let mut name = template.to_string();
    for (field, value) in fields {
        name = name.replace(field, value.map(|v| v.to_string()).as_deref().unwrap_or(""));
    }
    let name = name.replace('/', "-");
    format!(
        "{}.{format}",
        if name.trim().is_empty() {
            "截圖"
        } else {
            name.trim()
        }
    )
}

/// A path in the folder that names no file yet: the name, then the name with (2), (3)...
pub fn free_path(folder: &Path, name: &str) -> PathBuf {
    let path = folder.join(name);
    if !path.exists() {
        return path;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .expect("a free name")
}

/// What is saved and copied: drawn at its size in points when the preferences say so.
pub fn export(doc: &Doc, prefs: &Prefs) -> Img {
    let flat = doc.flatten(false);
    if prefs.downscale && doc.scale > 1.0 {
        flat.scaled(
            (flat.w as f64 / doc.scale).round() as i32,
            (flat.h as f64 / doc.scale).round() as i32,
        )
    } else {
        flat
    }
}

/// Saves it into the folder under the template's name, and returns where.
pub fn save(img: &Img, prefs: &Prefs) -> Result<PathBuf, String> {
    let folder = prefs.folder();
    std::fs::create_dir_all(&folder).map_err(|err| format!("{}: {err}", folder.display()))?;
    let path = free_path(&folder, &file_name(&prefs.template, &prefs.format));
    save_to(img, &path)?;
    Ok(path)
}

pub fn save_to(img: &Img, path: &Path) -> Result<(), String> {
    let format = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let format = if format == "jpeg" {
        "jpg".to_string()
    } else {
        format
    };
    let bytes = img.encode(&format)?;
    std::fs::write(path, bytes).map_err(|err| format!("{}: {err}", path.display()))
}

/// Puts a picture on the clipboard, through wl-copy, which keeps it there after 截圖 ends.
pub fn copy_image(img: &Img) -> Result<(), String> {
    pipe("wl-copy", &["--type", "image/png"], &img.png_fast())
}

pub fn copy_text(text: &str) -> Result<(), String> {
    pipe("wl-copy", &[], text.as_bytes())
}

fn pipe(program: &str, args: &[&str], input: &[u8]) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|err| format!("{program}: {err}"))?;
    child
        .stdin
        .take()
        .expect("its input")
        .write_all(input)
        .map_err(|err| format!("{program}: {err}"))?;
    let status = child.wait().map_err(|err| format!("{program}: {err}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{program} ended with {status}"))
}

/// The picture on the clipboard, if there is one.
pub fn clipboard_image() -> Option<Img> {
    let types = Command::new("wl-paste").arg("--list-types").output().ok()?;
    let types = String::from_utf8_lossy(&types.stdout);
    let kind = types.lines().find(|t| t.starts_with("image/"))?.to_string();
    let out = Command::new("wl-paste")
        .args(["--no-newline", "--type", &kind])
        .output()
        .ok()?;
    Img::from_bytes(out.stdout).ok()
}

/// Where the screenshots taken are kept, each in a folder of its own.
pub fn history_dir() -> PathBuf {
    glib::user_data_dir().join("nekoland/shots")
}

/// How many screenshots the history keeps.
const HISTORY: usize = 40;

/// What of a screenshot is kept beside its picture: its marks, to be changed again.
#[derive(Serialize, Deserialize)]
struct Saved {
    marks: Vec<Mark>,
    backdrop: Option<Backdrop>,
    scale: f64,
    step_start: i32,
}

/// Starts a screenshot's entry in the history, with the picture as it was taken.
pub fn new_entry(img: &Img) -> PathBuf {
    let now = glib::DateTime::now_local().expect("the local time");
    let id = now
        .format("%Y%m%d-%H%M%S-%f")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let dir = history_dir().join(id);
    if let Err(err) =
        std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("base.png"), img.png_fast()))
    {
        eprintln!("{}: {err}", dir.display());
    }
    prune_history();
    dir
}

fn prune_history() {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(history_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    entries.sort();
    while entries.len() > HISTORY {
        let old = entries.remove(0);
        if let Err(err) = std::fs::remove_dir_all(&old) {
            eprintln!("{}: {err}", old.display());
        }
    }
}

/// Keeps a screenshot's marks and picture in its entry, so that it opens again as it is.
pub fn keep(entry: &Path, doc: &Doc) {
    let saved = Saved {
        marks: doc.marks.clone(),
        backdrop: doc.backdrop,
        scale: doc.scale,
        step_start: doc.step_start,
    };
    let write = || -> std::io::Result<()> {
        std::fs::write(entry.join("base.png"), doc.base.png_fast())?;
        for (name, picture) in &doc.pictures {
            std::fs::write(entry.join(format!("picture-{name}.png")), picture.png_fast())?;
        }
        let json = serde_json::to_string(&saved).map_err(std::io::Error::other)?;
        std::fs::write(entry.join("doc.json"), json)
    };
    if let Err(err) = write() {
        eprintln!("{}: {err}", entry.display());
    }
}

pub fn restore(entry: &Path) -> Result<Doc, String> {
    let base = Img::load(&entry.join("base.png"))?;
    let saved: Option<Saved> = std::fs::read_to_string(entry.join("doc.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let Some(saved) = saved else {
        return Ok(Doc::new(base, 1.0));
    };
    let mut pictures = HashMap::new();
    for mark in &saved.marks {
        if mark.kind == super::marks::Kind::Picture {
            if let Ok(picture) = Img::load(&entry.join(format!("picture-{}.png", mark.text))) {
                pictures.insert(mark.text.clone(), std::rc::Rc::new(picture));
            }
        }
    }
    Ok(Doc {
        base: std::rc::Rc::new(base),
        marks: saved.marks,
        backdrop: saved.backdrop,
        scale: saved.scale,
        step_start: saved.step_start,
        pictures,
    })
}

/// The history's entries, the latest first, with when each was taken.
pub fn history() -> Vec<(PathBuf, String)> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(history_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("base.png").exists())
        .collect();
    entries.sort();
    entries.reverse();
    entries
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            // 20261120-104231-...: the day and the time.
            let when = if name.len() >= 15 {
                format!(
                    "{}/{} {}:{}",
                    &name[4..6],
                    &name[6..8],
                    &name[9..11],
                    &name[11..13]
                )
            } else {
                name
            };
            (path, when)
        })
        .collect()
}

/// Where the last area taken is kept, for 重複上次區域.
fn last_area_path() -> PathBuf {
    glib::home_dir().join(".local/state/nekoland/shot-area")
}

pub fn remember_area(area: (f64, f64, f64, f64)) {
    let path = last_area_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("{} {} {} {}", area.0, area.1, area.2, area.3));
}

pub fn last_area() -> Option<(f64, f64, f64, f64)> {
    let text = std::fs::read_to_string(last_area_path()).ok()?;
    let v: Vec<f64> = text.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    (v.len() == 4).then(|| (v[0], v[1], v[2], v[3]))
}

/// Shows a notice; `image` shows a picture in it. With `on_click`, the process stays until
/// the notice is clicked, which calls it, or closed.
pub fn notify(summary: &str, body: &str, image: Option<&Img>, timeout: i32, on_click: Option<Box<dyn Fn()>>) {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return;
    };
    let mut hints: HashMap<String, glib::Variant> = HashMap::new();
    if let Some(image) = image {
        // A small copy, sent whole: the notice draws it at the size of an icon.
        let k = (128.0 / image.w.max(image.h) as f64).min(1.0);
        let thumb = image.scaled(
            (image.w as f64 * k).round() as i32,
            (image.h as f64 * k).round() as i32,
        );
        let data = (
            thumb.w,
            thumb.h,
            thumb.w * 4,
            true,
            8i32,
            4i32,
            thumb.straight_rgba(),
        )
            .to_variant();
        hints.insert("image-data".into(), data);
    }
    let actions: Vec<String> = if on_click.is_some() {
        vec!["default".into(), "標記".into()]
    } else {
        Vec::new()
    };
    let args = ("截圖", 0u32, "", summary, body, actions, hints, timeout).to_variant();
    let reply = bus.call_sync(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
    );
    let id = match reply {
        Ok(reply) => reply.get::<(u32,)>().map(|(id,)| id).unwrap_or(0),
        Err(err) => return eprintln!("Notify: {err}"),
    };
    let Some(on_click) = on_click else {
        return;
    };
    let Some(app) = gio::Application::default() else {
        return;
    };
    let hold = std::rc::Rc::new(std::cell::RefCell::new(Some(app.hold())));
    let release = hold.clone();
    let on_click = std::rc::Rc::new(on_click);
    let invoked = on_click.clone();
    let subscription = std::rc::Rc::new(std::cell::RefCell::new(None));
    let unsubscribe = subscription.clone();
    let subscribed = bus.signal_subscribe(
        Some("org.freedesktop.Notifications"),
        Some("org.freedesktop.Notifications"),
        None,
        Some("/org/freedesktop/Notifications"),
        None,
        gio::DBusSignalFlags::NONE,
        move |bus, _, _, _, signal, params| {
            let ours = params.child_value(0).get::<u32>() == Some(id);
            if !ours {
                return;
            }
            if signal == "ActionInvoked" {
                invoked();
            }
            if signal == "ActionInvoked" || signal == "NotificationClosed" {
                if let Some(subscription) = unsubscribe.borrow_mut().take() {
                    #[allow(deprecated)]
                    bus.signal_unsubscribe(subscription);
                }
                release.borrow_mut().take();
            }
        },
    );
    subscription.replace(Some(subscribed));
    let _ = hold;
}

/// Uploads a picture to the S3 bucket and returns the address it can be read at.
pub fn upload(path: &Path, s3: &S3) -> Result<String, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let key: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let endpoint = s3.endpoint.trim_end_matches('/');
    let url = format!("{endpoint}/{}/{key}", s3.bucket);
    let region = if s3.region.is_empty() {
        "us-east-1"
    } else {
        &s3.region
    };
    // The keys reach curl on its input, as a config file: arguments are readable by any user.
    let quote = |v: &str| v.replace('\\', "\\\\").replace('"', "\\\"");
    let config = format!("user = \"{}:{}\"\n", quote(&s3.access_key), quote(&s3.secret_key));
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "--fail",
            "-K",
            "-",
            "--aws-sigv4",
            &format!("aws:amz:{region}:s3"),
        ])
        .args(["-H", "x-amz-acl: public-read", "-H"])
        .arg(if key.ends_with(".jpg") {
            "Content-Type: image/jpeg"
        } else {
            "Content-Type: image/png"
        })
        .arg("-T")
        .arg(path)
        .arg(&url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("curl: {err}"))?;
    child
        .stdin
        .take()
        .expect("its input")
        .write_all(config.as_bytes())
        .map_err(|err| format!("curl: {err}"))?;
    let out = child.wait_with_output().map_err(|err| format!("curl: {err}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(if s3.public_url.is_empty() {
        url
    } else {
        format!("{}/{key}", s3.public_url.trim_end_matches('/'))
    })
}

/// The text read out of a picture, in the languages installed of traditional Chinese and
/// English, and the contents of the QR codes in it.
pub fn recognize(path: &Path) -> Result<(String, Vec<String>), String> {
    let langs = Command::new("tesseract")
        .arg("--list-langs")
        .output()
        .map_err(|_| "未安裝 tesseract-ocr".to_string())?;
    let installed = String::from_utf8_lossy(&langs.stdout).to_string();
    let wanted: Vec<&str> = ["chi_tra", "chi_tra_vert", "eng"]
        .into_iter()
        .filter(|lang| installed.lines().any(|line| line.trim() == *lang) && !lang.ends_with("vert"))
        .collect();
    let lang = if wanted.is_empty() {
        "eng".to_string()
    } else {
        wanted.join("+")
    };
    let out = Command::new("tesseract")
        .arg(path)
        .args(["-", "-l", &lang, "--psm", "6"])
        .output()
        .map_err(|err| format!("tesseract: {err}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = Command::new("zbarimg")
        .args(["-q", "--raw"])
        .arg(path)
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_string)
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok((tidy_text(&text), codes))
}

/// OCR's text without the spaces it puts between Chinese characters, and without blank lines
/// at its ends.
fn tidy_text(text: &str) -> String {
    let wide = |c: char| ('\u{2e80}'..='\u{9fff}').contains(&c) || ('\u{ff00}'..='\u{ffef}').contains(&c);
    text.lines()
        .map(|line| {
            let chars: Vec<char> = line.trim().chars().collect();
            let mut out = String::new();
            for (i, c) in chars.iter().enumerate() {
                if *c == ' ' && i > 0 && i + 1 < chars.len() && wide(chars[i - 1]) && wide(chars[i + 1]) {
                    continue;
                }
                out.push(*c);
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub fn scratch_dir() -> PathBuf {
    glib::user_cache_dir().join("nekoland/shot")
}

/// A scratch file for a picture handed to another program: one pinned, dragged out or read.
pub fn scratch(img: &Img, name: &str) -> Option<PathBuf> {
    let dir = scratch_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(name);
    std::fs::write(&path, img.png()).ok()?;
    Some(path)
}

/// Pins a picture to the screen: a window of its own that stays in front.
pub fn pin(img: &Img) {
    let stamp = glib::monotonic_time();
    if let Some(path) = scratch(img, &format!("pin-{stamp}.png")) {
        if let Err(err) = Command::new(std::env::current_exe().unwrap_or_else(|_| "nekoland-shot".into()))
            .arg("pin")
            .arg(path)
            .spawn()
        {
            eprintln!("pin: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_fills_in_the_time() {
        let name = file_name("截圖 {年}-{月}", "png");
        assert!(name.starts_with("截圖 20"));
        assert!(name.ends_with(".png"));
        assert!(!name.contains('{'));
    }

    #[test]
    fn ocr_text_loses_the_spaces_between_chinese_characters() {
        assert_eq!(
            tidy_text("  黃 思 穎 siying.h@aurora.io  \n"),
            "黃思穎 siying.h@aurora.io"
        );
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let dir = std::env::temp_dir().join(format!("shot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), b"").unwrap();
        assert_eq!(free_path(&dir, "a.png"), dir.join("a (2).png"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prefs_read_back_what_was_written() {
        let prefs = Prefs {
            after: After::Thumbnail,
            ..Prefs::default()
        };
        let text = toml::to_string(&prefs).unwrap();
        let back: Prefs = toml::from_str(&text).unwrap();
        assert_eq!(back.after, After::Thumbnail);
        assert_eq!(
            toml::from_str::<Prefs>("").unwrap().template,
            Prefs::default().template
        );
    }

    /// An upload to a stand-in for the bucket, which keeps what it was sent.
    #[test]
    #[ignore = "needs curl and python3"]
    fn an_upload_is_signed_and_put_in_the_bucket() {
        let dir = std::env::temp_dir().join(format!("shot-s3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("stub.py");
        std::fs::write(
            &stub,
            r#"import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_PUT(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        with open(sys.argv[2], "w") as f:
            f.write("\n".join([self.path, self.headers.get("Authorization", ""), str(len(body)), self.headers.get("Content-Type", "")]))
        self.send_response(200)
        self.end_headers()
    def log_message(self, *args):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
open(sys.argv[1], "w").write(str(s.server_port))
s.handle_request()
"#,
        )
        .unwrap();
        let (port_file, log) = (dir.join("port"), dir.join("log"));
        let mut server = Command::new("python3")
            .arg("-I")
            .arg(&stub)
            .arg(&port_file)
            .arg(&log)
            .spawn()
            .unwrap();
        let port = (0..50)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                std::fs::read_to_string(&port_file).ok().filter(|p| !p.is_empty())
            })
            .expect("the stand-in's port");
        let picture = dir.join("截圖 1.png");
        let png = Img::new(3, 2, [1, 2, 3, 255]).png();
        std::fs::write(&picture, &png).unwrap();
        let s3 = S3 {
            endpoint: format!("http://127.0.0.1:{port}/"),
            bucket: "shots".into(),
            access_key: "AKIDEXAMPLE".into(),
            // Quotes and backslashes reach curl's config intact.
            secret_key: r#"se"cr\et"#.into(),
            ..S3::default()
        };
        let url = upload(&picture, &s3).unwrap();
        server.wait().unwrap();
        let sent = std::fs::read_to_string(&log).unwrap();
        let sent: Vec<&str> = sent.lines().collect();
        assert_eq!(url, format!("http://127.0.0.1:{port}/shots/---1.png"));
        assert_eq!(sent[0], "/shots/---1.png");
        assert!(
            sent[1].starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"),
            "{}",
            sent[1]
        );
        assert_eq!(sent[2], png.len().to_string());
        assert_eq!(sent[3], "image/png");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
