//! Settings' 印表機 page, through CUPS's own commands: the printers there are, network ones
//! CUPS finds by itself among them, which is the default, what is waiting to print, and adding
//! a printer by its address.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::{alert, as_root, group, output, run};

/// The page CUPS prints to check a printer.
const TEST_PAGE: &str = "/usr/share/cups/data/testprint";

#[derive(Clone, Debug, PartialEq)]
struct Job {
    id: String,
    printer: String,
}

/// The names `lpstat -e` lists, one a line.
fn destinations(listed: &str) -> Vec<String> {
    listed.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect()
}

/// `lpstat -d`'s "system default destination: NAME".
fn default_destination(said: &str) -> Option<String> {
    said.lines().find_map(|line| line.split_once("destination:")).map(|(_, name)| name.trim().to_string())
}

/// How `lpstat -p` describes `printer`, in a word: idle, printing or stopped.
fn state(listed: &str, printer: &str) -> Option<&'static str> {
    let line = listed.lines().find(|line| line.starts_with(&format!("printer {printer} ")))?;
    Some(if line.contains("disabled") {
        "已停用"
    } else if line.contains("now printing") {
        "列印中"
    } else {
        "閒置"
    })
}

/// `lpstat -o`'s jobs: "PRINTER-ID user size date".
fn jobs(listed: &str) -> Vec<Job> {
    listed
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter_map(|id| {
            let (printer, number) = id.rsplit_once('-')?;
            number.parse::<u32>().ok()?;
            Some(Job { id: id.to_string(), printer: printer.to_string() })
        })
        .collect()
}

/// A queue name CUPS takes for an address typed in: letters, digits, - and _.
fn queue_name(address: &str) -> String {
    let name: String = address
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("Printer_{name}")
}

#[derive(Default)]
struct PrintersPage {
    groups: std::cell::RefCell<Vec<adw::PreferencesGroup>>,
}

pub fn printers_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    fill(&page, &Rc::new(PrintersPage::default()));
    page
}

fn fill(page: &adw::PreferencesPage, shown: &Rc<PrintersPage>) {
    for old in shown.groups.take() {
        page.remove(&old);
    }
    let refresh: Rc<dyn Fn()> = {
        let (page, shown) = (page.downgrade(), shown.clone());
        Rc::new(move || {
            if let Some(page) = page.upgrade() {
                fill(&page, &shown);
            }
        })
    };

    let printers = group("印表機");
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("新增印表機")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    add.connect_clicked(glib::clone!(
        #[strong]
        refresh,
        #[weak]
        page,
        move |_| add_printer(&page, refresh.clone())
    ));
    printers.set_header_suffix(Some(&add));
    let Some(listed) = output("lpstat", &["-e"]) else {
        add.set_visible(false);
        printers.add(&adw::ActionRow::builder().title("未安裝列印服務").css_classes(["dim-label"]).build());
        page.add(&printers);
        shown.groups.replace(vec![printers]);
        return;
    };
    let names = destinations(&listed);
    let default = output("lpstat", &["-d"]).as_deref().and_then(default_destination);
    let states = output("lpstat", &["-p"]).unwrap_or_default();
    if names.is_empty() {
        printers.add(&adw::ActionRow::builder().title("沒有印表機").css_classes(["dim-label"]).build());
    }
    for name in &names {
        let is_default = default.as_deref() == Some(name.as_str());
        let mut subtitle = state(&states, name).unwrap_or("可使用").to_string();
        if is_default {
            subtitle = format!("預設 · {subtitle}");
        }
        let row = adw::ExpanderRow::builder()
            .title(name.as_str())
            .subtitle(subtitle.as_str())
            .use_markup(false)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("printer-symbolic"));
        if !is_default {
            let make_default = adw::ActionRow::builder().title("設為預設").activatable(true).build();
            let (name, refresh) = (name.clone(), refresh.clone());
            // The person's own default, which needs no password; the system's stays as it was.
            make_default.connect_activated(move |_| {
                run("lpoptions", &["-d", &name]);
                refresh();
            });
            row.add_row(&make_default);
        }
        let test = adw::ActionRow::builder().title("列印測試頁").activatable(true).build();
        let printer = name.clone();
        test.connect_activated(glib::clone!(
            #[strong]
            refresh,
            move |row| {
                if output("lp", &["-d", &printer, TEST_PAGE]).is_none() {
                    alert::notice(row, "無法列印測試頁", None);
                }
                refresh();
            }
        ));
        row.add_row(&test);
        // Only a queue made on this computer can be removed; one CUPS found comes back.
        if state(&states, name).is_some() {
            let remove = adw::ActionRow::builder().title("移除印表機").activatable(true).build();
            remove.add_css_class("error");
            let (name, refresh) = (name.clone(), refresh.clone());
            remove.connect_activated(glib::clone!(
                #[weak]
                page,
                move |_| {
                    let (name, refresh) = (name.clone(), refresh.clone());
                    let parent = page.clone();
                    alert::confirm(&parent, &format!("移除「{name}」？"), None, "移除", move || {
                        as_root("移除印表機", &["lpadmin", "-x", &name], None, move |worked| {
                            if !worked {
                                alert::notice(&page, "無法移除印表機", Some("請確認密碼正確後再試一次。"));
                            }
                            refresh();
                        });
                    });
                }
            ));
            row.add_row(&remove);
        }
        printers.add(&row);
    }
    page.add(&printers);

    let waiting = jobs(&output("lpstat", &["-o"]).unwrap_or_default());
    let queue = group("列印工作");
    queue.set_visible(!waiting.is_empty());
    for job in waiting {
        let row = adw::ActionRow::builder()
            .title(job.id.as_str())
            .subtitle(job.printer.as_str())
            .use_markup(false)
            .build();
        let cancel = gtk::Button::builder().label("取消").valign(gtk::Align::Center).build();
        let refresh = refresh.clone();
        cancel.connect_clicked(move |button| {
            if output("cancel", &[&job.id]).is_none() {
                alert::notice(button, "無法取消列印工作", None);
            }
            refresh();
        });
        row.add_suffix(&cancel);
        queue.add(&row);
    }
    page.add(&queue);
    shown.groups.replace(vec![printers, queue]);
}

/// Adds a network printer by its address, as IPP Everywhere, which needs no driver.
fn add_printer(page: &adw::PreferencesPage, refresh: Rc<dyn Fn()>) {
    let page = page.clone();
    let field = alert::prompt(&page.clone(), "新增印表機", "", -1, "新增", move |address| {
        let name = queue_name(&address);
        let uri = format!("ipp://{address}/ipp/print");
        as_root(
            "新增印表機",
            &["lpadmin", "-p", &name, "-E", "-v", &uri, "-m", "everywhere"],
            None,
            move |worked| {
                if !worked {
                    alert::notice(&page, "無法新增印表機", Some("請確認位址正確，且印表機已開啟並連上網路。"));
                }
                refresh();
            },
        );
    });
    field.set_placeholder_text(Some("IP 位址或主機名稱"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cups_is_understood() {
        assert_eq!(destinations("Office\nHP_LaserJet\n"), ["Office", "HP_LaserJet"]);
        assert_eq!(default_destination("system default destination: Office\n").as_deref(), Some("Office"));
        assert_eq!(default_destination("no system default destination\n"), None);
        let states = "printer Office is idle.  enabled since Tue\nprinter HP disabled since Mon -\n\treason unknown\nprinter Lab now printing Lab-3.  enabled since Tue\n";
        assert_eq!(state(states, "Office"), Some("閒置"));
        assert_eq!(state(states, "HP"), Some("已停用"));
        assert_eq!(state(states, "Lab"), Some("列印中"));
        assert_eq!(state(states, "Other"), None);
        assert_eq!(
            jobs("Office-12  neko  1024  Tue 18 Nov\nHP_Laser-Jet-3 neko 5 Tue\n"),
            [
                Job { id: "Office-12".into(), printer: "Office".into() },
                Job { id: "HP_Laser-Jet-3".into(), printer: "HP_Laser-Jet".into() },
            ]
        );
        assert_eq!(queue_name("192.168.1.20"), "Printer_192_168_1_20");
    }
}
