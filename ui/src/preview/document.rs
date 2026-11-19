//! The files themselves: opening, saving as the Mac's Preview does, keeping a version of a file
//! before it is written over, renaming and moving it, exporting it in another format, a PDF's
//! passwords, printing, and pictures new from the clipboard, a scanner or the screen.

use super::*;

/// Where the versions of the file at `path` are kept.
fn versions_dir(path: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    glib::user_data_dir().join(format!("nekoland/preview-versions/{:016x}", hasher.finish()))
}

/// How many versions of a file are kept.
const VERSIONS: usize = 20;

/// The versions kept of the file at `path`, newest first.
fn versions(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(versions_dir(path)) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|e| e != "txt")).collect();
    found.sort();
    found.reverse();
    found
}

/// Whether 預覽程式 writes the format `path` is in, so it can be saved without asking.
fn can_write(path: &Path) -> bool {
    save_format(path).is_some() || is_pdf(path)
}

/// The formats 輸出 writes, with their extensions.
const FORMATS: [(&str, &str); 8] = [
    ("HEIC", "heic"),
    ("JPEG", "jpg"),
    ("JPEG-2000", "jp2"),
    ("OpenEXR", "exr"),
    ("PDF", "pdf"),
    ("PNG", "png"),
    ("TIFF", "tiff"),
    ("BMP", "bmp"),
];

/// TIFF's compressions, as libtiff numbers them.
const TIFF_COMPRESSIONS: [(&str, &str); 4] = [("無", "1"), ("LZW", "5"), ("PackBits", "32773"), ("Deflate", "8")];

/// How a picture is exported.
#[derive(Clone)]
struct Export {
    quality: i32,
    alpha: bool,
    compression: &'static str,
    dpi: Option<f64>,
    profile: Option<PathBuf>,
}

/// `pixbuf` without its alpha, on white.
fn flatten(pixbuf: &Pixbuf) -> Pixbuf {
    if !pixbuf.has_alpha() {
        return pixbuf.clone();
    }
    let Some(flat) = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, pixbuf.width(), pixbuf.height()) else { return pixbuf.clone() };
    flat.fill(0xffff_ffff);
    pixbuf.composite(&flat, 0, 0, pixbuf.width(), pixbuf.height(), 0.0, 0.0, 1.0, 1.0, gtk::gdk_pixbuf::InterpType::Nearest, 255);
    flat
}

/// The options gdk-pixbuf takes to write `format` as `export` says.
fn pixbuf_options(format: &str, export: &Export) -> Vec<(String, String)> {
    let mut options: Vec<(String, String)> = Vec::new();
    match format {
        "jpeg" => options.push(("quality".into(), export.quality.to_string())),
        "tiff" => options.push(("compression".into(), export.compression.into())),
        _ => {}
    }
    if let (Some(dpi), "png" | "jpeg" | "tiff") = (export.dpi, format) {
        options.push(("x-dpi".into(), format!("{}", dpi.round())));
        options.push(("y-dpi".into(), format!("{}", dpi.round())));
    }
    if let (Some(profile), "png" | "jpeg" | "tiff") = (&export.profile, format) {
        if let Ok(bytes) = std::fs::read(profile) {
            options.push(("icc-profile".into(), glib::base64_encode(&bytes).to_string()));
        }
    }
    options
}

/// Writes `pixbuf` to `path` in the format its extension names.
fn write_picture(pixbuf: &Pixbuf, path: &Path, export: &Export) -> Result<(), String> {
    let pixbuf = if export.alpha { pixbuf.clone() } else { flatten(pixbuf) };
    if needs_encoder(path) {
        return export_with_encoder(&pixbuf, path, export.quality);
    }
    if is_pdf(path) {
        return export_pdf(&pixbuf, path);
    }
    let Some((format, _)) = save_format(path) else {
        return Err(String::from("可儲存為 PNG、JPEG、TIFF、BMP、PDF、HEIC、JPEG 2000 或 OpenEXR。"));
    };
    // JPEG and BMP have no alpha.
    let pixbuf = if matches!(format, "jpeg" | "bmp") { flatten(&pixbuf) } else { pixbuf };
    let options = pixbuf_options(format, export);
    let options: Vec<(&str, &str)> = options.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
    // Written beside it first, so the file is replaced whole or not at all.
    let kept = path.with_extension("pv-new");
    pixbuf.savev(&kept, format, &options).map_err(|err| err.to_string())?;
    std::fs::rename(&kept, path).map_err(|err| format!("{}: {err}", path.display()))
}

/// How large `pixbuf` is written as `format`, when gdk-pixbuf writes it.
fn written_size(pixbuf: &Pixbuf, format: &str, export: &Export) -> Option<usize> {
    let options = pixbuf_options(format, export);
    let options: Vec<(&str, &str)> = options.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
    let pixbuf = if export.alpha && format != "jpeg" { pixbuf.clone() } else { flatten(pixbuf) };
    pixbuf.save_to_bufferv(format, &options).ok().map(|bytes| bytes.len())
}

/// Page `index` of `document` drawn as a picture at `dpi`.
fn page_picture(document: &poppler::Document, index: usize, dpi: f64) -> Option<Pixbuf> {
    let page = document.page(index as i32)?;
    let scale = dpi / 72.0;
    let (width, height) = page.size();
    let (width, height) = (((width * scale).round() as i32).max(1), ((height * scale).round() as i32).max(1));
    let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, width, height).ok()?;
    {
        let cr = gtk::cairo::Context::new(&surface).ok()?;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok()?;
        cr.scale(scale, scale);
        page.render_for_printing(&cr);
    }
    surface.flush();
    #[allow(deprecated)]
    let pixbuf = gdk::pixbuf_get_from_surface(&surface, 0, 0, width, height);
    pixbuf
}

/// A page as a picture of it with the parts `places` names blacked out, as a PDF of one page,
/// so nothing under them is left in the file.
fn redacted_page(document: &poppler::Document, index: usize, places: &[Place], to: &Path) -> Result<(), String> {
    const DPI: f64 = 200.0;
    let page = document.page(index as i32).ok_or_else(|| String::from("沒有這一頁"))?;
    let (width, height) = page.size();
    let picture = page_picture(document, index, DPI).ok_or_else(|| String::from("記憶體不足"))?;
    let surface = gtk::cairo::PdfSurface::new(width, height, to).map_err(|err| err.to_string())?;
    let cr = gtk::cairo::Context::new(&surface).map_err(|err| err.to_string())?;
    cr.save().map_err(|err| err.to_string())?;
    cr.scale(72.0 / DPI, 72.0 / DPI);
    cr.set_source_pixbuf(&picture, 0.0, 0.0);
    cr.paint().map_err(|err| err.to_string())?;
    cr.restore().map_err(|err| err.to_string())?;
    cr.set_source_rgb(0.0, 0.0, 0.0);
    for (x1, y1, x2, y2) in places {
        cr.rectangle(*x1, *y1, x2 - x1, y2 - y1);
    }
    cr.fill().map_err(|err| err.to_string())?;
    drop(cr);
    surface.finish();
    Ok(())
}

impl Win {
    pub fn choose_files(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("圖片與 PDF"));
        for mime in MIME_TYPES {
            filter.add_mime_type(mime);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("打開").filters(&filters).default_filter(&filter).build();
        let weak = Rc::downgrade(self);
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            // Closing the dialog without choosing is an error that needs no telling.
            let (Some(win), Ok(files)) = (weak.upgrade(), chosen) else { return };
            let paths = files.iter::<gio::File>().filter_map(Result::ok).filter_map(|file| file.path()).collect();
            if win.items.borrow().is_empty() {
                win.open_files(paths);
            } else {
                Win::open(&win.app).open_files(paths);
            }
        });
    }

    /// Shows a picture saved nowhere yet, in this window when it is empty or a new one.
    fn open_untitled(self: &Rc<Self>, name: &str, pixbuf: Pixbuf) {
        let item = Item::untitled(name, pixbuf);
        if self.items.borrow().is_empty() {
            self.set_items(vec![item], 0);
        } else {
            Win::open(&self.app).set_items(vec![item], 0);
        }
    }

    /// 從剪貼板新增: the picture on the clipboard, in a window of its own.
    pub fn new_from_clipboard(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window.clipboard().read_texture_async(gio::Cancellable::NONE, move |texture| {
            let Some(win) = weak.upgrade() else { return };
            match texture.ok().flatten().and_then(|texture| edit::texture_pixbuf(&texture)) {
                Some(pixbuf) => win.open_untitled("未命名.png", pixbuf),
                None => alert::notice(&win.window, "剪貼板上沒有影像", None),
            }
        });
    }

    /// Keeps the file at `path` as it is now among its versions, before it is written over.
    pub fn keep_version(&self, path: &Path) {
        if !path.is_file() {
            return;
        }
        let dir = versions_dir(path);
        let stamp = glib::DateTime::now_local().ok().and_then(|now| now.format("%Y%m%d-%H%M%S").ok()).map(|stamp| stamp.to_string()).unwrap_or_default();
        let extension = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| String::from("bin"));
        let kept = dir.join(format!("{stamp}.{extension}"));
        let written = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(dir.join("path.txt"), path.to_string_lossy().as_bytes()))
            .and_then(|()| std::fs::copy(path, &kept).map(drop));
        if let Err(err) = written {
            eprintln!("{}: {err}", kept.display());
        }
        for old in versions(path).into_iter().skip(VERSIONS) {
            let _ = std::fs::remove_file(old);
        }
    }

    /// The changes to the PDF shown written into its file, when it has any.
    pub fn flush_pdf(self: &Rc<Self>) -> Result<(), String> {
        let Some(pdf) = self.pdf.borrow().clone().filter(|pdf| pdf.dirty.get()) else { return Ok(()) };
        self.save_pdf(&pdf, &pdf.path)
    }

    /// Writes the PDF with its annotations and form to `to`, what is to be blacked out made a
    /// picture of its page; written to its own file, it is then read again from it.
    fn save_pdf(self: &Rc<Self>, pdf: &Rc<Pdf>, to: &Path) -> Result<(), String> {
        let new = to.with_extension("pdf.new");
        pdf.document().save(&gio::File::for_path(&new).uri()).map_err(|err| err.to_string())?;
        let redactions = self.redactions.borrow().clone();
        if !redactions.is_empty() {
            let result = (|| {
                let written = open_pdf(&new, pdf.password.as_deref()).map_err(|err| err.to_string())?;
                let mut plan: Vec<(PathBuf, Option<String>, u32)> = Vec::new();
                for index in 0..pdf.sizes.len() {
                    let places: Vec<Place> = redactions.iter().filter(|(on, _)| *on == index).map(|(_, place)| *place).collect();
                    if places.is_empty() {
                        plan.push((new.clone(), pdf.password.clone(), index as u32 + 1));
                    } else {
                        let page = scratch_dir().join(format!("redacted-{index}-{}.pdf", glib::monotonic_time()));
                        std::fs::create_dir_all(scratch_dir()).map_err(|err| err.to_string())?;
                        redacted_page(&written, index, &places, &page)?;
                        plan.push((page, None, 1));
                    }
                }
                if pdfdoc::has_qpdf() {
                    pdfdoc::rebuild(&new, &plan)
                } else if pdf.password.is_none() {
                    rebuild_pdf(&new, &plan.into_iter().map(|(file, _, page)| (file, page)).collect::<Vec<_>>())
                } else {
                    Err(String::from("塗黑有密碼的 PDF 需要 qpdf。"))
                }
            })();
            if let Err(err) = result {
                let _ = std::fs::remove_file(&new);
                return Err(err);
            }
        }
        let redacting_in_place = !redactions.is_empty() && to == pdf.path;
        if !redacting_in_place {
            self.keep_version(to);
        }
        std::fs::rename(&new, to).map_err(|err| format!("{}: {err}", to.display()))?;
        if to == pdf.path {
            if redacting_in_place {
                // What was blacked out must not outlive the save: no version or 還原 snapshot keeps it.
                let versions = versions_dir(to);
                if let Err(err) = std::fs::remove_dir_all(&versions).or_else(|err| if versions.exists() { Err(err) } else { Ok(()) }) {
                    eprintln!("{}: {err}", versions.display());
                }
                for snapshot in pdf.undo.borrow_mut().drain(..).chain(pdf.redo.borrow_mut().drain(..)) {
                    let _ = std::fs::remove_file(snapshot);
                }
                // The pages blacked out are pictures now, read again with the rest.
                self.redactions.borrow_mut().clear();
                pdf.dirty.set(false);
                let page = self.item().and_then(|item| item.page).unwrap_or(1);
                self.reload(&pdf.path.clone(), page);
                return Ok(());
            }
            let document = open_pdf(to, pdf.password.as_deref()).map_err(|err| err.to_string())?;
            *pdf.document.borrow_mut() = document;
            pdf.dirty.set(false);
            pdf.undo.borrow_mut().clear();
            pdf.redo.borrow_mut().clear();
        }
        if let Some(item) = self.item() {
            self.update_title(&item);
        }
        self.update_actions();
        Ok(())
    }

    /// Writes the picture to `path`, kept first among the file's versions; written to its own
    /// file, it counts as saved.
    fn write(self: &Rc<Self>, item: &Rc<Item>, pixbuf: &Pixbuf, path: &Path) -> Result<(), String> {
        self.keep_version(path);
        let export = Export { quality: 90, alpha: true, compression: "5", dpi: item.dpi.get(), profile: item.profile.borrow().clone() };
        write_picture(pixbuf, path, &export)?;
        if path == item.target() && !item.untitled {
            *item.edited.borrow_mut() = None;
            if let Some(picture) = self.picture_of(item) {
                *self.shown.borrow_mut() = Some(picture);
            }
            self.update_title(item);
            self.update_actions();
        }
        Ok(())
    }

    fn picture_of(&self, item: &Item) -> Option<Pixbuf> {
        item.edited.borrow().clone().or_else(|| self.shown.borrow().clone())
    }

    /// The picture of `item`, as it is now.
    fn pixbuf_of(&self, item: &Rc<Item>) -> Option<Pixbuf> {
        if self.item().is_some_and(|shown| Rc::ptr_eq(&shown, item)) {
            return self.shown.borrow().clone();
        }
        item.edited.borrow().clone().or_else(|| load_picture(&item.path).ok())
    }

    pub fn save(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        if item.untitled {
            return self.save_as();
        }
        if is_locked(&item.path) {
            let alert = adw::AlertDialog::new(Some(&format!("「{}」已鎖定", item.name())), Some("解鎖後才能儲存變更。"));
            alert.add_responses(&[("cancel", "取消"), ("do", "解鎖")]);
            alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
            alert.set_default_response(Some("do"));
            alert.set_close_response("cancel");
            let weak = Rc::downgrade(self);
            return alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
                if let Some(win) = weak.upgrade().filter(|_| response == "do") {
                    win.set_locked(false);
                    win.save();
                }
            });
        }
        let result = match (item.page, self.pdf.borrow().clone()) {
            (Some(_), Some(pdf)) => {
                if !self.redactions.borrow().is_empty() {
                    let count = self.redactions.borrow().len();
                    let weak = Rc::downgrade(self);
                    return alert::confirm(
                        &self.window,
                        &format!("塗黑 {count} 處？"),
                        Some("塗黑處的文字和影像會從檔案中永久移除，頁面會轉為影像。"),
                        "塗黑",
                        move || {
                            if let Some(win) = weak.upgrade() {
                                if let Err(err) = win.save_pdf(&pdf, &pdf.path) {
                                    alert::notice(&win.window, "無法儲存", Some(&err));
                                }
                            }
                        },
                    );
                }
                self.save_pdf(&pdf, &pdf.path)
            }
            _ => match self.pixbuf_of(&item) {
                Some(pixbuf) => self.write(&item, &pixbuf, &item.target()),
                None => Ok(()),
            },
        };
        if let Err(err) = result {
            alert::notice(&self.window, "無法儲存", Some(&format!("{}\n{err}", item.target().display())));
        }
    }

    /// 鎖定: the file shown made read-only, so it is not changed by mistake; or writable again.
    pub fn set_locked(&self, locked: bool) {
        use std::os::unix::fs::PermissionsExt;
        let Some(item) = self.item().filter(|item| !item.untitled) else { return };
        let result = std::fs::metadata(&item.path).and_then(|meta| {
            let mut permissions = meta.permissions();
            let mode = permissions.mode();
            permissions.set_mode(if locked { mode & !0o222 } else { mode | 0o200 });
            std::fs::set_permissions(&item.path, permissions)
        });
        if let Err(err) = result {
            alert::notice(&self.window, if locked { "無法鎖定" } else { "無法解鎖" }, Some(&err.to_string()));
        }
        self.update_title(&item);
    }

    /// 另存新檔: written somewhere else, which the window then shows.
    pub fn save_as(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let dialog = gtk::FileDialog::builder().title("另存新檔").initial_name(item.target().file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()).build();
        if let Some(dir) = item.path.parent().filter(|dir| dir.is_dir() && !item.untitled) {
            dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
        }
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            let (Some(win), Ok(file)) = (weak.upgrade(), chosen) else { return };
            let Some(path) = file.path() else { return };
            let result = match (item.page, win.pdf.borrow().clone()) {
                (Some(_), Some(pdf)) if is_pdf(&path) => win.save_pdf(&pdf, &path),
                (Some(_), Some(pdf)) => {
                    let index = item.page.unwrap_or(1) as usize - 1;
                    match page_picture(&pdf.document(), index, 150.0) {
                        Some(picture) => write_picture(&picture, &path, &Export { quality: 90, alpha: false, compression: "5", dpi: Some(150.0), profile: None }),
                        None => Err(String::from("記憶體不足")),
                    }
                }
                _ => match win.pixbuf_of(&item) {
                    Some(pixbuf) => win.write(&item, &pixbuf, &path),
                    None => Ok(()),
                },
            };
            match result {
                Ok(()) if item.untitled || (item.page.is_none()) || is_pdf(&path) => win.replace_path(&item.path, &path),
                Ok(()) => {}
                Err(err) => alert::notice(&win.window, "無法儲存", Some(&format!("{}\n{err}", path.display()))),
            }
        });
    }

    /// The items of the file at `from` shown again as the file at `to`, which it now is.
    fn replace_path(self: &Rc<Self>, from: &Path, to: &Path) {
        if let Some(password) = self.passwords.borrow_mut().remove(from) {
            self.passwords.borrow_mut().insert(to.to_owned(), password);
        }
        {
            let mut prefs = self.prefs.borrow_mut();
            if let Some(page) = prefs.last_page.remove(from) {
                prefs.last_page.insert(to.to_owned(), page);
            }
            if let Some(marks) = prefs.bookmarks.remove(from) {
                prefs.bookmarks.insert(to.to_owned(), marks);
            }
            prefs.save();
        }
        let page = self.item().and_then(|item| item.page);
        let current = self.current.get();
        if self.pdf.borrow().as_ref().is_some_and(|pdf| pdf.path == from) {
            *self.pdf.borrow_mut() = None;
        }
        let items: Vec<Rc<Item>> = self
            .items
            .borrow()
            .iter()
            .map(|item| if item.path == from { Item::new(to.to_owned(), item.page) } else { item.clone() })
            .collect();
        remember_recent(to);
        self.set_items(items, current);
        let _ = page;
    }

    /// 複製: a copy saved nowhere yet, in a window of its own.
    pub fn duplicate(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let stem = item.path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        match (item.page, self.pdf.borrow().clone()) {
            (Some(_), Some(pdf)) => {
                let copy = scratch_dir().join(format!("copy-{}/{stem} 拷貝.pdf", glib::monotonic_time()));
                let written = std::fs::create_dir_all(copy.parent().unwrap_or(&scratch_dir())).map_err(|err| err.to_string()).and_then(|()| {
                    pdf.document().save(&gio::File::for_path(&copy).uri()).map_err(|err| err.to_string())
                });
                if let Err(err) = written {
                    return alert::notice(&self.window, "無法拷貝", Some(&err));
                }
                let win = Win::open(&self.app);
                if let Some(password) = pdf.password.clone() {
                    win.passwords.borrow_mut().insert(copy.clone(), password);
                }
                let items = (1..=pdf.sizes.len() as u32).map(|page| Item::copied(copy.clone(), page)).collect();
                win.set_items(items, item.page.unwrap_or(1) as usize - 1);
            }
            _ => {
                let Some(pixbuf) = self.pixbuf_of(&item) else { return };
                let extension = item.target().extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| String::from("png"));
                Win::open(&self.app).set_items(vec![Item::untitled(&format!("{stem} 拷貝.{extension}"), pixbuf)], 0);
            }
        }
    }

    /// 重新命名.
    pub fn rename(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let name = item.name();
        let stem = item.path.file_stem().map_or(0, |stem| stem.to_string_lossy().chars().count() as i32);
        let weak = Rc::downgrade(self);
        alert::prompt(&self.window, "重新命名", &name, stem, "重新命名", move |name| {
            let Some(win) = weak.upgrade() else { return };
            let to = item.path.with_file_name(&name);
            if to == item.path {
                return;
            }
            if to.exists() {
                return alert::notice(&win.window, "無法重新命名", Some(&format!("已經有一個名為「{name}」的檔案。")));
            }
            match std::fs::rename(&item.path, &to) {
                Ok(()) => win.replace_path(&item.path, &to),
                Err(err) => alert::notice(&win.window, "無法重新命名", Some(&err.to_string())),
            }
        });
    }

    /// 搬移至: the file put in another folder.
    pub fn move_to(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let dialog = gtk::FileDialog::builder().title("搬移至").build();
        let weak = Rc::downgrade(self);
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            let (Some(win), Ok(folder)) = (weak.upgrade(), chosen) else { return };
            let (Some(dir), Some(name)) = (folder.path(), item.path.file_name()) else { return };
            let to = dir.join(name);
            if to.exists() {
                return alert::notice(&win.window, "無法搬移", Some(&format!("「{}」已經有同名的檔案。", dir.display())));
            }
            // Across disks a rename fails, and the file is copied and the first taken away.
            let moved = std::fs::rename(&item.path, &to).or_else(|_| std::fs::copy(&item.path, &to).and_then(|_| std::fs::remove_file(&item.path)));
            match moved {
                Ok(()) => win.replace_path(&item.path, &to),
                Err(err) => alert::notice(&win.window, "無法搬移", Some(&err.to_string())),
            }
        });
    }

    /// 回復到上次儲存的版本: the changes let go.
    pub fn revert_saved(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let weak = Rc::downgrade(self);
        alert::confirm(&self.window, "回復到上次儲存的版本？", Some("目前的變更會遺失。"), "回復", move || {
            let Some(win) = weak.upgrade() else { return };
            if item.is_page() {
                win.redactions.borrow_mut().clear();
                if let Some(pdf) = win.pdf.borrow().as_ref() {
                    pdf.dirty.set(false);
                }
                return win.reload(&item.path, item.page.unwrap_or(1));
            }
            item.edited.take();
            item.undo.borrow_mut().clear();
            item.redo.borrow_mut().clear();
            item.save_to.take();
            win.set_mask(None, Vec::new());
            win.select(win.current.get());
        });
    }

    /// 瀏覽所有版本: the versions kept of the file, any of which it can be put back to.
    pub fn browse_versions(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let kept = versions(&item.path);
        let group = adw::PreferencesGroup::new();
        if kept.is_empty() {
            group.add(&adw::ActionRow::builder().title("沒有其他版本").build());
        }
        let dialog = adw::Dialog::builder().title("所有版本").content_width(440).content_height(480).build();
        for version in kept {
            let when = version
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|since| glib::DateTime::from_unix_local(since.as_secs() as i64).ok())
                .and_then(|when| Some(modified_label(&when, &glib::DateTime::now_local().ok()?)))
                .unwrap_or_default();
            let size = version.metadata().map(|meta| glib::format_size(meta.len()).to_string()).unwrap_or_default();
            let row = adw::ActionRow::builder().title(when).subtitle(size).build();
            let open = gtk::Button::builder().label("打開").valign(gtk::Align::Center).build();
            let restore = gtk::Button::builder().label("回復").valign(gtk::Align::Center).build();
            row.add_suffix(&open);
            row.add_suffix(&restore);
            let (weak, path) = (Rc::downgrade(self), version.clone());
            open.connect_clicked(move |_| {
                if let Some(win) = weak.upgrade() {
                    let window = Win::open(&win.app);
                    window.open_files(vec![path.clone()]);
                }
            });
            let (weak, path, item, dialog_weak) = (Rc::downgrade(self), version.clone(), item.clone(), dialog.downgrade());
            restore.connect_clicked(move |_| {
                let Some(win) = weak.upgrade() else { return };
                let (path, item, dialog_weak) = (path.clone(), item.clone(), dialog_weak.clone());
                let win_weak = Rc::downgrade(&win);
                alert::confirm(&win.window, "回復到這個版本？", Some("目前的版本會保留在所有版本中。"), "回復", move || {
                    let Some(win) = win_weak.upgrade() else { return };
                    win.keep_version(&item.path);
                    match std::fs::copy(&path, &item.path) {
                        Ok(_) => {
                            item.edited.take();
                            if let Some(pdf) = win.pdf.borrow().as_ref() {
                                pdf.dirty.set(false);
                            }
                            win.reload(&item.path, item.page.unwrap_or(1));
                            if let Some(dialog) = dialog_weak.upgrade() {
                                dialog.close();
                            }
                        }
                        Err(err) => alert::notice(&win.window, "無法回復", Some(&err.to_string())),
                    }
                });
            });
            group.add(&row);
        }
        let page = adw::PreferencesPage::new();
        page.add(&group);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&page));
        dialog.set_child(Some(&view));
        dialog.present(Some(&self.window));
    }

    /// 輸出: the picture, or the PDF or its page, in another format.
    pub fn export(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let pdf = self.pdf.borrow().clone().filter(|_| item.is_page());
        let format = adw::ComboRow::builder().title("格式").model(&gtk::StringList::new(&FORMATS.map(|(name, _)| name))).build();
        let start = match &pdf {
            Some(_) => 4,
            None => {
                let extension = item.target().extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                FORMATS.iter().position(|(_, e)| *e == extension || (extension == "jpeg" && *e == "jpg") || (extension == "tif" && *e == "tiff")).unwrap_or(5)
            }
        };
        format.set_selected(start as u32);
        let quality = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
        quality.set_value(85.0);
        quality.set_hexpand(true);
        quality.set_width_request(200);
        let quality_row = adw::ActionRow::builder().title("品質").build();
        quality_row.add_suffix(&quality);
        let alpha = adw::SwitchRow::builder().title("Alpha").active(true).build();
        let compression = adw::ComboRow::builder().title("壓縮").model(&gtk::StringList::new(&TIFF_COMPRESSIONS.map(|(name, _)| name))).selected(1).build();
        let resolution = adw::SpinRow::builder().title("解析度").subtitle("像素/英寸").adjustment(&gtk::Adjustment::new(150.0, 36.0, 1200.0, 1.0, 50.0, 0.0)).build();
        let filter = adw::ComboRow::builder().title("濾鏡").model(&gtk::StringList::new(&["無", "灰色調"])).build();
        let smaller = adw::SwitchRow::builder().title("縮減檔案大小").build();
        let encrypt = adw::SwitchRow::builder().title("加密").build();
        let password = adw::PasswordEntryRow::builder().title("密碼").visible(false).build();
        let verify = adw::PasswordEntryRow::builder().title("驗證").visible(false).build();
        let size = adw::ActionRow::builder().title("檔案大小").css_classes(["property"]).build();
        let group = adw::PreferencesGroup::new();
        for row in [
            format.upcast_ref::<gtk::Widget>(),
            quality_row.upcast_ref(),
            alpha.upcast_ref(),
            compression.upcast_ref(),
            resolution.upcast_ref(),
            filter.upcast_ref(),
            smaller.upcast_ref(),
            encrypt.upcast_ref(),
            password.upcast_ref(),
            verify.upcast_ref(),
            size.upcast_ref(),
        ] {
            group.add(row);
        }
        let picture: Rc<RefCell<Option<Pixbuf>>> = Rc::new(RefCell::new(if pdf.is_none() { self.pixbuf_of(&item) } else { None }));
        let page_index = item.page.map_or(0, |page| page as usize - 1);
        let settings = {
            let (format, quality, alpha, compression, resolution) = (format.clone(), quality.clone(), alpha.clone(), compression.clone(), resolution.clone());
            let (dpi, profile) = (item.dpi.get(), item.profile.borrow().clone());
            let from_pdf = pdf.is_some();
            Rc::new(move || {
                let extension = FORMATS[format.selected() as usize].1;
                let export = Export {
                    quality: quality.value() as i32,
                    alpha: alpha.is_active(),
                    compression: TIFF_COMPRESSIONS[compression.selected() as usize].1,
                    dpi: if from_pdf { Some(resolution.value()) } else { dpi },
                    profile: profile.clone(),
                };
                (extension, export)
            })
        };
        let refresh = {
            let (settings, quality_row, alpha, compression, resolution, smaller, encrypt, password, verify, size, picture, pdf, filter) =
                (settings.clone(), quality_row.clone(), alpha.clone(), compression.clone(), resolution.clone(), smaller.clone(), encrypt.clone(), password.clone(), verify.clone(), size.clone(), picture.clone(), pdf.clone(), filter.clone());
            Rc::new(move || {
                let (extension, export) = settings();
                let to_pdf = extension == "pdf";
                quality_row.set_visible(matches!(extension, "jpg" | "heic"));
                alpha.set_visible(matches!(extension, "png" | "tiff" | "heic" | "jp2" | "exr"));
                compression.set_visible(extension == "tiff");
                resolution.set_visible(pdf.is_some() && !to_pdf);
                filter.set_visible(to_pdf);
                smaller.set_visible(to_pdf && pdf.is_some());
                encrypt.set_visible(to_pdf);
                password.set_visible(to_pdf && encrypt.is_active());
                verify.set_visible(to_pdf && encrypt.is_active());
                if pdf.is_some() && !to_pdf && picture.borrow().is_none() {
                    *picture.borrow_mut() = pdf.as_ref().and_then(|pdf| page_picture(&pdf.document(), page_index, export.dpi.unwrap_or(150.0)));
                }
                let format = match extension {
                    "jpg" => Some("jpeg"),
                    "png" | "tiff" | "bmp" => Some(extension),
                    _ => None,
                };
                let estimate = format.zip(picture.borrow().clone()).and_then(|(format, pixbuf)| written_size(&pixbuf, format, &export));
                size.set_visible(estimate.is_some());
                if let Some(bytes) = estimate {
                    size.set_subtitle(&glib::format_size(bytes as u64));
                }
            })
        };
        refresh();
        for notify in [format.upcast_ref::<glib::Object>(), compression.upcast_ref()] {
            let refresh = refresh.clone();
            notify.connect_notify_local(Some("selected"), move |_, _| refresh());
        }
        for switch in [&alpha, &encrypt] {
            let refresh = refresh.clone();
            switch.connect_active_notify(move |_| refresh());
        }
        {
            let refresh = refresh.clone();
            quality.connect_value_changed(move |_| refresh());
        }
        {
            let (refresh, picture) = (refresh.clone(), picture.clone());
            let from_pdf = pdf.is_some();
            resolution.connect_value_notify(move |_| {
                if from_pdf {
                    picture.borrow_mut().take();
                }
                refresh();
            });
        }
        let alert = adw::AlertDialog::new(Some("輸出"), None);
        alert.set_extra_child(Some(&group));
        alert.add_responses(&[("cancel", "取消"), ("do", "輸出⋯")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("do"));
        alert.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
            let Some(win) = weak.upgrade().filter(|_| response == "do") else { return };
            let (extension, export) = settings();
            if encrypt.is_active() && extension == "pdf" && (password.text().is_empty() || password.text() != verify.text()) {
                return alert::notice(&win.window, "密碼不相符", None);
            }
            let stem = item.path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
            let dialog = gtk::FileDialog::builder().title("輸出").initial_name(format!("{stem}.{extension}")).build();
            if let Some(dir) = item.path.parent().filter(|dir| dir.is_dir() && !item.untitled) {
                dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
            }
            let (weak, picture, pdf, item) = (Rc::downgrade(&win), picture.clone(), pdf.clone(), item.clone());
            let (encrypting, secret, reduce, grey) = (encrypt.is_active(), password.text().to_string(), smaller.is_active(), filter.selected() == 1);
            if grey && pdf.as_ref().is_some_and(|pdf| pdf.password.is_some()) {
                return alert::notice(&win.window, "無法套用灰色調", Some("有密碼的 PDF 不能套用濾鏡。"));
            }
            dialog.save(Some(&win.window), gio::Cancellable::NONE, move |chosen| {
                let (Some(win), Ok(file)) = (weak.upgrade(), chosen) else { return };
                let Some(path) = file.path() else { return };
                let result = match (&pdf, extension) {
                    (Some(pdf), "pdf") => win.save_pdf(pdf, &path).and_then(|()| finish_pdf(&path, grey, reduce, encrypting.then_some(secret.as_str()))),
                    (None, "pdf") => win.pixbuf_of(&item).ok_or_else(String::new).and_then(|pixbuf| export_pdf(&pixbuf, &path)).and_then(|()| finish_pdf(&path, grey, false, encrypting.then_some(secret.as_str()))),
                    _ => match picture.borrow().clone() {
                        Some(pixbuf) => write_picture(&pixbuf, &path, &export),
                        None => Err(String::from("記憶體不足")),
                    },
                };
                if let Err(err) = result {
                    alert::notice(&win.window, "無法輸出", Some(&format!("{}\n{err}", path.display())));
                }
            });
        });
    }

    /// 輸出為 PDF.
    pub fn export_pdf(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let stem = item.path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let dialog = gtk::FileDialog::builder().title("輸出為 PDF").initial_name(format!("{stem}.pdf")).build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            let (Some(win), Ok(file)) = (weak.upgrade(), chosen) else { return };
            let Some(path) = file.path() else { return };
            let result = match (item.page, win.pdf.borrow().clone()) {
                (Some(_), Some(pdf)) => win.save_pdf(&pdf, &path),
                _ => win.pixbuf_of(&item).ok_or_else(String::new).and_then(|pixbuf| export_pdf(&pixbuf, &path)),
            };
            if let Err(err) = result {
                alert::notice(&win.window, "無法輸出", Some(&format!("{}\n{err}", path.display())));
            }
        });
    }

    /// 輸入密碼: the owner's password, which lifts what the PDF does not let be done.
    pub fn enter_password(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let weak = Rc::downgrade(self);
        let entry = alert::prompt(&self.window, "輸入密碼", "", -1, "解鎖", move |password| {
            let Some(win) = weak.upgrade() else { return };
            match open_pdf(&item.path, Some(&password)) {
                Ok(_) => {
                    win.passwords.borrow_mut().insert(item.path.clone(), password);
                    win.reload(&item.path, item.page.unwrap_or(1));
                }
                Err(_) => alert::notice(&win.window, "密碼不正確", None),
            }
        });
        entry.set_visibility(false);
        entry.set_input_purpose(gtk::InputPurpose::Password);
    }

    /// 編輯權限: the PDF's passwords, and what it lets be done without the owner's.
    pub fn edit_permissions(self: &Rc<Self>) {
        let (Some(item), Some(pdf)) = (self.item(), self.pdf.borrow().clone()) else { return };
        if !pdfdoc::has_qpdf() {
            return alert::notice(&self.window, "需要 qpdf", Some("變更 PDF 的密碼和權限需要 qpdf。"));
        }
        let allowed = pdfdoc::permissions(&pdf.document());
        let open_with = adw::SwitchRow::builder().title("需要密碼才能打開文件").active(pdf.password.is_some()).build();
        let user = adw::PasswordEntryRow::builder().title("文件密碼").build();
        let owner = adw::PasswordEntryRow::builder().title("權限密碼").build();
        let print = adw::SwitchRow::builder().title("列印").active(allowed.print).build();
        let copy = adw::SwitchRow::builder().title("拷貝文字和影像").active(allowed.copy).build();
        let assemble = adw::SwitchRow::builder().title("插入、刪除和旋轉頁面").active(allowed.assemble).build();
        let annotate = adw::SwitchRow::builder().title("加入註解和簽名").active(allowed.annotate).build();
        let forms = adw::SwitchRow::builder().title("填寫現有的表單欄位").active(allowed.forms).build();
        user.set_visible(open_with.is_active());
        {
            let user = user.clone();
            open_with.connect_active_notify(move |row| user.set_visible(row.is_active()));
        }
        let passwords = adw::PreferencesGroup::new();
        passwords.add(&open_with);
        passwords.add(&user);
        passwords.add(&owner);
        let rights = adw::PreferencesGroup::builder().title("允許").build();
        for row in [&print, &copy, &assemble, &annotate, &forms] {
            rights.add(row);
        }
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(EDGE).build();
        column.append(&passwords);
        column.append(&rights);
        let alert = adw::AlertDialog::new(Some("權限"), None);
        alert.set_extra_child(Some(&column));
        alert.add_responses(&[("cancel", "取消"), ("do", "套用")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
            let Some(win) = weak.upgrade().filter(|_| response == "do") else { return };
            let user_password = if open_with.is_active() { user.text().to_string() } else { String::new() };
            let owner_password = owner.text().to_string();
            let restricted = !(print.is_active() && copy.is_active() && assemble.is_active() && annotate.is_active() && forms.is_active());
            if restricted && owner_password.is_empty() && user_password.is_empty() {
                return alert::notice(&win.window, "需要權限密碼", Some("限制權限需要設定權限密碼。"));
            }
            let allowed = pdfdoc::Permissions { print: print.is_active(), copy: copy.is_active(), assemble: assemble.is_active(), annotate: annotate.is_active(), forms: forms.is_active() };
            let path = item.path.clone();
            let opens_with = (!user_password.is_empty()).then(|| user_password.clone());
            win.change_pages(
                move |path, password| {
                    let new = path.with_extension("pdf.locked");
                    pdfdoc::protect(path, password.as_deref(), &new, &user_password, &owner_password, allowed)?;
                    std::fs::rename(&new, path).map_err(|err| err.to_string())
                },
                None,
            );
            match opens_with {
                Some(password) => win.passwords.borrow_mut().insert(path, password),
                None => win.passwords.borrow_mut().remove(&path),
            };
        });
    }

    /// A page scanned, as `scanimage` reads it, handed to `then` as a PNG.
    pub fn scan(self: &Rc<Self>, then: impl FnOnce(PathBuf) + 'static) {
        let listed = Command::new("scanimage").arg("-L").output();
        let Ok(listed) = listed else {
            return alert::notice(&self.window, "找不到 scanimage", Some("從掃描器輸入需要 sane-utils。"));
        };
        // Each line names one: device `name' is a description
        let devices: Vec<(String, String)> = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("device `")?;
                let (name, description) = rest.split_once("' is ")?;
                let description = description.strip_prefix("a ").or_else(|| description.strip_prefix("an ")).unwrap_or(description);
                Some((name.to_owned(), description.to_owned()))
            })
            .collect();
        if devices.is_empty() {
            return alert::notice(&self.window, "找不到掃描器", None);
        }
        let device = adw::ComboRow::builder().title("掃描器").model(&gtk::StringList::new(&devices.iter().map(|(_, description)| description.as_str()).collect::<Vec<_>>())).build();
        const RESOLUTIONS: [&str; 4] = ["75", "150", "300", "600"];
        let resolution = adw::ComboRow::builder().title("解析度").model(&gtk::StringList::new(&RESOLUTIONS)).selected(2).build();
        const MODES: [(&str, &str); 3] = [("彩色", "Color"), ("灰階", "Gray"), ("黑白", "Lineart")];
        let mode = adw::ComboRow::builder().title("種類").model(&gtk::StringList::new(&MODES.map(|(name, _)| name))).build();
        let group = adw::PreferencesGroup::new();
        group.add(&device);
        group.add(&resolution);
        group.add(&mode);
        let alert = adw::AlertDialog::new(Some("從掃描器輸入"), None);
        alert.set_extra_child(Some(&group));
        alert.add_responses(&[("cancel", "取消"), ("do", "掃描")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("do"));
        alert.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
            let Some(win) = weak.upgrade().filter(|_| response == "do") else { return };
            let name = devices[device.selected() as usize].0.clone();
            let dpi = RESOLUTIONS[resolution.selected() as usize];
            let kind = MODES[mode.selected() as usize].1;
            let out = scratch_dir().join(format!("scan-{}.png", glib::monotonic_time()));
            let _ = std::fs::create_dir_all(scratch_dir());
            win.title.set_subtitle("正在掃描⋯");
            let weak = Rc::downgrade(&win);
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking({
                    let out = out.clone();
                    move || Command::new("scanimage").args(["-d", &name, "--resolution", dpi, "--mode", kind, "--format=png", "-o"]).arg(&out).output()
                })
                .await;
                let Some(win) = weak.upgrade() else { return };
                if let Some(item) = win.item() {
                    win.update_title(&item);
                } else {
                    win.title.set_subtitle("");
                }
                match result {
                    Ok(Ok(output)) if output.status.success() => then(out),
                    Ok(Ok(output)) => alert::notice(&win.window, "無法掃描", Some(String::from_utf8_lossy(&output.stderr).trim())),
                    _ => alert::notice(&win.window, "無法掃描", None),
                }
            });
        });
    }

    /// 從掃描器輸入: the page scanned, as a picture saved nowhere yet.
    pub fn import_scanner(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.scan(move |path| {
            let Some(win) = weak.upgrade() else { return };
            match Pixbuf::from_file(&path) {
                Ok(pixbuf) => win.open_untitled("掃描.png", pixbuf),
                Err(err) => alert::notice(&win.window, "無法打開", Some(&err.to_string())),
            }
        });
    }

    /// 拍攝螢幕快照: a part of the screen chosen with `slurp`, or all of it, by `grim`.
    pub fn screenshot(self: &Rc<Self>, kind: &str) {
        let out = scratch_dir().join(format!("screenshot-{}.png", glib::monotonic_time()));
        let _ = std::fs::create_dir_all(scratch_dir());
        let area = kind == "area";
        let weak = Rc::downgrade(self);
        // Out of the way while the screen is taken.
        self.window.minimize();
        glib::spawn_future_local(async move {
            glib::timeout_future(std::time::Duration::from_millis(400)).await;
            let result = gio::spawn_blocking({
                let out = out.clone();
                move || -> Result<(), String> {
                    let mut grim = Command::new("grim");
                    if area {
                        let chosen = Command::new("slurp").output().map_err(|err| format!("slurp: {err}"))?;
                        if !chosen.status.success() {
                            return Err(String::new());
                        }
                        grim.arg("-g").arg(String::from_utf8_lossy(&chosen.stdout).trim());
                    }
                    let taken = grim.arg(&out).output().map_err(|err| format!("grim: {err}"))?;
                    if taken.status.success() {
                        Ok(())
                    } else {
                        Err(String::from_utf8_lossy(&taken.stderr).trim().to_owned())
                    }
                }
            })
            .await
            .unwrap_or_else(|_| Err(String::new()));
            let Some(win) = weak.upgrade() else { return };
            win.window.present();
            match result.and_then(|()| Pixbuf::from_file(&out).map_err(|err| err.to_string())) {
                Ok(pixbuf) => win.open_untitled("螢幕快照.png", pixbuf),
                // Escape while choosing is no failure.
                Err(err) if err.is_empty() => {}
                Err(err) => alert::notice(&win.window, "無法拍攝螢幕快照", Some(&err)),
            }
        });
    }

    /// 列印: the PDF's pages or the pictures chosen, each fitted to its sheet or at its size,
    /// turned to fit when that fits better.
    pub fn print(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let pdf = self.pdf.borrow().clone().filter(|_| item.is_page());
        let chosen = self.chosen_items();
        let pictures: Vec<Rc<Item>> = match pdf {
            Some(_) => Vec::new(),
            None => chosen.iter().filter_map(|&index| self.items.borrow().get(index).cloned()).collect(),
        };
        let (count, current) = match &pdf {
            Some(pdf) => (pdf.sizes.len(), item.page.map_or(0, |page| page as usize - 1)),
            None => (pictures.len(), chosen.iter().position(|&index| index == self.current.get()).unwrap_or(0)),
        };
        let weak = Rc::downgrade(self);
        // The picture drawn last, which the preview asks for again and again.
        let last: RefCell<Option<(usize, Pixbuf)>> = RefCell::default();
        let page = move |index: usize| -> Option<printing::Page> {
            match &pdf {
                Some(pdf) => {
                    let page = pdf.document().page(index as i32)?;
                    let size = page.size();
                    Some(printing::Page { size, draw: Box::new(move |cr| page.render_for_printing(cr)) })
                }
                None => {
                    let item = pictures.get(index)?;
                    let cached = last.borrow().as_ref().filter(|(cached, _)| *cached == index).map(|(_, pixbuf)| pixbuf.clone());
                    let pixbuf = match cached {
                        Some(pixbuf) => pixbuf,
                        None => {
                            let pixbuf = weak.upgrade()?.pixbuf_of(item)?;
                            *last.borrow_mut() = Some((index, pixbuf.clone()));
                            pixbuf
                        }
                    };
                    let dpi = item.dpi.get().unwrap_or(72.0);
                    let size = (f64::from(pixbuf.width()) * 72.0 / dpi, f64::from(pixbuf.height()) * 72.0 / dpi);
                    Some(printing::Page {
                        size,
                        draw: Box::new(move |cr| {
                            cr.scale(72.0 / dpi, 72.0 / dpi);
                            cr.set_source_pixbuf(&pixbuf, 0.0, 0.0);
                            let _ = cr.paint();
                        }),
                    })
                }
            }
        };
        printing::show(self.window.upcast_ref(), printing::Source { name: item.name(), count, current, page: Box::new(page) });
    }

    /// 丟到垃圾桶: the file shown, and its items with it.
    pub fn trash(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        match gio::File::for_path(&item.path).trash(gio::Cancellable::NONE) {
            Ok(()) => {
                let index = self.current.get();
                let items: Vec<Rc<Item>> = self.items.borrow().iter().filter(|other| other.path != item.path).cloned().collect();
                self.show_items(items, index);
            }
            Err(err) => alert::notice(&self.window, "無法丟到垃圾桶", Some(&err.to_string())),
        }
    }

    /// Shows `items`, or nothing when there are none left.
    fn show_items(self: &Rc<Self>, items: Vec<Rc<Item>>, index: usize) {
        if items.is_empty() {
            self.items.borrow_mut().clear();
            while let Some(row) = self.list.row_at_index(0) {
                self.list.remove(&row);
            }
            *self.pdf.borrow_mut() = None;
            *self.shown.borrow_mut() = None;
            self.split.set_show_sidebar(false);
            return self.show_nothing();
        }
        let index = index.min(items.len() - 1);
        self.set_items(items, index);
    }

    /// What has changes saved, where it can be, before it goes; `then` is called once that is
    /// done, or not when some cannot be saved and their changes are kept.
    fn save_before_going(self: &Rc<Self>, items: &[Rc<Item>], then: impl FnOnce() + 'static) {
        let mut unsaved = Vec::new();
        let mut pdf_saved = false;
        for item in items {
            match item.page {
                Some(_) => {
                    let pdf = self.pdf.borrow().clone().filter(|pdf| pdf.path == item.path && pdf.dirty.get());
                    if let (Some(pdf), false) = (pdf, pdf_saved) {
                        pdf_saved = true;
                        if item.untitled || !self.redactions.borrow().is_empty() || self.save_pdf(&pdf, &pdf.path).is_err() {
                            unsaved.push(item.name());
                        }
                    }
                }
                None if item.edited.borrow().is_some() || item.untitled => {
                    let written = can_write(&item.target()) && !item.untitled && self.pixbuf_of(item).is_some_and(|pixbuf| self.write(item, &pixbuf, &item.target()).is_ok());
                    if !written {
                        unsaved.push(item.name());
                    }
                }
                None => {}
            }
        }
        if unsaved.is_empty() {
            return then();
        }
        let title = match unsaved.as_slice() {
            [name] => format!("捨棄「{name}」的變更？"),
            _ => format!("捨棄 {} 個文件的變更？", unsaved.len()),
        };
        alert::confirm(&self.window, &title, Some("這些變更尚未儲存。"), "捨棄", then);
    }

    /// Closing the window saves what it has changed, as the Mac's Preview does.
    pub fn close_window(self: &Rc<Self>) {
        let items = self.items.borrow().clone();
        let weak = Rc::downgrade(self);
        self.save_before_going(&items, move || {
            if let Some(win) = weak.upgrade() {
                win.end_slideshow();
                win.closing.set(true);
                win.window.close();
            }
        });
    }

    /// 關閉所選項目: the items chosen in the sidebar taken out of the window.
    pub fn close_selected(self: &Rc<Self>) {
        let chosen = self.chosen_items();
        let items = self.items.borrow().clone();
        let going: Vec<Rc<Item>> = chosen.iter().filter_map(|&index| items.get(index).cloned()).collect();
        let first = chosen.first().copied().unwrap_or(0);
        let weak = Rc::downgrade(self);
        self.save_before_going(&going, move || {
            let Some(win) = weak.upgrade() else { return };
            let kept: Vec<Rc<Item>> = items.iter().enumerate().filter(|(index, _)| !chosen.contains(index)).map(|(_, item)| item.clone()).collect();
            if kept.is_empty() {
                win.closing.set(true);
                return win.window.close();
            }
            win.show_items(kept, first);
        });
    }
}

/// A PDF just written turned grey, made smaller or given a password, as 輸出 asks.
fn finish_pdf(path: &Path, grey: bool, smaller: bool, password: Option<&str>) -> Result<(), String> {
    let new = path.with_extension("pdf.out");
    if grey {
        pdfdoc::grayscale(path, &new)?;
        std::fs::rename(&new, path).map_err(|err| err.to_string())?;
    }
    if !smaller && password.is_none() {
        return Ok(());
    }
    if !pdfdoc::has_qpdf() {
        return Err(String::from("縮減檔案大小和加密需要 qpdf。"));
    }
    if smaller {
        pdfdoc::compact(path, None, &new, true, false)?;
        std::fs::rename(&new, path).map_err(|err| err.to_string())?;
    }
    if let Some(password) = password {
        let all = pdfdoc::Permissions { print: true, copy: true, assemble: true, annotate: true, forms: true };
        pdfdoc::protect(path, None, &new, password, password, all)?;
        std::fs::rename(&new, path).map_err(|err| err.to_string())?;
    }
    Ok(())
}
