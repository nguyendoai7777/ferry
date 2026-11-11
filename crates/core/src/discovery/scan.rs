//! Quét LAN dự phòng (`PROTOCOL.md` 6.2): mạng không chuyển multicast giữa các máy WiFi (router / AP isolation — gặp thật
//! 11/11/2026: Mac không nhận gói mDNS nào của máy khác) hoặc điện thoại không quảng bá ⇒ mDNS im. PC gọi `GET /info` tới
//! mọi IPv4 trong subnet của interface mình, cổng mặc định.
//!
//! - [`targets`]: hàm thuần — mỗi interface IPv4 tối đa một /24 quanh IP của PC, bỏ loopback / link-local / chính PC.
//! - [`scan`]: nối TCP trước (hết giờ [`CONNECT_TIMEOUT`]), cổng mở mới gửi HTTP `info`; tối đa [`CONCURRENCY`] cùng lúc;
//!   chỉ nhận JSON đúng §6.3 có `app` = [`protocol::APP`].
//!
//! Chỉ chạy theo lệnh của màn Thêm điện thoại (daemon `PhoneOp::Scan`), không quét nền.

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
/// Số host thử cùng lúc (254 host ⇒ ~4 lượt ⇒ quét một /24 ≲ 3 s).
pub const CONCURRENCY: usize = 64;
/// Prefix ngắn nhất quét trọn; ngắn hơn (`/16` của mạng công ty, Docker…) ⇒ chỉ /24 quanh IP của PC.
pub const WIDEST_PREFIX: u8 = 24;
/// Trần tổng số host một lần quét (nhiều interface).
pub const MAX_TARGETS: usize = 1024;

/// Host cần thử: IPv4 của các interface (bỏ loopback, link-local `169.254/16`, prefix `/31`, `/32`), mỗi cái trong subnet
/// của nó nhưng không rộng hơn [`WIDEST_PREFIX`]; bỏ địa chỉ mạng, broadcast, IP của chính PC; không trùng; tối đa
/// [`MAX_TARGETS`]. Interface đầu danh sách được quét trước.
pub fn targets(locals: &[LocalInterface], port: u16) -> Vec<SocketAddr> {
    let own: Vec<Ipv4Addr> = locals
        .iter()
        .filter_map(|l| match l.ip {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        })
        .collect();
    let mut out: Vec<SocketAddr> = Vec::new();
    for l in locals {
        let IpAddr::V4(ip) = l.ip else { continue };
        if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || l.prefix_len > 30 {
            continue;
        }
        let len = l.prefix_len.max(WIDEST_PREFIX);
        let mask = u32::MAX << (32 - u32::from(len));
        let net = u32::from(ip) & mask;
        let broadcast = net | !mask;
        for host in (net + 1)..broadcast {
            let addr = Ipv4Addr::from(host);
            if own.contains(&addr) {
                continue;
            }
            let sa = SocketAddr::new(addr.into(), port);
            if !out.contains(&sa) {
                out.push(sa);
                if out.len() >= MAX_TARGETS {
                    return out;
                }
            }
        }
    }
    out
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
        let t = targets(&[v4([192, 168, 4, 110], 24)], 8080);
        assert_eq!(t.len(), 253);
        assert_eq!(t[0], "192.168.4.1:8080".parse().unwrap());
        assert_eq!(*t.last().unwrap(), "192.168.4.254:8080".parse().unwrap());
        assert!(!t.contains(&"192.168.4.110:8080".parse().unwrap()));
    }

    #[test]
    fn wide_prefix_is_capped_to_own_slash_24() {
        let t = targets(&[v4([10, 1, 7, 20], 16)], 8080);
        assert_eq!(t.len(), 253);
        assert!(t.iter().all(|a| matches!(a.ip(), IpAddr::V4(ip) if ip.octets()[..3] == [10, 1, 7])));
    }

    #[test]
    fn narrow_prefix_scans_only_its_subnet() {
        let t = targets(&[v4([192, 168, 1, 6], 30)], 8080);
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
        assert!(targets(&locals, 8080).is_empty());
    }

    #[test]
    fn two_interfaces_same_subnet_once_and_capped() {
        let t = targets(&[v4([192, 168, 4, 110], 24), v4([192, 168, 4, 111], 24)], 8080);
        assert_eq!(t.len(), 252);
        let many: Vec<_> = (0..10).map(|i| v4([10, i, 0, 1], 24)).collect();
        assert_eq!(targets(&many, 8080).len(), MAX_TARGETS);
    }
}
