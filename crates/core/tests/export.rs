//! Cắt / Sao chép sang máy tính (`ferry_core::export`, PROTOCOL §6.14, W-10) với điện thoại giả: route `GET /fetch`,
//! tải cây vào thư mục phiên (nối tiếp khi đứt), xoá trên điện thoại chỉ sau khi mục tạm bị dời đi.

mod fake_phone;

use std::path::Path;

use fake_phone::FakePhone;
use ferry_core::client::{ClientError, PhoneClient};
use ferry_core::device::DeviceId;
use ferry_core::export::{self, ExportEntry, ExportError, Meter};
use ferry_core::protocol::Secret;

fn pattern(len: usize, seed: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + seed * 7) % 251) as u8).collect()
}

async fn rig(features: &[&str]) -> (FakePhone, PhoneClient) {
    let phone = FakePhone::start().await;
    phone.set_features(features);
    let token = phone.issue_token("0e5e6d0c-test-export");
    let client =
        PhoneClient::new(phone.v4(), Some(&DeviceId(fake_phone::ID.into())), Some(Secret::new(token))).unwrap();
    (phone, client)
}

const ALL: [&str; 5] = ["browse", "edit", "copy", "fetch", "dirs"];

fn entry(path: &str, folder: bool) -> ExportEntry {
    ExportEntry { path: path.into(), folder }
}

async fn run(client: &PhoneClient, entries: &[ExportEntry], root: &Path) -> export::ExportPlan {
    let plan = export::plan(client, entries, root).await.unwrap();
    let meter = Meter::default();
    export::download(client, &plan, &meter).await.unwrap();
    assert_eq!(meter.bytes(), plan.total_bytes());
    assert_eq!(meter.files(), plan.files.len() as u64);
    plan
}

#[tokio::test]
async fn fetch_route_needs_the_feature_and_supports_range() {
    let (phone, client) = rig(&["browse"]).await;
    phone.add_file("a.bin", Some("Download"), pattern(1000, 1));
    assert!(matches!(client.fetch_range("Download/a.bin", 0, None).await, Err(ClientError::WrongPin)));

    phone.set_features(&ALL);
    let resp = client.fetch_range("Download/a.bin", 400, None).await.unwrap();
    assert!(resp.partial);
    assert_eq!((resp.start, resp.total), (400, Some(1000)));
    let etag = resp.etag.clone().unwrap();
    let mut buf = Vec::new();
    resp.write_to(&mut buf).await.unwrap();
    assert_eq!(buf, pattern(1000, 1)[400..]);
    // `If-Range` đúng ⇒ `206`; tệp đổi ⇒ `200` cả tệp.
    assert!(client.fetch_range("Download/a.bin", 10, Some(&etag)).await.unwrap().partial);
    let id = phone.files()[0].id.clone();
    phone.replace_file(&id, pattern(500, 2));
    assert!(!client.fetch_range("Download/a.bin", 10, Some(&etag)).await.unwrap().partial);
    assert!(matches!(client.fetch_range("Download", 0, None).await, Err(ClientError::NotFound)));
    assert!(matches!(client.fetch_range("Download/../x", 0, None).await, Err(ClientError::BadRequest)));
}

#[tokio::test]
async fn copy_keeps_the_tree_and_bytes() {
    let (phone, client) = rig(&ALL).await;
    phone.add_file("clip.mp4", Some("Download/Phim"), pattern(300_000, 1));
    phone.add_file("note.txt", Some("Download/Phim/Sub"), pattern(1200, 2));
    phone.add_file(".nomedia", Some("Download/Phim"), Vec::new());
    phone.add_file("lone.txt", Some("Download"), pattern(10, 3));
    let tmp = tempfile::tempdir().unwrap();
    let root = export::new_session_dir(tmp.path());
    let plan = run(&client, &[entry("Download/Phim", true), entry("Download/lone.txt", false)], &root).await;

    assert_eq!(plan.top_paths(), vec![root.join("Phim"), root.join("lone.txt")]);
    assert_eq!(std::fs::read(root.join("Phim/clip.mp4")).unwrap(), pattern(300_000, 1));
    assert_eq!(std::fs::read(root.join("Phim/Sub/note.txt")).unwrap(), pattern(1200, 2));
    assert_eq!(std::fs::read(root.join("lone.txt")).unwrap(), pattern(10, 3));
    // Mục ẩn liệt kê (W-11) ⇒ chép như tệp thường; không còn tệp `.ferry-part`.
    assert!(root.join("Phim/.nomedia").exists());
    let names: Vec<String> = walk(&root);
    assert!(names.iter().all(|n| !n.ends_with(".ferry-part")), "{names:?}");

    export::discard(&root);
    assert!(!root.exists());
}

#[tokio::test]
async fn cut_connection_resumes_with_range() {
    let (phone, client) = rig(&ALL).await;
    phone.add_file("big.bin", Some("Movies"), pattern(2_000_000, 4));
    phone.cut_next_get_after(700_000);
    let tmp = tempfile::tempdir().unwrap();
    let root = export::new_session_dir(tmp.path());
    run(&client, &[entry("Movies/big.bin", false)], &root).await;
    assert_eq!(std::fs::read(root.join("big.bin")).unwrap(), pattern(2_000_000, 4));
    let gets = phone.file_gets();
    assert_eq!(gets.len(), 2, "{gets:?}");
    assert_eq!(gets[1].range.as_deref(), Some("bytes=700000-"));
    assert!(gets[1].if_range.is_some());
}

#[tokio::test]
async fn missing_entry_fails_the_plan() {
    let (_phone, client) = rig(&ALL).await;
    let tmp = tempfile::tempdir().unwrap();
    let r = export::plan(&client, &[entry("Download/none.mp4", false)], tmp.path()).await;
    assert!(matches!(r, Err(ExportError::Missing(_))), "{r:?}");
}

#[tokio::test]
async fn cut_deletes_only_after_the_item_moved_away() {
    let (phone, client) = rig(&ALL).await;
    phone.add_file("a.mp4", Some("Download"), pattern(1000, 5));
    phone.add_file("x.mp4", Some("Phim"), pattern(2000, 6));
    phone.add_file("y.mp4", Some("Phim/Con"), pattern(3000, 7));
    let tmp = tempfile::tempdir().unwrap();
    let root = export::new_session_dir(tmp.path());
    let plan = run(&client, &[entry("Download/a.mp4", false), entry("Phim", true)], &root).await;
    // Tệp mới trên điện thoại sau khi tải ⇒ không thuộc kế hoạch, thư mục chứa nó phải được giữ.
    phone.add_file("z.mp4", Some("Phim/Con"), pattern(500, 9));

    // Chưa dán: không mục nào biến ⇒ không xoá.
    assert_eq!(export::moved_tops(&plan), Some(vec![]));

    // Trình quản lý tệp dời `a.mp4` đi.
    let dest = tmp.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::rename(root.join("a.mp4"), dest.join("a.mp4")).unwrap();
    assert_eq!(export::moved_tops(&plan), Some(vec![0]));
    let r = export::remove_moved(&client, &plan, 0).await.unwrap();
    assert_eq!((r.deleted, r.kept), (1, 0));
    assert!(phone.files().iter().all(|f| f.name != "a.mp4"));

    // Thư mục: tệp đã sửa trên điện thoại sau khi tải ⇒ giữ; thư mục còn tệp mới ⇒ giữ.
    let x = phone.files().into_iter().find(|f| f.name == "x.mp4").unwrap();
    phone.replace_file(&x.id, pattern(10, 8));
    std::fs::rename(root.join("Phim"), dest.join("Phim")).unwrap();
    assert_eq!(export::moved_tops(&plan), Some(vec![0, 1]));
    let r = export::remove_moved(&client, &plan, 1).await.unwrap();
    let left: Vec<String> = phone.files().iter().map(|f| f.path_key()).collect();
    assert!(left.contains(&"Phim/x.mp4".to_owned()) && left.contains(&"Phim/Con/z.mp4".to_owned()), "{left:?}");
    assert!(!left.contains(&"Phim/Con/y.mp4".to_owned()), "{left:?}");
    assert_eq!((r.deleted, r.kept), (1, 3));

    // Gọi lại an toàn.
    let again = export::remove_moved(&client, &plan, 0).await.unwrap();
    assert_eq!((again.deleted, again.kept), (0, 0));
}

#[tokio::test]
async fn empty_folder_is_removed_after_move() {
    let (phone, client) = rig(&ALL).await;
    phone.add_file("only.txt", Some("Trống/Sub"), pattern(5, 9));
    let tmp = tempfile::tempdir().unwrap();
    let root = export::new_session_dir(tmp.path());
    let plan = run(&client, &[entry("Trống", true)], &root).await;
    assert!(root.join("Trống/Sub/only.txt").is_file());
    std::fs::remove_dir_all(root.join("Trống")).unwrap();
    let r = export::remove_moved(&client, &plan, 0).await.unwrap();
    assert_eq!((r.deleted, r.kept), (3, 0));
    assert!(phone.files().is_empty());
    assert!(matches!(client.dirs("Trống").await, Err(ClientError::NotFound)));
}

#[tokio::test]
async fn stale_sessions_are_cleaned() {
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join("old");
    std::fs::create_dir_all(&old).unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(48 * 3600);
    std::fs::File::open(&old).unwrap().set_modified(past).unwrap();
    let fresh = tmp.path().join("fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    export::clean_stale(tmp.path(), export::KEEP_FOR);
    assert!(fresh.exists());
    assert!(!old.exists());
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        }
        out.push(p.to_string_lossy().into_owned());
    }
    out
}
