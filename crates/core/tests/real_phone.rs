//! Thử `PhoneClient` với app điện thoại thật (PROTOCOL mục 7) — chạy tay, không chạy trong `cargo test` thường:
//!
//! ```bash
//! adb forward tcp:18080 tcp:8080
//! FERRY_TEST_PHONE=127.0.0.1:18080 FERRY_TEST_PIN=<PIN trên màn Sync WiFi> \
//!   cargo test -p ferry-core --test real_phone -- --ignored --nocapture
//! ```
//!
//! Ghép (rồi huỷ ghép ở cuối), gửi một tệp nhỏ vào thư mục `ferry-gd1_02/Thử nghiệm` (để lại trên điện thoại), tải lại
//! bằng `Range`, tạo rồi huỷ một upload dở.

use std::time::Duration;

use ferry_core::client::{ClientError, PatchOutcome, PhoneClient};
use ferry_core::protocol::{Secret, feature};

#[tokio::test]
#[ignore = "cần điện thoại / emulator thật: FERRY_TEST_PHONE, FERRY_TEST_PIN"]
async fn real_phone_round_trip() {
    let (Ok(addr), Ok(pin)) = (std::env::var("FERRY_TEST_PHONE"), std::env::var("FERRY_TEST_PIN")) else {
        eprintln!("bỏ qua: thiếu FERRY_TEST_PHONE / FERRY_TEST_PIN");
        return;
    };
    let addr = addr.parse().expect("FERRY_TEST_PHONE = ip:port");
    let probe = PhoneClient::new(addr, None, None).unwrap();
    let info = probe.info().await.unwrap();
    eprintln!("info: {info:?}");
    let mut c = PhoneClient::new(addr, Some(&info.id), None).unwrap();

    assert!(matches!(
        c.pair(&Secret::new("999999"), "ferry-gd1-02-test", "ferry gd1_02").await,
        Err(ClientError::WrongPin)
    ));
    let paired = c.pair(&Secret::new(pin), "ferry-gd1-02-test", "Máy thử gd1_02").await.unwrap();
    assert_eq!(paired.id, info.id);
    c.set_token(Some(paired.token));
    assert!(c.info().await.unwrap().paired);

    let before = c.list().await.unwrap();
    eprintln!("list: {} mục", before.len());

    let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &data).unwrap();
    let dir = info.has_feature(feature::DIRS).then_some("ferry-gd1_02/Thử nghiệm");
    let up = c.upload_create("gd1_02 thử + nghiệm.mp4", data.len() as u64, dir).await.unwrap();
    eprintln!("created: {up:?}");

    let open = |off: u64| {
        let p = file.path().to_owned();
        async move {
            use tokio::io::AsyncSeekExt;
            let mut f = tokio::fs::File::open(p).await.unwrap();
            f.seek(std::io::SeekFrom::Start(off)).await.unwrap();
            f
        }
    };
    assert_eq!(c.upload_patch(&up.id, 0, open(0).await, 100_000).await.unwrap(), PatchOutcome::Partial(100_000));
    let wrong = c.upload_patch(&up.id, 0, open(0).await, 50_000).await;
    assert!(matches!(wrong, Err(ClientError::OffsetMismatch(100_000))), "{wrong:?}");
    let st = c.upload_head(&up.id).await.unwrap();
    assert_eq!((st.offset, st.length), (100_000, 300_000));
    let done = c.upload_patch(&up.id, 100_000, open(100_000).await, 200_000).await.unwrap();
    let PatchOutcome::Done(item) = done else { panic!("{done:?}") };
    eprintln!("done: {item:?}");
    assert_eq!(item.size, 300_000);
    if dir.is_some() {
        assert_eq!(item.dir.as_deref(), Some("ferry-gd1_02/Thử nghiệm"));
    }

    let meta = c.file_head(&item.id).await.unwrap();
    eprintln!("head: {meta:?}");
    assert_eq!(meta.length, 300_000);
    let d = c.file_get_range(&item.id, 250_000, meta.etag.as_deref()).await.unwrap();
    assert!(d.partial && d.start == 250_000);
    let mut tail = Vec::new();
    d.write_to(&mut tail).await.unwrap();
    assert_eq!(tail, data[250_000..]);
    let d = c.file_get_range(&item.id, 250_000, Some("\"stale-etag\"")).await.unwrap();
    assert!(!d.partial, "If-Range lệch ⇒ 200 cả tệp");
    drop(d);

    assert!(matches!(c.upload_create("x.mp3", 10, None).await, Err(ClientError::Unsupported)));
    if dir.is_some() {
        assert!(matches!(c.upload_create("x.mp4", 10, Some("a/../b")).await, Err(ClientError::BadRequest)));
    }
    let up2 = c.upload_create("gd1_02 huỷ.mp4", 1000, None).await.unwrap();
    c.upload_delete(&up2.id).await.unwrap();
    assert!(matches!(c.upload_head(&up2.id).await, Err(ClientError::NotFound)));

    c.unpair().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!c.info().await.unwrap().paired);
    assert!(matches!(c.list().await, Err(ClientError::Unauthorized)));
}

/// gd1_06: ghép với app thật, token vào kho khoá **thật** của OS (dịch vụ `ferry.dev` ở bản debug), rồi
/// chờ người thử bấm "Quên mọi máy tính" trên điện thoại (tối đa 120 s) ⇒ PC thấy `NeedsRepair` ⇒ quên phía PC xoá token.
///
/// ```bash
/// FERRY_TEST_PHONE=127.0.0.1:18080 FERRY_TEST_PIN=<PIN> \
///   cargo test -p ferry-core --test real_phone real_phone_keyring -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "cần điện thoại / emulator thật + kho khoá OS: FERRY_TEST_PHONE, FERRY_TEST_PIN"]
async fn real_phone_keyring() {
    use std::time::Instant;

    use ferry_core::config::{Config, PairedDevice};
    use ferry_core::device::{DeviceState, Devices};
    use ferry_core::pairing::{self, OsKeyring, TokenStore, Tokens};

    let (Ok(addr), Ok(pin)) = (std::env::var("FERRY_TEST_PHONE"), std::env::var("FERRY_TEST_PIN")) else {
        eprintln!("bỏ qua: thiếu FERRY_TEST_PHONE / FERRY_TEST_PIN");
        return;
    };
    let addr: std::net::SocketAddr = addr.parse().expect("FERRY_TEST_PHONE = ip:port");
    let info = PhoneClient::new(addr, None, None).unwrap().info().await.unwrap();
    let cfg =
        Config { client_id: "ferry-gd1-06-test".into(), client_name: "Máy thử gd1_06".into(), ..Config::default() };
    let tokens = Tokens::os();

    let mut c = PhoneClient::new(addr, Some(&info.id), None).unwrap();
    let paired = pairing::pair(&mut c, Some(&info.id), &Secret::new(pin), (&cfg).into(), &tokens).await.unwrap();
    assert!(OsKeyring.get(&paired.id).unwrap().is_some(), "token in the OS keyring");
    // Đọc lại qua bộ đệm mới (như daemon khởi động lại).
    let token = Tokens::os().get(&paired.id).await.unwrap().expect("token after restart");
    let saved =
        vec![PairedDevice { id: paired.id.clone(), name: paired.name, addresses: vec![addr.to_string()], alias: None }];
    let mut devices = Devices::from_config(&saved, Vec::new(), Instant::now());
    let fresh = PhoneClient::new(addr, Some(&info.id), Some(token.clone())).unwrap();
    devices.info_ok(&fresh.info().await.unwrap(), addr, Instant::now());
    assert_eq!(devices.get(&info.id).unwrap().state, DeviceState::Online);

    eprintln!(">>> bấm \"Quên mọi máy tính\" trên điện thoại (chờ 120 s)");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let i = fresh.info().await.unwrap();
        if !i.paired {
            devices.info_ok(&i, addr, Instant::now());
            break;
        }
        assert!(Instant::now() < deadline, "phone did not forget");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert_eq!(devices.get(&info.id).unwrap().state, DeviceState::NeedsRepair);
    assert!(matches!(fresh.list().await, Err(ClientError::Unauthorized)));

    let f = pairing::forget(&info.id, Some(&fresh), &tokens).await;
    eprintln!("forget: {f:?}");
    assert!(f.keyring_error.is_none());
    assert!(OsKeyring.get(&info.id).unwrap().is_none(), "token removed from the OS keyring");
}
