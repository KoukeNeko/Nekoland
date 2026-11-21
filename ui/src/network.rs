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
    bluetooth,
    wifi::{follow_network, nmcli, nmcli_fields, radio_on, setting, up_with},
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

/// How IPv4 can be set, as people read it and as NetworkManager's ipv4.method has it.
const IPV4_METHODS: [(&str, &str); 3] = [("使用 DHCP", "auto"), ("手動", "manual"), ("關閉", "disabled")];

/// MTUs as the Mac offers them, before one typed; NetworkManager's 0 leaves it to the port.
const MTUS: [(&str, u32); 3] = [("自動", 0), ("標準（1500）", 1500), ("巨型訊框（9000）", 9000)];
const CUSTOM_MTU: &str = "自訂";
/// What an MTU typed may be: the least IPv4 takes, and what a jumbo frame carries.
const MTU_LEAST: f64 = 576.0;
const MTU_MOST: f64 = 9000.0;
const STANDARD_MTU: u32 = 1500;

/// The Mac's 私密 Wi-Fi 位址, as NetworkManager's cloned-mac-address has it: the hardware's
/// own, one kept for each network, and a new one each time it is joined.
const PRIVATE_ADDRESSES: [(&str, &str); 3] = [("關閉", "permanent"), ("固定", "stable"), ("輪替", "random")];

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

/// The connection types that have hardware of their own to set, as nmcli names them.
pub(crate) const ETHERNET_TYPE: &str = "802-3-ethernet";
pub(crate) const WIFI_TYPE: &str = "802-11-wireless";
const VPN_PASSWORD: &str = "vpn.secrets.password";

/// The rows a connection's IPv4 is set with: how, and what is set by hand.
struct Ipv4Rows {
    how: adw::ComboRow,
    address: adw::EntryRow,
    mask: adw::EntryRow,
    router: adw::EntryRow,
}

/// The rows a connection's IPv6 is set with, when it is set one of IPV6_METHODS's ways.
struct Ipv6Rows {
    how: adw::ComboRow,
    address: adw::EntryRow,
    router: adw::EntryRow,
}

/// A connection's MTU and, for Wi-Fi, its private address, as the Mac's 硬體 has them.
struct HardwareRows {
    kind: String,
    mtu: adw::ComboRow,
    custom: adw::SpinRow,
    private: Option<adw::ComboRow>,
}

/// A connection's MTU; nmcli says "auto" for 0, which leaves it to the port.
fn current_mtu(uuid: &str, kind: &str) -> u32 {
    setting(uuid, &format!("{kind}.mtu"))
        .and_then(|mtu| mtu.parse().ok())
        .unwrap_or(0)
}

/// Which of PRIVATE_ADDRESSES a Wi-Fi connection has; its default, the hardware's own
/// address, reads as 關閉.
fn current_private(uuid: &str) -> u32 {
    let current = setting(uuid, &format!("{WIFI_TYPE}.cloned-mac-address")).unwrap_or_default();
    PRIVATE_ADDRESSES
        .iter()
        .position(|(_, value)| *value == current)
        .unwrap_or(0) as u32
}

/// What address_settings shows, read again as it is applied.
struct AddressRows {
    uuid: String,
    ipv4: Ipv4Rows,
    ipv6: Option<Ipv6Rows>,
    servers: adw::EntryRow,
    search: adw::EntryRow,
    hardware: Option<HardwareRows>,
}

fn entry_row(title: &str, text: &str, visible: bool) -> adw::EntryRow {
    adw::EntryRow::builder()
        .title(title)
        .text(text)
        .visible(visible)
        .build()
}

fn choice_row(title: &str, labels: &[&str], selected: u32) -> adw::ComboRow {
    adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(labels))
        .selected(selected)
        .build()
}

/// The first of what nmcli lists apart by commas.
fn first_listed(list: &str) -> String {
    list.split(',').next().unwrap_or_default().trim().to_string()
}

/// The mask a prefix length as nmcli prints it stands for, or nothing for none.
fn mask_text(prefix: &str) -> String {
    prefix
        .parse()
        .map(|prefix| prefix_mask(prefix).to_string())
        .unwrap_or_default()
}

/// Whether a connection is up, on whatever device.
fn is_up(uuid: &str) -> bool {
    setting(uuid, "GENERAL.STATE").is_some_and(|state| !state.is_empty())
}

/// How a connection's addresses, DNS servers and hardware are set, with what applies them,
/// for a connection whose IPv4 is DHCP's, set by hand, or off.
pub fn address_settings(page: &adw::PreferencesPage, uuid: &str) {
    let method = setting(uuid, "ipv4.method").unwrap_or_default();
    let Some(chosen) = IPV4_METHODS.iter().position(|(_, known)| *known == method) else {
        return;
    };
    // Each adds its group, in the Mac's order.
    let ipv4 = ipv4_rows(page, uuid, chosen);
    let ipv6 = ipv6_rows(page, uuid);
    let (servers, search) = dns_rows(page, uuid);
    let hardware = hardware_rows(page, uuid);
    let rows = AddressRows { uuid: uuid.to_string(), ipv4, ipv6, servers, search, hardware };

    let apply = gtk::Button::builder()
        .label("套用")
        .halign(gtk::Align::Center)
        .css_classes(["suggested-action", "pill"])
        .build();
    let end = group("");
    end.add(&apply);
    page.add(&end);
    apply.connect_clicked(move |button| match rows.modify_args() {
        Ok(args) => apply_settings(button, &rows.uuid, &args),
        Err(why) => alert::notice(button, "無法套用設定", Some(why)),
    });
}

fn ipv4_rows(page: &adw::PreferencesPage, uuid: &str, chosen: usize) -> Ipv4Rows {
    let manual = IPV4_METHODS[chosen].1 == "manual";
    let first = first_listed(&setting(uuid, "ipv4.addresses").unwrap_or_default());
    let (address, prefix) = first.split_once('/').unwrap_or((first.as_str(), ""));
    let how = choice_row("設定 IPv4", &IPV4_METHODS.map(|(label, _)| label), chosen as u32);
    let address = entry_row("IP 位址", address, manual);
    let mask = entry_row("子網路遮罩", &mask_text(prefix), manual);
    let router = entry_row("路由器", &setting(uuid, "ipv4.gateway").unwrap_or_default(), manual);
    let by_hand = [address.clone(), mask.clone(), router.clone()];
    how.connect_selected_notify(move |how| {
        for row in &by_hand {
            row.set_visible(IPV4_METHODS[how.selected() as usize].1 == "manual");
        }
    });
    let ipv4 = group("IPv4");
    if IPV4_METHODS[chosen].1 == "auto" && is_up(uuid) {
        ipv4.set_header_suffix(Some(&renew_button(uuid)));
    }
    for row in [how.upcast_ref::<gtk::Widget>(), address.upcast_ref(), mask.upcast_ref(), router.upcast_ref()] {
        ipv4.add(row);
    }
    page.add(&ipv4);
    Ipv4Rows { how, address, mask, router }
}

/// The Mac's 更新 DHCP 租約: the connection comes up again and asks DHCP anew.
fn renew_button(uuid: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .label("更新 DHCP 租約")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let uuid = uuid.to_string();
    button.connect_clicked(move |button| {
        button.set_sensitive(false);
        let button = button.clone();
        nmcli(&["connection", "up", "uuid", &uuid], move |result| {
            button.set_sensitive(true);
            if let Err(err) = result {
                alert::notice(&button, "無法更新 DHCP 租約", Some(&err));
            }
        });
    });
    button
}

/// IPv6's rows, but for a connection whose IPv6 is set some other way, as shared, which
/// keeps it.
fn ipv6_rows(page: &adw::PreferencesPage, uuid: &str) -> Option<Ipv6Rows> {
    let method = setting(uuid, "ipv6.method").unwrap_or_default();
    let chosen = IPV6_METHODS.iter().position(|(_, known)| *known == method)?;
    let manual = IPV6_METHODS[chosen].1 == "manual";
    let first = first_listed(&setting(uuid, "ipv6.addresses").unwrap_or_default());
    let how = choice_row("設定 IPv6", &IPV6_METHODS.map(|(label, _)| label), chosen as u32);
    let address = entry_row("IP 位址", &first, manual);
    let router = entry_row("路由器", &setting(uuid, "ipv6.gateway").unwrap_or_default(), manual);
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
    Some(Ipv6Rows { how, address, router })
}

/// The DNS servers and search domains, as they are typed.
fn dns_rows(page: &adw::PreferencesPage, uuid: &str) -> (adw::EntryRow, adw::EntryRow) {
    let current: Vec<String> = ["ipv4.dns", "ipv6.dns"]
        .iter()
        .filter_map(|key| setting(uuid, key))
        .filter(|servers| !servers.is_empty())
        .collect();
    let servers = entry_row("DNS 伺服器", &current.join(",").replace(',', ", "), true);
    let domains = setting(uuid, "ipv4.dns-search").unwrap_or_default();
    let search = entry_row("搜尋網域", &domains.replace(',', ", "), true);
    let dns = group("DNS");
    dns.add(&servers);
    dns.add(&search);
    page.add(&dns);
    (servers, search)
}

/// The MTU, and for Wi-Fi the private address, for an Ethernet or Wi-Fi connection.
fn hardware_rows(page: &adw::PreferencesPage, uuid: &str) -> Option<HardwareRows> {
    let kind = setting(uuid, "connection.type")?;
    if kind != ETHERNET_TYPE && kind != WIFI_TYPE {
        return None;
    }
    let shown_mtu = current_mtu(uuid, &kind);
    let custom_choice = MTUS.len() as u32;
    let chosen = MTUS
        .iter()
        .position(|(_, mtu)| *mtu == shown_mtu)
        .map_or(custom_choice, |index| index as u32);
    let labels: Vec<&str> = MTUS.iter().map(|(label, _)| *label).chain([CUSTOM_MTU]).collect();
    let mtu = choice_row("MTU", &labels, chosen);
    let custom = adw::SpinRow::with_range(MTU_LEAST, MTU_MOST, 1.0);
    custom.set_title("自訂 MTU");
    custom.set_value(f64::from(if chosen == custom_choice { shown_mtu } else { STANDARD_MTU }));
    custom.set_visible(chosen == custom_choice);
    let typed = custom.clone();
    mtu.connect_selected_notify(move |row| typed.set_visible(row.selected() == custom_choice));
    let hardware = group("硬體");
    hardware.add(&mtu);
    hardware.add(&custom);
    let private = (kind == WIFI_TYPE).then(|| {
        let labels = PRIVATE_ADDRESSES.map(|(label, _)| label);
        let row = choice_row("私密 Wi-Fi 位址", &labels, current_private(uuid));
        hardware.add(&row);
        row
    });
    page.add(&hardware);
    Some(HardwareRows { kind, mtu, custom, private })
}

impl HardwareRows {
    /// What differs from what the connection has, as nmcli's settings and their values; what
    /// is the same is not written, as a private address left at its default.
    fn changes(&self, uuid: &str) -> Vec<(String, String)> {
        let mut changes = Vec::new();
        let mtu = MTUS
            .get(self.mtu.selected() as usize)
            .map_or(self.custom.value() as u32, |(_, mtu)| *mtu);
        if mtu != current_mtu(uuid, &self.kind) {
            changes.push((format!("{}.mtu", self.kind), mtu.to_string()));
        }
        if let Some(row) = &self.private {
            if row.selected() != current_private(uuid) {
                let value = PRIVATE_ADDRESSES[row.selected() as usize].1;
                changes.push((format!("{WIFI_TYPE}.cloned-mac-address"), value.to_string()));
            }
        }
        changes
    }
}

impl AddressRows {
    /// What `nmcli` is given to set what is shown, or why it cannot be.
    fn modify_args(&self) -> Result<Vec<String>, &'static str> {
        let mut args: Vec<String> = ["connection", "modify", "uuid", self.uuid.as_str()]
            .map(String::from)
            .into();
        let mut set = |key: &str, value: String| args.extend([key.to_string(), value]);
        let v4_method = IPV4_METHODS[self.ipv4.how.selected() as usize].1;
        let ipv4 = &self.ipv4;
        // What was set by hand goes otherwise, or DHCP's addresses come with it.
        let (address, router) = if v4_method == "manual" {
            manual_addresses(&ipv4.address.text(), &ipv4.mask.text(), &ipv4.router.text())?
        } else {
            Default::default()
        };
        set("ipv4.method", v4_method.into());
        set("ipv4.addresses", address);
        set("ipv4.gateway", router);
        let (v4_servers, v6_servers) = dns_servers(&self.servers.text())?;
        let v6_method = self.ipv6.as_ref().map(|rows| IPV6_METHODS[rows.how.selected() as usize].1);
        // NetworkManager refuses servers for an IP version that is off, and drops IPv6's on a
        // connection whose IPv6 is set some other way.
        if !v4_servers.is_empty() && v4_method == "disabled" {
            return Err("IPv4 未開啟，無法使用 IPv4 的 DNS 伺服器。");
        }
        if !v6_servers.is_empty() && v6_method.is_none_or(|method| method == "disabled") {
            return Err("IPv6 未開啟，無法使用 IPv6 的 DNS 伺服器。");
        }
        // Servers typed are used instead of DHCP's, as the Mac does.
        let ignore = if v4_servers.is_empty() && v6_servers.is_empty() { "no" } else { "yes" };
        set("ipv4.dns", v4_servers);
        set("ipv4.ignore-auto-dns", ignore.into());
        set("ipv4.dns-search", typed_list(&self.search.text()));
        if let (Some(rows), Some(method)) = (&self.ipv6, v6_method) {
            let (address, router) = if method == "manual" {
                manual_v6(&rows.address.text(), &rows.router.text())?
            } else {
                Default::default()
            };
            set("ipv6.method", method.into());
            set("ipv6.addresses", address);
            set("ipv6.gateway", router);
            set("ipv6.dns", v6_servers);
            if method != "disabled" {
                set("ipv6.ignore-auto-dns", ignore.into());
            }
        }
        for (key, value) in self.hardware.iter().flat_map(|hardware| hardware.changes(&self.uuid)) {
            set(&key, value);
        }
        Ok(args)
    }
}

/// Sets a connection with `args`, and brings it up again when it is up, so that it takes them.
fn apply_settings(button: &gtk::Button, uuid: &str, args: &[String]) {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (button, uuid) = (button.clone(), uuid.to_string());
    button.set_sensitive(false);
    nmcli(&args, move |result| {
        let failed = |button: &gtk::Button, err: &str| {
            button.set_sensitive(true);
            alert::notice(button, "無法套用設定", Some(err));
        };
        match result {
            Err(err) => failed(&button, &err),
            Ok(_) if is_up(&uuid) => {
                nmcli(&["connection", "up", "uuid", &uuid], move |result| match result {
                    Ok(_) => button.set_sensitive(true),
                    Err(err) => failed(&button, &err),
                });
            }
            Ok(_) => button.set_sensitive(true),
        }
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
    let kept = [hotspot_group(), Some(proxy_group()), firewall_group()]
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

    if radio_on().is_some() {
        groups.push(airplane_group());
    }

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

/// 飛航模式: every radio off at once, the Wi-Fi and mobile radios through NetworkManager and
/// the Bluetooth adapter through BlueZ.
fn airplane_group() -> adw::PreferencesGroup {
    let has_bluetooth = bluetooth::has_bluetooth();
    let off = radio_on() == Some(false) && (!has_bluetooth || bluetooth::powered() == Some(false));
    let switch = adw::SwitchRow::builder().title("飛航模式").active(off).build();
    switch.connect_active_notify(move |row| {
        let (radios, power) = if row.is_active() { ("off", "off") } else { ("on", "on") };
        nmcli(&["radio", "all", radios], |_| {});
        if has_bluetooth {
            bluetooth::bluetoothctl(&["power", power], |_| {});
        }
    });
    let airplane = group("");
    airplane.add(&switch);
    airplane
}

/// The connection the hotspot is kept as, which nmcli's own `device wifi hotspot` names too.
const HOTSPOT: &str = "Hotspot";
/// The shortest password WPA takes.
const WPA_PASSWORD_MIN: usize = 8;

/// 個人熱點: this computer's Wi-Fi shared as an access point, with the name and password it
/// is joined by; none without a Wi-Fi device.
fn hotspot_group() -> Option<adw::PreferencesGroup> {
    let devices = output("nmcli", &["-t", "-f", "DEVICE,TYPE", "device", "status"])?;
    let wifi = devices.lines().find_map(|line| line.strip_suffix(":wifi"))?.to_string();
    let kept = |field: &str| {
        output("nmcli", &["-s", "-g", field, "connection", "show", HOTSPOT]).map(|value| value.trim().to_string())
    };
    let active = output("nmcli", &["-t", "-f", "NAME", "connection", "show", "--active"])
        .is_some_and(|names| names.lines().any(|name| name == HOTSPOT));
    let ssid = kept("802-11-wireless.ssid")
        .filter(|ssid| !ssid.is_empty())
        .or_else(|| output("hostname", &[]).map(|name| name.trim().to_string()))
        .unwrap_or_default();

    let name = adw::EntryRow::builder().title("網路名稱").text(ssid).build();
    let password = adw::PasswordEntryRow::builder().title("密碼").build();
    if let Some(saved) = kept("802-11-wireless-security.psk").filter(|psk| !psk.is_empty()) {
        password.set_text(&saved);
    }
    let switch = adw::SwitchRow::builder().title("個人熱點").active(active).build();
    let (name_row, password_row) = (name.clone(), password.clone());
    switch.connect_active_notify(move |row| {
        if !row.is_active() {
            nmcli(&["connection", "down", HOTSPOT], |_| {});
            return;
        }
        let (ssid, secret) = (name_row.text().to_string(), password_row.text().to_string());
        if ssid.is_empty() || secret.chars().count() < WPA_PASSWORD_MIN {
            alert::notice(row, "無法開啟個人熱點", Some("名稱不能是空白，密碼至少 8 個字元。"));
            row.set_active(false);
            return;
        }
        let exists = output("nmcli", &["-t", "-f", "NAME", "connection", "show"])
            .is_some_and(|names| names.lines().any(|name| name == HOTSPOT));
        let setup: Vec<&str> = if exists {
            vec!["connection", "modify", HOTSPOT, "802-11-wireless.ssid", &ssid]
        } else {
            vec![
                "connection", "add", "type", "wifi", "ifname", &wifi, "con-name", HOTSPOT, "autoconnect", "no",
                "ssid", &ssid, "802-11-wireless.mode", "ap", "ipv4.method", "shared",
                "802-11-wireless-security.key-mgmt", "wpa-psk",
            ]
        };
        let row = row.clone();
        let failed = move |row: &adw::SwitchRow, why: &str| {
            alert::notice(row, "無法開啟個人熱點", Some(why));
            row.set_active(false);
        };
        nmcli(&setup, move |result| {
            if let Err(why) = result {
                return failed(&row, &why);
            }
            let Some(uuid) = output("nmcli", &["-g", "connection.uuid", "connection", "show", HOTSPOT]) else {
                return failed(&row, "找不到熱點的連線設定。");
            };
            up_with(uuid.trim(), "802-11-wireless-security.psk", &secret, move |result| {
                if let Err(why) = result {
                    failed(&row, &why);
                }
            });
        });
    });
    let hotspot = group("");
    hotspot.add(&switch);
    hotspot.add(&name);
    hotspot.add(&password);
    Some(hotspot)
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
        match crate::sudo(reason, &["ufw", "--force", verb], gio::SubprocessFlags::NONE) {
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
    // A port left to systemd-networkd has its address all the same.
    if port.state.starts_with("connected") || port.state == "unmanaged" {
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
    if port.state == "unmanaged" && std::path::Path::new(NETPLAN).exists() {
        row.add_suffix(&hand_over_button(done));
        return row;
    }
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

/// netplan, through which Ubuntu Server leaves its Ethernet ports to systemd-networkd.
const NETPLAN: &str = "/usr/sbin/netplan";

/// Gives the ports netplan sets up to NetworkManager, as the installer does.
const HAND_OVER: &str = "nekoland-hand-over-network";

/// Lets NetworkManager take over the ports systemd-networkd keeps, so they can be set here.
fn hand_over_button(done: &Rc<dyn Fn()>) -> gtk::Button {
    let button = gtk::Button::builder()
        .label("改用 NetworkManager")
        .valign(gtk::Align::Center)
        .build();
    let done = done.clone();
    button.connect_clicked(move |button| {
        let (target, done) = (button.clone(), done.clone());
        alert::confirm(
            button,
            "改由 NetworkManager 管理乙太網路？",
            Some("網路會中斷幾秒。"),
            "改用",
            move || hand_over(&target, done),
        );
    });
    button
}

fn hand_over(button: &gtk::Button, done: Rc<dyn Fn()>) {
    button.set_sensitive(false);
    let failed = {
        let button = button.clone();
        move |why: Option<String>| {
            button.set_sensitive(true);
            if let Some(why) = why {
                alert::notice(&button, "無法改用 NetworkManager", Some(&why));
            }
        }
    };
    let program = crate::program::path(HAND_OVER);
    let program = program.to_string_lossy();
    let sudo = match crate::sudo("改由 NetworkManager 管理網路", &[&program], gio::SubprocessFlags::STDERR_PIPE) {
        Ok(sudo) => sudo,
        Err(err) => return failed(Some(err.message().to_string())),
    };
    sudo.communicate_utf8_async(None, gio::Cancellable::NONE, glib::clone!(
        #[strong]
        sudo,
        move |result| {
            match result {
                Ok(_) if sudo.is_successful() => {}
                Ok((_, said)) => failed(hand_over_failure(said.as_deref().unwrap_or_default())),
                Err(err) => failed(Some(err.message().to_string())),
            }
            done();
        }
    ));
}

/// What sudo says when askpass was turned down: sudo-rs's, then the original sudo's.
const NOT_GIVEN: [&str; 2] = ["not attempted", "no password was provided"];

/// Why handing over failed, from what sudo and netplan said; None when the password was not
/// given, which is turning it down.
fn hand_over_failure(said: &str) -> Option<String> {
    if NOT_GIVEN.iter().any(|words| said.contains(words)) {
        return None;
    }
    if said.contains("incorrect") {
        return Some("密碼不正確。".into());
    }
    let last = said.lines().map(str::trim).filter(|line| !line.is_empty()).last();
    Some(last.unwrap_or("netplan 無法套用設定。").to_string())
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

/// A switch for one of a connection's settings, set as it is turned; `on` and `off` are the
/// setting's values.
fn setting_switch(uuid: &str, title: &str, key: &'static str, (on, off): (&'static str, &'static str)) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(title)
        .active(setting(uuid, key).as_deref() == Some(on))
        .build();
    let (uuid, failure) = (uuid.to_string(), format!("無法變更「{title}」"));
    row.connect_active_notify(move |row| {
        let value = if row.is_active() { on } else { off };
        let (row, failure) = (row.clone(), failure.clone());
        nmcli(&["connection", "modify", "uuid", &uuid, key, value], move |result| {
            if let Err(err) = result {
                alert::notice(&row, &failure, Some(&err));
            }
        });
    });
    row
}

/// What a connection does on its own: come up whenever it can; Wi-Fi calls it joining.
pub(crate) fn autoconnect_row(uuid: &str, title: &str) -> adw::SwitchRow {
    setting_switch(uuid, title, "connection.autoconnect", ("yes", "no"))
}

/// The Mac's 低數據模式: the connection marked metered, which what downloads in the
/// background heeds; off leaves it to NetworkManager's guess, as for a phone's hotspot.
pub(crate) fn low_data_row(uuid: &str) -> adw::SwitchRow {
    setting_switch(uuid, "低數據模式", "connection.metered", ("yes", "unknown"))
}

/// The IPv6 addresses of a device's list as nmcli prints it, without their prefixes, and but
/// for the link's own, which are of no use elsewhere.
fn global_v6(list: &str) -> String {
    list.split(" | ")
        .filter(|address| !address.is_empty() && !address.starts_with("fe80:"))
        .filter_map(|address| address.split('/').next())
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a device has while it is up, as the Mac's details show it.
pub(crate) fn device_facts(device: &str) -> adw::PreferencesGroup {
    let facts = group("");
    let first = device_field(device, "IP4.ADDRESS");
    let first = first.split(" | ").next().unwrap_or_default();
    let (address, prefix) = first.split_once('/').unwrap_or((first, ""));
    let servers: Vec<String> = ["IP4.DNS", "IP6.DNS"]
        .iter()
        .map(|field| device_field(device, field))
        .filter(|servers| !servers.is_empty())
        .collect();
    for (name, value) in [
        ("IP 位址", address.to_string()),
        ("子網路遮罩", mask_text(prefix)),
        ("路由器", device_field(device, "IP4.GATEWAY")),
        ("IPv6 位址", global_v6(&device_field(device, "IP6.ADDRESS"))),
        ("IPv6 路由器", device_field(device, "IP6.GATEWAY")),
        ("DNS", servers.join(" | ").replace(" | ", "\n")),
        ("硬體位址", device_field(device, "GENERAL.HWADDR")),
        ("速度", device_field(device, "CAPABILITIES.SPEED")),
    ] {
        if !value.is_empty() && value != "unknown" {
            facts.add(&fact_row(name, &value));
        }
    }
    facts
}

/// A port's addresses, its hardware, and how its addresses are set.
fn port_dialog(parent: &gtk::Button, port: &Port, uuid: &str, title: &str, done: Rc<dyn Fn()>) {
    let page = adw::PreferencesPage::new();
    page.add(&device_facts(&port.device));
    let joining = group("");
    joining.add(&autoconnect_row(uuid, "自動連線"));
    joining.add(&low_data_row(uuid));
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
        up_with(&uuid, VPN_PASSWORD, &typed, move |result| {
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
    facts.add(&autoconnect_row(&vpn.uuid, "自動連線"));
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
    fn facts_leave_out_what_is_of_no_use() {
        assert_eq!(
            global_v6("2001:db8::5/64 | fe80::215:5dff:fe00:903/64 | 2001:db8::6/128"),
            "2001:db8::5\n2001:db8::6"
        );
        assert_eq!(global_v6("fe80::1/64"), "");
        assert_eq!(mask_text("20"), "255.255.240.0");
        assert_eq!(mask_text(""), "");
        assert_eq!(first_listed("10.0.0.2/24, 10.0.0.3/24"), "10.0.0.2/24");
    }

    #[test]
    fn hand_over_failures_say_why() {
        assert_eq!(hand_over_failure("sudo: no password was provided\nsudo: a password is required\n"), None);
        assert_eq!(hand_over_failure("sudo: Authentication required but not attempted\n"), None);
        assert_eq!(
            hand_over_failure("sudo: maximum 3 incorrect authentication attempts\n"),
            Some("密碼不正確。".into())
        );
        assert_eq!(
            hand_over_failure("Error in network definition: unknown key\n\n"),
            Some("Error in network definition: unknown key".into())
        );
        assert_eq!(hand_over_failure(""), Some("netplan 無法套用設定。".into()));
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
