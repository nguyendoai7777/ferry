//! `client::PhoneClient` với điện thoại giả `fake_phone` (PROTOCOL 6) — nghiệm thu gd1_02 C1–C6, C-W3a, C-W3b.
//! Mỗi ca chạy qua cả `127.0.0.1` và `[::1]` (bỏ IPv6 khi máy không có loopback IPv6).

mod fake_phone;

use std::io::SeekFrom;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use fake_phone::{FakePhone, Family};
use ferry_core::client::{ClientError, PatchOutcome, PhoneClient};
use ferry_core::device::DeviceId;
use ferry_core::protocol::{Secret, feature};
use tokio::io::AsyncSeekExt;

const CLIENT_ID: &str = "0e5e6d0c-test-pc";

/// Điện thoại giả mới + địa chỉ theo họ; `None` khi máy không có IPv6 loopback.
async fn phone(family: Family) -> Option<(FakePhone, SocketAddr)> {
    let p = FakePhone::start().await;
    let addr = p.addr(family)?;
    Some((p, addr))
}

fn client(addr: SocketAddr, token: Option<String>) -> PhoneClient {
    PhoneClient::new(addr, Some(&DeviceId(fake_phone::ID.into())), token.map(Secret::new)).unwrap()
}

async fn paired(family: Family) -> Option<(FakePhone, PhoneClient)> {
    let (p, addr) = phone(family).await?;
    let token = p.issue_token(CLIENT_ID);
    Some((p, client(addr, Some(token))))
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

fn temp_file(data: &[u8]) -> tempfile::NamedTempFile {
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), data).unwrap();
    f
}

async fn reader_at(path: &Path, offset: u64) -> tokio::fs::File {
    let mut f = tokio::fs::File::open(path).await.unwrap();
    f.seek(SeekFrom::Start(offset)).await.unwrap();
    f
}

/// Gửi phần còn lại từ `offset` theo khúc `chunk`, trả kết quả khúc cuối + các offset trung gian.
async fn send_rest(
    c: &PhoneClient,
    uid: &str,
    path: &Path,
    mut offset: u64,
    total: u64,
    chunk: u64,
) -> (Vec<u64>, PatchOutcome) {
    let mut partials = Vec::new();
    loop {
        let len = chunk.min(total - offset);
        match c.upload_patch(uid, offset, reader_at(path, offset).await, len).await.unwrap() {
            PatchOutcome::Partial(o) => {
                partials.push(o);
                offset = o;
            }
            done @ PatchOutcome::Done(_) => return (partials, done),
        }
    }
}

const FAMILIES: [Family; 2] = [Family::V4, Family::V6];

// C1 — info / pair đúng PIN / sai PIN / 5 lần sai.
#[tokio::test]
async fn c1_info_and_pair() {
    for fam in FAMILIES {
        let Some((p, addr)) = phone(fam).await else { continue };
        let mut c = client(addr, None);

        let info = c.info().await.unwrap();
        assert_eq!(info.id, DeviceId(fake_phone::ID.into()));
        assert!(!info.paired);
        assert_eq!(info.port, Some(addr.port()));
        assert!(info.has_feature(feature::DIRS));

        let wrong = Secret::new("000000");
        assert!(matches!(c.pair(&wrong, CLIENT_ID, "Máy bàn").await, Err(ClientError::WrongPin)));
        let ok = c.pair(&Secret::new(fake_phone::PIN), CLIENT_ID, "Máy bàn của Đạt").await.unwrap();
        assert_eq!(ok.token.expose().len(), 64);
        c.set_token(Some(ok.token));
        assert!(c.info().await.unwrap().paired);

        // Sai 5 lần liền ⇒ 429 + Retry-After; trong lúc khoá, đúng PIN cũng 429.
        for _ in 0..4 {
            assert!(matches!(c.pair(&wrong, CLIENT_ID, "PC").await, Err(ClientError::WrongPin)));
        }
        match c.pair(&wrong, CLIENT_ID, "PC").await {
            Err(ClientError::RateLimited { retry_after }) => assert_eq!(retry_after, Some(Duration::from_secs(30))),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            c.pair(&Secret::new(fake_phone::PIN), CLIENT_ID, "PC").await,
            Err(ClientError::RateLimited { retry_after: Some(_) })
        ));

        // Mỗi request một kết nối (pool 0): 2 info + 8 pair.
        assert_eq!(p.connections(), 10);
    }
}

#[tokio::test]
async fn unpair_and_forgotten_token_give_unauthorized() {
    for fam in FAMILIES {
        let Some((p, c)) = paired(fam).await else { continue };
        assert!(c.list().await.unwrap().is_empty());
        c.unpair().await.unwrap();
        assert!(matches!(c.list().await, Err(ClientError::Unauthorized)));
        assert!(!c.info().await.unwrap().paired);

        let c = client(c.addr(), Some(p.issue_token(CLIENT_ID)));
        p.forget_tokens();
        assert!(matches!(c.upload_create("a.mp4", 10, None).await, Err(ClientError::Unauthorized)));
    }
}

// C2 — upload 3 khúc ⇒ 204, 204, 201 + mục thư viện có tên server đặt.
#[tokio::test]
async fn c2_upload_three_chunks() {
    for fam in FAMILIES {
        let Some((p, c)) = paired(fam).await else { continue };
        let data = pattern(250_000);
        let file = temp_file(&data);
        p.add_file("clip.mp4", Some(""), vec![1, 2, 3]);

        let created = c.upload_create("clip.mp4", data.len() as u64, None).await.unwrap();
        assert_eq!((created.offset, created.length, created.dir.as_str()), (0, 250_000, ""));
        let (partials, done) = send_rest(&c, &created.id, file.path(), 0, 250_000, 100_000).await;
        assert_eq!(partials, vec![100_000, 200_000]);
        let PatchOutcome::Done(item) = done else { unreachable!() };
        assert_eq!(item.name, "clip (1).mp4", "điện thoại đổi tên khi trùng");
        assert_eq!(item.size, 250_000);
        assert_eq!(item.dir.as_deref(), Some(""));
        assert_eq!(*p.files()[0].data, data);
        assert_eq!(p.sessions(), 0);
        assert_eq!(c.list().await.unwrap()[0], item, "mới nhất trước");
    }
}

// C3 — đứt giữa khúc ⇒ HEAD trả offset đã tới; PATCH sai offset ⇒ OffsetMismatch(đúng).
#[tokio::test]
async fn c3_cut_mid_chunk_then_resume() {
    for fam in FAMILIES {
        let Some((p, c)) = paired(fam).await else { continue };
        let data = pattern(300_000);
        let file = temp_file(&data);
        let up = c.upload_create("trip.mkv", 300_000, None).await.unwrap();

        p.cut_next_patch_after(123_457);
        let err = c.upload_patch(&up.id, 0, reader_at(file.path(), 0).await, 300_000).await.unwrap_err();
        assert!(matches!(err, ClientError::Network(_)), "{err:?}");

        let st = c.upload_head(&up.id).await.unwrap();
        assert_eq!((st.offset, st.length), (123_457, 300_000));

        let err = c.upload_patch(&up.id, 0, reader_at(file.path(), 0).await, 1000).await.unwrap_err();
        assert!(matches!(err, ClientError::OffsetMismatch(123_457)), "{err:?}");

        let (_, done) = send_rest(&c, &up.id, file.path(), st.offset, 300_000, 1 << 20).await;
        assert!(matches!(done, PatchOutcome::Done(_)));
        assert_eq!(*p.files()[0].data, data, "không mất / lặp byte quanh chỗ đứt");
    }
}

#[tokio::test]
async fn lost_session_storage_busy_and_type_errors() {
    for fam in FAMILIES {
        let Some((p, c)) = paired(fam).await else { continue };
        let up = c.upload_create("a.mp4", 1000, None).await.unwrap();
        p.restart();
        assert!(matches!(c.upload_head(&up.id).await, Err(ClientError::NotFound)));
        let file = temp_file(&pattern(1000));
        assert!(matches!(
            c.upload_patch(&up.id, 0, reader_at(file.path(), 0).await, 1000).await,
            Err(ClientError::NotFound)
        ));

        assert!(matches!(c.upload_create("song.mp3", 10, None).await, Err(ClientError::Unsupported)));
        assert!(matches!(c.upload_create("a.mp4", 10, Some("x/../y")).await, Err(ClientError::BadRequest)));

        p.set_free_space(fake_phone::STORAGE_MARGIN + 500);
        assert!(matches!(c.upload_create("a.mp4", 1000, None).await, Err(ClientError::InsufficientStorage)));
        p.set_free_space(u64::MAX / 2);

        p.fail_next(503, Some(2));
        match c.upload_create("a.mp4", 10, None).await {
            Err(ClientError::Busy { retry_after }) => assert_eq!(retry_after, Some(Duration::from_secs(2))),
            other => panic!("{other:?}"),
        }
        p.fail_next(423, None);
        assert!(matches!(c.upload_head("x").await, Err(ClientError::Locked)));
        p.fail_next(500, None);
        assert!(matches!(c.list().await, Err(ClientError::Server(500))));

        let up = c.upload_create("a.mp4", 1000, None).await.unwrap();
        c.upload_delete(&up.id).await.unwrap();
        assert_eq!(p.sessions(), 0);

        // Chunk vượt Upload-Length ⇒ 413.
        let up = c.upload_create("b.mp4", 10, None).await.unwrap();
        assert!(matches!(
            c.upload_patch(&up.id, 0, reader_at(file.path(), 0).await, 11).await,
            Err(ClientError::TooLarge)
        ));
    }
}

#[tokio::test]
async fn short_source_file_is_local_error() {
    let Some((_p, c)) = paired(Family::V4).await else { return };
    let file = temp_file(&pattern(500));
    let up = c.upload_create("a.mp4", 1000, None).await.unwrap();
    let err = c.upload_patch(&up.id, 0, reader_at(file.path(), 0).await, 1000).await.unwrap_err();
    assert!(matches!(&err, ClientError::Local(e) if e.kind() == std::io::ErrorKind::UnexpectedEof), "{err:?}");
}

#[tokio::test]
async fn dead_address_is_network_error() {
    let p = FakePhone::start().await;
    let addr = p.v4();
    p.stop();
    drop(p);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let err = client(addr, None).info().await.unwrap_err();
    assert!(matches!(&err, ClientError::Network(_)), "{err:?}");
}

// C4 — tải về Range + If-Range khớp ⇒ 206; ETag đổi ⇒ 200 cả tệp.
#[tokio::test]
async fn c4_download_range_and_if_range() {
    for fam in FAMILIES {
        let Some((p, c)) = paired(fam).await else { continue };
        let data = pattern(10_000);
        let id = p.add_file("Chuyến đi.mp4", Some("Chuyến đi"), data.clone());

        let meta = c.file_head(&id).await.unwrap();
        assert_eq!(meta.length, 10_000);
        assert_eq!(meta.file_name.as_deref(), Some("Chuyến đi.mp4"));
        let etag = meta.etag.unwrap();

        let d = c.file_get_range(&id, 4000, Some(&etag)).await.unwrap();
        assert!(d.partial);
        assert_eq!((d.start, d.total), (4000, Some(10_000)));
        let mut out = data[..4000].to_vec();
        assert_eq!(d.write_to(&mut out).await.unwrap(), 6000);
        assert_eq!(out, data);

        let changed = pattern(7000).into_iter().rev().collect::<Vec<_>>();
        p.replace_file(&id, changed.clone());
        let d = c.file_get_range(&id, 4000, Some(&etag)).await.unwrap();
        assert!(!d.partial, "ETag đổi ⇒ 200 cả tệp");
        assert_ne!(d.etag.as_deref(), Some(etag.as_str()));
        let mut out = Vec::new();
        d.write_to(&mut out).await.unwrap();
        assert_eq!(out, changed);

        assert!(matches!(c.file_get_range(&id, 7000, None).await, Err(ClientError::RangeNotSatisfiable)));
        assert!(matches!(c.file_head("nope").await, Err(ClientError::NotFound)));
    }
}

// C-W3b — X-File-Dir mã hoá đúng tiếng Việt + `/`; `\` của Windows thành `/`; list có `dir`; info điện thoại cũ parse.
#[tokio::test]
async fn w3b_file_dir_and_legacy_info() {
    for fam in FAMILIES {
        let Some((p, c)) = paired(fam).await else { continue };
        let data = pattern(2000);
        let file = temp_file(&data);
        let up = c.upload_create("Ngày 1 + 2.mp4", 2000, Some(r"Chuyến đi\Đà Lạt/ngày 1")).await.unwrap();
        assert_eq!(up.dir, "Chuyến đi/Đà Lạt/ngày 1");
        let (_, done) = send_rest(&c, &up.id, file.path(), 0, 2000, 2000).await;
        let PatchOutcome::Done(item) = done else { unreachable!() };
        assert_eq!(item.name, "Ngày 1 + 2.mp4");
        assert_eq!(item.dir.as_deref(), Some("Chuyến đi/Đà Lạt/ngày 1"));

        p.add_file("outside.mp4", None, vec![0; 5]);
        let list = c.list().await.unwrap();
        assert_eq!(list[0].dir, None, "null = ngoài nơi lưu");
        assert_eq!(list[1].dir.as_deref(), Some("Chuyến đi/Đà Lạt/ngày 1"));

        p.set_legacy_info(true);
        let info = c.info().await.unwrap();
        assert!(info.features.is_empty() && info.addresses.is_empty() && info.port.is_none());
    }
}

// C-W3a — `SocketAddrV6` có `scope_id` dựng được request (không panic `url`); `::1` với scope 0 tới được server.
#[tokio::test]
async fn w3a_v6_with_scope_id() {
    let Some((p, addr)) = phone(Family::V6).await else { return };
    let SocketAddr::V6(v6) = addr else { unreachable!() };
    let scoped = std::net::SocketAddrV6::new(*v6.ip(), v6.port(), 0, 0);
    let c = PhoneClient::new(scoped.into(), None, None).unwrap();
    assert_eq!(c.info().await.unwrap().name, fake_phone::NAME);

    // Link-local + scope: chỉ cần dựng client và gửi được (không có máy ⇒ lỗi mạng hoặc chờ nối, không panic `url`).
    let ll: SocketAddr = std::net::SocketAddrV6::new("fe80::1234".parse().unwrap(), p.v4().port(), 0, 1).into();
    let c = PhoneClient::new(ll, Some(&DeviceId(fake_phone::ID.into())), None).unwrap();
    match tokio::time::timeout(Duration::from_millis(500), c.info()).await {
        Err(_elapsed) => {}
        Ok(r) => assert!(matches!(r, Err(ClientError::Network(_))), "{r:?}"),
    }
}

// C6 — `curl` mẫu của PROTOCOL mục 7 chạy được với server giả. Bỏ qua khi máy không có `curl`.
#[tokio::test]
async fn c6_curl_samples_match_contract() {
    if std::process::Command::new("curl").arg("--version").output().is_err() {
        return;
    }
    let p = FakePhone::start().await;
    let base = format!("http://{}", p.v4());
    let curl = |args: Vec<String>| async move {
        let out = tokio::process::Command::new("curl").args(&args).output().await.unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let s = |v: &str| v.to_string();

    let info = curl(vec![s("-s"), format!("{base}/api/v1/info")]).await;
    assert!(info.contains("\"api\":1"), "{info}");

    let pair = curl(vec![
        s("-s"),
        s("-X"),
        s("POST"),
        s("-H"),
        s("X-Client-Id: 0e5e6d0c-test"),
        s("-H"),
        s("X-Client-Name: My%20PC"),
        s("--data"),
        s(fake_phone::PIN),
        format!("{base}/api/v1/pair"),
    ])
    .await;
    let token = serde_json::from_str::<serde_json::Value>(&pair).unwrap()["token"].as_str().unwrap().to_owned();
    let auth = format!("Authorization: Bearer {token}");

    let created = curl(vec![
        s("-s"),
        s("-i"),
        s("-X"),
        s("POST"),
        s("-H"),
        auth.clone(),
        s("-H"),
        s("X-File-Name: a.mp4"),
        s("-H"),
        s("Upload-Length: 7340032"),
        format!("{base}/api/v1/uploads"),
    ])
    .await;
    assert!(created.starts_with("HTTP/1.1 201"), "{created}");
    assert!(created.contains("Location: /api/v1/uploads/"));
    let uid = created.lines().find_map(|l| l.strip_prefix("Location: /api/v1/uploads/")).unwrap().trim().to_owned();

    let part = temp_file(&pattern(1_048_576));
    let patched = curl(vec![
        s("-s"),
        s("-i"),
        s("-X"),
        s("PATCH"),
        s("-H"),
        auth,
        s("-H"),
        s("Upload-Offset: 0"),
        s("--data-binary"),
        format!("@{}", part.path().display()),
        format!("{base}/api/v1/uploads/{uid}"),
    ])
    .await;
    assert!(patched.starts_with("HTTP/1.1 204"), "{patched}");
    assert!(patched.contains("Upload-Offset: 1048576"), "{patched}");
}

// C5 — upload 1 GB từ tệp tạm: RAM của tiến trình không tăng quá ~2 khúc (32 MB). Chạy tay:
// `cargo test -p ferry-core --test client --release -- --ignored c5 --nocapture`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "upload 1 GB, đo RSS bằng `ps` (opus-run)"]
async fn c5_one_gigabyte_upload_keeps_ram_flat() {
    const TOTAL: u64 = 1 << 30;
    const CHUNK: u64 = ferry_core::client::CHUNK_SIZE;
    let Some((p, c)) = paired(Family::V4).await else { return };
    p.set_discard_data(true);
    let file = tempfile::NamedTempFile::new().unwrap();
    file.as_file().set_len(TOTAL).unwrap(); // tệp thưa: đọc ra số 0, không tốn đĩa

    let rss = || {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().parse::<u64>().unwrap() * 1024
    };
    let before = rss();
    let peak = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(before));
    let sampler = {
        let peak = peak.clone();
        tokio::spawn(async move {
            loop {
                peak.fetch_max(rss(), std::sync::atomic::Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    };
    let started = std::time::Instant::now();
    let up = c.upload_create("big.mp4", TOTAL, None).await.unwrap();
    let (partials, done) = send_rest(&c, &up.id, file.path(), 0, TOTAL, CHUNK).await;
    let elapsed = started.elapsed();
    sampler.abort();
    assert_eq!(partials.len() as u64, TOTAL / CHUNK - 1);
    assert!(matches!(done, PatchOutcome::Done(ref i) if i.size == TOTAL));
    let peak = peak.load(std::sync::atomic::Ordering::Relaxed);
    let grew = peak.saturating_sub(before);
    eprintln!(
        "C5: RSS trước {:.1} MB, đỉnh {:.1} MB (+{:.1} MB), {:.1} s, {:.0} MB/s",
        before as f64 / 1e6,
        peak as f64 / 1e6,
        grew as f64 / 1e6,
        elapsed.as_secs_f64(),
        TOTAL as f64 / 1e6 / elapsed.as_secs_f64()
    );
    assert!(grew < 2 * CHUNK, "RSS tăng {grew} byte");
}

/// W-7 (§6.11): `dirs` liệt kê con trực tiếp, đường có dấu / khoảng trắng mã hoá đúng; thiếu `browse` ⇒ `403`.
#[tokio::test]
async fn dirs_lists_direct_children() {
    for family in [Family::V4, Family::V6] {
        let Some((p, c)) = paired(family).await else { continue };
        p.add_file("a.mp4", Some("DCIM/Camera"), vec![1; 3]);
        p.add_file("b.txt", Some("Download/Phim hè"), vec![1; 5]);
        p.add_file("root.mp4", Some(""), vec![1; 2]);
        assert!(matches!(c.dirs("").await, Err(ClientError::WrongPin)), "chưa khai browse ⇒ 403");
        p.set_features(&["dirs", "browse"]);
        let root = c.dirs("").await.unwrap();
        let names: Vec<_> = root.dirs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["DCIM", "Download"]);
        assert_eq!(root.files.len(), 1);
        let sub = c.dirs("Download/Phim hè").await.unwrap();
        assert_eq!(sub.path, "Download/Phim hè");
        assert_eq!(sub.files[0].name, "b.txt");
        assert_eq!(sub.files[0].size, 5);
        assert!(matches!(c.dirs("Không có").await, Err(ClientError::NotFound)));
        assert!(feature::BROWSE == "browse");
    }
}

/// W-8 (§6.12): thư mục mới / đổi tên / xoá; vùng cấm `403`, trùng tên `409`, thiếu `edit` ⇒ `403`.
#[tokio::test]
async fn edit_makes_renames_and_deletes() {
    let Some((p, c)) = paired(Family::V4).await else { return };
    p.add_file("a.mp4", Some("Download/Phim hè"), vec![1; 3]);
    p.add_file("b.txt", Some("Download"), vec![1; 5]);
    p.set_features(&["dirs", "browse"]);
    assert!(matches!(c.make_dir("Download/Mới").await, Err(ClientError::WrongPin)), "chưa khai edit ⇒ 403");
    p.set_features(&["dirs", "browse", "edit"]);

    assert_eq!(c.make_dir("Download/Mới").await.unwrap(), "Download/Mới");
    assert!(matches!(c.make_dir("download/mới").await, Err(ClientError::Conflict)), "trùng tên không phân biệt hoa");
    assert!(matches!(c.make_dir("Android/data/x").await, Err(ClientError::WrongPin)));
    assert!(matches!(c.make_dir("").await, Err(ClientError::WrongPin)), "gốc cấm");
    let sub = c.dirs("Download").await.unwrap();
    let names: Vec<_> = sub.dirs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["Mới", "Phim hè"]);

    assert_eq!(c.rename("Download/b.txt", "c.txt").await.unwrap(), "Download/c.txt");
    assert!(matches!(c.rename("Download/c.txt", "Mới").await, Err(ClientError::Conflict)));
    assert!(matches!(c.rename("Download/c.txt", ".ẩn").await, Err(ClientError::BadRequest)));
    assert!(matches!(c.rename("Download/không có", "x").await, Err(ClientError::NotFound)));
    assert_eq!(c.rename("Download/Phim hè", "Phim thu").await.unwrap(), "Download/Phim thu");
    assert_eq!(c.dirs("Download/Phim thu").await.unwrap().files[0].name, "a.mp4");
    assert!(matches!(c.dirs("Download/Phim hè").await, Err(ClientError::NotFound)));

    c.delete_entry("Download/c.txt").await.unwrap();
    c.delete_entry("Download/Phim thu").await.unwrap();
    assert!(matches!(c.delete_entry("Download/Phim thu").await, Err(ClientError::NotFound)));
    assert!(matches!(c.delete_entry("Android").await, Err(ClientError::WrongPin)));
    assert!(matches!(c.delete_entry("../x").await, Err(ClientError::BadRequest | ClientError::WrongPin)));
    let left = c.dirs("Download").await.unwrap();
    assert!(left.files.is_empty());
    assert_eq!(left.dirs.len(), 1);
    assert!(p.files().is_empty());
    // Xoá tệp / thư mục con không xoá thư mục cha (như bộ nhớ thật).
    assert_eq!(p.made_dirs(), ["Download", "Download/Mới"]);
}

/// W-9 (§6.13): chép / dời; trùng tên ⇒ điện thoại đặt `tên (2)`, thư mục vào cây con ⇒ `400`, thiếu `copy` ⇒ `403`.
#[tokio::test]
async fn copy_and_move_pick_free_names() {
    let Some((p, c)) = paired(Family::V4).await else { return };
    p.add_file("a.mp4", Some("Download/Phim"), vec![1; 3]);
    p.add_file("a.mp4", Some("Movies"), vec![2; 4]);
    p.set_features(&["dirs", "browse", "edit"]);
    assert!(matches!(c.copy_entry("Download/Phim/a.mp4", "Movies").await, Err(ClientError::WrongPin)));
    p.set_features(&["dirs", "browse", "edit", "copy"]);

    assert_eq!(c.copy_entry("Download/Phim/a.mp4", "Movies").await.unwrap(), "Movies/a (2).mp4");
    assert_eq!(c.copy_entry("Download/Phim/a.mp4", "Download/Phim").await.unwrap(), "Download/Phim/a (2).mp4");
    assert_eq!(c.copy_entry("Download/Phim", "Movies").await.unwrap(), "Movies/Phim");
    assert_eq!(c.dirs("Movies/Phim").await.unwrap().files.len(), 2);
    assert!(matches!(c.copy_entry("Download/Phim", "Download/Phim/x").await, Err(ClientError::NotFound)));
    assert!(matches!(c.move_entry("Movies", "Movies/Phim").await, Err(ClientError::BadRequest)));
    assert!(matches!(c.copy_entry("Download/Phim", "Android").await, Err(ClientError::WrongPin)));

    assert_eq!(c.move_entry("Movies/a.mp4", "Download").await.unwrap(), "Download/a.mp4");
    assert_eq!(
        c.move_entry("Download/a.mp4", "Download").await.unwrap(),
        "Download/a.mp4",
        "cùng thư mục: không làm gì"
    );
    assert_eq!(c.move_entry("Movies/Phim", "Download").await.unwrap(), "Download/Phim (2)");
    assert!(matches!(c.dirs("Movies/Phim").await, Err(ClientError::NotFound)));
    assert_eq!(c.dirs("Download/Phim (2)").await.unwrap().files.len(), 2);
    assert_eq!(p.files().len(), 6);
}
