//! Wallpapers an image service makes from a description, as One UI's generated wallpapers
//! are: OpenAI's, Google's Gemini, Stability AI's with the person's own API key, or ChatGPT's
//! through the Codex CLI signed in with a ChatGPT account. The service and the keys are kept
//! in `~/.config/nekoland/image-services`, which only its owner may read. The same services
//! cut a wallpaper's subject out for the lock's depth effect.

use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};

use gtk::{gdk_pixbuf, gdk_pixbuf::prelude::*, glib};

/// Each service by the name it is kept under, and as it is shown.
pub const SERVICES: [(&str, &str); 4] = [
    ("openai", "OpenAI"),
    ("gemini", "Google Gemini"),
    ("stability", "Stability AI"),
    ("chatgpt", "ChatGPT"),
];

/// The service signed in with a ChatGPT account rather than a key.
pub const CHATGPT: &str = "chatgpt";

const OPENAI_MODEL: &str = "gpt-image-2";
const GEMINI_MODEL: &str = "gemini-3.1-flash-image";

/// What every service is told besides the description: a wallpaper fills a wide screen.
const FRAMING: &str = "A wallpaper for a wide 16:9 screen, with no text or lettering.";

/// What a service that cannot take a background away itself is asked for instead.
const MASK: &str = "Return a black and white mask of this picture, at exactly its framing and \
aspect ratio: its main subject solid white, everything else solid black. No grey, no text.";

/// The longest side a picture is sent at: what the services take, and enough for a mask.
const SENT: i32 = 1536;

/// How long a picture may take; Codex plans before it draws.
const WAIT: Duration = Duration::from_secs(300);

fn store() -> PathBuf {
    glib::user_config_dir().join("nekoland").join("image-services")
}

/// A value kept for `name`: `service`, or a service's key.
pub fn read(name: &str) -> Option<String> {
    let kept = std::fs::read_to_string(store()).ok()?;
    kept.lines()
        .find_map(|line| line.split_once('=').filter(|(key, _)| *key == name).map(|(_, value)| value.trim().to_string()))
        .filter(|value| !value.is_empty())
}

/// Keeps `value` for `name`, the file readable by its owner alone; an empty value forgets it.
pub fn write(name: &str, value: &str) -> std::io::Result<()> {
    let path = store();
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = kept
        .lines()
        .filter(|line| line.split_once('=').is_none_or(|(key, _)| key != name))
        .map(str::to_string)
        .collect();
    let value = value.trim().replace(['\n', '\r'], "");
    if !value.is_empty() {
        lines.push(format!("{name}={value}"));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    // A file made before with wider permissions is narrowed too.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all((lines.join("\n") + "\n").as_bytes())
}

/// Where Codex is: on the PATH, or in `~/.local/bin`, where its own release is put and
/// which a session started before that directory existed does not have on its PATH.
pub fn codex() -> Option<PathBuf> {
    glib::find_program_in_path("codex").or_else(|| {
        let own = glib::home_dir().join(".local/bin/codex");
        own.is_file().then_some(own)
    })
}

/// Whether Codex is signed in with ChatGPT, or None when it is not installed.
pub fn chatgpt_signed_in() -> Option<bool> {
    let codex = codex()?;
    Some(Command::new(codex).args(["login", "status"]).output().is_ok_and(|done| done.status.success()))
}

/// Makes a wallpaper of `description` with `service`, waiting for it, and keeps it among the
/// person's pictures; what went wrong, in the service's words where it gave any.
pub fn generate(service: &str, description: &str) -> Result<PathBuf, String> {
    let prompt = format!("{} {FRAMING}", description.trim());
    let picture = if service == CHATGPT {
        by_codex(&prompt, None)?
    } else {
        let key = read(service).ok_or("尚未設定 API 金鑰")?;
        by_api(service, &key, &prompt, None)?
    };
    let extension = match picture.get(..4) {
        Some([0xff, 0xd8, ..]) => "jpg",
        Some(b"RIFF") => "webp",
        Some([0x89, b'P', b'N', b'G']) => "png",
        _ => return Err("服務沒有傳回圖片".into()),
    };
    keep("generated", extension, &picture)
}

/// Keeps a picture among the person's wallpapers, named for what made it and when.
fn keep(made: &str, extension: &str, picture: &[u8]) -> Result<PathBuf, String> {
    let dir = glib::user_data_dir().join("nekoland").join("wallpapers");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let seconds = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let path = dir.join(format!("{made}-{seconds}.{extension}"));
    std::fs::write(&path, picture).map_err(|err| err.to_string())?;
    Ok(path)
}

/// Cuts the subject of the picture at `wallpaper` out with `service`, as One UI's depth
/// effect does, and keeps it beside the wallpapers: a PNG framed as the picture is, clear
/// where the subject is not. Stability AI takes the background away itself; the others are
/// asked for a mask, which is laid on the picture.
pub fn cut_out(service: &str, wallpaper: &Path) -> Result<PathBuf, String> {
    let original = gdk_pixbuf::Pixbuf::from_file(wallpaper).map_err(|err| err.to_string())?;
    let shrink = (f64::from(SENT) / f64::from(original.width().max(original.height()))).min(1.0);
    let (width, height) = (
        ((f64::from(original.width()) * shrink).round() as i32).max(1),
        ((f64::from(original.height()) * shrink).round() as i32).max(1),
    );
    let sent = original.scale_simple(width, height, gdk_pixbuf::InterpType::Hyper).ok_or("無法讀取圖片")?;
    let sent_path = glib::user_runtime_dir().join(format!("nekoland-cutout-{}.png", std::process::id()));
    sent.savev(&sent_path, "png", &[]).map_err(|err| err.to_string())?;
    let answer = if service == CHATGPT {
        by_codex(MASK, Some(&sent_path))
    } else {
        read(service).ok_or_else(|| "尚未設定 API 金鑰".to_string()).and_then(|key| by_api(service, &key, MASK, Some(&sent_path)))
    };
    let _ = std::fs::remove_file(&sent_path);
    let answer = answer?;
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(&answer).and_then(|()| loader.close()).map_err(|_| "服務沒有傳回圖片".to_string())?;
    let returned = loader.pixbuf().ok_or("服務沒有傳回圖片")?;
    let cut = if service == "stability" { returned } else { masked(&sent, &returned).ok_or("無法套用遮罩")? };
    let png = cut.save_to_bufferv("png", &[]).map_err(|err| err.to_string())?;
    keep("subject", "png", &png)
}

/// `picture`, opaque where `mask`, stretched over it, is white and clear where it is black.
fn masked(picture: &gdk_pixbuf::Pixbuf, mask: &gdk_pixbuf::Pixbuf) -> Option<gdk_pixbuf::Pixbuf> {
    let (width, height) = (picture.width(), picture.height());
    let mask = mask.scale_simple(width, height, gdk_pixbuf::InterpType::Bilinear)?;
    let picture = picture.add_alpha(false, 0, 0, 0).ok()?;
    let (from, from_stride, from_channels) = (mask.read_pixel_bytes(), mask.rowstride() as usize, mask.n_channels() as usize);
    let stride = picture.rowstride() as usize;
    // SAFETY: the pixbuf was just made and is not shared; nothing else reads it meanwhile.
    let pixels = unsafe { picture.pixels() };
    for y in 0..height as usize {
        for x in 0..width as usize {
            let shade = &from[y * from_stride + x * from_channels..];
            let light = (u32::from(shade[0]) * 299 + u32::from(shade[1]) * 587 + u32::from(shade[2]) * 114) / 1000;
            // What is near black or near white is taken as such: masks are not drawn clean.
            pixels[y * stride + x * 4 + 3] = (light.saturating_sub(24) * 255 / (255 - 48)).min(255) as u8;
        }
    }
    Some(picture)
}

/// Asks a service's API through curl. The key goes in a configuration file only its owner
/// may read, never on a command line others can see.
fn by_api(service: &str, key: &str, prompt: &str, picture: Option<&Path>) -> Result<Vec<u8>, String> {
    let quoted = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"").replace(['\n', '\r'], " "));
    // Given a picture, the form fields besides its own; fields as `name=value`.
    let mut fields: Vec<String> = Vec::new();
    let (url, headers, body): (String, Vec<String>, Option<serde_json::Value>) = match (service, picture) {
        ("openai", Some(_)) => {
            fields = vec![format!("model={OPENAI_MODEL}"), format!("prompt={prompt}"), "size=auto".into()];
            ("https://api.openai.com/v1/images/edits".into(), vec![format!("Authorization: Bearer {key}")], None)
        }
        ("gemini", Some(path)) => {
            let sent = std::fs::read(path).map_err(|err| err.to_string())?;
            (
                format!("https://generativelanguage.googleapis.com/v1beta/models/{GEMINI_MODEL}:generateContent"),
                vec![format!("x-goog-api-key: {key}"), "Content-Type: application/json".into()],
                Some(serde_json::json!({
                    "contents": [{ "parts": [
                        { "inline_data": { "mime_type": "image/png", "data": glib::base64_encode(&sent).as_str() } },
                        { "text": prompt },
                    ] }],
                    "generationConfig": { "responseModalities": ["TEXT", "IMAGE"] },
                })),
            )
        }
        ("stability", Some(_)) => {
            fields = vec!["output_format=png".into()];
            (
                "https://api.stability.ai/v2beta/stable-image/edit/remove-background".into(),
                vec![format!("Authorization: Bearer {key}"), "Accept: image/*".into()],
                None,
            )
        }
        ("openai", None) => (
            "https://api.openai.com/v1/images/generations".into(),
            vec![format!("Authorization: Bearer {key}"), "Content-Type: application/json".into()],
            Some(serde_json::json!({ "model": OPENAI_MODEL, "prompt": prompt, "size": "1536x1024", "n": 1 })),
        ),
        ("gemini", None) => (
            format!("https://generativelanguage.googleapis.com/v1beta/models/{GEMINI_MODEL}:generateContent"),
            vec![format!("x-goog-api-key: {key}"), "Content-Type: application/json".into()],
            Some(serde_json::json!({
                "contents": [{ "parts": [{ "text": prompt }] }],
                "generationConfig": { "responseModalities": ["TEXT", "IMAGE"], "imageConfig": { "aspectRatio": "16:9" } },
            })),
        ),
        ("stability", None) => {
            fields = vec![format!("prompt={prompt}"), "aspect_ratio=16:9".into(), "output_format=png".into()];
            (
                "https://api.stability.ai/v2beta/stable-image/generate/core".into(),
                vec![format!("Authorization: Bearer {key}"), "Accept: image/*".into()],
                None,
            )
        }
        _ => return Err(format!("{service}: 不明的服務")),
    };
    let mut config = format!("url = {}\n", quoted(&url));
    for header in &headers {
        config += &format!("header = {}\n", quoted(header));
    }
    match &body {
        Some(_) => config += "data-binary = \"@-\"\n",
        None => {
            for field in &fields {
                config += &format!("form-string = {}\n", quoted(field));
            }
            // Ours, in the runtime directory: a name curl's form syntax reads as it is.
            if let Some(path) = picture {
                config += &format!("form = {}\n", quoted(&format!("image=@{}", path.display())));
            }
        }
    }
    let runtime = glib::user_runtime_dir();
    let config_path = runtime.join(format!("nekoland-image-{}.curl", std::process::id()));
    let answer_path = runtime.join(format!("nekoland-image-{}.out", std::process::id()));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&config_path)
        .and_then(|mut file| file.write_all(config.as_bytes()));
    let asked = written.map_err(|err| err.to_string()).and_then(|()| {
        curl(&config_path, &answer_path, body.as_ref().map(serde_json::Value::to_string))
    });
    let _ = std::fs::remove_file(&config_path);
    let answer = std::fs::read(&answer_path).unwrap_or_default();
    let _ = std::fs::remove_file(&answer_path);
    let status = asked?;
    if !(200..300).contains(&status) {
        return Err(said_wrong(&answer).unwrap_or_else(|| format!("服務回應 HTTP {status}")));
    }
    if answer.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Ok(answer);
    }
    let json: serde_json::Value = serde_json::from_slice(&answer).map_err(|_| "服務沒有傳回圖片".to_string())?;
    // The picture is the answer's longest string: base64, or for some models a link to it.
    let longest = longest_string(&json).ok_or("服務沒有傳回圖片")?;
    if longest.starts_with("https://") {
        let fetched = Command::new("curl").args(["-sSf", "--max-time", "60", longest]).output().map_err(|err| err.to_string())?;
        return fetched.status.success().then_some(fetched.stdout).ok_or_else(|| "無法下載圖片".to_string());
    }
    let decoded = glib::base64_decode(longest);
    Ok(decoded)
}

/// Runs curl with `config`, feeding it `body`, its answer into `answer`; the HTTP status.
fn curl(config: &Path, answer: &Path, body: Option<String>) -> Result<u32, String> {
    let mut child = Command::new("curl")
        .arg("-sS")
        .args(["--max-time", &WAIT.as_secs().to_string(), "-w", "%{http_code}", "-o"])
        .arg(answer)
        .arg("-K")
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("curl: {err}"))?;
    if let (Some(mut stdin), Some(body)) = (child.stdin.take(), body) {
        stdin.write_all(body.as_bytes()).map_err(|err| err.to_string())?;
    }
    let done = child.wait_with_output().map_err(|err| err.to_string())?;
    if !done.status.success() {
        return Err(String::from_utf8_lossy(&done.stderr).trim().trim_start_matches("curl: ").to_string());
    }
    String::from_utf8_lossy(&done.stdout).trim().parse().map_err(|_| "無法連線到服務".to_string())
}

/// What a service said went wrong, as each puts it.
fn said_wrong(answer: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(answer).ok()?;
    let said = [&json["error"]["message"], &json["error"], &json["message"], &json["errors"][0], &json["name"]]
        .into_iter()
        .find_map(|said| said.as_str().filter(|said| !said.is_empty()).map(str::to_string));
    said
}

fn longest_string(json: &serde_json::Value) -> Option<&str> {
    match json {
        serde_json::Value::String(text) => Some(text),
        serde_json::Value::Array(items) => items.iter().filter_map(longest_string).max_by_key(|text| text.len()),
        serde_json::Value::Object(fields) => fields.values().filter_map(longest_string).max_by_key(|text| text.len()),
        _ => None,
    }
}

/// Has Codex draw it with its own image tool, which a ChatGPT sign-in pays for, and takes
/// the picture it saved.
fn by_codex(prompt: &str, picture: Option<&Path>) -> Result<Vec<u8>, String> {
    if chatgpt_signed_in() != Some(true) {
        return Err("尚未以 ChatGPT 登入".into());
    }
    let started = SystemTime::now();
    let scratch = glib::user_cache_dir().join("nekoland").join("codex");
    std::fs::create_dir_all(&scratch).map_err(|err| err.to_string())?;
    let mut child = Command::new(codex().ok_or("尚未安裝 Codex")?)
        .args(["exec", "--skip-git-repo-check", "-C"])
        .arg(&scratch)
        .args(picture.map(|path| format!("--image={}", path.display())))
        .arg(format!("$imagegen {prompt}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("codex: {err}"))?;
    let waited = wait_for(&mut child, WAIT)?;
    let home = std::env::var_os("CODEX_HOME").map_or_else(|| glib::home_dir().join(".codex"), PathBuf::from);
    let newest = newest_picture(&home.join("generated_images"), started);
    match (waited, newest) {
        (_, Some(path)) => std::fs::read(path).map_err(|err| err.to_string()),
        (Some(said), None) => Err(said),
        (None, None) => Err("Codex 沒有產生圖片".into()),
    }
}

/// Waits for `child` up to `wait`, ending it after; what it said if it failed.
fn wait_for(child: &mut std::process::Child, wait: Duration) -> Result<Option<String>, String> {
    let until = std::time::Instant::now() + wait;
    loop {
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            if status.success() {
                return Ok(None);
            }
            let mut said = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut stderr, &mut said);
            }
            return Ok(Some(said.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("Codex 失敗").trim().to_string()));
        }
        if std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            return Err("生成逾時".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The newest picture under `dir`, at any depth, written since `since`.
fn newest_picture(dir: &Path, since: SystemTime) -> Option<PathBuf> {
    let mut found: Option<(SystemTime, PathBuf)> = None;
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                dirs.push(path);
                continue;
            }
            let picture = path.extension().is_some_and(|ext| ["png", "jpg", "jpeg", "webp"].contains(&ext.to_string_lossy().to_lowercase().as_str()));
            let Ok(written) = meta.modified() else { continue };
            if picture && written >= since && found.as_ref().is_none_or(|(newest, _)| written > *newest) {
                found = Some((written, path));
            }
        }
    }
    found.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_is_the_longest_string_and_errors_are_read() {
        let openai = serde_json::json!({ "created": 1, "data": [{ "b64_json": "QUJD".repeat(100), "revised_prompt": "a lake" }] });
        assert_eq!(longest_string(&openai).map(str::len), Some(400));
        let gemini = serde_json::json!({ "candidates": [{ "content": { "parts": [
            { "text": "Here is your wallpaper" },
            { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo".repeat(50) } },
        ] } }] });
        assert!(longest_string(&gemini).is_some_and(|data| data.starts_with("iVBORw0KGgo")));
        assert_eq!(said_wrong(br#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#).as_deref(), Some("Incorrect API key provided"));
        assert_eq!(said_wrong(br#"{"name":"unauthorized","errors":["missing authorization header"]}"#).as_deref(), Some("missing authorization header"));
        assert_eq!(said_wrong(b"not json"), None);
    }
}
