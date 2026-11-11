//! Tải về nối tiếp (`transfer::download`) trong runner của thiết bị với điện thoại giả `fake_phone` (PROTOCOL 5, 6.4)
//! — nghiệm thu gd1_10 C1–C3 + nhánh lỗi `404` / `503` / `401` / `416`, tạm dừng / huỷ, mở lại app, thứ tự với upload.

mod fake_phone;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake_phone::FakePhone;
use ferry_core::client::PhoneClient;
use ferry_core::device::DeviceId;
use ferry_core::protocol::Secret;
use ferry_core::queue::state::DownloadFailReason;
use ferry_core::queue::store::NewDownload;
use ferry_core::queue::{DownloadEvent, DownloadState, ItemId, QueueStore, UploadState, now_ms, plan_batch};
use ferry_core::transfer::download::{default_dest, part_path};
use ferry_core::transfer::{
    Command, ItemAction, Link, LinkSender, RunnerConfig, RunnerHandle, SharedStore, Throttle, TransferEvent,
    UploadRunner, link_channel,
};
use tokio::sync::mpsc;

const CLIENT_ID: &str = "0e5e6d0c-test-download";
const KB: u64 = 1024;
const MB: u64 = 1024 * 1024;
const WAIT: Duration = Duration::from_secs(30);

fn device() -> DeviceId {
    DeviceId(fake_phone::ID.into())
}

fn pattern(len: u64, seed: u64) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + seed * 7) % 251) as u8).collect()
}

async fn link_at(addr: SocketAddr, token: &str) -> Link {
    let client = PhoneClient::new(addr, Some(&device()), Some(Secret::new(token))).unwrap();
    let info = client.info().await.unwrap();
    Link::new(client, &info)
}

fn fast() -> RunnerConfig {
    RunnerConfig { progress_interval: Duration::from_millis(50), ..RunnerConfig::default() }
}

struct Rig {
    phone: FakePhone,
    token: String,
    store: SharedStore,
    db: PathBuf,
    link: LinkSender,
    handle: RunnerHandle,
    events: mpsc::UnboundedReceiver<TransferEvent>,
    log: Vec<TransferEvent>,
    tmp: tempfile::TempDir,
}

impl Rig {
    async fn new(throttle: Option<u64>) -> Rig {
        let phone = FakePhone::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("queue.sqlite3");
        let store = Arc::new(Mutex::new(QueueStore::open(&db, now_ms()).unwrap()));
        let token = phone.issue_token(CLIENT_ID);
        let (link, watch) = link_channel();
        link.send_replace(Some(link_at(phone.v4(), &token).await));
        let (tx, events) = mpsc::unbounded_channel();
        let (runner, handle) = UploadRunner::new(device(), store.clone(), watch, Throttle::new(throttle), tx, fast());
        tokio::spawn(runner.run());
        Rig { phone, token, store, db, link, handle, events, log: Vec::new(), tmp }
    }

    /// Tắt runner (như tắt app) rồi mở lại store + runner mới trên cùng DB.
    async fn restart_app(&mut self) {
        self.handle.send(Command::Stop);
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.store = Arc::new(Mutex::new(QueueStore::open(&self.db, now_ms()).unwrap()));
        let (link, watch) = link_channel();
        link.send_replace(Some(link_at(self.phone.v4(), &self.token).await));
        self.link = link;
        let (tx, events) = mpsc::unbounded_channel();
        let (runner, handle) =
            UploadRunner::new(device(), self.store.clone(), watch, Throttle::unlimited(), tx, fast());
        tokio::spawn(runner.run());
        self.handle = handle;
        self.events = events;
    }

    fn downloads_dir(&self) -> PathBuf {
        self.tmp.path().join("Downloads")
    }

    fn enqueue(&self, remote_id: &str) -> ItemId {
        let f = self.phone.files().into_iter().find(|f| f.id == remote_id).unwrap();
        let d = NewDownload {
            remote_id: f.id.clone(),
            name: f.name.clone(),
            remote_dir: f.dir.clone(),
            size: f.size,
            dest: default_dest(&self.downloads_dir(), &f.name),
        };
        let id = self.store.lock().unwrap().add_download(&device(), &d, now_ms()).unwrap();
        self.handle.send(Command::Wake);
        id
    }

    fn state(&self, id: ItemId) -> DownloadState {
        self.store.lock().unwrap().download(id).unwrap().unwrap().state
    }

    fn dest(&self, id: ItemId) -> PathBuf {
        self.store.lock().unwrap().download(id).unwrap().unwrap().dest
    }

    async fn until(&mut self, mut f: impl FnMut(&TransferEvent) -> bool) -> TransferEvent {
        let events = &mut self.events;
        let log = &mut self.log;
        tokio::time::timeout(WAIT, async {
            loop {
                let ev = events.recv().await.expect("runner stopped");
                log.push(ev.clone());
                if f(&ev) {
                    return ev;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timeout; events: {:#?}", self.log))
    }

    async fn until_state(&mut self, id: ItemId, f: impl Fn(&DownloadState) -> bool) -> DownloadState {
        match self.until(|e| matches!(e, TransferEvent::DownloadChanged { id: i, state } if *i == id && f(state))).await
        {
            TransferEvent::DownloadChanged { state, .. } => state,
            _ => unreachable!(),
        }
    }

    async fn until_done(&mut self, id: ItemId) {
        self.until_state(id, |s| matches!(s, DownloadState::Done)).await;
    }
}

fn parts_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".part"))
                .collect()
        })
        .unwrap_or_default()
}

// C1. Đứt giữa chừng ⇒ tiếp từ offset (`Range` + `If-Range`), nội dung khớp từng byte.
#[tokio::test]
async fn c1_cut_mid_body_resumes_from_offset() {
    let mut rig = Rig::new(None).await;
    let data = pattern(2 * MB, 1);
    let rid = rig.phone.add_file("clip.mp4", Some(""), data.clone());
    let etag = rig.phone.etag_of(&rid).unwrap();
    rig.phone.cut_next_get_after(700 * KB);
    let id = rig.enqueue(&rid);

    rig.until_state(id, |s| matches!(s, DownloadState::Waiting { .. })).await;
    rig.until_done(id).await;

    let dest = rig.dest(id);
    assert_eq!(dest, rig.downloads_dir().join("clip.mp4"));
    assert_eq!(std::fs::read(&dest).unwrap(), data, "content matches byte for byte");
    assert!(parts_in(&rig.downloads_dir()).is_empty(), "no .part left");

    let gets = rig.phone.file_gets();
    assert_eq!(gets.len(), 2, "{gets:?}");
    assert_eq!(gets[0].range, None);
    assert_eq!(gets[1].range.as_deref(), Some(format!("bytes={}-", 700 * KB).as_str()));
    assert_eq!(gets[1].if_range.as_deref(), Some(etag.as_str()));
}

// C1b. Tắt app giữa lúc tải ⇒ mở lại, tải tiếp từ cỡ `.part` (không từ 0).
#[tokio::test]
async fn c1b_resume_after_app_restart() {
    let mut rig = Rig::new(Some(2 * MB)).await;
    let data = pattern(3 * MB, 2);
    let rid = rig.phone.add_file("dai.mp4", Some(""), data.clone());
    let id = rig.enqueue(&rid);
    rig.until(|e| matches!(e, TransferEvent::Progress { id: i, sent, .. } if *i == id && *sent > 256 * KB)).await;
    rig.restart_app().await;
    rig.until_done(id).await;

    assert_eq!(std::fs::read(rig.dest(id)).unwrap(), data);
    let gets = rig.phone.file_gets();
    assert_eq!(gets.len(), 2, "{gets:?}");
    let from: u64 =
        gets[1].range.as_deref().unwrap().trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap();
    assert!(from > 256 * KB && from < 3 * MB, "resumed at {from}");
    assert!(gets[1].if_range.is_some());
}

// C2. ETag đổi giữa hai lần ⇒ điện thoại trả `200` cho `If-Range` cũ ⇒ tải lại từ 0, ra đúng bản mới.
#[tokio::test]
async fn c2_etag_change_restarts_from_zero() {
    let mut rig = Rig::new(None).await;
    let rid = rig.phone.add_file("doi.mp4", Some(""), pattern(MB, 3));
    let old_etag = rig.phone.etag_of(&rid).unwrap();
    rig.phone.cut_next_get_after(300 * KB);
    let id = rig.enqueue(&rid);
    rig.until_state(id, |s| matches!(s, DownloadState::Waiting { .. })).await;

    let new_data = pattern(MB + 123, 4);
    rig.phone.replace_file(&rid, new_data.clone());
    assert_ne!(rig.phone.etag_of(&rid).unwrap(), old_etag);
    rig.until_done(id).await;

    assert_eq!(std::fs::read(rig.dest(id)).unwrap(), new_data, "new version, not old head + new tail");
    let gets = rig.phone.file_gets();
    assert_eq!(gets[1].if_range.as_deref(), Some(old_etag.as_str()));
}

// C3. Tên có ký tự cấm Windows ⇒ tên hợp lệ; trùng tệp có sẵn ⇒ ` (2)`.
#[tokio::test]
async fn c3_forbidden_chars_and_duplicate_name() {
    let mut rig = Rig::new(None).await;
    let data = pattern(64 * KB, 5);
    let rid = rig.phone.add_file("Chuyến đi: ngày 1?.mp4", Some("Chuyến đi"), data.clone());
    std::fs::create_dir_all(rig.downloads_dir()).unwrap();
    std::fs::write(rig.downloads_dir().join("Chuyến đi_ ngày 1_.mp4"), b"cu").unwrap();
    let id = rig.enqueue(&rid);
    rig.until_done(id).await;

    let dest = rig.dest(id);
    assert_eq!(dest, rig.downloads_dir().join("Chuyến đi_ ngày 1_ (2).mp4"));
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    assert_eq!(std::fs::read(rig.downloads_dir().join("Chuyến đi_ ngày 1_.mp4")).unwrap(), b"cu", "old file kept");
}

// `404` ⇒ Failed "không còn trên điện thoại", `.part` bị xoá.
#[tokio::test]
async fn gone_on_phone_fails_and_discards_partial() {
    let mut rig = Rig::new(None).await;
    let rid = rig.phone.add_file("mat.mp4", Some(""), pattern(MB, 6));
    rig.phone.cut_next_get_after(200 * KB);
    let id = rig.enqueue(&rid);
    rig.until_state(id, |s| matches!(s, DownloadState::Waiting { .. })).await;
    assert_eq!(parts_in(&rig.downloads_dir()).len(), 1);
    rig.phone.fail_next(404, None);
    let s = rig.until_state(id, DownloadState::is_terminal).await;
    assert_eq!(s, DownloadState::Failed { reason: DownloadFailReason::RemoteGone });
    assert!(parts_in(&rig.downloads_dir()).is_empty());
}

// `503` + `Retry-After` ⇒ Waiting rồi tự tải tiếp.
#[tokio::test]
async fn busy_waits_then_succeeds() {
    let mut rig = Rig::new(None).await;
    let data = pattern(100 * KB, 7);
    let rid = rig.phone.add_file("ban.mp4", Some(""), data.clone());
    rig.phone.fail_next(503, Some(1));
    let id = rig.enqueue(&rid);
    let s = rig.until_state(id, |s| matches!(s, DownloadState::Waiting { .. })).await;
    let DownloadState::Waiting { until, .. } = s else { unreachable!() };
    assert!(until - now_ms() >= 500, "Retry-After honoured");
    rig.until_done(id).await;
    assert_eq!(std::fs::read(rig.dest(id)).unwrap(), data);
}

// `401` ⇒ cần ghép lại, hàng dừng; `Link` mới (đã ghép) ⇒ chạy tiếp.
#[tokio::test]
async fn unauthorized_blocks_until_repaired() {
    let mut rig = Rig::new(None).await;
    let data = pattern(100 * KB, 8);
    let rid = rig.phone.add_file("khoa.mp4", Some(""), data.clone());
    rig.phone.forget_tokens();
    let id = rig.enqueue(&rid);
    rig.until(|e| matches!(e, TransferEvent::DeviceNeedsRepair { .. })).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(matches!(rig.state(id), DownloadState::Waiting { .. }));
    // `401` trả trước route tệp nên `file_gets` không ghi; sau khi ghép lại chỉ một `GET`.

    let token = rig.phone.issue_token(CLIENT_ID);
    rig.link.send_replace(Some(link_at(rig.phone.v4(), &token).await));
    rig.until_done(id).await;
    assert_eq!(std::fs::read(rig.dest(id)).unwrap(), data);
    assert_eq!(rig.phone.file_gets().len(), 1, "no retries while blocked");
}

// `416` khi `.part` đã đủ cỡ (phản hồi cuối bị mất) ⇒ coi như xong, không tải lại.
#[tokio::test]
async fn range_not_satisfiable_with_full_part_finishes() {
    let mut rig = Rig::new(None).await;
    rig.handle.send(Command::Stop);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let data = pattern(50 * KB, 9);
    let rid = rig.phone.add_file("du.mp4", Some(""), data.clone());
    let id = rig.enqueue(&rid);
    let item = rig.store.lock().unwrap().download(id).unwrap().unwrap();
    std::fs::create_dir_all(rig.downloads_dir()).unwrap();
    std::fs::write(part_path(&item.dest, id), &data).unwrap();
    {
        let mut s = rig.store.lock().unwrap();
        let etag = rig.phone.etag_of(&rid);
        s.apply_download(id, &DownloadEvent::Start, now_ms()).unwrap();
        s.apply_download(id, &DownloadEvent::Responded { etag, full: true }, now_ms()).unwrap();
        s.apply_download(id, &DownloadEvent::Progress { offset: data.len() as u64 }, now_ms()).unwrap();
    }
    rig.restart_app().await;
    rig.until_done(id).await;
    assert_eq!(std::fs::read(rig.dest(id)).unwrap(), data);
    let gets = rig.phone.file_gets();
    assert_eq!(gets.len(), 1);
    assert_eq!(gets[0].range.as_deref(), Some(format!("bytes={}-", data.len()).as_str()));
}

// Tạm dừng giữa lúc tải ⇒ giữ `.part`; chạy tiếp ⇒ `Range`; huỷ ⇒ xoá `.part`.
#[tokio::test]
async fn pause_resume_and_cancel() {
    let mut rig = Rig::new(Some(2 * MB)).await;
    let data = pattern(3 * MB, 10);
    let rid = rig.phone.add_file("dung.mp4", Some(""), data.clone());
    let id = rig.enqueue(&rid);
    rig.until(|e| matches!(e, TransferEvent::Progress { id: i, sent, .. } if *i == id && *sent > 128 * KB)).await;
    rig.handle.send(Command::Item { id, action: ItemAction::Pause });
    rig.until_state(id, |s| matches!(s, DownloadState::Paused { .. })).await;
    assert_eq!(parts_in(&rig.downloads_dir()).len(), 1);
    rig.handle.send(Command::Item { id, action: ItemAction::Resume });
    rig.until_done(id).await;
    assert_eq!(std::fs::read(rig.dest(id)).unwrap(), data);
    let gets = rig.phone.file_gets();
    assert_eq!(gets.len(), 2);
    assert!(gets[1].range.is_some());

    let rid2 = rig.phone.add_file("huy.mp4", Some(""), pattern(3 * MB, 11));
    let id2 = rig.enqueue(&rid2);
    rig.until(|e| matches!(e, TransferEvent::Progress { id: i, sent, .. } if *i == id2 && *sent > 0)).await;
    rig.handle.send(Command::Item { id: id2, action: ItemAction::Cancel });
    rig.until_state(id2, |s| matches!(s, DownloadState::Cancelled)).await;
    assert!(parts_in(&rig.downloads_dir()).is_empty());
    assert!(!rig.downloads_dir().join("huy.mp4").exists());
}

// Upload và tải về chung một hàng của điện thoại: tuần tự, theo thứ tự thả vào (PROTOCOL 2.8).
#[tokio::test]
async fn shares_device_queue_with_upload_in_order() {
    let mut rig = Rig::new(None).await;
    let rid = rig.phone.add_file("ve.mp4", Some(""), pattern(200 * KB, 12));
    let src = rig.tmp.path().join("src").join("di.mp4");
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    std::fs::write(&src, pattern(MB, 13)).unwrap();
    rig.handle.send(Command::Stop);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let plan = plan_batch(&[src], "").unwrap();
    let (_, ups) = rig.store.lock().unwrap().add_batch(&device(), &plan, now_ms()).unwrap();
    let down = rig.enqueue(&rid);
    rig.restart_app().await;
    rig.until_done(down).await;

    let up_done = rig
        .log
        .iter()
        .position(|e| matches!(e, TransferEvent::ItemChanged { id, state: UploadState::Done { .. } } if *id == ups[0]))
        .expect("upload finished first");
    let down_started =
        rig.log.iter().position(|e| matches!(e, TransferEvent::DownloadChanged { id, .. } if *id == down)).unwrap();
    assert!(up_done < down_started, "{:#?}", rig.log);
}
