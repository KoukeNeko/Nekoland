//! Settings' 網路 page, through NetworkManager's `nmcli`: each Ethernet port, joined and
//! left, with its addresses and how they are set, and the VPNs, turned on and off, added
//! from a file and removed. Wi-Fi has its own page; its networks' addresses are set here.

use std::{
    cell::RefCell,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{
    alert, fact_row, group, output,
    wifi::{follow_network, nmcli, nmcli_fields, setting, up_with},
};

/// An Ethernet port, as `nmcli device status` has it.
#[derive(Clone, Debug, PartialEq)]
struct Port {
    device: String,
    state: String,
    /// The connection up on it, when there is one.
    uuid: Option<String>,
}

impl Port {
    fn up(&self) -> bool {
        self.state.starts_with("connected") || self.state.starts_with("connecting")
    }

    /// Whether it can be joined or left from here.
    fn usable(&self) -> bool {
        self.state != "unavailable" && self.state != "unmanaged"
    }

    fn status(&self) -> &'static str {
        match self.state.as_str() {
            state if state.starts_with("connected") => "已連線",
            state if state.starts_with("connecting") => "正在連線…",
            "unavailable" => "未接上網路線",
            "unmanaged" => "不由 NetworkManager 管理",
            _ => "未連線",
        }
    }
}

/// A VPN NetworkManager keeps, WireGuard's among them.
#[derive(Clone, Debug, PartialEq)]
struct Vpn {
    uuid: String,
    name: String,
    kind: String,
    /// The connection's state, as activated or activating, or empty when it is down.
    state: String,
}

/// What `nmcli -t -f DEVICE,TYPE,STATE,CON-UUID device status` printed: the Ethernet ports.
fn parse_ports(text: &str) -> Vec<Port> {
    text.lines()
        .filter_map(|line| {
            let [device, kind, state, uuid] = &nmcli_fields(line)[..] else {
                return None;
            };
            (kind == "ethernet").then(|| Port {
                device: device.clone(),
                state: state.clone(),
                uuid: (!uuid.is_empty()).then(|| uuid.clone()),
            })
        })
        .collect()
}

/// What `nmcli -t -f UUID,TYPE,STATE,NAME connection show` printed: the VPNs.
fn parse_vpns(text: &str) -> Vec<Vpn> {
    text.lines()
        .filter_map(|line| {
            let [uuid, kind, state, name] = &nmcli_fields(line)[..] else {
                return None;
            };
            matches!(kind.as_str(), "vpn" | "wireguard").then(|| Vpn {
                uuid: uuid.clone(),
                name: name.clone(),
                kind: kind.clone(),
                state: state.clone(),
            })
        })
        .collect()
}

/// A VPN's kind as people call it, from its type, or for a plugin's from its service type,
/// as org.freedesktop.NetworkManager.openvpn.
fn vpn_kind(vpn: &Vpn) -> String {
    if vpn.kind == "wireguard" {
        return "WireGuard".into();
    }
    let service = setting(&vpn.uuid, "vpn.service-type").unwrap_or_default();
    match service.rsplit('.').next().unwrap_or_default() {
        "openvpn" => "OpenVPN".into(),
        "openconnect" => "OpenConnect".into(),
        "vpnc" => "Cisco IPsec".into(),
        "l2tp" => "L2TP".into(),
        "pptp" => "PPTP".into(),
        other => other.into(),
    }
}

/// One of what a port has, as IP4.ADDRESS, from `nmcli device show`.
fn device_field(device: &str, name: &str) -> String {
    output("nmcli", &["-t", "-g", name, "device", "show", device])
        .map(|text| nmcli_fields(text.trim_end_matches('\n')).join(":"))
        .unwrap_or_default()
}

/// The mask a prefix length stands for, as 255.255.255.0 for 24.
fn prefix_mask(prefix: u8) -> Ipv4Addr {
    Ipv4Addr::from(u32::MAX.checked_shl(32 - u32::from(prefix.min(32))).unwrap_or(0))
}

/// The prefix length of a mask typed as 255.255.255.0, or as 24 or /24.
fn mask_prefix(mask: &str) -> Option<u8> {
    let mask = mask.trim();
    if let Ok(prefix) = mask.trim_start_matches('/').parse::<u8>() {
        return (prefix <= 32).then_some(prefix);
    }
    let bits = u32::from(mask.parse::<Ipv4Addr>().ok()?);
    // The ones all before the zeros.
    (bits.leading_ones() + bits.trailing_zeros() == 32).then_some(bits.leading_ones() as u8)
}

/// The addresses set by hand, as nmcli takes them: "address/prefix" and the router.
fn manual_addresses(address: &str, mask: &str, router: &str) -> Result<(String, String), &'static str> {
    let address: Ipv4Addr = address.trim().parse().map_err(|_| "IP 位址的格式不正確。")?;
    let prefix = mask_prefix(mask).ok_or("子網路遮罩的格式不正確。")?;
    let router = router.trim();
    if !router.is_empty() && router.parse::<Ipv4Addr>().is_err() {
        return Err("路由器的格式不正確。");
    }
    Ok((format!("{address}/{prefix}"), router.to_string()))
}

/// How IPv6 can be set, as people read it and as NetworkManager's ipv6.method has it.
const IPV6_METHODS: [(&str, &str); 4] = [
    ("自動", "auto"),
    ("僅本機連結", "link-local"),
    ("手動", "manual"),
    ("關閉", "disabled"),
];

/// An IPv6 address set by hand, "address/prefix" with 64 when no prefix is typed, and its
/// router, as nmcli takes them.
fn manual_v6(address: &str, router: &str) -> Result<(String, String), &'static str> {
    let wrong = "IPv6 位址的格式不正確。";
    let (address, prefix) = address.trim().split_once('/').unwrap_or((address.trim(), "64"));
    let address: Ipv6Addr = address.parse().map_err(|_| wrong)?;
    let prefix: u8 = prefix.parse().ok().filter(|prefix| *prefix <= 128).ok_or(wrong)?;
    let router = router.trim();
    if !router.is_empty() && router.parse::<Ipv6Addr>().is_err() {
        return Err("IPv6 路由器的格式不正確。");
    }
    Ok((format!("{address}/{prefix}"), router.to_string()))
}

/// DNS servers typed apart by commas or spaces, as nmcli takes them: IPv4's, then IPv6's.
fn dns_servers(typed: &str) -> Result<(String, String), &'static str> {
    let (mut v4, mut v6) = (Vec::new(), Vec::new());
    for server in typed.split([',', ' ']).filter(|server| !server.is_empty()) {
        match server.parse::<IpAddr>() {
            Ok(IpAddr::V4(_)) => v4.push(server),
            Ok(IpAddr::V6(_)) => v6.push(server),
            Err(_) => return Err("DNS 伺服器的格式不正確。"),
        }
    }
    Ok((v4.join(","), v6.join(",")))
}

/// Words typed apart by commas or spaces, as nmcli takes a list.
fn typed_list(typed: &str) -> String {
    typed
        .split([',', ' '])
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// How a connection's IPv4 and IPv6 addresses and DNS servers are set, with what applies
/// them, for a connection that takes its IPv4 addresses from DHCP or has them set by hand.
pub fn address_settings(page: &adw::PreferencesPage, uuid: &str) {
    let method = setting(uuid, "ipv4.method").unwrap_or_default();
    if method != "auto" && method != "manual" {
        return;
    }
    let manual = method == "manual";
    let first = setting(uuid, "ipv4.addresses").unwrap_or_default();
    let first = first.split(',').next().unwrap_or_default().trim().to_string();
    let (address, prefix) = first.split_once('/').unwrap_or((first.as_str(), ""));
    let mask = prefix
        .parse()
        .map(prefix_mask)
        .map(|mask| mask.to_string())
        .unwrap_or_default();
    let entry = |title: &str, text: &str, visible: bool| {
        adw::EntryRow::builder()
            .title(title)
            .text(text)
            .visible(visible)
            .build()
    };

    let ipv4 = group("IPv4");
    let how = adw::ComboRow::builder()
        .title("設定 IPv4")
        .model(&gtk::StringList::new(&["使用 DHCP", "手動"]))
        .selected(u32::from(manual))
        .build();
    let address = entry("IP 位址", address, manual);
    let mask = entry("子網路遮罩", &mask, manual);
    let router = entry(
        "路由器",
        &setting(uuid, "ipv4.gateway").unwrap_or_default(),
        manual,
    );
    let by_hand = [address.clone(), mask.clone(), router.clone()];
    how.connect_selected_notify(move |how| {
        for row in &by_hand {
            row.set_visible(how.selected() == 1);
        }
    });
    ipv4.add(&how);
    for row in [&address, &mask, &router] {
        ipv4.add(row);
    }
    page.add(&ipv4);

    // A connection whose IPv6 is set some other way, as shared, keeps it.
    let v6_method = setting(uuid, "ipv6.method").unwrap_or_default();
    let ipv6 = IPV6_METHODS
        .iter()
        .position(|(_, method)| *method == v6_method)
        .map(|chosen| {
            let manual = IPV6_METHODS[chosen].1 == "manual";
            let first = setting(uuid, "ipv6.addresses").unwrap_or_default();
            let first = first.split(',').next().unwrap_or_default().trim().to_string();
            let how = adw::ComboRow::builder()
                .title("設定 IPv6")
                .model(&gtk::StringList::new(&IPV6_METHODS.map(|(label, _)| label)))
                .selected(chosen as u32)
                .build();
            let address = entry("IP 位址", &first, manual);
            let router = entry(
                "路由器",
                &setting(uuid, "ipv6.gateway").unwrap_or_default(),
                manual,
            );
            let by_hand = [address.clone(), router.clone()];
            how.connect_selected_notify(move |how| {
                for row in &by_hand {
                    row.set_visible(IPV6_METHODS[how.selected() as usize].1 == "manual");
                }
            });
            let ipv6 = group("IPv6");
            ipv6.add(&how);
            ipv6.add(&address);
            ipv6.add(&router);
            page.add(&ipv6);
            (how, address, router)
        });

    let dns = group("DNS");
    let current: Vec<String> = ["ipv4.dns", "ipv6.dns"]
        .iter()
        .filter_map(|key| setting(uuid, key))
        .filter(|servers| !servers.is_empty())
        .collect();
    let servers = adw::EntryRow::builder()
        .title("DNS 伺服器")
        .text(current.join(",").replace(',', ", "))
        .build();
    let search = adw::EntryRow::builder()
        .title("搜尋網域")
        .text(
            setting(uuid, "ipv4.dns-search")
                .unwrap_or_default()
                .replace(',', ", "),
        )
        .build();
    dns.add(&servers);
    dns.add(&search);
    page.add(&dns);

    let apply = gtk::Button::builder()
        .label("套用")
        .halign(gtk::Align::Center)
        .css_classes(["suggested-action", "pill"])
        .build();
    let end = group("");
    end.add(&apply);
    page.add(&end);
    let uuid = uuid.to_string();
    apply.connect_clicked(move |button| {
        let settings = || -> Result<Vec<String>, &'static str> {
            let mut settings: Vec<String> = ["connection", "modify", "uuid", uuid.as_str()]
                .map(String::from)
                .into();
            let mut set = |key: &str, value: String| settings.extend([key.to_string(), value]);
            if how.selected() == 1 {
                let (address, router) = manual_addresses(&address.text(), &mask.text(), &router.text())?;
                set("ipv4.method", "manual".into());
                set("ipv4.addresses", address);
                set("ipv4.gateway", router);
            } else {
                // What was set by hand goes, or DHCP's addresses come with it.
                set("ipv4.method", "auto".into());
                set("ipv4.addresses", String::new());
                set("ipv4.gateway", String::new());
            }
            let (v4_servers, v6_servers) = dns_servers(&servers.text())?;
            // Without IPv6, or with IPv6 set some other way, an IPv6 server would be dropped.
            let v6_kept = ipv6
                .as_ref()
                .is_some_and(|(how, _, _)| IPV6_METHODS[how.selected() as usize].1 != "disabled");
            if !v6_servers.is_empty() && !v6_kept {
                return Err("IPv6 未開啟，無法使用 IPv6 的 DNS 伺服器。");
            }
            // Servers typed are used instead of DHCP's, as the Mac does.
            let ignore = if v4_servers.is_empty() && v6_servers.is_empty() {
                "no"
            } else {
                "yes"
            };
            set("ipv4.dns", v4_servers);
            set("ipv4.ignore-auto-dns", ignore.into());
            set("ipv4.dns-search", typed_list(&search.text()));
            if let Some((how, address, router)) = &ipv6 {
                let method = IPV6_METHODS[how.selected() as usize].1;
                if method == "manual" {
                    let (address, router) = manual_v6(&address.text(), &router.text())?;
                    set("ipv6.method", method.into());
                    set("ipv6.addresses", address);
                    set("ipv6.gateway", router);
                } else {
                    set("ipv6.method", method.into());
                    set("ipv6.addresses", String::new());
                    set("ipv6.gateway", String::new());
                }
                if method == "disabled" {
                    // NetworkManager refuses IPv6 DNS servers on a connection without IPv6.
                    set("ipv6.dns", String::new());
                } else {
                    set("ipv6.dns", v6_servers);
                    set("ipv6.ignore-auto-dns", ignore.into());
                }
            }
            Ok(settings)
        };
        let args = match settings() {
            Ok(args) => args,
            Err(why) => return alert::notice(button, "無法套用設定", Some(why)),
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let (button, uuid) = (button.clone(), uuid.clone());
        button.set_sensitive(false);
        nmcli(&args, move |result| {
            let failed = |button: &gtk::Button, err: &str| {
                button.set_sensitive(true);
                alert::notice(button, "無法套用設定", Some(err));
            };
            match result {
                Err(err) => failed(&button, &err),
                // A connection that is up takes them as it comes up again.
                Ok(_) if setting(&uuid, "GENERAL.STATE").is_some_and(|state| !state.is_empty()) => {
                    nmcli(&["connection", "up", "uuid", &uuid], move |result| match result {
                        Ok(_) => button.set_sensitive(true),
                        Err(err) => failed(&button, &err),
                    });
                }
                Ok(_) => button.set_sensitive(true),
            }
        });
    });
}

/// What the page shows, which is put there again as NetworkManager changes.
#[derive(Default)]
struct NetworkPage {
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    /// The proxy and the firewall, not NetworkManager's, kept as they are under the rest.
    kept: Vec<adw::PreferencesGroup>,
}

pub fn network_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let kept = [Some(proxy_group()), firewall_group()]
        .into_iter()
        .flatten()
        .collect();
    let shown = Rc::new(NetworkPage {
        groups: RefCell::default(),
        kept,
    });
    fill_network_page(&page, &shown);
    follow_network(&page, move |page| fill_network_page(page, &shown));
    page
}

fn fill_network_page(page: &adw::PreferencesPage, shown: &Rc<NetworkPage>) {
    for group in shown.groups.take().iter().chain(&shown.kept) {
        if group.parent().is_some() {
            page.remove(group);
        }
    }
    let mut groups = Vec::new();
    let (weak, again) = (page.downgrade(), shown.clone());
    let done: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(page) = weak.upgrade() {
            fill_network_page(&page, &again);
        }
    });

    let Some(devices) = output(
        "nmcli",
        &["-t", "-f", "DEVICE,TYPE,STATE,CON-UUID", "device", "status"],
    ) else {
        let missing = group("");
        missing.add(&adw::ActionRow::builder().title("未安裝 NetworkManager").build());
        for group in std::iter::once(&missing).chain(&shown.kept) {
            page.add(group);
        }
        shown.groups.replace(vec![missing]);
        return;
    };

    let ports = parse_ports(&devices);
    if !ports.is_empty() {
        let wired = group("乙太網路");
        for port in &ports {
            wired.add(&port_row(port, ports.len() > 1, &done));
        }
        groups.push(wired);
    }

    let vpns = parse_vpns(
        &output(
            "nmcli",
            &["-t", "-f", "UUID,TYPE,STATE,NAME", "connection", "show"],
        )
        .unwrap_or_default(),
    );
    let vpn = group("VPN");
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("從檔案加入 VPN")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let again = done.clone();
    add.connect_clicked(move |button| import_vpn(button, again.clone()));
    vpn.set_header_suffix(Some(&add));
    for network in &vpns {
        vpn.add(&vpn_row(network, &done));
    }
    if vpns.is_empty() {
        vpn.add(&adw::ActionRow::builder().title("沒有 VPN").build());
    }
    groups.push(vpn);

    for group in groups.iter().chain(&shown.kept) {
        page.add(group);
    }
    shown.groups.replace(groups);
}

/// GNOME's proxy settings, which GIO's programs and the browsers follow.
const PROXY: &str = "org.gnome.system.proxy";
const PROXY_MODES: [(&str, &str); 3] = [("無", "none"), ("手動", "manual"), ("自動設定", "auto")];

/// One of the proxy's settings, without the quotes gsettings prints a string in.
fn proxy_setting(schema: &str, key: &str) -> String {
    output("gsettings", &["get", schema, key])
        .map(|value| value.trim().trim_matches('\'').to_string())
        .unwrap_or_default()
}

/// A proxy typed as host:port, [IPv6]:port taken too; None when nothing is typed.
fn host_port(typed: &str) -> Result<Option<(String, u16)>, &'static str> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Ok(None);
    }
    let wrong = "請以「主機:連接埠」的格式輸入代理伺服器。";
    let (host, port) = typed.rsplit_once(':').ok_or(wrong)?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port: u16 = port.parse().ok().filter(|port| *port > 0).ok_or(wrong)?;
    if host.is_empty() || host.contains(|c: char| c.is_whitespace() || c == '\'' || c == '\\') {
        return Err(wrong);
    }
    Ok(Some((host.to_string(), port)))
}

/// Hosts typed apart by commas or spaces, as gsettings takes a list of strings.
fn host_list(typed: &str) -> Result<String, &'static str> {
    let hosts: Vec<&str> = typed.split([',', ' ']).filter(|host| !host.is_empty()).collect();
    if hosts.iter().any(|host| host.contains(['\'', '\\'])) {
        return Err("略過的主機的格式不正確。");
    }
    Ok(format!(
        "[{}]",
        hosts
            .iter()
            .map(|host| format!("'{host}'"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// The proxy: none, by hand for each protocol, or from a configuration's URL; each field
/// takes effect as it is applied.
fn proxy_group() -> adw::PreferencesGroup {
    let proxy = group("代理伺服器");
    let mode = proxy_setting(PROXY, "mode");
    let chosen = PROXY_MODES
        .iter()
        .position(|(_, value)| *value == mode)
        .unwrap_or(0);
    let how = adw::ComboRow::builder()
        .title("設定")
        .model(&gtk::StringList::new(&PROXY_MODES.map(|(label, _)| label)))
        .selected(chosen as u32)
        .build();
    proxy.add(&how);

    let field = |title: &str, text: &str| {
        adw::EntryRow::builder()
            .title(title)
            .text(text)
            .show_apply_button(true)
            .build()
    };
    let mut manual = Vec::new();
    for (title, protocol) in [("HTTP", "http"), ("HTTPS", "https"), ("SOCKS", "socks")] {
        let schema = format!("{PROXY}.{protocol}");
        let host = proxy_setting(&schema, "host");
        let port = proxy_setting(&schema, "port");
        let shown = if host.is_empty() {
            String::new()
        } else {
            format!("{host}:{port}")
        };
        let row = field(title, &shown);
        row.connect_apply(move |row| match host_port(&row.text()) {
            Ok(typed) => {
                let (host, port) = typed.unwrap_or_default();
                crate::run("gsettings", &["set", &schema, "host", &format!("'{host}'")]);
                crate::run("gsettings", &["set", &schema, "port", &port.to_string()]);
            }
            Err(why) => alert::notice(row, "無法設定代理伺服器", Some(why)),
        });
        manual.push(row);
    }
    let ignored = proxy_setting(PROXY, "ignore-hosts");
    let ignored = ignored.trim_matches(['[', ']']).replace('\'', "");
    let skip = field("略過的主機", &ignored);
    skip.connect_apply(|row| match host_list(&row.text()) {
        Ok(hosts) => crate::run("gsettings", &["set", PROXY, "ignore-hosts", &hosts]),
        Err(why) => alert::notice(row, "無法設定代理伺服器", Some(why)),
    });
    manual.push(skip);
    let url = field("設定檔 URL", &proxy_setting(PROXY, "autoconfig-url"));
    url.connect_apply(|row| {
        let typed = row.text().trim().replace(['\'', '\\'], "");
        crate::run(
            "gsettings",
            &["set", PROXY, "autoconfig-url", &format!("'{typed}'")],
        );
    });

    let show = {
        let (manual, url) = (manual.clone(), url.clone());
        move |chosen: u32| {
            for row in &manual {
                row.set_visible(chosen == 1);
            }
            url.set_visible(chosen == 2);
        }
    };
    show(chosen as u32);
    for row in &manual {
        proxy.add(row);
    }
    proxy.add(&url);
    how.connect_selected_notify(move |how| {
        crate::run(
            "gsettings",
            &["set", PROXY, "mode", PROXY_MODES[how.selected() as usize].1],
        );
        show(how.selected());
    });
    proxy
}

/// Where ufw keeps whether it is on, which anyone may read.
const UFW_CONF: &str = "/etc/ufw/ufw.conf";

fn firewall_on() -> bool {
    std::fs::read_to_string(UFW_CONF).is_ok_and(|conf| conf.lines().any(|line| line.trim() == "ENABLED=yes"))
}

/// ufw, turned on and off as root; none without ufw.
fn firewall_group() -> Option<adw::PreferencesGroup> {
    if !std::path::Path::new(UFW_CONF).exists() {
        return None;
    }
    let firewall = group("");
    let switch = adw::SwitchRow::builder()
        .title("防火牆")
        .subtitle("阻擋其他電腦連入")
        .active(firewall_on())
        .build();
    // Set from what ufw says after a change, which is not a change of its own.
    let showing = Rc::new(std::cell::Cell::new(false));
    switch.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        let (verb, reason) = if row.is_active() {
            ("enable", "開啟防火牆")
        } else {
            ("disable", "關閉防火牆")
        };
        row.set_sensitive(false);
        let shown = {
            let (row, showing) = (row.clone(), showing.clone());
            move || {
                showing.set(true);
                row.set_active(firewall_on());
                showing.set(false);
                row.set_sensitive(true);
            }
        };
        match crate::sudo(reason, &["ufw", "--force", verb]) {
            // Turned down or failed, the switch goes back to what ufw is.
            Ok(sudo) => sudo.wait_check_async(gio::Cancellable::NONE, move |result| {
                if let Err(err) = result {
                    eprintln!("ufw {verb}: {err}");
                }
                shown();
            }),
            Err(err) => {
                eprintln!("sudo: {err}");
                shown();
            }
        }
    });
    firewall.add(&switch);
    Some(firewall)
}

/// A port, joined and left with its switch, with its details while a connection is up on it.
fn port_row(port: &Port, named: bool, done: &Rc<dyn Fn()>) -> adw::ActionRow {
    let title: String = if named {
        format!("乙太網路（{}）", port.device)
    } else {
        "乙太網路".into()
    };
    let mut status = port.status().to_string();
    if port.state.starts_with("connected") {
        let address = device_field(&port.device, "IP4.ADDRESS");
        if let Some(address) = address
            .split(" | ")
            .next()
            .and_then(|a| a.split('/').next())
            .filter(|a| !a.is_empty())
        {
            status = format!("{status} · {address}");
        }
    }
    let row = adw::ActionRow::builder().title(&title).subtitle(&status).build();
    let switch = gtk::Switch::builder()
        .active(port.up())
        .sensitive(port.usable())
        .valign(gtk::Align::Center)
        .build();
    // Named for screen readers by the row it turns on and off.
    switch.update_property(&[gtk::accessible::Property::Label(&title)]);
    let (device, again) = (port.device.clone(), done.clone());
    switch.connect_active_notify(move |switch| {
        let (verb, failure) = if switch.is_active() {
            ("connect", "無法連線到乙太網路")
        } else {
            ("disconnect", "無法中斷乙太網路連線")
        };
        let (switch, again) = (switch.clone(), again.clone());
        nmcli(&["device", verb, &device], move |result| {
            if let Err(err) = result {
                alert::notice(&switch, failure, Some(&err));
            }
            again();
        });
    });
    row.add_suffix(&switch);
    if let Some(uuid) = port.uuid.clone() {
        let details = details_button();
        let (port, again) = (port.clone(), done.clone());
        details.connect_clicked(move |button| port_dialog(button, &port, &uuid, &title, again.clone()));
        row.add_suffix(&details);
    }
    row
}

fn details_button() -> gtk::Button {
    gtk::Button::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text("詳細資訊")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build()
}

/// A dialog of `page`'s groups under a header bar.
fn present_dialog(parent: &impl IsA<gtk::Widget>, title: &str, page: &adw::PreferencesPage) -> adw::Dialog {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(page));
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(440)
        .child(&view)
        .build();
    dialog.present(Some(parent));
    dialog
}

/// What a connection does on its own: come up whenever it can.
fn autoconnect_row(uuid: &str) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title("自動連線")
        .active(setting(uuid, "connection.autoconnect").as_deref() == Some("yes"))
        .build();
    let uuid = uuid.to_string();
    row.connect_active_notify(move |row| {
        let value = if row.is_active() { "yes" } else { "no" };
        nmcli(
            &[
                "connection",
                "modify",
                "uuid",
                &uuid,
                "connection.autoconnect",
                value,
            ],
            |_| {},
        );
    });
    row
}

/// A port's addresses, its hardware, and how its addresses are set.
fn port_dialog(parent: &gtk::Button, port: &Port, uuid: &str, title: &str, done: Rc<dyn Fn()>) {
    let page = adw::PreferencesPage::new();
    let facts = group("");
    let first = device_field(&port.device, "IP4.ADDRESS");
    let first = first.split(" | ").next().unwrap_or_default();
    let (address, prefix) = first.split_once('/').unwrap_or((first, ""));
    let mask = prefix
        .parse()
        .map(prefix_mask)
        .map(|mask| mask.to_string())
        .unwrap_or_default();
    for (name, value) in [
        ("IP 位址", address.to_string()),
        ("子網路遮罩", mask),
        ("路由器", device_field(&port.device, "IP4.GATEWAY")),
        ("DNS", device_field(&port.device, "IP4.DNS").replace(" | ", "\n")),
        ("硬體位址", device_field(&port.device, "GENERAL.HWADDR")),
        ("速度", device_field(&port.device, "CAPABILITIES.SPEED")),
    ] {
        if !value.is_empty() && value != "unknown" {
            facts.add(&fact_row(name, &value));
        }
    }
    page.add(&facts);
    let joining = group("");
    joining.add(&autoconnect_row(uuid));
    page.add(&joining);
    address_settings(&page, uuid);
    let dialog = present_dialog(parent, title, &page);
    dialog.connect_closed(move |_| done());
}

/// A VPN, turned on and off with its switch.
fn vpn_row(vpn: &Vpn, done: &Rc<dyn Fn()>) -> adw::ActionRow {
    let kind = vpn_kind(vpn);
    let subtitle = match vpn.state.as_str() {
        "activated" => format!("{kind} · 已連線"),
        "activating" => format!("{kind} · 正在連線…"),
        _ => kind.clone(),
    };
    let row = adw::ActionRow::builder()
        .title(&vpn.name)
        .subtitle(&subtitle)
        .build();
    let switch = gtk::Switch::builder()
        .active(matches!(vpn.state.as_str(), "activated" | "activating"))
        .valign(gtk::Align::Center)
        .build();
    switch.update_property(&[gtk::accessible::Property::Label(&vpn.name)]);
    let (uuid, name, again) = (vpn.uuid.clone(), vpn.name.clone(), done.clone());
    switch.connect_active_notify(move |switch| {
        let on = switch.is_active();
        let (switch, again, name, uuid) = (switch.clone(), again.clone(), name.clone(), uuid.clone());
        let id = uuid.clone();
        let args = ["connection", if on { "up" } else { "down" }, "uuid", id.as_str()];
        nmcli(&args, move |result| match result {
            // Nekoland runs no secret agent, so a password NetworkManager does not keep is
            // asked for here and given as the VPN comes up.
            Err(err) if on && err.contains("ecrets") => ask_vpn_password(&switch, uuid, name, again),
            Err(err) => {
                let title = if on {
                    format!("無法連線到「{name}」")
                } else {
                    format!("無法中斷「{name}」")
                };
                alert::notice(&switch, &title, Some(&err));
                again();
            }
            Ok(_) => again(),
        });
    });
    row.add_suffix(&switch);
    let details = details_button();
    let (vpn, again) = (vpn.clone(), done.clone());
    details.connect_clicked(move |button| vpn_dialog(button, &vpn, &kind, again.clone()));
    row.add_suffix(&details);
    row
}

/// Asks for a VPN's password and brings it up with it; the password goes through a pipe,
/// never a command line, and is not kept.
fn ask_vpn_password(parent: &gtk::Switch, uuid: String, name: String, done: Rc<dyn Fn()>) {
    let password = gtk::PasswordEntry::builder()
        .placeholder_text("密碼")
        .show_peek_icon(true)
        .width_request(alert::FIELD_WIDTH)
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::new(Some(&format!("輸入「{name}」的密碼")), None);
    dialog.set_extra_child(Some(&password));
    dialog.add_responses(&[("cancel", "取消"), ("connect", "連線")]);
    dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("connect"));
    dialog.set_close_response("cancel");
    password.connect_map(|field| {
        let field = field.clone();
        glib::idle_add_local_once(move || {
            field.grab_focus();
        });
    });
    let parent = parent.clone();
    dialog.choose(&parent.clone(), gio::Cancellable::NONE, move |response| {
        let typed = password.text().to_string();
        if response != "connect" || typed.is_empty() {
            return done();
        }
        up_with(&uuid, &format!("vpn.secrets.password:{typed}\n"), move |result| {
            if let Err(err) = result {
                alert::notice(&parent, &format!("無法連線到「{name}」"), Some(&err));
            }
            done();
        });
    });
}

/// A VPN's kind, whether it comes up on its own, and removing it.
fn vpn_dialog(parent: &gtk::Button, vpn: &Vpn, kind: &str, done: Rc<dyn Fn()>) {
    let page = adw::PreferencesPage::new();
    let facts = group("");
    facts.add(&fact_row("類型", kind));
    facts.add(&autoconnect_row(&vpn.uuid));
    page.add(&facts);
    let remove = gtk::Button::builder()
        .label("移除 VPN")
        .halign(gtk::Align::Center)
        .css_classes(["destructive-action", "pill"])
        .build();
    let end = group("");
    end.add(&remove);
    page.add(&end);
    let dialog = present_dialog(parent, &vpn.name, &page);
    let (uuid, name) = (vpn.uuid.clone(), vpn.name.clone());
    remove.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |button| {
            let (uuid, done) = (uuid.clone(), done.clone());
            let title = format!("移除「{name}」？");
            alert::confirm(
                button,
                &title,
                Some("再次使用時需要重新加入設定檔。"),
                "移除",
                move || {
                    nmcli(&["connection", "delete", "uuid", &uuid], move |_| done());
                    dialog.close();
                },
            );
        }
    ));
}

/// Adds a VPN from a WireGuard .conf or an OpenVPN .ovpn file.
fn import_vpn(button: &gtk::Button, done: Rc<dyn Fn()>) {
    let profiles = gtk::FileFilter::new();
    profiles.set_name(Some("VPN 設定檔"));
    profiles.add_suffix("conf");
    profiles.add_suffix("ovpn");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&profiles);
    let dialog = gtk::FileDialog::builder()
        .title("選擇 VPN 設定檔")
        .filters(&filters)
        .build();
    let window = button.root().and_downcast::<gtk::Window>();
    let button = button.clone();
    dialog.open(window.as_ref(), gio::Cancellable::NONE, move |chosen| {
        let path = match chosen.map(|file| file.path()) {
            Ok(Some(path)) => path,
            Ok(None) => return alert::notice(&button, "無法加入 VPN", Some("請選擇這部電腦上的檔案。")),
            Err(err) if err.matches(gtk::DialogError::Dismissed) => return,
            Err(err) => return alert::notice(&button, "無法加入 VPN", Some(err.message())),
        };
        let kind = match path.extension().and_then(|ext| ext.to_str()) {
            Some("ovpn") => "openvpn",
            _ => "wireguard",
        };
        // NetworkManager names it after the file, and fails an assertion and restarts when
        // a second one of that name is imported.
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let names = output("nmcli", &["-t", "-f", "NAME", "connection", "show"]).unwrap_or_default();
        if names.lines().any(|line| nmcli_fields(line).join(":") == name) {
            let why = format!("已經有名為「{name}」的連線。");
            return alert::notice(&button, "無法加入 VPN", Some(&why));
        }
        let file = path.to_string_lossy().into_owned();
        nmcli(
            &["connection", "import", "type", kind, "file", &file],
            move |result| {
                if let Err(err) = result {
                    alert::notice(&button, "無法加入 VPN", Some(&err));
                }
                done();
            },
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_the_ethernet_devices() {
        let status = "enp0s4:ethernet:connected:5f0c\nwlp2s0:wifi:disconnected:\nenp0s5:ethernet:unavailable:\nlo:loopback:unmanaged:\n";
        let ports = parse_ports(status);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].uuid.as_deref(), Some("5f0c"));
        assert_eq!(ports[0].status(), "已連線");
        assert_eq!(ports[1].uuid, None);
        assert!(!ports[1].usable());
        assert_eq!(ports[1].status(), "未接上網路線");
    }

    #[test]
    fn vpns_are_vpn_and_wireguard_connections() {
        let list = "a1:wireguard:activated:wg\\:home\nb2:vpn::Office\nc3:802-3-ethernet:activated:Wired\n";
        assert_eq!(
            parse_vpns(list),
            vec![
                Vpn {
                    uuid: "a1".into(),
                    name: "wg:home".into(),
                    kind: "wireguard".into(),
                    state: "activated".into()
                },
                Vpn {
                    uuid: "b2".into(),
                    name: "Office".into(),
                    kind: "vpn".into(),
                    state: String::new()
                },
            ]
        );
    }

    #[test]
    fn masks_and_prefixes_match() {
        assert_eq!(prefix_mask(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(prefix_mask(0), Ipv4Addr::new(0, 0, 0, 0));
        assert_eq!(prefix_mask(32), Ipv4Addr::new(255, 255, 255, 255));
        assert_eq!(mask_prefix("255.255.252.0"), Some(22));
        assert_eq!(mask_prefix("/24"), Some(24));
        assert_eq!(mask_prefix("255.0.255.0"), None);
        assert_eq!(mask_prefix("33"), None);
    }

    #[test]
    fn addresses_set_by_hand_are_checked() {
        assert_eq!(
            manual_addresses("192.168.1.20", "255.255.255.0", "192.168.1.1"),
            Ok(("192.168.1.20/24".into(), "192.168.1.1".into()))
        );
        assert!(manual_addresses("192.168.1", "24", "").is_err());
        assert!(manual_addresses("192.168.1.20", "24", "router").is_err());
        assert_eq!(
            dns_servers("1.1.1.1, 2606:4700::1111 8.8.8.8"),
            Ok(("1.1.1.1,8.8.8.8".into(), "2606:4700::1111".into()))
        );
        assert_eq!(dns_servers(""), Ok((String::new(), String::new())));
        assert_eq!(
            manual_v6("2001:db8::2", ""),
            Ok(("2001:db8::2/64".into(), String::new()))
        );
        assert!(manual_v6("2001:db8::2/129", "").is_err());
        assert!(manual_v6("2001:db8::2/48", "fe80::1::1").is_err());
        assert!(dns_servers("dns.example").is_err());
    }

    #[test]
    fn proxies_are_host_and_port() {
        assert_eq!(
            host_port(" proxy.lan:3128 "),
            Ok(Some(("proxy.lan".into(), 3128)))
        );
        assert_eq!(host_port("[::1]:8080"), Ok(Some(("::1".into(), 8080))));
        assert_eq!(host_port(""), Ok(None));
        assert!(host_port("proxy.lan").is_err());
        assert!(host_port("proxy.lan:0").is_err());
        assert_eq!(
            host_list("localhost, 127.0.0.0/8"),
            Ok("['localhost', '127.0.0.0/8']".into())
        );
        assert_eq!(host_list(""), Ok("[]".into()));
    }
}
