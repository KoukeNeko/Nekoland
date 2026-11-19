//! What a PDF holds beside its pages' drawing, read through poppler: its table of contents, the
//! labels its pages are numbered with, its links and its form's fields. And what poppler cannot
//! write, done by `qpdf`: pages put in another order, turned, cropped, a password put on or
//! taken off, and the file made smaller.

use std::ffi::{CStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use gtk::glib;
use gtk::glib::translate::{from_glib_none, ToGlibPtr};
use poppler::ffi;

use super::Place;

/// An entry of the table of contents: its title, the page it goes to, 0-based, how far down
/// that page in points when it says, and the entries under it.
pub struct Entry {
    pub title: String,
    pub page: Option<usize>,
    pub top: Option<f64>,
    pub children: Vec<Entry>,
}

/// Where a link or an entry of the contents goes.
pub enum Target {
    Page(usize, Option<f64>),
    Uri(String),
    Next,
    Previous,
    First,
    Last,
}

/// The page and how far down it, from its top, a destination names.
unsafe fn dest_target(document: &poppler::Document, dest: *mut ffi::PopplerDest) -> Option<(usize, Option<f64>)> {
    if dest.is_null() {
        return None;
    }
    let mut dest = dest;
    let mut found = std::ptr::null_mut();
    if (*dest).type_ == ffi::POPPLER_DEST_NAMED {
        let name = (*dest).named_dest;
        if name.is_null() {
            return None;
        }
        found = ffi::poppler_document_find_dest(document.to_glib_none().0, name);
        if found.is_null() {
            return None;
        }
        dest = found;
    }
    let page = (*dest).page_num - 1;
    let top = matches!((*dest).type_, ffi::POPPLER_DEST_XYZ | ffi::POPPLER_DEST_FITH | ffi::POPPLER_DEST_FITBH).then(|| (*dest).top);
    if !found.is_null() {
        ffi::poppler_dest_free(found);
    }
    if page < 0 {
        return None;
    }
    let height = document.page(page).map_or(0.0, |page| page.size().1);
    Some((page as usize, top.map(|top| (height - top).max(0.0))))
}

unsafe fn action_target(document: &poppler::Document, action: *mut ffi::PopplerAction) -> Option<Target> {
    if action.is_null() {
        return None;
    }
    match (*action).type_ {
        ffi::POPPLER_ACTION_GOTO_DEST => {
            let (page, top) = dest_target(document, (*action).goto_dest.dest)?;
            Some(Target::Page(page, top))
        }
        ffi::POPPLER_ACTION_URI => {
            let uri = (*action).uri.uri;
            (!uri.is_null()).then(|| Target::Uri(CStr::from_ptr(uri).to_string_lossy().into_owned()))
        }
        ffi::POPPLER_ACTION_NAMED => {
            let name = (*action).named.named_dest;
            if name.is_null() {
                return None;
            }
            match CStr::from_ptr(name).to_str().ok()? {
                "NextPage" => Some(Target::Next),
                "PrevPage" => Some(Target::Previous),
                "FirstPage" => Some(Target::First),
                "LastPage" => Some(Target::Last),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The PDF's table of contents, empty when it has none.
pub fn outline(document: &poppler::Document) -> Vec<Entry> {
    unsafe fn walk(document: &poppler::Document, iter: *mut ffi::PopplerIndexIter) -> Vec<Entry> {
        let mut entries = Vec::new();
        if iter.is_null() {
            return entries;
        }
        loop {
            let action = ffi::poppler_index_iter_get_action(iter);
            if !action.is_null() {
                let title = (*action).any.title;
                let title = if title.is_null() { String::new() } else { CStr::from_ptr(title).to_string_lossy().into_owned() };
                let (page, top) = match action_target(document, action) {
                    Some(Target::Page(page, top)) => (Some(page), top),
                    _ => (None, None),
                };
                let child = ffi::poppler_index_iter_get_child(iter);
                let children = walk(document, child);
                if !child.is_null() {
                    ffi::poppler_index_iter_free(child);
                }
                ffi::poppler_action_free(action);
                entries.push(Entry { title: title.trim().to_owned(), page, top, children });
            }
            if ffi::poppler_index_iter_next(iter) == glib::ffi::GFALSE {
                break;
            }
        }
        entries
    }
    unsafe {
        let iter = ffi::poppler_index_iter_new(document.to_glib_none().0);
        let entries = walk(document, iter);
        if !iter.is_null() {
            ffi::poppler_index_iter_free(iter);
        }
        entries
    }
}

/// What page `index` is numbered with: its label when `logical` and the PDF has one, or its
/// place counting from 1.
pub fn page_label(document: &poppler::Document, index: usize, logical: bool) -> String {
    let label = logical.then(|| document.page(index as i32).and_then(|page| page.label())).flatten();
    label.map(|label| label.to_string()).filter(|label| !label.is_empty()).unwrap_or_else(|| (index + 1).to_string())
}

/// The page whose label or number is `wanted`, 0-based.
pub fn page_named(document: &poppler::Document, wanted: &str, logical: bool) -> Option<usize> {
    let wanted = wanted.trim();
    let count = document.n_pages().max(0) as usize;
    if logical {
        if let Some(index) = (0..count).find(|&index| page_label(document, index, true).eq_ignore_ascii_case(wanted)) {
            return Some(index);
        }
    }
    let number: usize = wanted.parse().ok()?;
    (1..=count).contains(&number).then(|| number - 1)
}

/// The links on a page: where each is, from the page's top left, and where it goes.
pub fn links(document: &poppler::Document, page: &poppler::Page) -> Vec<(Place, Target)> {
    let height = page.size().1;
    let mut found = Vec::new();
    unsafe {
        let list = ffi::poppler_page_get_link_mapping(page.to_glib_none().0);
        let mut node = list;
        while !node.is_null() {
            let mapping = (*node).data as *mut ffi::PopplerLinkMapping;
            if let Some(target) = action_target(document, (*mapping).action) {
                let area = (*mapping).area;
                found.push(((area.x1, height - area.y2, area.x2, height - area.y1), target));
            }
            node = (*node).next;
        }
        ffi::poppler_page_free_link_mapping(list);
    }
    found
}

/// The form's fields on a page, each where it is from the page's top left.
pub fn fields(page: &poppler::Page) -> Vec<(Place, poppler::FormField)> {
    let height = page.size().1;
    let mut found = Vec::new();
    unsafe {
        let list = ffi::poppler_page_get_form_field_mapping(page.to_glib_none().0);
        let mut node = list;
        while !node.is_null() {
            let mapping = (*node).data as *mut ffi::PopplerFormFieldMapping;
            let field: poppler::FormField = from_glib_none((*mapping).field);
            let area = (*mapping).area;
            found.push(((area.x1, height - area.y2, area.x2, height - area.y1), field));
            node = (*node).next;
        }
        ffi::poppler_page_free_form_field_mapping(list);
    }
    found
}

/// Whether `qpdf` is installed, which the changes to a PDF's pages need.
pub fn has_qpdf() -> bool {
    Command::new("qpdf").arg("--version").output().is_ok_and(|output| output.status.success())
}

/// Runs qpdf with `args`, handed over in a file only this user can read, one a line, so the
/// passwords among them are not in its command line for any user to see.
fn qpdf(args: Vec<OsString>) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = super::scratch_dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let listed = dir.join(format!("qpdf-{}-{}.args", std::process::id(), glib::monotonic_time()));
    let written = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&listed).and_then(|mut file| {
        for arg in &args {
            file.write_all(arg.as_bytes())?;
            file.write_all(b"\n")?;
        }
        Ok(())
    });
    let output = written.and_then(|()| Command::new("qpdf").arg(format!("@{}", listed.display())).output());
    let _ = std::fs::remove_file(&listed);
    let output = output.map_err(|err| format!("qpdf: {err}"))?;
    // 3 is a success with warnings, such as a damaged file it mended.
    match output.status.code() {
        Some(0 | 3) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

fn password_arg(password: Option<&str>) -> Option<OsString> {
    password.map(|password| OsString::from(format!("--password={password}")))
}

/// Writes `to`, then puts it in `path`'s place, so the file is replaced whole or not at all.
fn replace(path: &Path, write: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let kept = path.with_extension("pdf.new");
    let result = write(&kept).and_then(|()| std::fs::rename(&kept, path).map_err(|err| format!("{}: {err}", path.display())));
    if result.is_err() {
        let _ = std::fs::remove_file(&kept);
    }
    result
}

/// The PDF at `path` written again with the pages `plan` names, each a file, its password if
/// it has one, and a page of it counting from 1, in that order. Annotations and form fields
/// go with their pages.
pub fn rebuild(path: &Path, plan: &[(PathBuf, Option<String>, u32)]) -> Result<(), String> {
    replace(path, |to| {
        let mut args: Vec<OsString> = vec!["--empty".into(), "--pages".into()];
        // Pages from the same file in a row go together, as qpdf takes a range of them.
        let mut runs: Vec<(PathBuf, Option<String>, Vec<u32>)> = Vec::new();
        for (file, password, page) in plan {
            match runs.last_mut() {
                Some((last, _, pages)) if last == file => pages.push(*page),
                _ => runs.push((file.clone(), password.clone(), vec![*page])),
            }
        }
        for (file, password, pages) in runs {
            args.push(file.into());
            args.extend(password_arg(password.as_deref()));
            args.push(pages.iter().map(u32::to_string).collect::<Vec<_>>().join(",").into());
        }
        args.push("--".into());
        args.push(to.into());
        qpdf(args).map(drop)
    })
}

/// Turns pages, counting from 1, by `degrees` clockwise.
pub fn rotate(path: &Path, password: Option<&str>, pages: &[u32], degrees: i32) -> Result<(), String> {
    replace(path, |to| {
        let list = pages.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let mut args: Vec<OsString> = vec![path.into()];
        args.extend(password_arg(password));
        args.push(format!("--rotate={:+}:{list}", degrees).into());
        args.push(to.into());
        qpdf(args).map(drop)
    })
}

/// Crops pages, counting from 1, to `place`, from the top left of the page as it is shown,
/// which hides the rest of it but keeps it in the file, as Preview does.
pub fn crop(path: &Path, password: Option<&str>, pages: &[u32], place: Place) -> Result<(), String> {
    let mut json_args: Vec<OsString> = vec!["--json=2".into(), "--json-key=pages".into()];
    json_args.extend(password_arg(password));
    json_args.push(path.into());
    let listing: serde_json::Value = serde_json::from_str(&qpdf(json_args)?).map_err(|err| err.to_string())?;
    let objects: Vec<String> = listing["pages"]
        .as_array()
        .map(|pages| pages.iter().filter_map(|page| page["object"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let mut header = serde_json::Value::Null;
    let mut changed: Vec<String> = Vec::new();
    for &number in pages {
        let Some(object) = objects.get(number as usize - 1) else { continue };
        let mut args: Vec<OsString> = vec!["--json=2".into(), "--json-key=qpdf".into(), format!("--json-object={object}").into()];
        args.extend(password_arg(password));
        args.push(path.into());
        let dumped: serde_json::Value = serde_json::from_str(&qpdf(args)?).map_err(|err| err.to_string())?;
        let Some(parts) = dumped["qpdf"].as_array() else { continue };
        header = parts.first().cloned().unwrap_or(serde_json::Value::Null);
        let key = format!("obj:{object}");
        let Some(mut value) = parts.get(1).and_then(|objects| objects[&key]["value"].as_object().cloned()) else { continue };
        let number_of = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        let boxed = |name: &str| -> Option<[f64; 4]> {
            let list = value.get(name)?.as_array()?;
            let numbers: Vec<f64> = list.iter().filter_map(number_of).collect();
            (numbers.len() == 4).then(|| [numbers[0], numbers[1], numbers[2], numbers[3]])
        };
        let Some([bx1, by1, bx2, by2]) = boxed("/CropBox").or_else(|| boxed("/MediaBox")) else { continue };
        let (x1, y1, x2, y2) = place;
        let (width, height) = (bx2 - bx1, by2 - by1);
        let rotation = value.get("/Rotate").and_then(number_of).unwrap_or(0.0).rem_euclid(360.0) as i32;
        // From the page as shown, its top left the origin, to the page's own space.
        let corner = |x: f64, y: f64| match rotation {
            90 => (bx1 + y, by1 + x),
            180 => (bx2 - x, by1 + y),
            270 => (bx2 - y, by2 - x),
            _ => (bx1 + x, by2 - y),
        };
        let (ax, ay) = corner(x1, y1);
        let (cx, cy) = corner(x2, y2);
        let cropped = [ax.min(cx).max(bx1), ay.min(cy).max(by1), ax.max(cx).min(bx1 + width), ay.max(cy).min(by1 + height)];
        // qpdf takes a /CropBox only after the /MediaBox, and serde_json writes the keys in
        // order, so it goes on the end of the dictionary as written.
        value.remove("/CropBox");
        let written = serde_json::Value::Object(value).to_string();
        let page = format!("{},\"/CropBox\":{}}}", &written[..written.len() - 1], serde_json::json!(cropped));
        changed.push(format!("{}:{{\"value\":{page}}}", serde_json::json!(key)));
    }
    if changed.is_empty() {
        return Err(String::from("沒有可裁切的頁面"));
    }
    let patch = format!("{{\"qpdf\":[{header},{{{}}}]}}", changed.join(","));
    let patch_path = super::scratch_dir().join(format!("crop-{}.json", glib::monotonic_time()));
    std::fs::write(&patch_path, patch).map_err(|err| err.to_string())?;
    let result = replace(path, |to| {
        let mut args: Vec<OsString> = vec![path.into()];
        args.extend(password_arg(password));
        args.push(format!("--update-from-json={}", patch_path.display()).into());
        args.push(to.into());
        qpdf(args).map(drop)
    });
    let _ = std::fs::remove_file(&patch_path);
    result
}

/// What a password-protected PDF lets a reader do without the owner's password.
#[derive(Clone, Copy, Default)]
pub struct Permissions {
    pub print: bool,
    pub copy: bool,
    pub assemble: bool,
    pub annotate: bool,
    pub forms: bool,
}

/// Writes `path` to `to` with the passwords given, an empty one meaning none: the user's to
/// open it, the owner's to do what `allowed` leaves out. With neither, any password is taken off.
pub fn protect(path: &Path, password: Option<&str>, to: &Path, user: &str, owner: &str, allowed: Permissions) -> Result<(), String> {
    let mut args: Vec<OsString> = vec![path.into()];
    args.extend(password_arg(password));
    if user.is_empty() && owner.is_empty() {
        args.push("--decrypt".into());
    } else {
        // An owner's password is needed for the permissions to hold; the user's stands in.
        let owner = if owner.is_empty() { user } else { owner };
        let yes_no = |on: bool| if on { "y" } else { "n" };
        args.extend(["--encrypt".into(), user.into(), owner.into(), "256".into()]);
        args.push(format!("--print={}", if allowed.print { "full" } else { "none" }).into());
        args.push(format!("--extract={}", yes_no(allowed.copy)).into());
        args.push(format!("--assemble={}", yes_no(allowed.assemble)).into());
        args.push(format!("--annotate={}", yes_no(allowed.annotate)).into());
        args.push(format!("--form={}", yes_no(allowed.forms)).into());
        args.push(format!("--modify-other={}", yes_no(allowed.assemble && allowed.annotate)).into());
        args.push("--".into());
    }
    args.push(to.into());
    qpdf(args).map(drop)
}

/// Writes `path` to `to` smaller, its streams and pictures compressed again, or laid out to
/// be read over the web while it loads when `linearize`.
pub fn compact(path: &Path, password: Option<&str>, to: &Path, smaller: bool, linearize: bool) -> Result<(), String> {
    let mut args: Vec<OsString> = vec![path.into()];
    args.extend(password_arg(password));
    if smaller {
        args.extend(["--object-streams=generate".into(), "--compress-streams=y".into(), "--recompress-flate".into(), "--optimize-images".into()]);
    }
    if linearize {
        args.push("--linearize".into());
    }
    args.push(to.into());
    qpdf(args).map(drop)
}

/// What the PDF lets be done, as the 加密 inspector tells it.
pub fn permissions(document: &poppler::Document) -> Permissions {
    let allowed = document.permissions();
    Permissions {
        print: allowed.contains(poppler::Permissions::OK_TO_PRINT),
        copy: allowed.contains(poppler::Permissions::OK_TO_COPY),
        assemble: allowed.contains(poppler::Permissions::OK_TO_ASSEMBLE),
        annotate: allowed.contains(poppler::Permissions::OK_TO_ADD_NOTES),
        forms: allowed.contains(poppler::Permissions::OK_TO_FILL_FORM),
    }
}

/// A PDF of one blank page `size` points across and down.
pub fn blank_page(path: &Path, size: (f64, f64)) -> Result<(), String> {
    let surface = gtk::cairo::PdfSurface::new(size.0, size.1, path).map_err(|err| err.to_string())?;
    let cr = gtk::cairo::Context::new(&surface).map_err(|err| err.to_string())?;
    cr.show_page().map_err(|err| err.to_string())?;
    drop(cr);
    surface.finish();
    Ok(())
}
