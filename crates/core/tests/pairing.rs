//! Ghép / quên điện thoại với `fake_phone` + kho khoá giả (PROTOCOL 6.3, 2.2, mục 5) — nghiệm thu gd1_06 C1, C2.

mod fake_phone;

use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use fake_phone::{FakePhone, Family};
use ferry_core::client::{ClientError, PhoneClient};
use ferry_core::config::{self, Config, PairedDevice};
use ferry_core::device::{DeviceId, DeviceState, Devices};
use ferry_core::discovery;
use ferry_core::pairing::{self, KeyringError, MemoryKeyring, PairError, TokenStore, Tokens};
use ferry_core::protocol::Secret;

/// Mọi dòng log của tiến trình test (mức trace) — soát token / PIN không lọt vào log.
static LOG: Mutex<String> = Mutex::new(String::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        let line = format!("{} {} {}\n", r.level(), r.target(), r.args());
        LOG.lock().unwrap().push_str(&line);
    }
    fn flush(&self) {}
}

fn capture_logs() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
}

fn logs() -> String {
    LOG.lock().unwrap().clone()
}

fn me() -> Config {
    Config { client_id: "0e5e6d0c-gd1-06-pc".into(), client_name: "Máy thử gd1_06".into(), ..Config::default() }
}

fn phone_id() -> DeviceId {
    DeviceId(fake_phone::ID.into())
}

fn pin() -> Secret {
    Secret::new(fake_phone::PIN)
}

/// Daemon thu nhỏ: nối `info` bằng token trong kho khoá ⇒ đưa kết quả vào sổ thiết bị.
async fn refresh(devices: &mut Devices, tokens: &Tokens) -> Option<PhoneClient> {
    let id = phone_id();
    let Some(token) = tokens.get(&id).await.unwrap() else {
        devices.unauthorized(&id);
        return None;
    };
    match discovery::connect(&id, &devices.addresses(&id), Some(&token)).await {
        Ok(c) => {
            devices.info_ok(&c.info, c.addr, Instant::now());
            Some(c.client)
        }
        Err(_) => {
            devices.connect_failed(&id, Instant::now());
            None
        }
    }
}

fn state(devices: &Devices) -> DeviceState {
    devices.get(&phone_id()).expect("device in registry").state
}

// C1 — ghép ⇒ token trong kho khoá, không trong config.json, không trong log. Sai PIN / 429 / máy khác.
#[tokio::test]
async fn c1_pair_stores_token_in_keyring_only() {
    capture_logs();
    let p = FakePhone::start().await;
    let addr = p.addr(Family::V4).unwrap();
    let mem = Arc::new(MemoryKeyring::new());
    let tokens = Tokens::new(mem.clone());
    let mut cfg = me();
    let mut c = PhoneClient::new(addr, Some(&phone_id()), None).unwrap();

    let wrong = pairing::pair(&mut c, Some(&phone_id()), &Secret::new("000000"), (&cfg).into(), &tokens).await;
    assert!(matches!(wrong, Err(PairError::WrongPin)), "{wrong:?}");
    assert!(mem.is_empty() && !c.has_token());

    // Máy khác trả lời ở địa chỉ này ⇒ không cất token.
    let other = DeviceId("someone-else".into());
    let r = pairing::pair(&mut c, Some(&other), &pin(), (&cfg).into(), &tokens).await;
    assert!(matches!(&r, Err(PairError::OtherPhone { got }) if *got == phone_id()), "{r:?}");
    assert!(mem.is_empty() && !c.has_token());

    let paired = pairing::pair(&mut c, Some(&phone_id()), &pin(), (&cfg).into(), &tokens).await.unwrap();
    assert_eq!((paired.id.clone(), paired.name.as_str()), (phone_id(), fake_phone::NAME));
    let token = mem.get(&phone_id()).unwrap().expect("token in keyring");
    assert!(c.has_token());
    assert!(c.info().await.unwrap().paired);

    cfg.remember_device(PairedDevice { id: paired.id, name: paired.name, addresses: vec![addr.to_string()], alias: None });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    config::save(&path, &cfg).unwrap();
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(file.contains(fake_phone::ID), "device is remembered");
    assert!(!file.contains(token.expose()), "token must not be in config.json");
    assert!(!format!("{c:?}{token:?}{tokens:?}").contains(token.expose()));

    let log = logs();
    assert!(log.contains("paired with"), "pairing is logged");
    assert!(!log.contains(token.expose()), "token must not be in the log");
    assert!(!log.contains(fake_phone::PIN), "PIN must not be in the log");
}

// C1 — sai PIN quá nhiều ⇒ RateLimited có thời gian chờ cho UI đếm ngược.
#[tokio::test]
async fn c1_rate_limited_has_countdown() {
    let p = FakePhone::start().await;
    let tokens = Tokens::new(Arc::new(MemoryKeyring::new()));
    let cfg = me();
    let mut c = PhoneClient::new(p.v4(), Some(&phone_id()), None).unwrap();
    let mut last = None;
    for _ in 0..6 {
        last = Some(pairing::pair(&mut c, None, &Secret::new("000000"), (&cfg).into(), &tokens).await);
    }
    match last.unwrap() {
        Err(PairError::RateLimited { retry_after }) => assert!(retry_after > Duration::ZERO),
        other => panic!("{other:?}"),
    }
}

// C1 — kho khoá hỏng ⇒ lỗi Keyring, client không mang token (không ghép nửa vời).
#[tokio::test]
async fn c1_keyring_failure_is_reported() {
    let p = FakePhone::start().await;
    let mem = Arc::new(MemoryKeyring::new());
    mem.fail_with(Some(KeyringError::Unavailable("no secret service".into())));
    let tokens = Tokens::new(mem.clone());
    let cfg = me();
    let mut c = PhoneClient::new(p.v4(), Some(&phone_id()), None).unwrap();
    let r = pairing::pair(&mut c, None, &pin(), (&cfg).into(), &tokens).await;
    assert!(matches!(r, Err(PairError::Keyring(KeyringError::Unavailable(_)))), "{r:?}");
    assert!(!c.has_token());
}

// C2 — điện thoại "Quên mọi máy tính" ⇒ NeedsRepair (cả qua `info.paired=false` lẫn `401`); ghép lại ⇒ Online.
#[tokio::test]
async fn c2_phone_forgets_then_repair() {
    capture_logs();
    let p = FakePhone::start().await;
    let addr = p.v4();
    let mem = Arc::new(MemoryKeyring::new());
    let tokens = Tokens::new(mem.clone());
    let mut cfg = me();
    let mut c = PhoneClient::new(addr, Some(&phone_id()), None).unwrap();
    let paired = pairing::pair(&mut c, Some(&phone_id()), &pin(), (&cfg).into(), &tokens).await.unwrap();
    let old = mem.get(&phone_id()).unwrap().unwrap();
    cfg.remember_device(PairedDevice { id: paired.id, name: paired.name, addresses: vec![addr.to_string()], alias: None });

    // Khởi động daemon: nạp config ⇒ Paired ⇒ `info` với token ⇒ Online.
    let mut devices = Devices::from_config(&cfg.devices, Vec::new(), Instant::now());
    assert_eq!(state(&devices), DeviceState::Paired);
    refresh(&mut devices, &tokens).await.expect("online");
    assert_eq!(state(&devices), DeviceState::Online);

    p.forget_tokens();
    // Request đang chạy nhận 401.
    assert!(matches!(c.list().await, Err(ClientError::Unauthorized)));
    devices.unauthorized(&phone_id());
    assert_eq!(state(&devices), DeviceState::NeedsRepair);
    // `info` lần sau cũng nói paired=false ⇒ vẫn NeedsRepair (không nhảy về Online).
    let mut devices2 = Devices::from_config(&cfg.devices, Vec::new(), Instant::now());
    refresh(&mut devices2, &tokens).await;
    assert_eq!(state(&devices2), DeviceState::NeedsRepair);

    // Ghép lại ⇒ token mới đè token cũ ⇒ Paired ⇒ Online.
    let mut c2 = PhoneClient::new(addr, Some(&phone_id()), None).unwrap();
    pairing::pair(&mut c2, Some(&phone_id()), &pin(), (&cfg).into(), &tokens).await.unwrap();
    let new = mem.get(&phone_id()).unwrap().unwrap();
    assert_ne!(new, old);
    assert_eq!(mem.len(), 1);
    devices2.paired(&phone_id(), Instant::now());
    assert_eq!(state(&devices2), DeviceState::Paired);
    refresh(&mut devices2, &tokens).await.expect("online again");
    assert_eq!(state(&devices2), DeviceState::Online);
    assert!(c2.list().await.is_ok());
    assert!(!logs().contains(new.expose()));
}

// Kho khoá mất token (người dùng xoá tay) ⇒ thiết bị trong config thành NeedsRepair, không gửi request không token.
#[tokio::test]
async fn missing_token_needs_repair() {
    let cfg = Config {
        devices: vec![PairedDevice { id: phone_id(), name: "P".into(), addresses: vec!["127.0.0.1:9".into()], alias: None }],
        ..me()
    };
    let tokens = Tokens::new(Arc::new(MemoryKeyring::new()));
    let mut devices = Devices::from_config(&cfg.devices, Vec::new(), Instant::now());
    assert!(refresh(&mut devices, &tokens).await.is_none());
    assert_eq!(state(&devices), DeviceState::NeedsRepair);
}

// Quên trên PC: Online ⇒ DELETE /pair tới điện thoại + xoá token; Offline ⇒ vẫn quên phía PC.
#[tokio::test]
async fn forget_online_and_offline() {
    let p = FakePhone::start().await;
    let mem = Arc::new(MemoryKeyring::new());
    let tokens = Tokens::new(mem.clone());
    let mut cfg = me();
    let mut c = PhoneClient::new(p.v4(), Some(&phone_id()), None).unwrap();
    pairing::pair(&mut c, None, &pin(), (&cfg).into(), &tokens).await.unwrap();
    let token = mem.get(&phone_id()).unwrap().unwrap();
    cfg.remember_device(PairedDevice { id: phone_id(), name: "P".into(), addresses: vec![], alias: None });

    let f = pairing::forget(&phone_id(), Some(&c), &tokens).await;
    assert!(f.phone_notified && f.keyring_error.is_none());
    assert!(mem.is_empty());
    assert_eq!(tokens.get(&phone_id()).await.unwrap(), None);
    assert!(cfg.forget_device(&phone_id()) && cfg.devices.is_empty());
    let probe = PhoneClient::new(p.v4(), Some(&phone_id()), Some(token)).unwrap();
    assert!(!probe.info().await.unwrap().paired, "phone dropped the token");

    // Offline: điện thoại tắt ⇒ lỗi mạng ⇒ vẫn quên phía PC.
    let mut c = PhoneClient::new(p.v4(), Some(&phone_id()), None).unwrap();
    pairing::pair(&mut c, None, &pin(), (&cfg).into(), &tokens).await.unwrap();
    assert_eq!(mem.len(), 1);
    p.stop();
    let f = pairing::forget(&phone_id(), Some(&c), &tokens).await;
    assert!(!f.phone_notified);
    assert!(mem.is_empty());
}
