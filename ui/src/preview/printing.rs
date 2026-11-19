//! 列印: the dialog that asks which printer, which pages and how many, with the page shown as
//! it will be printed, and the printing itself. GTK's own print dialog is not used; the
//! printers, the paper and the printing are GTK's.

use std::sync::{Arc, Mutex};

use gtk::cairo;
use gtk::glib::translate::ToGlibPtr;

use super::*;

/// A page to print: its size in points and how to draw it there.
pub struct Page {
    pub size: (f64, f64),
    pub draw: Box<dyn Fn(&cairo::Context)>,
}

/// What is printed, page by page.
pub struct Source {
    pub name: String,
    pub count: usize,
    /// The page the window shows, 0-based.
    pub current: usize,
    pub page: Box<dyn Fn(usize) -> Option<Page>>,
}

/// What 列印 was asked for.
struct Job {
    printer: String,
    to_file: bool,
    /// The pages in the order they are printed, 0-based.
    pages: Vec<usize>,
    copies: i32,
    collate: bool,
    fit: bool,
    rotate: bool,
    /// Pages printed on each sheet.
    per_sheet: i32,
    setup: gtk::PageSetup,
}

/// How many pages a sheet can take.
const PER_SHEET: [i32; 6] = [1, 2, 4, 6, 9, 16];

#[derive(Clone, Copy, PartialEq)]
enum Range {
    All,
    Current,
    Pages,
}

struct PrinterInfo {
    name: String,
    title: String,
    detail: String,
    to_file: bool,
    default: bool,
}

/// Draws a page of `size` points on a sheet `width` by `height` of which a point is
/// `units_per_point`: fitted to it, or as large as it is, turned when that fits better.
fn place(cr: &cairo::Context, (width, height): (f64, f64), units_per_point: f64, size: (f64, f64), fit: bool, rotate: bool, draw: &dyn Fn(&cairo::Context)) {
    let turned = rotate && (size.0 > size.1) != (width > height);
    let (w, h) = if turned { (size.1, size.0) } else { size };
    let scale = if fit { (width / w).min(height / h) } else { units_per_point };
    cr.translate((width - w * scale) / 2.0, (height - h * scale) / 2.0);
    cr.scale(scale, scale);
    if turned {
        cr.translate(w, 0.0);
        cr.rotate(std::f64::consts::FRAC_PI_2);
    }
    draw(cr);
}

/// The columns and rows `per_sheet` pages take on a sheet, more of them across a wide one.
fn grid(per_sheet: usize, wide: bool) -> (usize, usize) {
    let (across, down) = match per_sheet {
        2 => (1, 2),
        6 => (2, 3),
        n => {
            let side = (n as f64).sqrt().ceil() as usize;
            (side, side)
        }
    };
    if wide { (down, across) } else { (across, down) }
}

/// Draws `pages` on a sheet `size` units wide and high, across then down a row at a time,
/// each fitted to its share of the sheet when there is more than one.
fn place_sheet(cr: &cairo::Context, size: (f64, f64), units_per_point: f64, pages: &[Page], per_sheet: usize, fit: bool, rotate: bool) {
    if per_sheet <= 1 {
        if let Some(page) = pages.first() {
            place(cr, size, units_per_point, page.size, fit, rotate, &*page.draw);
        }
        return;
    }
    let (across, down) = grid(per_sheet, size.0 > size.1);
    let cell = (size.0 / across as f64, size.1 / down as f64);
    for (at, page) in pages.iter().enumerate().take(per_sheet) {
        cr.save().ok();
        cr.translate((at % across) as f64 * cell.0, (at / across) as f64 * cell.1);
        cr.rectangle(0.0, 0.0, cell.0, cell.1);
        cr.clip();
        place(cr, cell, units_per_point, page.size, true, rotate, &*page.draw);
        cr.restore().ok();
    }
}

/// The pages `text` names, as "1-3, 5" does, 0-based and in that order; none if it names one
/// that is not there.
fn parse_pages(text: &str, count: usize) -> Option<Vec<usize>> {
    let mut pages = Vec::new();
    for part in text.split([',', '，']) {
        let (first, last) = match part.split_once('-') {
            Some((first, last)) => (first.trim().parse::<usize>().ok()?, last.trim().parse::<usize>().ok()?),
            None => {
                let page = part.trim().parse::<usize>().ok()?;
                (page, page)
            }
        };
        if first == 0 || first > last || last > count {
            return None;
        }
        pages.extend(first - 1..last);
    }
    Some(pages)
}

/// What `get` says of `printer`, empty when it says nothing: the getters of its description and
/// location are not allowed their NULL, which 列印到檔案 has.
fn printer_text(printer: &gtk::Printer, get: unsafe extern "C" fn(*mut gtk::ffi::GtkPrinter) -> *const std::ffi::c_char) -> String {
    // SAFETY: `get` is called with a live printer and a non-NULL answer is a NUL-terminated string it owns.
    unsafe {
        let text = get(printer.to_glib_none().0);
        if text.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned()
        }
    }
}

/// The printers GTK knows, the default first and 列印到檔案 last.
fn printers() -> Vec<PrinterInfo> {
    let found = Arc::new(Mutex::new(Vec::new()));
    let sink = found.clone();
    gtk::enumerate_printers(
        move |printer| {
            let to_file = printer.is_virtual();
            let status = if !printer.is_accepting_jobs() {
                "已停用"
            } else if printer.is_paused() {
                "已暫停"
            } else if printer.job_count() > 0 {
                "列印中"
            } else {
                "閒置"
            };
            let (description, location) = (printer_text(printer, gtk::ffi::gtk_printer_get_description), printer_text(printer, gtk::ffi::gtk_printer_get_location));
            let detail = [location, if to_file { String::new() } else { status.to_owned() }];
            sink.lock().unwrap().push(PrinterInfo {
                name: printer.name().to_string(),
                title: if to_file { "列印到檔案".to_owned() } else if description.is_empty() { printer.name().to_string() } else { description },
                detail: detail.iter().filter(|part| !part.is_empty()).cloned().collect::<Vec<_>>().join(" · "),
                to_file,
                default: printer.is_default(),
            });
            false
        },
        true,
    );
    let mut found = std::mem::take(&mut *found.lock().unwrap());
    found.sort_by(|a, b| (a.to_file, !a.default, &a.title).cmp(&(b.to_file, !b.default, &b.title)));
    found
}

/// "A4 直向".
fn describe(setup: &gtk::PageSetup) -> String {
    let turned = matches!(setup.orientation(), gtk::PageOrientation::Landscape | gtk::PageOrientation::ReverseLandscape);
    format!("{} {}", setup.paper_size().display_name(), if turned { "橫向" } else { "直向" })
}

/// Linked buttons of which one is on, as a choice of a few words.
fn choice(labels: &[&str], on: usize) -> (gtk::Box, Vec<gtk::ToggleButton>) {
    let row = gtk::Box::builder().css_classes(["linked"]).valign(gtk::Align::Center).build();
    let mut buttons: Vec<gtk::ToggleButton> = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let button = gtk::ToggleButton::builder().label(*label).active(index == on).hexpand(true).build();
        if let Some(first) = buttons.first() {
            button.set_group(Some(first));
        }
        row.append(&button);
        buttons.push(button);
    }
    (row, buttons)
}

/// Asks what to print, then prints it.
pub fn show(window: &gtk::Window, source: Source) {
    let source = Rc::new(source);
    let count = source.count;
    let setup = Rc::new(RefCell::new(gtk::PageSetup::new()));
    let shown = Rc::new(Cell::new(source.current.min(count.saturating_sub(1))));
    let fit = Rc::new(Cell::new(true));
    let rotate = Rc::new(Cell::new(true));
    let per_sheet = Rc::new(Cell::new(1_usize));

    // The sheet the page shown is on, as it will be printed.
    let preview = gtk::DrawingArea::builder().hexpand(true).vexpand(true).content_width(240).content_height(320).build();
    preview.set_draw_func({
        let (source, setup, shown, fit, rotate, per_sheet) = (source.clone(), setup.clone(), shown.clone(), fit.clone(), rotate.clone(), per_sheet.clone());
        move |_, cr, width, height| {
            let setup = setup.borrow();
            let (paper_w, paper_h) = (setup.paper_width(gtk::Unit::Points), setup.paper_height(gtk::Unit::Points));
            let (area_w, area_h) = (f64::from(width) - f64::from(EDGE) * 2.0, f64::from(height) - f64::from(EDGE) * 2.0);
            let scale = (area_w / paper_w).min(area_h / paper_h);
            cr.translate((f64::from(width) - paper_w * scale) / 2.0, (f64::from(height) - paper_h * scale) / 2.0);
            cr.scale(scale, scale);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.rectangle(0.0, 0.0, paper_w, paper_h);
            let _ = cr.fill_preserve();
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.3);
            cr.set_line_width(1.0 / scale);
            let _ = cr.stroke();
            let printable = (setup.page_width(gtk::Unit::Points), setup.page_height(gtk::Unit::Points));
            cr.translate(setup.left_margin(gtk::Unit::Points), setup.top_margin(gtk::Unit::Points));
            cr.rectangle(0.0, 0.0, printable.0, printable.1);
            cr.clip();
            let per_sheet = per_sheet.get();
            let start = shown.get() / per_sheet * per_sheet;
            let pages: Vec<Page> = (start..(start + per_sheet).min(count)).filter_map(|page| (source.page)(page)).collect();
            place_sheet(cr, printable, 1.0, &pages, per_sheet, fit.get(), rotate.get());
        }
    });
    let previous = gtk::Button::builder().icon_name("go-previous-symbolic").css_classes(["flat"]).build();
    let next = gtk::Button::builder().icon_name("go-next-symbolic").css_classes(["flat"]).build();
    let number = gtk::Label::builder().css_classes(["numeric"]).width_chars(7).build();
    let paper = gtk::Label::builder().css_classes(["dim-label"]).build();
    let pager = gtk::Box::builder().halign(gtk::Align::Center).visible(count > 1).build();
    pager.append(&previous);
    pager.append(&number);
    pager.append(&next);
    let turn = Rc::new(glib::clone!(
        #[weak]
        preview,
        #[weak]
        previous,
        #[weak]
        next,
        #[weak]
        number,
        #[strong]
        shown,
        move || {
            number.set_text(&format!("{} / {count}", shown.get() + 1));
            previous.set_sensitive(shown.get() > 0);
            next.set_sensitive(shown.get() + 1 < count);
            preview.queue_draw();
        }
    ));
    for (button, step) in [(&previous, -1_i64), (&next, 1)] {
        button.connect_clicked({
            let (shown, turn) = (shown.clone(), turn.clone());
            move |_| {
                shown.set((shown.get() as i64 + step).clamp(0, count as i64 - 1) as usize);
                turn();
            }
        });
    }
    let side = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(GAP).margin_bottom(EDGE).width_request(280).css_classes(["view"]).build();
    side.append(&preview);
    side.append(&pager);
    side.append(&paper);

    // The printers.
    let printers = printers();
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["boxed-list"]).build();
    let marks: Vec<gtk::Image> = printers
        .iter()
        .map(|printer| {
            let row = adw::ActionRow::builder().title(&printer.title).subtitle(&printer.detail).activatable(true).build();
            row.add_prefix(&gtk::Image::from_icon_name(if printer.to_file { "document-save-symbolic" } else { "printer-symbolic" }));
            let mark = gtk::Image::from_icon_name("object-select-symbolic");
            row.add_suffix(&mark);
            list.append(&row);
            mark
        })
        .collect();
    let printers_group = adw::PreferencesGroup::builder().title("印表機").build();
    printers_group.add(&list);

    // The range.
    let (range_row, range_buttons) = choice(&["全部", "目前頁面", "頁面"], 0);
    let range_item = adw::PreferencesRow::builder().activatable(false).child(&range_row).build();
    range_row.set_margin_top(GAP);
    range_row.set_margin_bottom(GAP);
    range_row.set_margin_start(EDGE);
    range_row.set_margin_end(EDGE);
    let pages_row = adw::EntryRow::builder().title("頁面").visible(false).build();
    let range_group = adw::PreferencesGroup::builder().title("範圍").build();
    range_group.add(&range_item);
    range_group.add(&pages_row);

    // The copies.
    let copies_row = adw::SpinRow::builder().title("份數").adjustment(&gtk::Adjustment::new(1.0, 1.0, 999.0, 1.0, 10.0, 0.0)).build();
    let collate_row = adw::SwitchRow::builder().title("逐份列印").active(true).build();
    let reverse_row = adw::SwitchRow::builder().title("反向順序").build();
    let copies_group = adw::PreferencesGroup::builder().title("份數").build();
    copies_group.add(&copies_row);
    copies_group.add(&collate_row);
    copies_group.add(&reverse_row);

    // How the page sits on the sheet.
    let (size_buttons_row, size_buttons) = choice(&["縮放至符合", "實際大小"], 0);
    let size_row = adw::ActionRow::builder().title("大小").build();
    size_row.add_suffix(&size_buttons_row);
    let rotate_row = adw::SwitchRow::builder().title("自動旋轉").active(true).build();
    let per_sheet_row = adw::ComboRow::builder().title("每張紙的頁數").model(&gtk::StringList::new(&PER_SHEET.map(|count| count.to_string()).iter().map(String::as_str).collect::<Vec<_>>())).build();
    let setup_row = adw::ActionRow::builder().title("頁面設定").activatable(true).build();
    setup_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let layout_group = adw::PreferencesGroup::builder().title("版面").build();
    layout_group.add(&size_row);
    layout_group.add(&rotate_row);
    layout_group.add(&per_sheet_row);
    layout_group.add(&setup_row);

    let settings_page = adw::PreferencesPage::new();
    for group in [&printers_group, &range_group, &copies_group, &layout_group] {
        settings_page.add(group);
    }

    // The header: cancel, what is printed, and print.
    let cancel = gtk::Button::builder().label("取消").build();
    let print = gtk::Button::builder().label("列印").css_classes(["suggested-action"]).build();
    let header = adw::HeaderBar::builder().show_start_title_buttons(false).show_end_title_buttons(false).title_widget(&adw::WindowTitle::new("列印", &source.name)).build();
    header.pack_start(&cancel);
    header.pack_end(&print);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&side);
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    settings_page.set_hexpand(true);
    body.append(&settings_page);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&body));
    let dialog = adw::Dialog::builder().title("列印").content_width(800).content_height(600).child(&view).default_widget(&print).focus_widget(&print).build();

    let range = {
        let buttons = range_buttons.clone();
        move || [Range::All, Range::Current, Range::Pages][buttons.iter().position(|button| button.is_active()).unwrap_or(0)]
    };
    // The pages chosen, none while what is typed does not name them.
    let chosen_pages = {
        let (range, pages_row, source) = (range.clone(), pages_row.clone(), source.clone());
        move || match range() {
            Range::All => Some((0..count).collect::<Vec<_>>()),
            Range::Current => Some(vec![source.current]),
            Range::Pages => parse_pages(&pages_row.text(), count),
        }
    };
    let to_file: Vec<bool> = printers.iter().map(|printer| printer.to_file).collect();
    let selected = {
        let list = list.clone();
        move || list.selected_row().map(|row| row.index() as usize)
    };
    // What the choices so far decide: what shows, what can be asked and whether to print.
    let update = Rc::new({
        let (print, pages_row, copies_group, paper, preview) = (print.clone(), pages_row.clone(), copies_group.clone(), paper.clone(), preview.clone());
        let (setup, chosen_pages, range, selected, to_file) = (setup.clone(), chosen_pages.clone(), range.clone(), selected.clone(), to_file.clone());
        move || {
            let pages = chosen_pages();
            pages_row.set_visible(range() == Range::Pages);
            if pages.is_some() {
                pages_row.remove_css_class("error");
            } else {
                pages_row.add_css_class("error");
            }
            let printer = selected();
            for (index, mark) in marks.iter().enumerate() {
                mark.set_visible(Some(index) == printer);
            }
            copies_group.set_sensitive(printer.is_some_and(|index| !to_file[index]));
            print.set_sensitive(pages.is_some() && printer.is_some());
            paper.set_text(&describe(&setup.borrow()));
            preview.queue_draw();
        }
    });
    list.connect_row_selected({
        let update = update.clone();
        move |_, _| update()
    });
    for button in &range_buttons {
        button.connect_toggled({
            let update = update.clone();
            move |_| update()
        });
    }
    pages_row.connect_changed({
        let update = update.clone();
        move |_| update()
    });
    size_buttons[0].connect_toggled({
        let (fit, update) = (fit.clone(), update.clone());
        move |button| {
            fit.set(button.is_active());
            update();
        }
    });
    per_sheet_row.connect_selected_notify({
        let (per_sheet, update) = (per_sheet.clone(), update.clone());
        move |row| {
            per_sheet.set(PER_SHEET[row.selected() as usize] as usize);
            update();
        }
    });
    rotate_row.connect_active_notify({
        let (rotate, update) = (rotate.clone(), update.clone());
        move |row| {
            rotate.set(row.is_active());
            update();
        }
    });
    setup_row.connect_activated({
        let (setup, update, dialog_parent) = (setup.clone(), update.clone(), window.clone());
        move |_| {
            let chooser = gtk::PageSetupUnixDialog::new(Some("頁面設定"), Some(&dialog_parent));
            chooser.set_modal(true);
            chooser.set_page_setup(&setup.borrow());
            let (setup, update) = (setup.clone(), update.clone());
            // GtkDialog is deprecated, and GTK's page setup is one.
            #[allow(deprecated)]
            chooser.connect_response(move |chooser, response| {
                if response == gtk::ResponseType::Ok {
                    *setup.borrow_mut() = chooser.page_setup();
                    update();
                }
                chooser.destroy();
            });
            chooser.present();
        }
    });
    cancel.connect_clicked({
        let dialog = dialog.clone();
        move |_| {
            dialog.close();
        }
    });
    print.connect_clicked({
        let (dialog, window, source, setup) = (dialog.clone(), window.clone(), source.clone(), setup.clone());
        let (fit, rotate, collate_row, reverse_row, copies_row, list, per_sheet_row) = (fit.clone(), rotate.clone(), collate_row.clone(), reverse_row.clone(), copies_row.clone(), list.clone(), per_sheet_row.clone());
        move |_| {
            let (Some(mut pages), Some(index)) = (chosen_pages(), list.selected_row().map(|row| row.index() as usize)) else { return };
            if reverse_row.is_active() {
                pages.reverse();
            }
            let job = Job {
                printer: printers[index].name.clone(),
                to_file: printers[index].to_file,
                pages,
                copies: copies_row.value() as i32,
                collate: collate_row.is_active(),
                fit: fit.get(),
                rotate: rotate.get(),
                per_sheet: PER_SHEET[per_sheet_row.selected() as usize],
                setup: setup.borrow().clone(),
            };
            dialog.close();
            start(&window, source.clone(), job);
        }
    });

    turn();
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }
    update();
    dialog.present(Some(window));
}

/// Prints `job`; to a file, the one asked for first.
fn start(window: &gtk::Window, source: Rc<Source>, job: Job) {
    if !job.to_file {
        return run(window, source, job, None);
    }
    let stem = Path::new(&source.name).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let dialog = gtk::FileDialog::builder().title("列印到檔案").initial_name(format!("{stem}.pdf")).build();
    let parent = window.clone();
    dialog.save(Some(window), gio::Cancellable::NONE, move |chosen| {
        let Some(path) = chosen.ok().and_then(|file| file.path()) else { return };
        run(&parent, source, job, Some(path));
    });
}

fn run(window: &gtk::Window, source: Rc<Source>, job: Job, file: Option<PathBuf>) {
    let operation = gtk::PrintOperation::new();
    operation.set_job_name(&source.name);
    operation.set_default_page_setup(Some(&job.setup));
    let settings = gtk::PrintSettings::new();
    settings.set_printer(&job.printer);
    settings.set_n_copies(job.copies);
    settings.set_collate(job.collate);
    operation.set_print_settings(Some(&settings));
    // GTK lays several pages on a sheet only for some printers, never in a file, so it is done here.
    let per_sheet = job.per_sheet as usize;
    operation.set_n_pages(job.pages.len().div_ceil(per_sheet) as i32);
    operation.connect_draw_page(move |_, context, index| {
        let start = index as usize * per_sheet;
        let pages: Vec<Page> = job.pages.iter().skip(start).take(per_sheet).filter_map(|&page| (source.page)(page)).collect();
        let size = (context.width(), context.height());
        place_sheet(&context.cairo_context(), size, context.dpi_x() / 72.0, &pages, per_sheet, job.fit, job.rotate);
    });
    // Not blocking the window, nor anything else waiting on it, while it prints.
    operation.set_allow_async(true);
    let parent = window.clone();
    operation.connect_done(move |operation, result| {
        if result == gtk::PrintOperationResult::Error {
            let message = operation.error().map(|err| err.to_string());
            alert::notice(&parent, "無法列印", message.as_deref());
        }
    });
    let action = match &file {
        Some(path) => {
            operation.set_export_filename(path);
            gtk::PrintOperationAction::Export
        }
        None => gtk::PrintOperationAction::Print,
    };
    if let Err(err) = operation.run(action, Some(window)) {
        alert::notice(window, "無法列印", Some(&err.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::{grid, parse_pages};

    #[test]
    fn pages_on_a_sheet_lie_in_a_grid_turned_with_it() {
        assert_eq!(grid(1, false), (1, 1));
        assert_eq!(grid(2, false), (1, 2));
        assert_eq!(grid(2, true), (2, 1));
        assert_eq!(grid(4, false), (2, 2));
        assert_eq!(grid(6, false), (2, 3));
        assert_eq!(grid(6, true), (3, 2));
        assert_eq!(grid(9, false), (3, 3));
        assert_eq!(grid(16, false), (4, 4));
    }

    #[test]
    fn pages_are_named_by_numbers_and_ranges() {
        assert_eq!(parse_pages("1-3, 5", 12), Some(vec![0, 1, 2, 4]));
        assert_eq!(parse_pages("2，4-4", 12), Some(vec![1, 3]));
        assert_eq!(parse_pages("", 12), None);
        assert_eq!(parse_pages("0", 12), None);
        assert_eq!(parse_pages("3-2", 12), None);
        assert_eq!(parse_pages("1-13", 12), None);
    }
}
