//! The Sound page, through PipeWire.

use super::*;

pub(crate) fn sound_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let shown = Rc::new(SoundPage::default());
    fill_sound_page(&page, shown.clone());
    follow_devices(&page, shown);
    page
}

/// Puts the devices on the page again when one comes or goes, as `pw-dump --monitor` says
/// PipeWire changed: once it has been quiet for half a second, so that a restarting
/// WirePlumber, whose devices go and come back, is looked at only once it is back. A
/// restarted PipeWire ends the monitor, which starts again while the page is there.
pub(crate) fn follow_devices(page: &adw::PreferencesPage, shown: Rc<SoundPage>) {
    let argv = ["pw-dump", "--monitor", "--no-colors"].map(std::ffi::OsStr::new);
    let child = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_PIPE) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("pw-dump --monitor: {err}");
            return;
        }
    };
    let Some(changes) = child.stdout_pipe() else { return };
    SOUND_MONITOR.set(Some(child.clone()));
    let weak = page.downgrade();
    let quiet: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    glib::spawn_future_local(async move {
        let (mut text, mut clients) = (String::new(), HashSet::new());
        while let Ok(bytes) = changes.read_bytes_future(64 * 1024, glib::Priority::DEFAULT).await {
            let Some(page) = weak.upgrade().filter(|_| !bytes.is_empty()) else { break };
            text.push_str(&String::from_utf8_lossy(&bytes));
            // The page's own pw-dump and pw-top come and go as clients; looking again for them
            // would start them again, and so on for as long as the page is open.
            if !pipewire_changed(&mut text, &mut clients) {
                continue;
            }
            if let Some(waiting) = quiet.take() {
                waiting.remove();
            }
            let (shown, quiet_then) = (shown.clone(), quiet.clone());
            let waiting = glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                quiet_then.take();
                let dump = output("pw-dump", &[]).unwrap_or_default();
                let nodes = parse_audio_nodes(&dump);
                if page_names(&nodes, &app_streams(&dump)) != *shown.devices.borrow() {
                    fill_sound_page(&page, shown);
                } else {
                    show_status(&shown, &nodes, default_clock(&dump));
                }
            });
            *quiet.borrow_mut() = Some(waiting);
        }
        child.force_exit();
        if let Some(page) = weak.upgrade() {
            glib::timeout_add_seconds_local_once(1, move || follow_devices(&page, shown));
        }
    });
}

/// Whether the updates `pw-dump --monitor` has finished printing in `text`, each a JSON array
/// ending in a line of its own `]`, change more than which clients are connected; they are
/// taken out of `text`, and `clients` follows the clients there are.
pub(crate) fn pipewire_changed(text: &mut String, clients: &mut HashSet<u64>) -> bool {
    let mut changed = false;
    while let Some(end) = text.find("\n]\n") {
        let update: String = text.drain(..end + 3).collect();
        let Ok(serde_json::Value::Array(objects)) = serde_json::from_str(&update) else {
            changed = true;
            continue;
        };
        for object in objects {
            let id = object["id"].as_u64().unwrap_or(u64::MAX);
            if object["type"] == "PipeWire:Interface:Client" {
                clients.insert(id);
            } else if !(object["info"].is_null() && clients.remove(&id)) {
                changed = true;
            }
        }
    }
    changed
}

/// What the sound page shows: its groups, and the devices they were made for.
#[derive(Default)]
pub(crate) struct SoundPage {
    pub(crate) groups: RefCell<Vec<adw::PreferencesGroup>>,
    pub(crate) devices: RefCell<Vec<(String, String)>>,
    /// The rows whose subtitles say what is going on: PipeWire's clock, and each card's.
    pub(crate) clock: RefCell<Option<adw::ActionRow>>,
    pub(crate) cards: RefCell<Vec<(u64, adw::ComboRow)>>,
}

/// Which devices there are, by kind and name, to tell when one comes or goes.
pub(crate) fn device_names(nodes: &[AudioDevice]) -> Vec<(String, String)> {
    let mut names: Vec<_> = nodes.iter().map(|device| (device.class.clone(), device.name.clone())).collect();
    names.sort();
    names
}

/// Puts each kind of sound device on the page, again after the default one changes and when
/// a device comes or goes.
pub(crate) fn fill_sound_page(page: &adw::PreferencesPage, shown: Rc<SoundPage>) {
    for group in shown.groups.take() {
        page.remove(&group);
    }
    let weak = page.downgrade();
    let again = {
        let shown = shown.clone();
        Rc::new(move || {
            if let Some(page) = weak.upgrade() {
                let shown = shown.clone();
                // Not while the row that changed is still handling it.
                glib::idle_add_local_once(move || fill_sound_page(&page, shown));
            }
        }) as Rc<dyn Fn()>
    };
    let dump = output("pw-dump", &[]).unwrap_or_default();
    let (nodes, cards) = (parse_audio_nodes(&dump), parse_sound_cards(&dump));
    let streams = app_streams(&dump);
    *shown.devices.borrow_mut() = page_names(&nodes, &streams);
    let clock_group = group("");
    let clock = adw::ActionRow::builder().title("PipeWire").build();
    clock_group.add(&clock);
    page.add(&clock_group);
    shown.groups.borrow_mut().push(clock_group);
    shown.clock.replace(Some(clock));
    for (title, class, default, none) in [
        ("輸出", "Audio/Sink", SINK, "沒有音訊輸出"),
        ("輸入", "Audio/Source", SOURCE, "沒有音訊輸入"),
    ] {
        let devices = nodes.iter().filter(|device| device.class == class).cloned().collect();
        let group = device_group(title, default, devices, none, again.clone());
        page.add(&group);
        shown.groups.borrow_mut().push(group);
    }
    if let Some(group) = app_volume_group(&streams) {
        page.add(&group);
        shown.groups.borrow_mut().push(group);
    }
    let outputs: Vec<AudioDevice> = nodes
        .iter()
        .filter(|device| device.class == "Audio/Sink" && device.name != MULTI_OUTPUT)
        .cloned()
        .collect();
    let mut more = vec![rates_group()];
    if !cards.is_empty() {
        more.insert(0, profile_group(cards, &shown));
    }
    if outputs.len() >= 2 {
        more.insert(0, multi_output_group(&outputs, again));
    }
    for group in more {
        page.add(&group);
        shown.groups.borrow_mut().push(group);
    }
    show_status(&shown, &nodes, default_clock(&dump));
}

/// Says under each card how its devices are open, or that it is idle, and under PipeWire
/// the rate and buffer it runs with: `clock`, its default, until `pw-top` has said, a second
/// later, what it runs with now.
pub(crate) fn show_status(shown: &SoundPage, nodes: &[AudioDevice], clock: Option<(u64, u64)>) {
    for (card, row) in shown.cards.borrow().iter() {
        let open: Vec<&str> =
            nodes.iter().filter(|node| node.card == Some(*card)).filter_map(|node| node.open_as.as_deref()).collect();
        row.set_subtitle(&if open.is_empty() { "閒置".to_string() } else { open.join(" · ") });
    }
    let Some(row) = shown.clock.borrow().clone() else { return };
    row.set_subtitle(&clock.map(clock_text).unwrap_or_default());
    let argv = ["pw-top", "--batch-mode", "--iterations", "2"].map(std::ffi::OsStr::new);
    let child = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_PIPE) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("pw-top: {err}");
            return;
        }
    };
    let row = row.downgrade();
    glib::spawn_future_local(async move {
        match child.communicate_utf8_future(None).await {
            Ok((Some(text), _)) => {
                if let (Some(row), Some(clock)) = (row.upgrade(), running_clock(&text)) {
                    row.set_subtitle(&clock_text(clock));
                }
            }
            Ok((None, _)) => {}
            Err(err) => eprintln!("pw-top: {err}"),
        }
    });
}

/// A rate and a buffer, in samples: "48000 Hz · 緩衝區 1024 個取樣（21.3 毫秒）".
pub(crate) fn clock_text((rate, quantum): (u64, u64)) -> String {
    let ms = quantum as f64 * 1000.0 / rate.max(1) as f64;
    format!("{rate} Hz · 緩衝區 {quantum} 個取樣（{ms:.1} 毫秒）")
}

/// The rate and buffer PipeWire runs with when nothing asks for others, from its settings in
/// `pw-dump`'s output.
pub(crate) fn default_clock(dump: &str) -> Option<(u64, u64)> {
    let objects: Vec<serde_json::Value> = serde_json::from_str(dump).ok()?;
    let settings = objects.iter().find(|object| object["props"]["metadata.name"] == "settings")?;
    let value = |key: &str| {
        let entry = settings["metadata"].as_array()?.iter().find(|entry| entry["key"] == key)?;
        let value = &entry["value"];
        value.as_u64().or_else(|| value.as_str()?.parse().ok())
    };
    Some((value("clock.rate")?, value("clock.quantum")?))
}

/// The rate and buffer the first device driving PipeWire runs with, in `pw-top`'s last
/// look; None when nothing plays or records. A driver's line is the one not marked as
/// following another with "+" or "=", and one at a rate of 0 drives nothing.
pub(crate) fn running_clock(text: &str) -> Option<(u64, u64)> {
    let last = text.rsplit_once("QUANT")?.1;
    last.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first() != Some(&"R") || fields.iter().any(|field| *field == "+" || *field == "=") {
            return None;
        }
        let clock: (u64, u64) = (fields.get(3)?.parse().ok()?, fields.get(2)?.parse().ok()?);
        (clock.0 > 0 && clock.1 > 0).then_some(clock)
    })
}

/// What PipeWire knows the output that plays on several at once by.
pub(crate) const MULTI_OUTPUT: &str = "nekoland_multi_output";

pub(crate) fn pipewire_conf(name: &str) -> PathBuf {
    home().join(".config/pipewire/pipewire.conf.d").join(name)
}

/// Writes `text` to `path`, or removes it when `text` is None, gone already or not.
pub(crate) fn write_or_remove(path: &Path, text: Option<String>) -> std::io::Result<()> {
    match text {
        Some(text) => {
            std::fs::create_dir_all(path.parent().unwrap_or(path)).and_then(|()| std::fs::write(path, text))
        }
        None => std::fs::remove_file(path).or_else(|err| match err.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(err),
        }),
    }
}

/// The outputs that play together as one, as macOS's multi-output device: PipeWire's combine
/// stream, which there is while two or more are chosen.
pub(crate) fn multi_output_group(outputs: &[AudioDevice], changed: Rc<dyn Fn()>) -> adw::PreferencesGroup {
    let group = group("多重輸出裝置");
    let path = pipewire_conf("51-nekoland-multi-output.conf");
    // Those chosen that are not plugged in stay chosen.
    let members = Rc::new(RefCell::new(match std::fs::read_to_string(&path) {
        Ok(text) => multi_output_members(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            Vec::new()
        }
    }));
    for device in outputs {
        let row = adw::SwitchRow::builder()
            .title(&device.description)
            .active(members.borrow().contains(&device.name))
            .build();
        let (members, changed, path) = (members.clone(), changed.clone(), path.clone());
        let name = device.name.clone();
        row.connect_active_notify(move |row| {
            let mut members = members.borrow_mut();
            let was = members.len() >= 2;
            members.retain(|member| *member != name);
            if row.is_active() {
                members.push(name.clone());
            }
            let is = members.len() >= 2;
            if !was && !is {
                return;
            }
            if let Err(err) = write_or_remove(&path, is.then(|| multi_output_conf(&members))) {
                eprintln!("{}: {err}", path.display());
                return;
            }
            // PipeWire loads its modules only as it starts.
            run("systemctl", &["--user", "restart", "pipewire", "pipewire-pulse", "wireplumber"]);
            when_multi_output_is(is, changed.clone());
        });
        group.add(&row);
    }
    group
}

/// Calls `then` once PipeWire is back with the multi-output device there or not, or after
/// five seconds.
pub(crate) fn when_multi_output_is(there: bool, then: Rc<dyn Fn()>) {
    let mut tries = 0;
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        tries += 1;
        let nodes = audio_nodes();
        let back = nodes.iter().any(|device| device.class == "Audio/Sink" && device.name != MULTI_OUTPUT);
        if (back && nodes.iter().any(|device| device.name == MULTI_OUTPUT) == there) || tries == 25 {
            then();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

pub(crate) fn multi_output_conf(members: &[String]) -> String {
    let matches: Vec<String> = members
        .iter()
        .map(|name| format!("            {{ media.class = \"Audio/Sink\" node.name = {} }}\n", quoted(name)))
        .collect();
    format!(
        "# Written by Nekoland's Settings.\n\
         context.modules = [\n  {{\n    name = libpipewire-module-combine-stream\n    args = {{\n      \
         combine.mode = sink\n      node.name = \"{MULTI_OUTPUT}\"\n      \
         node.description = \"多重輸出裝置\"\n      combine.latency-compensate = true\n      \
         combine.props = {{ audio.position = [ FL FR ] }}\n      stream.rules = [\n        {{\n          \
         matches = [\n{}          ]\n          actions = {{ create-stream = {{ }} }}\n        }}\n      \
         ]\n    }}\n  }}\n]\n",
        matches.concat()
    )
}

/// The outputs `multi_output_conf` was written with.
pub(crate) fn multi_output_members(conf: &str) -> Vec<String> {
    conf.lines()
        .filter_map(|line| {
            line.trim().strip_prefix("{ media.class = \"Audio/Sink\" node.name = ")?.strip_suffix(" }")
        })
        .filter_map(|name| serde_json::from_str(name).ok())
        .collect()
}

/// The value `pw-metadata` prints for a key: "update: id:0 key:'clock.rate' value:'48000' type:''".
pub(crate) fn metadata_value(text: &str) -> Option<String> {
    let value = text.lines().find_map(|line| line.split_once(" value:'"))?.1;
    Some(value.split_once('\'')?.0.to_string())
}

/// A string as PipeWire's configuration, a kind of JSON, takes it.
pub(crate) fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// Sample rates PipeWire may run at, following what plays, instead of always its default;
/// set at once, and kept for when PipeWire starts again.
pub(crate) fn rates_group() -> adw::PreferencesGroup {
    let group = group("");
    let path = pipewire_conf("51-nekoland-rates.conf");
    let row = adw::SwitchRow::builder().title("取樣率跟隨播放內容").active(path.exists()).build();
    row.connect_active_notify(move |row| {
        let rates: Vec<String> = COMMON_RATES.iter().map(u32::to_string).collect();
        let rates = format!("[ {} ]", rates.join(" "));
        let conf = row.is_active().then(|| {
            format!(
                "# Written by Nekoland's Settings.\n\
                 context.properties = {{ default.clock.allowed-rates = {rates} }}\n"
            )
        });
        if let Err(err) = write_or_remove(&path, conf) {
            eprintln!("{}: {err}", path.display());
            return;
        }
        // Off, it is the default rate alone, as PipeWire has it unless told otherwise; not
        // deleted, which PipeWire 1.6 crashes on.
        let rates = if row.is_active() {
            rates
        } else {
            let rate = output("pw-metadata", &["-n", "settings", "0", "clock.rate"])
                .and_then(|text| metadata_value(&text))
                .unwrap_or_else(|| "48000".into());
            format!("[ {rate} ]")
        };
        run("pw-metadata", &["-n", "settings", "0", "clock.allowed-rates", &rates]);
    });
    group.add(&row);
    group
}

/// One kind of sound device: which is the default, its volume and whether it is muted, and
/// the format a sound card is opened with.
pub(crate) fn device_group(
    title: &str,
    default: &'static str,
    devices: Vec<AudioDevice>,
    none: &str,
    changed: Rc<dyn Fn()>,
) -> adw::PreferencesGroup {
    let group = group(title);
    let Some(current) = devices.iter().find(|device| device.default).or(devices.first()).cloned() else {
        group.add(&adw::ActionRow::builder().title(none).build());
        return group;
    };
    let names: Vec<&str> = devices.iter().map(|device| device.description.as_str()).collect();
    let chosen = adw::ComboRow::builder()
        .title("裝置")
        .model(&gtk::StringList::new(&names))
        .selected(devices.iter().position(|device| device.name == current.name).unwrap_or(0) as u32)
        .build();
    chosen.connect_selected_notify(move |row| {
        let Some(name) = devices.get(row.selected() as usize).map(|device| &device.name) else {
            return;
        };
        // Looked up again: restarting WirePlumber numbers the devices anew.
        match audio_nodes().into_iter().find(|device| device.name == *name) {
            Some(device) => run("wpctl", &["set-default", &device.id.to_string()]),
            None => eprintln!("{name}: no longer there"),
        }
        changed();
    });
    group.add(&chosen);

    let (level, muted) = volume(default).unwrap_or_default();
    group.add(&scale_row(
        "音量",
        f64::from(level.round()),
        (0.0, 100.0, 5.0),
        |level| format!("{level:.0}%"),
        move |level| run("wpctl", &["set-volume", default, &format!("{:.2}", level / 100.0)]),
    ));
    let mute = adw::SwitchRow::builder().title("靜音").active(muted).build();
    mute.connect_active_notify(move |row| {
        run("wpctl", &["set-mute", default, if row.is_active() { "1" } else { "0" }])
    });
    group.add(&mute);

    if current.alsa {
        add_format_rows(&group, current);
    }
    group
}

/// The programs playing sound, as PipeWire's playback streams: the stream's ID and the name
/// of the program.
pub(crate) fn app_streams(dump: &str) -> Vec<(u32, String)> {
    let objects: Vec<serde_json::Value> = serde_json::from_str(dump).unwrap_or_default();
    objects
        .iter()
        .filter(|object| object["info"]["props"]["media.class"] == PLAYBACK_STREAM)
        .filter_map(|object| {
            let props = &object["info"]["props"];
            let name = props["application.name"].as_str().or(props["node.name"].as_str())?;
            Some((object["id"].as_u64()? as u32, name.to_string()))
        })
        .collect()
}

/// What the page was made for: the devices, and the programs playing, to tell when one
/// comes or goes.
pub(crate) fn page_names(nodes: &[AudioDevice], streams: &[(u32, String)]) -> Vec<(String, String)> {
    let mut names = device_names(nodes);
    names.extend(streams.iter().map(|(_, name)| (PLAYBACK_STREAM.to_string(), name.clone())));
    names
}

/// A volume for each program that is playing sound, as Windows' mixer has; none while
/// nothing plays. The page is made again as a program starts or stops.
pub(crate) fn app_volume_group(streams: &[(u32, String)]) -> Option<adw::PreferencesGroup> {
    if streams.is_empty() {
        return None;
    }
    let apps = group("應用程式");
    for (id, name) in streams {
        let id = id.to_string();
        let (level, _) = volume(&id).unwrap_or_default();
        apps.add(&scale_row(
            name,
            f64::from(level.round()),
            (0.0, 100.0, 5.0),
            |level| format!("{level:.0}%"),
            move |level| run("wpctl", &["set-volume", &id, &format!("{:.2}", level / 100.0)]),
        ));
    }
    Some(apps)
}

/// Bit depths a sound card may be opened with, as PipeWire names them.
pub(crate) const SAMPLE_FORMATS: [(&str, &str); 6] = [
    ("U8", "8 位元"),
    ("S16LE", "16 位元"),
    ("S24LE", "24 位元"),
    ("S24_32LE", "24 位元"),
    ("S32LE", "32 位元"),
    ("F32LE", "32 位元浮點"),
];

/// The sample rate and bit depth a sound card is opened with, each "自動" to leave it to
/// PipeWire, which resamples between it and the other devices.
pub(crate) fn add_format_rows(group: &adw::PreferencesGroup, device: AudioDevice) {
    let mut formats: Vec<(&str, &str)> =
        SAMPLE_FORMATS.into_iter().filter(|(name, _)| device.formats.iter().any(|f| f == name)).collect();
    // A card with both kinds of 24 bits is opened with the first.
    formats.dedup_by_key(|(_, label)| *label);
    let rates = device.rates.clone();
    let chosen = Rc::new(RefCell::new((device.rate, device.format.clone())));

    let mut labels = vec!["自動".to_string()];
    labels.extend(rates.iter().map(|rate| format!("{} kHz", f64::from(*rate) / 1000.0)));
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let rate_row = adw::ComboRow::builder()
        .title("取樣率")
        .model(&gtk::StringList::new(&labels))
        .selected(rates.iter().position(|rate| *rate == device.rate).map_or(0, |at| at as u32 + 1))
        .build();
    let name = device.name.clone();
    let rate_chosen = chosen.clone();
    rate_row.connect_selected_notify(move |row| {
        let rate = (row.selected() as usize).checked_sub(1).and_then(|at| rates.get(at));
        rate_chosen.borrow_mut().0 = rate.copied().unwrap_or(0);
        let (rate, format) = &*rate_chosen.borrow();
        save_device_format(&name, *rate, format);
    });
    group.add(&rate_row);

    let mut labels = vec!["自動"];
    labels.extend(formats.iter().map(|(_, label)| *label));
    let depth_row = adw::ComboRow::builder()
        .title("位元深度")
        .model(&gtk::StringList::new(&labels))
        .selected(formats.iter().position(|(name, _)| *name == device.format).map_or(0, |at| at as u32 + 1))
        .build();
    depth_row.connect_selected_notify(move |row| {
        let format = (row.selected() as usize).checked_sub(1).and_then(|at| formats.get(at));
        chosen.borrow_mut().1 = format.map(|(name, _)| name.to_string()).unwrap_or_default();
        let (rate, format) = &*chosen.borrow();
        save_device_format(&device.name, *rate, format);
    });
    group.add(&depth_row);
}

/// Keeps the format a sound card is opened with as a WirePlumber rule of its own, none when
/// both are left to PipeWire, and restarts WirePlumber, which reads its rules only then.
pub(crate) fn save_device_format(name: &str, rate: u32, format: &str) {
    let path = home()
        .join(".config/wireplumber/wireplumber.conf.d")
        .join(format!("51-nekoland-{}.conf", name.replace('/', "_")));
    let mut props = Vec::new();
    if rate != 0 {
        props.push(format!("audio.rate = {rate}"));
    }
    if !format.is_empty() {
        props.push(format!("audio.format = {}", quoted(format)));
    }
    let rule = (!props.is_empty()).then(|| {
        format!(
            "# Written by Nekoland's Settings.\n\
             monitor.alsa.rules = [\n  {{\n    matches = [ {{ node.name = {name} }} ]\n    \
             actions = {{ update-props = {{ {} }} }}\n  }}\n]\n",
            props.join(", "),
            name = quoted(name),
        )
    });
    match write_or_remove(&path, rule) {
        Ok(()) => run("systemctl", &["--user", "restart", "wireplumber"]),
        Err(err) => eprintln!("{}: {err}", path.display()),
    }
}

#[derive(Clone)]
pub(crate) struct AudioDevice {
    pub(crate) id: u32,
    /// What PipeWire knows it by, which stays the same across restarts.
    pub(crate) name: String,
    pub(crate) description: String,
    /// "Audio/Sink" or "Audio/Source".
    pub(crate) class: String,
    pub(crate) default: bool,
    /// Whether it is a sound card, whose format can be chosen.
    pub(crate) alsa: bool,
    /// The sample rates and formats it can be opened with, and the ones it is set to: 0 and
    /// "" for PipeWire's choice.
    pub(crate) rates: Vec<u32>,
    pub(crate) formats: Vec<String>,
    pub(crate) rate: u32,
    pub(crate) format: String,
    /// The sound card it is on, and how it is open now: "S16LE 48000 Hz 2ch".
    pub(crate) card: Option<u64>,
    pub(crate) open_as: Option<String>,
}

/// A sound card, and the profiles it can be used with, as PipeWire describes them: stereo,
/// surround, Pro Audio or off.
pub(crate) struct SoundCard {
    pub(crate) id: u64,
    pub(crate) description: String,
    /// By index, and the one it is in.
    pub(crate) profiles: Vec<(u64, String)>,
    pub(crate) profile: u64,
}

/// The sound cards in `pw-dump`'s output; one it cannot read, `parse_audio_nodes` says so.
pub(crate) fn parse_sound_cards(dump: &str) -> Vec<SoundCard> {
    use serde_json::Value;
    let objects: Vec<Value> = serde_json::from_str(dump).unwrap_or_default();
    objects
        .iter()
        .filter(|object| object["info"]["props"]["media.class"] == "Audio/Device")
        .filter_map(|object| {
            let (info, props) = (&object["info"], &object["info"]["props"]);
            let current = info["params"]["Profile"][0]["index"].as_u64()?;
            // Those PipeWire says cannot be used, unless it is in one.
            let profiles = info["params"]["EnumProfile"]
                .as_array()?
                .iter()
                .filter(|profile| profile["available"] != "no" || profile["index"] == current)
                .filter_map(|profile| {
                    Some((profile["index"].as_u64()?, profile["description"].as_str()?.into()))
                })
                .collect();
            Some(SoundCard {
                id: object["id"].as_u64()?,
                description: props["device.description"].as_str().or(props["device.name"].as_str())?.into(),
                profiles,
                profile: current,
            })
        })
        .collect()
}

/// Each sound card's profile, as macOS's Audio MIDI Setup and pavucontrol pick it.
pub(crate) fn profile_group(cards: Vec<SoundCard>, shown: &SoundPage) -> adw::PreferencesGroup {
    let group = group("裝置模式");
    let mut rows = Vec::new();
    for card in cards {
        let names: Vec<&str> = card.profiles.iter().map(|(_, name)| name.as_str()).collect();
        let row = adw::ComboRow::builder()
            .title(&card.description)
            .model(&gtk::StringList::new(&names))
            .selected(card.profiles.iter().position(|(index, _)| *index == card.profile).unwrap_or(0) as u32)
            .build();
        rows.push((card.id, row.clone()));
        // The page follows as the card's devices change.
        row.connect_selected_notify(move |row| {
            if let Some((index, _)) = card.profiles.get(row.selected() as usize) {
                run("wpctl", &["set-profile", &card.id.to_string(), &index.to_string()]);
            }
        });
        group.add(&row);
    }
    shown.cards.replace(rows);
    group
}

pub(crate) fn audio_nodes() -> Vec<AudioDevice> {
    output("pw-dump", &[]).map(|dump| parse_audio_nodes(&dump)).unwrap_or_default()
}

/// Sample rates offered when a card takes any in a range.
pub(crate) const COMMON_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// The audio devices in `pw-dump`'s output.
pub(crate) fn parse_audio_nodes(dump: &str) -> Vec<AudioDevice> {
    use serde_json::Value;
    let objects: Vec<Value> = match serde_json::from_str(dump) {
        Ok(objects) => objects,
        Err(err) => {
            eprintln!("pw-dump: {err}");
            return Vec::new();
        }
    };
    // The defaults are kept by name, as metadata.
    let default_of = |key: &str| {
        objects
            .iter()
            .filter(|object| object["props"]["metadata.name"] == "default")
            .flat_map(|object| object["metadata"].as_array().into_iter().flatten())
            .find(|entry| entry["key"] == key)
            .and_then(|entry| entry["value"]["name"].as_str())
            .map(String::from)
    };
    let defaults = [default_of("default.audio.sink"), default_of("default.audio.source")];
    // A choice is a plain value, or {"default", "alt1", ...}, or {"default", "min", "max"}.
    let choices = |value: &Value| -> Vec<Value> {
        match value.as_object() {
            None => vec![value.clone()],
            Some(choice) => {
                choice.iter().filter(|(key, _)| *key != "default").map(|(_, value)| value.clone()).collect()
            }
        }
    };
    let mut devices = Vec::new();
    for object in &objects {
        let info = &object["info"];
        let props = &info["props"];
        let class = props["media.class"].as_str().unwrap_or_default();
        if class != "Audio/Sink" && class != "Audio/Source" {
            continue;
        }
        let (Some(id), Some(name)) = (object["id"].as_u64(), props["node.name"].as_str()) else {
            continue;
        };
        let (mut rates, mut formats) = (Vec::new(), Vec::new());
        for format in info["params"]["EnumFormat"].as_array().into_iter().flatten() {
            let rate = &format["rate"];
            match (rate["min"].as_u64(), rate["max"].as_u64()) {
                (Some(min), Some(max)) => {
                    rates.extend(COMMON_RATES.into_iter().filter(|r| (min..=max).contains(&u64::from(*r))))
                }
                _ => rates.extend(choices(rate).iter().filter_map(Value::as_u64).map(|r| r as u32)),
            }
            formats.extend(choices(&format["format"]).iter().filter_map(Value::as_str).map(String::from));
        }
        rates.sort_unstable();
        rates.dedup();
        formats.sort_unstable();
        formats.dedup();
        devices.push(AudioDevice {
            id: id as u32,
            name: name.into(),
            description: props["node.description"].as_str().unwrap_or(name).into(),
            class: class.into(),
            default: defaults.iter().flatten().any(|default| default == name),
            alsa: props["device.api"] == "alsa",
            rates,
            formats,
            rate: props["audio.rate"].as_u64().unwrap_or(0) as u32,
            format: props["audio.format"].as_str().unwrap_or_default().into(),
            card: props["device.id"].as_u64(),
            open_as: info["params"]["Format"].get(0).and_then(|open| {
                let (format, rate, channels) =
                    (open["format"].as_str()?, open["rate"].as_u64()?, open["channels"].as_u64()?);
                Some(format!("{format} {rate} Hz {channels}ch"))
            }),
        });
    }
    devices
}
