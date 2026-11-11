//! Dò điện thoại trên LAN và in sự kiện — kiểm tay C4 của gd1_05 (điện thoại thật bật Sync WiFi) và đo CPU / RAM rảnh:
//!
//! ```bash
//! cargo run -p ferry-core --example discover            # Ctrl+C để thoát
//! FERRY_NO_MDNS=1 cargo run -p ferry-core --example discover   # mốc so sánh: chỉ runtime, không mDNS
//! FERRY_SCAN=1 cargo run -p ferry-core --example discover      # quét LAN dự phòng một lượt (PROTOCOL 6.2) rồi thoát
//! ```
//!
//! Vòng lặp đúng kiểu daemon sẽ chạy (gd1_07): chờ tin mDNS hoặc mốc `Devices::next_deadline`, không poll.

use std::time::{Duration, Instant};

use ferry_core::device::Devices;
use ferry_core::discovery::{self, address, mdns};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let t0 = Instant::now();
    let mut devices = Devices::from_config(&[], address::local_interfaces(), t0);
    if std::env::var_os("FERRY_SCAN").is_some() {
        let targets = discovery::scan::targets(&address::local_interfaces(), true, &[], address::DEFAULT_PORT);
        eprintln!("scanning {} hosts", targets.len());
        for (addr, info) in discovery::scan::scan(targets).await {
            println!("{addr} {} id={} features={:?}", info.name, info.id, info.features);
        }
        eprintln!("done in {:.2}s", t0.elapsed().as_secs_f64());
        return;
    }
    if std::env::var_os("FERRY_NO_MDNS").is_some() {
        eprintln!("FERRY_NO_MDNS: idle runtime only (pid {})", std::process::id());
        std::future::pending::<()>().await;
    }
    let browser = mdns::Browser::start().expect("mdns");
    eprintln!("browsing {} (pid {})", mdns::SERVICE_TYPE, std::process::id());
    loop {
        let sleep = devices.next_deadline().map(|d| d.saturating_duration_since(Instant::now()));
        let event = tokio::select! {
            e = browser.next() => match e { Some(e) => Some(e), None => break },
            () = sleep_opt(sleep) => None,
        };
        let now = Instant::now();
        let at = format!("{:>7.2}s", t0.elapsed().as_secs_f64());
        match event {
            Some(mdns::MdnsEvent::Announced(a)) => {
                for ev in devices.announce(a, now) {
                    println!("{at} {ev:?}");
                }
            }
            Some(mdns::MdnsEvent::Withdrawn { fullname }) => devices.withdraw(&fullname, now),
            None => {}
        }
        for ev in devices.tick(now) {
            println!("{at} {ev:?}");
        }
        // Thiết bị mới thấy: đua `info` để in tên + địa chỉ thắng (thật ra chỉ daemon làm cho máy đã ghép).
        let fresh: Vec<_> = devices.iter().filter(|d| d.winner().is_none()).map(|d| d.id.clone()).collect();
        for id in fresh {
            let addrs = devices.addresses(&id);
            match discovery::connect(&id, &addrs, None).await {
                Ok(c) => {
                    println!(
                        "{at} info via {}: {} api={} features={:?}",
                        c.addr, c.info.name, c.info.api, c.info.features
                    );
                    for ev in devices.info_ok(&c.info, c.addr, now) {
                        println!("{at} {ev:?}");
                    }
                }
                Err(e) => println!("{at} {id}: {e}"),
            }
        }
    }
}

async fn sleep_opt(d: Option<Duration>) {
    match d {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}
