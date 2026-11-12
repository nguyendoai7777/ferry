//! Địa chỉ của điện thoại (`PROTOCOL.md` 6.2, 6.8): thứ tự thử, nhập tay, dạng chuỗi lưu config — hàm thuần, interface
//! của PC truyền vào ([`LocalInterface`]) để test được mọi mạng (chỉ v4, chỉ v6, link-local).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

/// Cổng mặc định của điện thoại khi người dùng nhập tay không ghi cổng (màn Sync hiện `:8080`).
pub const DEFAULT_PORT: u16 = 8080;

/// Một địa chỉ IP trên một interface của PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalInterface {
    /// `en0`, `wlan0`, `Wi-Fi`…
    pub name: String,
    /// Index OS (scope id của link-local). `0` = không rõ.
    pub index: u32,
    pub ip: IpAddr,
    pub prefix_len: u8,
}

/// Interface của PC lúc này (mọi họ, bỏ interface đang tắt). Lỗi đọc ⇒ rỗng: thứ tự rơi về hạng "còn lại", vẫn thử hết.
pub fn local_interfaces() -> Vec<LocalInterface> {
    let all = match if_addrs::get_if_addrs() {
        Ok(a) => a,
        Err(e) => {
            log::warn!("discovery: cannot list interfaces: {e}");
            return Vec::new();
        }
    };
    all.into_iter()
        .filter(|i| i.is_oper_up() || matches!(i.oper_status, if_addrs::IfOperStatus::Unknown))
        .map(|i| {
            let (ip, prefix_len) = match &i.addr {
                if_addrs::IfAddr::V4(a) => (IpAddr::V4(a.ip), a.prefixlen),
                if_addrs::IfAddr::V6(a) => (IpAddr::V6(a.ip), a.prefixlen),
            };
            LocalInterface { name: i.name, index: i.index.unwrap_or(0), ip, prefix_len }
        })
        .collect()
}

/// Hạng thử theo `PROTOCOL.md` 6.2 — số nhỏ thử trước.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// IPv4 cùng subnet với một interface của PC.
    SameSubnetV4,
    /// IPv6 global / ULA cùng /64 với PC.
    SamePrefixV6,
    /// IPv6 link-local có scope id (interface PC nhận bản ghi mDNS).
    LinkLocalScoped,
    /// Mọi địa chỉ còn lại (khác subnet, link-local thiếu scope…).
    Other,
}

pub fn rank(addr: &SocketAddr, locals: &[LocalInterface]) -> Rank {
    match addr {
        SocketAddr::V4(a) => {
            let ip = *a.ip();
            let same = locals.iter().any(|l| match l.ip {
                IpAddr::V4(local) => same_prefix_v4(ip, local, l.prefix_len),
                IpAddr::V6(_) => false,
            });
            if same { Rank::SameSubnetV4 } else { Rank::Other }
        }
        SocketAddr::V6(a) => {
            let ip = *a.ip();
            if is_link_local_v6(&ip) {
                return if a.scope_id() != 0 { Rank::LinkLocalScoped } else { Rank::Other };
            }
            let same = locals.iter().any(|l| match l.ip {
                IpAddr::V6(local) => !is_link_local_v6(&local) && same_prefix_v6(ip, local, 64),
                IpAddr::V4(_) => false,
            });
            if same { Rank::SamePrefixV6 } else { Rank::Other }
        }
    }
}

/// Thứ tự thử (`PROTOCOL.md` 6.2): theo [`Rank`], cùng hạng giữ thứ tự vào (ổn định), bỏ trùng.
pub fn order_addresses(candidates: &[SocketAddr], locals: &[LocalInterface]) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::with_capacity(candidates.len());
    for a in candidates {
        if !out.contains(a) {
            out.push(*a);
        }
    }
    out.sort_by_key(|a| rank(a, locals));
    out
}

pub fn is_link_local_v6(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn same_prefix_v4(a: Ipv4Addr, b: Ipv4Addr, len: u8) -> bool {
    let len = u32::from(len.min(32));
    if len == 0 {
        return true;
    }
    let mask = u32::MAX << (32 - len);
    (u32::from(a) & mask) == (u32::from(b) & mask)
}

fn same_prefix_v6(a: Ipv6Addr, b: Ipv6Addr, len: u8) -> bool {
    let len = u32::from(len.min(128));
    if len == 0 {
        return true;
    }
    let mask = u128::MAX << (128 - len);
    (u128::from(a) & mask) == (u128::from(b) & mask)
}

/// Địa chỉ nhập tay / trong config không đọc được.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    /// Không phải `ip`, `ip:port`, `[v6]:port`, `[v6%iface]:port`.
    Invalid,
    /// Link-local mà tên interface (`%en0`) không có trên PC lúc này.
    UnknownInterface(String),
}

impl std::fmt::Display for AddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => f.write_str("invalid address"),
            Self::UnknownInterface(i) => write!(f, "unknown network interface {i}"),
        }
    }
}

impl std::error::Error for AddressError {}

/// Đọc địa chỉ nhập tay (`PROTOCOL.md` 6.2) hoặc dòng `addresses` của config: `1.2.3.4:8080`, `1.2.3.4`,
/// `[fd12::5]:8080`, `fd12::5`, `[fe80::5%en0]:8080`, `[fe80::5%12]:8080` (Windows). Chấp nhận cả URL màn Sync hiện
/// (`http://[fd12::5]:8080/`). Thiếu cổng ⇒ [`DEFAULT_PORT`]. Tên interface ⇒ index qua `locals`.
pub fn parse_address(text: &str, locals: &[LocalInterface]) -> Result<SocketAddr, AddressError> {
    let mut s = text.trim();
    for scheme in ["http://", "HTTP://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest;
        }
    }
    let s = s.split('/').next().unwrap_or("").trim();
    if s.is_empty() {
        return Err(AddressError::Invalid);
    }
    // `[v6(%zone)]` hoặc `[v6(%zone)]:port`.
    if let Some(rest) = s.strip_prefix('[') {
        let (inside, after) = rest.split_once(']').ok_or(AddressError::Invalid)?;
        let port = match after {
            "" => DEFAULT_PORT,
            p => p.strip_prefix(':').and_then(|p| p.parse().ok()).ok_or(AddressError::Invalid)?,
        };
        return v6_with_zone(inside, port, locals);
    }
    if let Ok(ip) = s.parse::<Ipv4Addr>() {
        return Ok(SocketAddr::new(IpAddr::V4(ip), DEFAULT_PORT));
    }
    if let Ok(a) = s.parse::<std::net::SocketAddrV4>() {
        return Ok(SocketAddr::V4(a));
    }
    // v6 trần không ngoặc (không có cổng: `:` là của địa chỉ).
    if s.contains(':') {
        return v6_with_zone(s, DEFAULT_PORT, locals);
    }
    Err(AddressError::Invalid)
}

fn v6_with_zone(text: &str, port: u16, locals: &[LocalInterface]) -> Result<SocketAddr, AddressError> {
    let (ip, zone) = match text.split_once('%') {
        Some((ip, zone)) => (ip, Some(zone)),
        None => (text, None),
    };
    let ip: Ipv6Addr = ip.parse().map_err(|_| AddressError::Invalid)?;
    let scope_id = match zone {
        None => 0,
        Some("") => return Err(AddressError::Invalid),
        Some(z) => match z.parse::<u32>() {
            Ok(n) => n,
            Err(_) => locals
                .iter()
                .find(|l| l.name == z && l.index != 0)
                .map(|l| l.index)
                .ok_or_else(|| AddressError::UnknownInterface(z.to_owned()))?,
        },
    };
    Ok(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, scope_id)))
}

/// Dạng chuỗi để lưu config (`config::PairedDevice::addresses`): link-local ghi **tên** interface (`%en0`) vì index có
/// thể đổi sau khởi động lại; không rõ tên thì ghi số. [`parse_address`] đọc lại được.
pub fn format_address(addr: &SocketAddr, locals: &[LocalInterface]) -> String {
    match addr {
        SocketAddr::V6(a) if a.scope_id() != 0 => {
            let zone = locals
                .iter()
                .find(|l| l.index == a.scope_id())
                .map_or_else(|| a.scope_id().to_string(), |l| l.name.clone());
            format!("[{}%{zone}]:{}", a.ip(), a.port())
        }
        SocketAddr::V6(a) => format!("[{}]:{}", a.ip(), a.port()),
        SocketAddr::V4(a) => a.to_string(),
    }
}

/// Địa chỉ từ `info.addresses` (IP không cổng, PROTOCOL 6.3) + cổng. Link-local trong `info` bị bỏ (scope phía điện
/// thoại vô nghĩa với PC); chuỗi hỏng bị bỏ.
pub fn from_info(addresses: &[String], port: u16) -> Vec<SocketAddr> {
    addresses
        .iter()
        .filter_map(|a| a.trim().parse::<IpAddr>().ok())
        // Link-local của điện thoại vô nghĩa với PC (scope phía điện thoại; iPhone gửi cả `169.254.x` của AWDL).
        .filter(|ip| match ip {
            IpAddr::V6(v6) => !is_link_local_v6(v6),
            IpAddr::V4(v4) => !v4.is_link_local(),
        })
        .map(|ip| SocketAddr::new(ip, port))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lif(name: &str, index: u32, ip: &str, prefix_len: u8) -> LocalInterface {
        LocalInterface { name: name.into(), index, ip: ip.parse().unwrap(), prefix_len }
    }

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// PC dual-stack điển hình: Wi-Fi `en0` 192.168.1.10/24 + ULA fd12:3456:789a::10/64 + link-local.
    fn dual_stack() -> Vec<LocalInterface> {
        vec![
            lif("lo0", 1, "127.0.0.1", 8),
            lif("en0", 4, "192.168.1.10", 24),
            lif("en0", 4, "fd12:3456:789a::10", 64),
            lif("en0", 4, "fe80::1c2d:3e4f:5a6b:7c8d", 64),
        ]
    }

    /// C3: IPv6 + IPv4 cùng subnet ⇒ IPv4 trước.
    #[test]
    fn same_subnet_v4_beats_v6() {
        let got = order_addresses(&[sa("[fd12:3456:789a::23]:8080"), sa("192.168.1.23:8080")], &dual_stack());
        assert_eq!(got, vec![sa("192.168.1.23:8080"), sa("[fd12:3456:789a::23]:8080")]);
    }

    /// C-W3a: bảng ca thứ tự.
    #[test]
    fn order_table() {
        let ll_scoped = SocketAddr::V6(SocketAddrV6::new("fe80::23".parse().unwrap(), 8080, 0, 4));
        let ll_bare = sa("[fe80::24]:8080");
        type Case = (&'static str, Vec<LocalInterface>, Vec<SocketAddr>, Vec<SocketAddr>);
        let cases: Vec<Case> = vec![
            (
                "full dual stack: v4 same subnet, v6 /64, link-local, others",
                dual_stack(),
                vec![
                    ll_bare,
                    sa("10.0.0.5:8080"),
                    ll_scoped,
                    sa("[2001:db8::1]:8080"),
                    sa("[fd12:3456:789a::23]:8080"),
                    sa("192.168.1.23:8080"),
                ],
                vec![
                    sa("192.168.1.23:8080"),
                    sa("[fd12:3456:789a::23]:8080"),
                    ll_scoped,
                    ll_bare,
                    sa("10.0.0.5:8080"),
                    sa("[2001:db8::1]:8080"),
                ],
            ),
            (
                "v4-only network: v6 addresses fall to the end, v4 still first",
                vec![lif("en0", 4, "192.168.1.10", 24)],
                vec![sa("[fd12:3456:789a::23]:8080"), sa("192.168.1.23:8080")],
                vec![sa("192.168.1.23:8080"), sa("[fd12:3456:789a::23]:8080")],
            ),
            (
                "v6-only network (no DHCPv4): ULA first, then link-local, v4 last",
                vec![lif("en0", 4, "fd12:3456:789a::10", 64), lif("en0", 4, "fe80::1", 64)],
                vec![sa("192.168.1.23:8080"), ll_scoped, sa("[fd12:3456:789a::23]:8080")],
                vec![sa("[fd12:3456:789a::23]:8080"), ll_scoped, sa("192.168.1.23:8080")],
            ),
            (
                "only link-local: scoped before unscoped",
                vec![lif("en0", 4, "fe80::1", 64)],
                vec![ll_bare, ll_scoped],
                vec![ll_scoped, ll_bare],
            ),
            (
                "v4 in another subnet ranks as other, after same-/64 v6",
                dual_stack(),
                vec![sa("192.168.7.23:8080"), sa("[fd12:3456:789a::23]:8080")],
                vec![sa("[fd12:3456:789a::23]:8080"), sa("192.168.7.23:8080")],
            ),
            (
                "PC link-local alone does not make a global v6 'same prefix'",
                vec![lif("en0", 4, "fe80::1", 64)],
                vec![sa("[fe80::1:2]:8080"), sa("[2001:db8::1]:8080")],
                vec![sa("[fe80::1:2]:8080"), sa("[2001:db8::1]:8080")],
            ),
            (
                "no interfaces known: input order kept, duplicates dropped",
                vec![],
                vec![sa("192.168.1.23:8080"), sa("[fd12::1]:8080"), sa("192.168.1.23:8080")],
                vec![sa("192.168.1.23:8080"), sa("[fd12::1]:8080")],
            ),
        ];
        for (name, locals, input, want) in cases {
            assert_eq!(order_addresses(&input, &locals), want, "{name}");
        }
    }

    #[test]
    fn prefix_math() {
        assert!(same_prefix_v4("10.1.2.3".parse().unwrap(), "10.1.9.9".parse().unwrap(), 16));
        assert!(!same_prefix_v4("10.1.2.3".parse().unwrap(), "10.2.2.3".parse().unwrap(), 16));
        assert!(same_prefix_v4("10.1.2.3".parse().unwrap(), "99.1.2.3".parse().unwrap(), 0));
        assert!(same_prefix_v4("10.1.2.3".parse().unwrap(), "10.1.2.3".parse().unwrap(), 32));
        assert!(same_prefix_v6("fd12::1".parse().unwrap(), "fd12::ffff:1".parse().unwrap(), 64));
        assert!(!same_prefix_v6("fd12:0:0:1::1".parse().unwrap(), "fd12::1".parse().unwrap(), 64));
        assert!(is_link_local_v6(&"fe80::1".parse().unwrap()));
        assert!(is_link_local_v6(&"febf::1".parse().unwrap()));
        assert!(!is_link_local_v6(&"fec0::1".parse().unwrap()));
    }

    #[test]
    fn parse_manual_forms() {
        let locals = dual_stack();
        let ok = |s: &str| parse_address(s, &locals).unwrap();
        assert_eq!(ok("192.168.1.23:8080"), sa("192.168.1.23:8080"));
        assert_eq!(ok(" 192.168.1.23 "), sa("192.168.1.23:8080"));
        assert_eq!(ok("127.0.0.1:18080"), sa("127.0.0.1:18080"));
        assert_eq!(ok("[fd12::5]:9000"), sa("[fd12::5]:9000"));
        assert_eq!(ok("[fd12::5]"), sa("[fd12::5]:8080"));
        assert_eq!(ok("fd12::5"), sa("[fd12::5]:8080"));
        assert_eq!(ok("http://[fd12::5]:8080/"), sa("[fd12::5]:8080"));
        assert_eq!(ok("http://192.168.1.23:8080"), sa("192.168.1.23:8080"));
        let ll = ok("[fe80::5%en0]:8080");
        assert_eq!(ll, SocketAddr::V6(SocketAddrV6::new("fe80::5".parse().unwrap(), 8080, 0, 4)));
        let win = ok("[fe80::5%12]:8080");
        assert_eq!(win, SocketAddr::V6(SocketAddrV6::new("fe80::5".parse().unwrap(), 8080, 0, 12)));
        assert_eq!(ok("fe80::5%en0"), ll);

        for bad in
            ["", "phone", "1.2.3.4:", "1.2.3.4:99999", "[fd12::5", "[fd12::5]8080", "[fe80::5%]:8080", "300.1.1.1"]
        {
            assert_eq!(parse_address(bad, &locals), Err(AddressError::Invalid), "{bad:?}");
        }
        assert_eq!(parse_address("[fe80::5%wlan9]:8080", &locals), Err(AddressError::UnknownInterface("wlan9".into())));
    }

    #[test]
    fn format_round_trips_with_interface_name() {
        let locals = dual_stack();
        let ll = SocketAddr::V6(SocketAddrV6::new("fe80::5".parse().unwrap(), 8080, 0, 4));
        for (addr, text) in [
            (ll, "[fe80::5%en0]:8080"),
            (SocketAddr::V6(SocketAddrV6::new("fe80::5".parse().unwrap(), 8080, 0, 9)), "[fe80::5%9]:8080"),
            (sa("[fd12::5]:8080"), "[fd12::5]:8080"),
            (sa("192.168.1.23:8080"), "192.168.1.23:8080"),
        ] {
            assert_eq!(format_address(&addr, &locals), text);
            assert_eq!(parse_address(text, &locals).unwrap(), addr);
        }
    }

    #[test]
    fn info_addresses_drop_link_local_and_junk() {
        let got = from_info(
            &["192.168.1.23".into(), "fd12::23".into(), "fe80::1".into(), "169.254.172.30".into(), "nope".into()],
            8081,
        );
        assert_eq!(got, vec![sa("192.168.1.23:8081"), sa("[fd12::23]:8081")]);
    }
}
