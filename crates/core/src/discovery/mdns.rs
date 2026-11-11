//! Browse `_vpsync._tcp` (`PROTOCOL.md` 6.2) bằng `mdns-sd`: một luồng nền của crate (mio, không runtime async), dò
//! trên mọi interface cả hai họ. [`Browser::next`] chờ tin kế (không poll), đổi thành [`MdnsEvent`].

use std::net::{SocketAddr, SocketAddrV6};

use mdns_sd::{ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent};

use crate::device::{Announcement, DeviceId};
use crate::protocol;

/// Kiểu dịch vụ (`PROTOCOL.md` 6.2), dạng `mdns-sd` cần (đuôi `.local.`).
pub const SERVICE_TYPE: &str = "_vpsync._tcp.local.";
/// `mdns-sd` soát interface đổi (WiFi bật / tắt, IP mới) theo nhịp này; mặc định 5 s là dày cho app nằm khay.
pub const IP_CHECK_INTERVAL_SECS: u32 = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdnsEvent {
    /// Thấy / làm mới instance có TXT `id` ⇒ `Devices::announce`.
    Announced(Announcement),
    /// Instance bị gỡ (goodbye / hết TTL) ⇒ `Devices::withdraw`.
    Withdrawn { fullname: String },
}

pub struct Browser {
    daemon: ServiceDaemon,
    events: mdns_sd::Receiver<ServiceEvent>,
}

impl Browser {
    pub fn start() -> Result<Self, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        if let Err(e) = daemon.set_ip_check_interval(IP_CHECK_INTERVAL_SECS) {
            log::debug!("mdns: cannot set ip check interval: {e}");
        }
        let events = daemon.browse(SERVICE_TYPE)?;
        log::info!("mdns: browsing {SERVICE_TYPE}");
        Ok(Self { daemon, events })
    }

    /// Tin kế có ích. `None` ⇒ luồng mDNS đã dừng.
    pub async fn next(&self) -> Option<MdnsEvent> {
        loop {
            let ev = self.events.recv_async().await.ok()?;
            match ev {
                ServiceEvent::ServiceResolved(info) => match announcement(&info) {
                    Some(a) => {
                        log::debug!("mdns: {} id={} {:?}", a.fullname, a.id, a.addresses);
                        return Some(MdnsEvent::Announced(a));
                    }
                    None => log::debug!("mdns: ignore {} (no id)", info.fullname),
                },
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    log::debug!("mdns: removed {fullname}");
                    return Some(MdnsEvent::Withdrawn { fullname });
                }
                ServiceEvent::SearchStopped(_) => return None,
                _ => {}
            }
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

/// Đọc một instance đã resolve: TXT `id` bắt buộc (khoá nhận dạng), `api` thiếu / hỏng ⇒ 1. Địa chỉ = A + AAAA + cổng
/// SRV; link-local mang `scope_id` của interface PC đã nhận bản ghi (`PROTOCOL.md` 6.2 bước 3).
pub fn announcement(info: &ResolvedService) -> Option<Announcement> {
    let id = info.get_property_val_str("id").map(str::trim).filter(|s| !s.is_empty())?;
    let api = info.get_property_val_str("api").and_then(|v| v.trim().parse().ok()).unwrap_or(protocol::API_VERSION);
    let port = info.get_port();
    let mut addresses: Vec<SocketAddr> = info.get_addresses().iter().map(|ip| scoped(ip, port)).collect();
    addresses.sort();
    let name = instance_name(&info.fullname);
    Some(Announcement { fullname: info.fullname.clone(), id: DeviceId(id.to_owned()), name, api, addresses })
}

fn scoped(ip: &ScopedIp, port: u16) -> SocketAddr {
    match ip {
        ScopedIp::V4(v4) => SocketAddr::new((*v4.addr()).into(), port),
        ScopedIp::V6(v6) => {
            let scope = if super::address::is_link_local_v6(v6.addr()) { v6.scope_id().index } else { 0 };
            SocketAddr::V6(SocketAddrV6::new(*v6.addr(), port, 0, scope))
        }
        // `ScopedIp` là `#[non_exhaustive]`.
        other => SocketAddr::new(other.to_ip_addr(), port),
    }
}

/// `Pixel 8._vpsync._tcp.local.` ⇒ `Pixel 8` (bỏ escape `\.` của DNS-SD).
pub fn instance_name(fullname: &str) -> String {
    let base = fullname.strip_suffix(SERVICE_TYPE).unwrap_or(fullname);
    let base = base.strip_suffix('.').unwrap_or(base);
    let mut out = String::with_capacity(base.len());
    let mut chars = base.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_names() {
        assert_eq!(instance_name("Pixel 8._vpsync._tcp.local."), "Pixel 8");
        assert_eq!(instance_name("Pixel 8 (2)._vpsync._tcp.local."), "Pixel 8 (2)");
        assert_eq!(instance_name(r"Mr\. Phone._vpsync._tcp.local."), "Mr. Phone");
    }
}
