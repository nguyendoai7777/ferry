//! Thử `PhoneClient` với app điện thoại thật (PROTOCOL mục 7) — chạy tay, không chạy trong `cargo test` thường:
//!
//! ```bash
//! adb forward tcp:18080 tcp:8080
//! MWS_TEST_PHONE=127.0.0.1:18080 MWS_TEST_PIN=<PIN trên màn Sync WiFi> \
//!   cargo test -p mws-core --test real_phone -- --ignored --nocapture
//! ```
//!
//! Ghép (rồi huỷ ghép ở cuối), gửi một tệp nhỏ vào thư mục `mws-gd1_02/Thử nghiệm` (để lại trên điện thoại), tải lại
//! bằng `Range`, tạo rồi huỷ một upload dở.

use std::time::Duration;

use mws_core::client::{ClientError, PatchOutcome, PhoneClient};
use mws_core::protocol::{Secret, feature};

#[tokio::test]
#[ignore = "cần điện thoại / emulator thật: MWS_TEST_PHONE, MWS_TEST_PIN"]
async fn real_phone_round_trip() {
    let (Ok(addr), Ok(pin)) = (std::env::var("MWS_TEST_PHONE"), std::env::var("MWS_TEST_PIN")) else {
        eprintln!("bỏ qua: thiếu MWS_TEST_PHONE / MWS_TEST_PIN");
        return;
    };
    let addr = addr.parse().expect("MWS_TEST_PHONE = ip:port");
    let probe = PhoneClient::new(addr, None, None).unwrap();
    let info = probe.info().await.unwrap();
    eprintln!("info: {info:?}");
    let mut c = PhoneClient::new(addr, Some(&info.id), None).unwrap();

    assert!(matches!(
        c.pair(&Secret::new("999999"), "mws-gd1-02-test", "mws gd1_02").await,
        Err(ClientError::WrongPin)
    ));
    let paired = c.pair(&Secret::new(pin), "mws-gd1-02-test", "Máy thử gd1_02").await.unwrap();
    assert_eq!(paired.id, info.id);
    c.set_token(Some(paired.token));
    assert!(c.info().await.unwrap().paired);

    let before = c.list().await.unwrap();
    eprintln!("list: {} mục", before.len());

    let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &data).unwrap();
    let dir = info.has_feature(feature::DIRS).then_some("mws-gd1_02/Thử nghiệm");
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
        assert_eq!(item.dir.as_deref(), Some("mws-gd1_02/Thử nghiệm"));
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
