//! Dò + nối (gd1_05): mDNS thật trong tiến trình (C2), happy eyeballs với socket thật (C-W3b), nhập tay IPv6.

mod fake_phone;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use fake_phone::FakePhone;
use ferry_core::device::{DeviceEvent, DeviceId, DeviceState, Devices, LOST_AFTER};
use ferry_core::discovery::mdns::{Browser, MdnsEvent, SERVICE_TYPE};
use ferry_core::discovery::{self, ConnectError, address};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use tokio::net::TcpListener;
use tokio::time::timeout;

/// C2: quảng bá `_vpsync._tcp` bằng chính `mdns-sd` ⇒ `Seen` đúng `id`; dừng quảng bá ⇒ `Lost` sau 30 s (đồng hồ giả
/// của `Devices`). Máy không có interface multicast (CI cách ly mạng) ⇒ bỏ qua, in lý do.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mdns_announce_then_goodbye() {
    let has_lan = address::local_interfaces().iter().any(|l| !l.ip.is_loopback());
    if !has_lan {
        eprintln!("bỏ qua: không có interface mạng ngoài loopback");
        return;
    }
    let id = format!("{:016x}", rand_u64());
    let instance = format!("ferry test {}", &id[..6]);
    let browser = Browser::start().expect("browser");

    let publisher = ServiceDaemon::new().expect("publisher");
    let props = HashMap::from([("api".to_string(), "1".to_string()), ("id".to_string(), id.clone())]);
    let service =
        ServiceInfo::new(SERVICE_TYPE, &instance, &format!("ferry-test-{}.local.", &id[..6]), (), 18080, props)
            .expect("service info")
            .enable_addr_auto();
    let fullname = service.get_fullname().to_owned();
    publisher.register(service).expect("register");

    let t0 = Instant::now();
    let mut devices = Devices::from_config(&[], address::local_interfaces(), t0);
    let announced = timeout(Duration::from_secs(10), async {
        loop {
            match browser.next().await.expect("browser alive") {
                MdnsEvent::Announced(a) if a.id.0 == id => return a,
                _ => {}
            }
        }
    })
    .await
    .expect("announcement within 10 s");
    let seen_after = t0.elapsed();
    assert_eq!(announced.fullname, fullname);
    assert_eq!(announced.name, instance);
    assert_eq!(announced.api, 1);
    assert!(!announced.addresses.is_empty());
    assert!(announced.addresses.iter().all(|a| a.port() == 18080));
    let dev = DeviceId(id.clone());
    let ev = devices.announce(announced, t0);
    assert!(ev.contains(&DeviceEvent::Seen { id: dev.clone(), name: instance.clone() }), "{ev:?}");
    assert_eq!(devices.get(&dev).unwrap().state, DeviceState::Discovered);
    eprintln!("seen after {seen_after:?}");

    publisher.unregister(&fullname).expect("unregister");
    timeout(Duration::from_secs(10), async {
        loop {
            if let MdnsEvent::Withdrawn { fullname: f } = browser.next().await.expect("browser alive")
                && f == fullname
            {
                return;
            }
        }
    })
    .await
    .expect("goodbye within 10 s");
    let t1 = Instant::now();
    devices.withdraw(&fullname, t1);
    assert!(devices.tick(t1 + LOST_AFTER - Duration::from_secs(1)).is_empty());
    let ev = devices.tick(t1 + LOST_AFTER);
    assert_eq!(ev.first(), Some(&DeviceEvent::Lost { id: dev.clone() }));
    assert!(devices.get(&dev).is_none());
    let _ = publisher.shutdown();
}

/// C-W3b: địa chỉ đầu không nối được ⇒ địa chỉ kế (server chỉ nghe `[::1]`) thắng trong ≤ 300 ms.
#[tokio::test]
async fn dead_v4_then_v6_only_phone_wins_fast() {
    let Some(phone) = FakePhone::start_v6_only().await else {
        eprintln!("bỏ qua: máy không có IPv6 loopback");
        return;
    };
    let id = DeviceId(fake_phone::ID.into());
    let v6 = phone.v6().unwrap();

    // (a) v4 bị từ chối ngay (không ai nghe).
    let start = Instant::now();
    let c = discovery::connect(&id, &[phone.v4(), v6], None).await.expect("connect");
    assert_eq!(c.addr, v6);
    assert!(start.elapsed() <= Duration::from_millis(300), "{:?}", start.elapsed());
    assert_eq!(c.info.id, id);
    assert!(!c.info.paired);

    // (b) v4 nhận kết nối mà im lặng (như máy treo) ⇒ sau 250 ms thử v6 song song, v6 thắng.
    let silent = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let silent_addr = silent.local_addr().unwrap();
    let start = Instant::now();
    let c = discovery::connect(&id, &[silent_addr, v6], None).await.expect("connect");
    let took = start.elapsed();
    assert_eq!(c.addr, v6);
    assert!(took >= discovery::HAPPY_EYEBALLS_DELAY, "{took:?}");
    assert!(took <= Duration::from_millis(300) + Duration::from_millis(100), "{took:?}");
    drop(silent);

    // Client thắng dùng được tiếp, token đi kèm.
    let token = phone.issue_token("ferry-gd1-05");
    let c = discovery::connect(&id, &[v6], Some(&ferry_core::protocol::Secret::new(token))).await.unwrap();
    assert!(c.info.paired);
    assert!(c.client.list().await.is_ok());
}

/// IP được cấp lại cho điện thoại khác ⇒ không nhận nhầm.
#[tokio::test]
async fn other_phone_at_address_is_not_accepted() {
    let phone = FakePhone::start().await;
    let err = discovery::connect(&DeviceId("0000000000000000".into()), &[phone.v4()], None).await.unwrap_err();
    let ConnectError::Unreachable(errs) = err;
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].0, phone.v4());
}

/// Nhập tay `[v6]:port` ⇒ `info` lấy `id` ⇒ vào sổ ở `Discovered`, có cả địa chỉ của `info.addresses`.
#[tokio::test]
async fn manual_ipv6_address_probe() {
    let phone = FakePhone::start().await;
    let Some(v6) = phone.v6() else {
        eprintln!("bỏ qua: máy không có IPv6 loopback");
        return;
    };
    let typed = format!("[::1]:{}", v6.port());
    let addr: SocketAddr = address::parse_address(&typed, &[]).unwrap();
    assert_eq!(addr, v6);
    let info = discovery::probe(addr).await.expect("probe");
    assert_eq!(info.id.0, fake_phone::ID);

    let now = Instant::now();
    let mut devices = Devices::from_config(&[], Vec::new(), now);
    devices.info_ok(&info, addr, now);
    let d = devices.get(&info.id).unwrap();
    assert_eq!(d.state, DeviceState::Discovered);
    let addrs = devices.addresses(&info.id);
    assert_eq!(addrs[0], v6, "address that answered goes first");
    assert!(addrs.contains(&phone.v4()), "info.addresses + port added IPv4");

    // Địa chỉ chết ⇒ lỗi mạng (không treo).
    phone.stop();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(discovery::probe(addr).await.is_err());
}

fn rand_u64() -> u64 {
    let u = uuid::Uuid::new_v4();
    u64::from_le_bytes(u.as_bytes()[..8].try_into().unwrap())
}

/// App điện thoại cài lại: địa chỉ cũ trả lời bằng `id` khác ⇒ `connect` theo mã cũ thất bại, `find_replacement` trả
/// máy mới; địa chỉ vẫn là mã cũ ⇒ không có máy thay.
#[tokio::test]
async fn reinstalled_phone_is_found_as_replacement() {
    let phone = FakePhone::start().await;
    let addr = phone.addr(fake_phone::Family::V4).expect("v4");
    let old = DeviceId("0123456789abcdef".into());
    assert!(discovery::connect(&old, &[addr], None).await.is_err());
    let (at, info) = discovery::find_replacement(&old, &[addr]).await.expect("replacement");
    assert_eq!((at, info.id.0.as_str()), (addr, fake_phone::ID));
    assert!(discovery::find_replacement(&DeviceId(fake_phone::ID.into()), &[addr]).await.is_none());
}
