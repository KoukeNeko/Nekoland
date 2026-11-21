//! The Updates page, through unattended-upgrades and apt.

use super::*;

/// The software updates there are, counted by apt, which Market installs, and whether the
/// system installs security updates by itself.
pub(crate) fn updates_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let nekoland = group("");
    nekoland.add(&selfupdate::row(None));
    page.add(&nekoland);
    let updates = group("");
    let available = adw::ActionRow::builder().title("可用的更新").subtitle("正在檢查").build();
    let open = gtk::Button::builder().label("在 Market 中更新").valign(gtk::Align::Center).build();
    open.connect_clicked(|_| {
        if let Err(err) = Command::new("nekoland-store").env("NEKOLAND_STORE_PAGE", "updates").spawn() {
            eprintln!("nekoland-store: {err}");
        }
    });
    available.add_suffix(&open);
    updates.add(&available);
    match gio::Subprocess::newv(
        &["apt".as_ref(), "list".as_ref(), "--upgradable".as_ref()],
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    ) {
        Ok(apt) => apt.communicate_utf8_async(None, gio::Cancellable::NONE, glib::clone!(
            #[weak]
            available,
            move |result| {
                let count = result.ok().and_then(|(listed, _)| listed).map_or(0, |listed| upgradable(&listed));
                available.set_subtitle(&if count == 0 { "已是最新版本".to_string() } else { format!("{count} 個套件") });
            }
        )),
        Err(err) => {
            eprintln!("apt: {err}");
            available.set_subtitle("無法檢查");
        }
    }

    if Path::new(UNATTENDED).exists() {
        let automatic = adw::SwitchRow::builder()
            .title("自動安裝安全性更新")
            .active(
                std::fs::read_to_string(AUTO_UPGRADES)
                    .is_ok_and(|text| text.contains("Unattended-Upgrade \"1\"")),
            )
            .build();
        automatic.connect_active_notify(|row| {
            let on = if row.is_active() { "1" } else { "0" };
            let script = format!(
                "printf 'APT::Periodic::Update-Package-Lists \"1\";\\nAPT::Periodic::Unattended-Upgrade \"%s\";\\n' \"$1\" > {AUTO_UPGRADES}"
            );
            row.set_sensitive(false);
            let row = row.clone();
            as_root("變更自動更新", &["sh", "-c", &script, "sh", on], None, move |worked| {
                row.set_sensitive(true);
                if !worked {
                    alert::notice(&row, "無法變更自動更新", Some("請確認密碼正確後再試一次。"));
                }
            });
        });
        updates.add(&automatic);
        updates.add(&pause_row());
    }
    page.add(&updates);
    if let Some((history, latest)) = history_group() {
        let more = group("");
        more.add(&subpage_row("更新記錄", move || latest.clone(), page_of_groups(&[history])));
        page.add(&more);
    }
    page
}

/// The shell that takes a pause away: its drop-in, which keeps automatic updates off, and the
/// timer that ends it.
pub(crate) fn end_pause() -> String {
    format!(
        "systemctl disable --now nekoland-resume-updates.timer 2>/dev/null; rm -f {PAUSE_FILE} {RESUME_TIMER} {RESUME_SERVICE}; systemctl daemon-reload"
    )
}

/// The day a pause ends, as the timer it left says.
pub(crate) fn paused_until() -> Option<String> {
    let timer = std::fs::read_to_string(RESUME_TIMER).ok()?;
    timer.lines().find_map(|line| line.strip_prefix("OnCalendar=")).map(str::to_string)
}

/// 暫停更新: automatic updates kept off for some weeks by a drop-in a timer takes away
/// again, so that nothing but the system has to remember.
pub(crate) fn pause_row() -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title("暫停更新")
        .model(&gtk::StringList::new(&["不暫停", "1 週", "2 週", "5 週"]))
        .build();
    let until = |day: Option<String>| day.map(|day| format!("至 {day}")).unwrap_or_default();
    row.set_subtitle(&until(paused_until()));
    row.connect_selected_notify(move |row| {
        let Some(weeks) = PAUSE_WEEKS.get(row.selected() as usize).copied() else { return };
        if weeks == 0 && paused_until().is_none() {
            return;
        }
        let script = if weeks == 0 {
            end_pause()
        } else {
            format!(
                "set -e; day=$(date -d \"+$1 weeks\" +%Y-%m-%d); \
                 printf 'APT::Periodic::Unattended-Upgrade \"0\";\\n' > {PAUSE_FILE}; \
                 printf '[Unit]\\nDescription=End the pause of automatic updates\\n[Service]\\nType=oneshot\\nExecStart=/bin/rm -f {PAUSE_FILE}\\nExecStartPost=/usr/bin/systemctl disable nekoland-resume-updates.timer\\nExecStartPost=/bin/rm -f {RESUME_TIMER} {RESUME_SERVICE}\\n' > {RESUME_SERVICE}; \
                 printf '[Unit]\\nDescription=End the pause of automatic updates\\n[Timer]\\nOnCalendar=%s\\nPersistent=true\\n[Install]\\nWantedBy=timers.target\\n' \"$day\" > {RESUME_TIMER}; \
                 systemctl daemon-reload; systemctl enable --now nekoland-resume-updates.timer"
            )
        };
        row.set_sensitive(false);
        let row = row.clone();
        as_root("暫停更新", &["sh", "-c", &script, "sh", &weeks.to_string()], None, move |worked| {
            row.set_sensitive(true);
            if worked {
                row.set_subtitle(&until(paused_until()));
            } else {
                alert::notice(&row, "無法暫停更新", Some("請確認密碼正確後再試一次。"));
            }
        });
    });
    row
}

/// One run of apt, as its history lists it.
#[derive(Debug, PartialEq)]
pub(crate) struct AptRun {
    pub(crate) date: String,
    pub(crate) installed: usize,
    pub(crate) upgraded: usize,
    pub(crate) removed: usize,
    pub(crate) packages: Vec<String>,
}

/// The runs in apt's history that changed packages, newest first.
pub(crate) fn parse_apt_history(text: &str) -> Vec<AptRun> {
    let names = |list: &str| -> Vec<String> {
        list.split("), ")
            .filter_map(|package| package.split(" (").next())
            .map(|name| name.split(':').next().unwrap_or(name).to_string())
            .collect()
    };
    let mut runs: Vec<AptRun> = text
        .split("\n\n")
        .filter_map(|block| {
            let field = |key: &str| block.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(": "));
            let date = field("Start-Date")?.split_whitespace().next()?.to_string();
            let (install, upgrade, remove) = (field("Install"), field("Upgrade"), field("Remove"));
            let count = |list: Option<&str>| list.map_or(0, |list| names(list).len());
            let packages = [install, upgrade, remove].into_iter().flatten().flat_map(names).collect();
            let run = AptRun {
                date,
                installed: count(install),
                upgraded: count(upgrade),
                removed: count(remove),
                packages,
            };
            (run.installed + run.upgraded + run.removed > 0).then_some(run)
        })
        .collect();
    runs.reverse();
    runs
}

/// 更新記錄: what apt installed, upgraded and removed, latest first; none without a history.
/// The latest apt runs, with the newest told in a line.
pub(crate) fn history_group() -> Option<(adw::PreferencesGroup, String)> {
    let runs = parse_apt_history(&std::fs::read_to_string(APT_HISTORY).ok()?);
    if runs.is_empty() {
        return None;
    }
    let history = group("");
    let mut latest = String::new();
    for run in runs.iter().take(HISTORY_ENTRIES) {
        let counts: Vec<String> = [("升級", run.upgraded), ("安裝", run.installed), ("移除", run.removed)]
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .map(|(what, count)| format!("{what} {count} 個"))
            .collect();
        if latest.is_empty() {
            latest = format!("{}・{}", run.date, counts.join("、"));
        }
        let row = adw::ExpanderRow::builder().title(run.date.as_str()).subtitle(counts.join("、")).build();
        for package in &run.packages {
            row.add_row(&adw::ActionRow::builder().title(package.as_str()).build());
        }
        history.add(&row);
    }
    Some((history, latest))
}

/// How many packages `apt list --upgradable` lists, past its "Listing..." line.
pub(crate) fn upgradable(listed: &str) -> usize {
    listed.lines().filter(|line| line.contains("[upgradable from")).count()
}
