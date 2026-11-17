//! Pictures of files: thumbnails as the freedesktop thumbnail spec keeps them, which Files,
//! the file chooser and every GTK program read too, made here for pictures, PDFs and videos
//! when none is kept yet; and pictures read whole for Quick Look and the gallery. Both are
//! read on a few threads of their own, never the window's.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
};

use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// The spec's large thumbnails.
const LARGE: i32 = 256;
const WORKERS: usize = 3;

enum Job {
    Thumbnail {
        uri: String,
        path: PathBuf,
        mtime: u64,
        content_type: String,
    },
    Picture {
        path: PathBuf,
        size: i32,
    },
}

enum Done {
    Thumbnail(String, Option<PathBuf>),
    Picture(u64, Option<Pixels>),
}

/// A picture's pixels, which unlike a Pixbuf can cross from a thread to the window's.
struct Pixels {
    bytes: glib::Bytes,
    width: i32,
    height: i32,
    stride: usize,
    alpha: bool,
}

impl Pixels {
    fn of(pixbuf: &Pixbuf) -> Pixels {
        Pixels {
            bytes: pixbuf.read_pixel_bytes(),
            width: pixbuf.width(),
            height: pixbuf.height(),
            stride: pixbuf.rowstride() as usize,
            alpha: pixbuf.has_alpha(),
        }
    }

    fn texture(&self) -> gdk::Texture {
        let format = if self.alpha {
            gdk::MemoryFormat::R8g8b8a8
        } else {
            gdk::MemoryFormat::R8g8b8
        };
        gdk::MemoryTexture::new(self.width, self.height, format, &self.bytes, self.stride).upcast()
    }
}

type Shown = Box<dyn FnOnce(Option<PathBuf>)>;
type Read = Box<dyn FnOnce(Option<gdk::Texture>)>;

thread_local! {
    static QUEUE: RefCell<Option<mpsc::Sender<(u64, Job)>>> = const { RefCell::new(None) };
    static NEXT: Cell<u64> = const { Cell::new(0) };
    static THUMBNAILS: RefCell<HashMap<String, Vec<Shown>>> = RefCell::new(HashMap::new());
    static PICTURES: RefCell<HashMap<u64, Read>> = RefCell::new(HashMap::new());
    /// What could not be made a thumbnail of, not tried again while the window is open.
    static FAILED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

fn queue(job: Job) -> u64 {
    let id = NEXT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    QUEUE.with_borrow_mut(|queue| {
        let sender = queue.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel::<(u64, Job)>();
            let receiver = Arc::new(Mutex::new(receiver));
            for _ in 0..WORKERS {
                let receiver = receiver.clone();
                std::thread::spawn(move || loop {
                    let Ok((id, job)) = receiver.lock().map_err(drop).and_then(|r| r.recv().map_err(drop))
                    else {
                        return;
                    };
                    let done = match job {
                        Job::Thumbnail {
                            uri,
                            path,
                            mtime,
                            content_type,
                        } => {
                            let made = make_thumbnail(&uri, &path, mtime, &content_type);
                            Done::Thumbnail(uri, made)
                        }
                        Job::Picture { path, size } => {
                            Done::Picture(id, read_picture(&path, size).as_ref().map(Pixels::of))
                        }
                    };
                    glib::MainContext::default().invoke(move || deliver(done));
                });
            }
            sender
        });
        if sender.send((id, job)).is_err() {
            eprintln!("nekoland-files: the picture threads have stopped");
        }
    });
    id
}

fn deliver(done: Done) {
    match done {
        Done::Thumbnail(uri, made) => {
            if made.is_none() {
                FAILED.with_borrow_mut(|failed| failed.insert(uri.clone()));
            }
            let waiting = THUMBNAILS
                .with_borrow_mut(|waiting| waiting.remove(&uri))
                .unwrap_or_default();
            for shown in waiting {
                shown(made.clone());
            }
        }
        Done::Picture(id, pixbuf) => {
            if let Some(read) = PICTURES.with_borrow_mut(|waiting| waiting.remove(&id)) {
                read(pixbuf.map(|pixels| pixels.texture()));
            }
        }
    }
}

/// Whether a thumbnail of this kind of file can be made here.
pub fn can_make(content_type: &str) -> bool {
    content_type.starts_with("image/")
        || content_type.starts_with("video/")
        || content_type == "application/pdf"
}

/// Shows a file's thumbnail in `picture`, with its icon in `image` until there is one or
/// when there can be none. `picture` remembers whose it is: a list reuses it for another
/// file, and a thumbnail made meanwhile must not land on that one.
pub fn show(picture: &gtk::Picture, image: &gtk::Image, info: &gio::FileInfo, file: Option<&gio::File>) {
    image.set_from_gicon(&icon(info));
    picture.set_paintable(None::<&gdk::Paintable>);
    picture.set_visible(false);
    image.set_visible(true);
    let Some(file) = file else { return };
    let uri = file.uri().to_string();
    picture.set_widget_name(&uri);
    let set = {
        let (picture, image, uri) = (picture.downgrade(), image.downgrade(), uri.clone());
        move |path: Option<PathBuf>| {
            let (Some(picture), Some(image), Some(path)) = (picture.upgrade(), image.upgrade(), path) else {
                return;
            };
            if picture.widget_name() == uri {
                picture.set_filename(Some(&path));
                picture.set_visible(true);
                image.set_visible(false);
            }
        }
    };
    if let Some(kept) = info
        .attribute_byte_string("thumbnail::path")
        .map(PathBuf::from)
        .filter(|path| path.exists())
    {
        set(Some(kept));
        return;
    }
    let content_type = info.content_type().map(|t| t.to_string()).unwrap_or_default();
    let (Some(path), true) = (file.path(), can_make(&content_type)) else {
        return;
    };
    if FAILED.with_borrow(|failed| failed.contains(&uri)) {
        return;
    }
    let first = THUMBNAILS.with_borrow_mut(|waiting| {
        let list = waiting.entry(uri.clone()).or_default();
        list.push(Box::new(set));
        list.len() == 1
    });
    if first {
        let mtime = info
            .modification_date_time()
            .map_or(0, |t| t.to_unix().max(0) as u64);
        queue(Job::Thumbnail {
            uri,
            path,
            mtime,
            content_type,
        });
    }
}

/// Reads a picture file whole, at most `size` pixels on its longer side, turned as its
/// camera says.
pub fn read(path: &Path, size: i32, read: impl FnOnce(Option<gdk::Texture>) + 'static) {
    let id = queue(Job::Picture {
        path: path.to_path_buf(),
        size,
    });
    PICTURES.with_borrow_mut(|waiting| waiting.insert(id, Box::new(read)));
}

pub fn icon(info: &gio::FileInfo) -> gio::Icon {
    info.icon()
        .unwrap_or_else(|| gio::ThemedIcon::new("text-x-generic").upcast())
}

fn read_picture(path: &Path, size: i32) -> Option<Pixbuf> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return read_page(path, size);
    }
    let pixbuf = Pixbuf::from_file_at_scale(path, size, size, true).ok()?;
    Some(pixbuf.apply_embedded_orientation().unwrap_or(pixbuf))
}

/// A PDF's first page, by pdftoppm.
fn read_page(path: &Path, size: i32) -> Option<Pixbuf> {
    // A name of its own per call, as the workers read pages at once.
    static PAGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let page = PAGES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let prefix = std::env::temp_dir().join(format!("nekoland-files-page-{}-{page}", std::process::id()));
    let png = prefix.with_extension("png");
    let made = run(Command::new("pdftoppm")
        .args(["-png", "-singlefile", "-f", "1", "-l", "1", "-scale-to"])
        .arg(size.to_string())
        .arg(path)
        .arg(&prefix));
    let pixbuf = made.then(|| Pixbuf::from_file(&png).ok()).flatten();
    let _ = std::fs::remove_file(&png);
    pixbuf
}

fn make_thumbnail(uri: &str, path: &Path, mtime: u64, content_type: &str) -> Option<PathBuf> {
    let folder = glib::user_cache_dir().join("thumbnails/large");
    let name = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri)?;
    let out = folder.join(format!("{name}.png"));
    let mtime = mtime.to_string();
    if let Ok(kept) = Pixbuf::from_file(&out) {
        if kept.option("tEXt::Thumb::MTime").as_deref() == Some(mtime.as_str()) {
            return Some(out);
        }
    }
    std::fs::create_dir_all(&folder).ok()?;
    let pixbuf = if content_type.starts_with("image/") || content_type == "application/pdf" {
        read_picture(path, LARGE)?
    } else {
        // A frame of the video, by ffmpeg, which install.sh adds.
        let frame_png = folder.join(format!(".{name}.frame.png"));
        // Two seconds in, past a fade from black; a shorter video from its start.
        let made = {
            ["2", "0"].iter().find_map(|at| {
                run(Command::new("ffmpeg")
                    .args(["-v", "error", "-nostdin", "-ss", at, "-i"])
                    .arg(path)
                    .args(["-frames:v", "1", "-vf", "scale=256:-2", "-y"])
                    .arg(&frame_png))
                .then(|| frame_png.clone())
                .filter(|p| p.exists())
            })
        };
        let pixbuf = made.and_then(|frame| Pixbuf::from_file_at_scale(frame, LARGE, LARGE, true).ok());
        let _ = std::fs::remove_file(&frame_png);
        pixbuf?
    };
    let temporary = folder.join(format!(".{name}.png.part"));
    pixbuf
        .savev(
            &temporary,
            "png",
            &[("tEXt::Thumb::URI", uri), ("tEXt::Thumb::MTime", &mtime)],
        )
        .ok()?;
    std::fs::rename(&temporary, &out).ok()?;
    Some(out)
}

fn run(command: &mut Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
