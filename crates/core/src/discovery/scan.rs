//! Quét LAN dự phòng (`PROTOCOL.md` 6.2): mạng không chuyển multicast giữa các máy WiFi (router / AP isolation — gặp thật
//! 11/11/2026: Mac không nhận gói mDNS nào của máy khác), PC và điện thoại khác subnet (mDNS không qua router — WiFi công ty
//! `172.16.0.0/23` ↔ `172.16.4.0/23`) hoặc điện thoại không quảng bá ⇒ mDNS im. PC gọi `GET /info` tới subnet của mình
//! và tới `/24` của các IP điện thoại từng dùng, cổng mặc định.
//!
//! - [`targets`]: hàm thuần — subnet của interface (tới [`WIDEST_PREFIX`]) + `/24` của từng gợi ý; bỏ loopback /
//!   link-local / chính PC.
//! - [`scan`]: nối TCP trước (hết giờ [`CONNECT_TIMEOUT`]), cổng mở mới gửi HTTP `info`; tối đa [`CONCURRENCY`] cùng lúc;
//!   chỉ nhận JSON đúng §6.3 có `app` = [`protocol::APP`].
//!
//! Chạy theo lệnh của màn Thêm điện thoại (daemon `PhoneOp::ScanLan`, cả hai dải) và khi máy đã ghép Offline (chỉ `/24`
//! của địa chỉ máy đó, thưa) — không quét nền gì khác.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::address::LocalInterface;
use crate::protocol::{self, Info};

/// Hết giờ nối TCP mỗi host. LAN trả lời trong vài ms; host không tồn tại thì treo tới ARP hỏng — cắt sớm.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(600);
/// Hết giờ cả request `info` sau khi cổng đã mở.
pub const INFO_TIMEOUT: Duration = Duration::from_secs(3);
/// Số host thử cùng lúc (`/22` = 1022 host ⇒ 8 lượt ⇒ ≲ 5 s; dưới trần 256 tệp mở mặc định của macOS).
pub const CONCURRENCY: usize = 128;
/// Prefix ngắn nhất quét trọn (`/23` của WiFi công ty vẫn trọn); ngắn hơn (`/16`, Docker…) ⇒ chỉ `/22` quanh IP của PC.
pub const WIDEST_PREFIX: u8 = 22;
/// Prefix quét quanh một IP điện thoại từng dùng (gợi ý).
pub const HINT_PREFIX: u8 = 24;
/// Trần tổng số host một lần quét (nhiều interface + gợi ý).
pub const MAX_TARGETS: usize = 2048;

/// Host cần thử, theo thứ tự quét:
/// 1. `own_subnets`: IPv4 của các interface (bỏ loopback, link-local `169.254/16`, prefix `/31`, `/32`), mỗi cái trong subnet
///    của nó nhưng không rộng hơn [`WIDEST_PREFIX`];
/// 2. `/24` ([`HINT_PREFIX`]) quanh mỗi IPv4 trong `hints` (IP điện thoại từng dùng; loopback / link-local bị bỏ).
///
/// Bỏ địa chỉ mạng, broadcast, IP của chính PC; không trùng; tối đa [`MAX_TARGETS`].
pub fn targets(locals: &[LocalInterface], own_subnets: bool, hints: &[IpAddr], port: u16) -> Vec<SocketAddr> {
    let own: Vec<Ipv4Addr> = locals
        .iter()
        .filter_map(|l| match l.ip {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        })
        .collect();
    let mut ranges: Vec<(Ipv4Addr, u8)> = Vec::new();
    if own_subnets {
        for l in locals {
            let IpAddr::V4(ip) = l.ip else { continue };
            if usable(ip) && l.prefix_len <= 30 {
                ranges.push((ip, l.prefix_len.max(WIDEST_PREFIX)));
            }
        }
    }
    for h in hints {
        if let IpAddr::V4(ip) = h
            && usable(*ip)
        {
            ranges.push((*ip, HINT_PREFIX));
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<SocketAddr> = Vec::new();
    for (ip, len) in ranges {
        let mask = u32::MAX << (32 - u32::from(len));
        let net = u32::from(ip) & mask;
        let broadcast = net | !mask;
        for host in (net + 1)..broadcast {
            let addr = Ipv4Addr::from(host);
            if own.contains(&addr) || !seen.insert(host) {
                continue;
            }
            out.push(SocketAddr::new(addr.into(), port));
            if out.len() >= MAX_TARGETS {
                return out;
            }
        }
    }
    out
}

fn usable(ip: Ipv4Addr) -> bool {
    !(ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast())
}

/// Thử mọi `targets`, trả máy trả lời đúng §6.3 (`app` của điện thoại), theo thứ tự `targets`. Không lỗi: host không
/// trả lời / không phải điện thoại chỉ ghi log mức `trace`.
pub async fn scan(targets: Vec<SocketAddr>) -> Vec<(SocketAddr, Info)> {
    let limit = Arc::new(Semaphore::new(CONCURRENCY));
    let mut set = JoinSet::new();
    for (ix, addr) in targets.into_iter().enumerate() {
        let limit = limit.clone();
        set.spawn(async move {
            let _permit = limit.acquire_owned().await.ok()?;
            let info = probe_host(addr).await?;
            Some((ix, addr, info))
        });
    }
    let mut found = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(Some(hit)) = res {
            found.push(hit);
        }
    }
    found.sort_by_key(|(ix, _, _)| *ix);
    found.into_iter().map(|(_, a, i)| (a, i)).collect()
}

async fn probe_host(addr: SocketAddr) -> Option<Info> {
    // Nối thử trước: rẻ hơn dựng client HTTP cho mọi host; host chết / cổng đóng dừng ở đây.
    match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => drop(stream),
        _ => return None,
    }
    let client = crate::client::PhoneClient::new(addr, None, None).ok()?;
    let info = match tokio::time::timeout(INFO_TIMEOUT, client.info()).await {
        Ok(Ok(info)) => info,
        Ok(Err(e)) => {
            log::trace!("scan: {addr} open but not a phone: {e}");
            return None;
        }
        Err(_) => return None,
    };
    if info.app != protocol::APP || info.id.0.is_empty() {
        log::debug!("scan: {addr} answers info as app {:?}, ignored", info.app);
        return None;
    }
    log::info!("scan: found {} ({}) at {addr}", info.name, info.id);
    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(ip: [u8; 4], prefix_len: u8) -> LocalInterface {
        LocalInterface { name: "en0".into(), index: 1, ip: IpAddr::from(ip), prefix_len }
    }

    #[test]
    fn slash_24_skips_network_broadcast_and_self() {
        let t = targets(&[v4([192, 168, 4, 110], 24)], true, &[], 8080);
        assert_eq!(t.len(), 253);
        assert_eq!(t[0], "192.168.4.1:8080".parse().unwrap());
        assert_eq!(*t.last().unwrap(), "192.168.4.254:8080".parse().unwrap());
        assert!(!t.contains(&"192.168.4.110:8080".parse().unwrap()));
    }

    #[test]
    fn slash_23_is_scanned_whole() {
        // WiFi công ty: PC 172.16.0.57/23 ⇒ cả 172.16.0.x lẫn 172.16.1.x.
        let t = targets(&[v4([172, 16, 0, 57], 23)], true, &[], 8080);
        assert_eq!(t.len(), 509);
        assert!(t.contains(&"172.16.1.200:8080".parse().unwrap()));
        assert!(!t.contains(&"172.16.2.1:8080".parse().unwrap()));
    }

    #[test]
    fn wide_prefix_is_capped_to_own_slash_22() {
        let t = targets(&[v4([10, 1, 7, 20], 16)], true, &[], 8080);
        assert_eq!(t.len(), 1021);
        assert!(t.iter().all(
            |a| matches!(a.ip(), IpAddr::V4(ip) if ip.octets()[..2] == [10, 1] && (4..8).contains(&ip.octets()[2]))
        ));
    }

    #[test]
    fn hints_add_the_slash_24_of_a_routed_phone() {
        // Điện thoại ở 172.16.4.0/23 (subnet khác, nối được qua router) ⇒ quét thêm 172.16.4.x sau subnet của PC.
        let hints = ["172.16.4.21".parse().unwrap(), "172.16.0.9".parse().unwrap(), "fd12::5".parse().unwrap()];
        let t = targets(&[v4([172, 16, 0, 57], 23)], true, &hints, 8080);
        assert_eq!(t.len(), 509 + 254);
        assert_eq!(t[509], "172.16.4.1:8080".parse().unwrap());
        // Máy Offline: chỉ dải gợi ý, không quét subnet của PC.
        let only = targets(&[v4([172, 16, 0, 57], 23)], false, &hints[..1], 8080);
        assert_eq!(only.len(), 254);
    }

    #[test]
    fn narrow_prefix_scans_only_its_subnet() {
        let t = targets(&[v4([192, 168, 1, 6], 30)], true, &[], 8080);
        assert_eq!(t, vec!["192.168.1.5:8080".parse::<SocketAddr>().unwrap()]);
    }

    #[test]
    fn skips_loopback_link_local_v6_and_point_to_point() {
        let locals = [
            v4([127, 0, 0, 1], 8),
            v4([169, 254, 3, 4], 16),
            v4([100, 64, 0, 9], 32),
            LocalInterface { name: "en0".into(), index: 1, ip: "fd12::5".parse().unwrap(), prefix_len: 64 },
        ];
        assert!(
            targets(&locals, true, &["127.0.0.1".parse().unwrap(), "169.254.1.1".parse().unwrap()], 8080).is_empty()
        );
    }

    #[test]
    fn two_interfaces_same_subnet_once_and_capped() {
        let t = targets(&[v4([192, 168, 4, 110], 24), v4([192, 168, 4, 111], 24)], true, &[], 8080);
        assert_eq!(t.len(), 252);
        let many: Vec<_> = (0..10).map(|i| v4([10, i * 4, 0, 1], 22)).collect();
        assert_eq!(targets(&many, true, &[], 8080).len(), MAX_TARGETS);
    }
}
