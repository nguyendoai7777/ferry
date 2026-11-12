//! `transfer::UploadRunner` với điện thoại giả `fake_phone` (PROTOCOL 5, 6.5, 6.8, 6.9) — nghiệm thu gd1_04 C1–C7,
//! C-W3a, C-W3b + Checking theo lô, tạm dừng / huỷ, Offline.

mod fake_phone;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fake_phone::{FakePhone, Family};
use ferry_core::client::PhoneClient;
use ferry_core::device::DeviceId;
use ferry_core::protocol::Secret;
use ferry_core::queue::state::{FailReason, PauseReason};
use ferry_core::queue::{BatchId, ItemId, QueueStore, UploadState, now_ms, plan_batch};
use ferry_core::transfer::{
    Command, ItemAction, Link, LinkSender, RunnerConfig, RunnerHandle, SharedStore, TransferEvent, UploadRunner,
    link_channel,
};
use tokio::sync::mpsc;

const CLIENT_ID: &str = "0e5e6d0c-test-upload";
const KB: u64 = 1024;
const MB: u64 = 1024 * 1024;
const WAIT: Duration = Duration::from_secs(30);

fn device() -> DeviceId {
    DeviceId(fake_phone::ID.into())
}

fn pattern(len: u64, seed: u64) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + seed * 7) % 251) as u8).collect()
}

fn write(root: &Path, rel: &str, len: u64, seed: u64) -> PathBuf {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, pattern(len, seed)).unwrap();
    p
}

async fn link_at(addr: SocketAddr, token: &str) -> Link {
    let client = PhoneClient::new(addr, Some(&device()), Some(Secret::new(token))).unwrap();
    let info = client.info().await.unwrap();
    Link::new(client, &info)
}

struct Rig {
    phone: FakePhone,
    token: String,
    store: SharedStore,
    link: LinkSender,
    handle: RunnerHandle,
    events: mpsc::UnboundedReceiver<TransferEvent>,
    log: Vec<TransferEvent>,
    bytes_read: Arc<AtomicU64>,
    tmp: tempfile::TempDir,
}

impl Rig {
    /// Runner chạy, điện thoại Online ở IPv4 (trừ khi `online = false`); `pace` như [`Rig::with_phone`].
    async fn new(cfg: RunnerConfig, pace: Option<u64>, online: bool) -> Rig {
        Rig::with_phone(FakePhone::start().await, cfg, pace, online).await
    }

    /// `pace`: điện thoại giả nhận chậm (byte / s) để test bắt được lúc đang gửi.
    async fn with_phone(phone: FakePhone, cfg: RunnerConfig, pace: Option<u64>, online: bool) -> Rig {
        phone.set_pace(pace);
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(QueueStore::open(&tmp.path().join("queue.sqlite3"), now_ms()).unwrap()));
        let token = phone.issue_token(CLIENT_ID);
        let (link, watch) = link_channel();
        if online {
            link.send_replace(Some(link_at(phone.v4(), &token).await));
        }
        let (tx, events) = mpsc::unbounded_channel();
        let (runner, handle) = UploadRunner::new(device(), store.clone(), watch, tx, cfg);
        let bytes_read = runner.bytes_read();
        tokio::spawn(runner.run());
        Rig { phone, token, store, link, handle, events, log: Vec::new(), bytes_read, tmp }
    }

    fn src(&self) -> PathBuf {
        self.tmp.path().join("src")
    }

    fn add(&self, paths: &[PathBuf], dest: &str) -> (BatchId, Vec<ItemId>) {
        let plan = plan_batch(paths, dest).unwrap();
        let r = self.store.lock().unwrap().add_batch(&device(), &plan, now_ms()).unwrap();
        self.handle.send(Command::Wake);
        r
    }

    fn state(&self, id: ItemId) -> UploadState {
        self.store.lock().unwrap().upload(id).unwrap().unwrap().state
    }

    /// Đọc sự kiện tới khi `f` đúng (giữ cả lịch sử ở `log`).
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

    async fn until_state(&mut self, id: ItemId, f: impl Fn(&UploadState) -> bool) -> UploadState {
        match self.until(|e| matches!(e, TransferEvent::ItemChanged { id: i, state } if *i == id && f(state))).await {
            TransferEvent::ItemChanged { state, .. } => state,
            _ => unreachable!(),
        }
    }

    async fn until_done(&mut self, id: ItemId) -> UploadState {
        self.until_state(id, |s| s.is_terminal()).await
    }

    /// Trạng thái mục `id` theo thứ tự đã phát.
    fn states(&self, id: ItemId) -> Vec<&'static str> {
        self.log
            .iter()
            .filter_map(|e| match e {
                TransferEvent::ItemChanged { id: i, state } if *i == id => Some(state.status()),
                _ => None,
            })
            .collect()
    }

    /// Nội dung tệp trên điện thoại theo tên.
    fn phone_data(&self, name: &str) -> Vec<u8> {
        let f = self.phone.files().into_iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no {name}"));
        f.data.as_ref().clone()
    }

    async fn wait_no_sessions(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.phone.sessions() > 0 {
            assert!(Instant::now() < deadline, "upload session left on phone");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn small_chunks() -> RunnerConfig {
    RunnerConfig { chunk_size: 256 * KB, progress_interval: Duration::from_millis(50), ..RunnerConfig::default() }
}

fn is_done(s: &UploadState) -> bool {
    matches!(s, UploadState::Done { .. })
}

fn sending_past_zero(s: &UploadState) -> bool {
    matches!(s, UploadState::Sending { offset, .. } if *offset > 0)
}

/// Không quay lại Creating sau khi đã Sending = không tạo phiên mới.
fn no_new_session(states: &[&str]) -> bool {
    let first_send = states.iter().position(|s| *s == "sending").unwrap();
    !states[first_send..].contains(&"creating")
}

// ---------------------------------------------------------------- đường chính + Checking

#[tokio::test]
async fn batch_checks_once_and_skips_duplicates() {
    let mut rig = Rig::new(small_chunks(), None, true).await;
    let src = rig.src();
    let a = write(&src, "a.mp4", 100 * KB, 1);
    let b = write(&src, "b.mp4", 120 * KB, 2);
    let c = write(&src, "c.mkv", 90 * KB, 3);
    // Điện thoại đã có `b.mp4` cùng cỡ trong `Phim` ⇒ bỏ qua; `c.mkv` cùng tên nhưng ở gốc ⇒ vẫn gửi.
    rig.phone.add_file("b.mp4", Some("Phim"), vec![0; 120 * KB as usize]);
    rig.phone.add_file("c.mkv", Some(""), vec![0; 90 * KB as usize]);
    let before = rig.phone.connections();
    let (batch, ids) = rig.add(&[a.clone(), b, c.clone()], "Phim");
    let fin = rig.until(|e| matches!(e, TransferEvent::BatchFinished { .. })).await;
    assert_eq!(fin, TransferEvent::BatchFinished { batch, sent: 2, skipped: 1, failed: 0 });
    assert!(is_done(&rig.state(ids[0])));
    assert_eq!(rig.state(ids[1]), UploadState::Skipped);
    // 1 list + 2 × (POST + PATCH): `list` một lần cho cả lô.
    assert_eq!(rig.phone.connections() - before, 5);
    assert_eq!(rig.phone_data("a.mp4"), std::fs::read(&a).unwrap());
    let c_remote = rig.phone.files().into_iter().find(|f| f.name == "c.mkv" && f.dir.as_deref() == Some("Phim"));
    assert_eq!(c_remote.unwrap().data.as_ref(), &std::fs::read(&c).unwrap());
}

// C1
#[tokio::test]
async fn c1_cut_mid_chunk_heads_and_resumes() {
    let mut rig = Rig::new(small_chunks(), None, true).await;
    let size = MB + 77;
    let f = write(&rig.src(), "clip.mp4", size, 9);
    rig.phone.cut_next_patch_after(100_000);
    let (_, ids) = rig.add(std::slice::from_ref(&f), "");
    assert!(is_done(&rig.until_done(ids[0]).await));
    assert!(rig.log.iter().any(|e| matches!(e, TransferEvent::ConnectionLost { .. })));
    let states = rig.states(ids[0]);
    assert!(states.contains(&"waiting") && no_new_session(&states), "{states:?}");
    assert_eq!(rig.phone_data("clip.mp4"), std::fs::read(&f).unwrap());
    // Gửi lại tối đa phần khúc dở (đọc trước vào bộ đệm socket), không gửi lại từ đầu.
    let read = rig.bytes_read.load(Ordering::Relaxed);
    assert!(read >= size && read <= size + 256 * KB, "read {read} of {size}");
}

// C2
#[tokio::test]
async fn c2_server_restart_recreates_from_zero() {
    let mut rig = Rig::new(small_chunks(), Some(2 * MB), true).await;
    let size = MB;
    let f = write(&rig.src(), "clip.mp4", size, 4);
    let (_, ids) = rig.add(std::slice::from_ref(&f), "");
    rig.until_state(ids[0], sending_past_zero).await;
    rig.phone.restart();
    assert!(is_done(&rig.until_done(ids[0]).await));
    let states = rig.states(ids[0]);
    let first_send = states.iter().position(|s| *s == "sending").unwrap();
    assert!(states[first_send..].contains(&"creating"), "{states:?}");
    assert_eq!(rig.phone_data("clip.mp4"), std::fs::read(&f).unwrap());
    assert!(rig.bytes_read.load(Ordering::Relaxed) > size);
}

// C3
#[tokio::test]
async fn c3_insufficient_storage_pauses_until_retry() {
    let mut rig = Rig::new(small_chunks(), None, true).await;
    let f = write(&rig.src(), "big.mp4", 300 * KB, 5);
    rig.phone.set_free_space(MB);
    let (_, ids) = rig.add(std::slice::from_ref(&f), "");
    let s = rig.until_state(ids[0], |s| matches!(s, UploadState::Paused { .. })).await;
    // Điện thoại không cho biết chỗ trống (PROTOCOL 6.3) ⇒ `shortBy` chưa có.
    assert!(matches!(s, UploadState::Paused { reason: PauseReason::NoSpace { short_by: None }, .. }), "{s:?}");
    rig.phone.set_free_space(u64::MAX / 2);
    rig.handle.send(Command::Item { id: ids[0], action: ItemAction::Resume });
    assert!(is_done(&rig.until_done(ids[0]).await));
    assert_eq!(rig.phone_data("big.mp4"), std::fs::read(&f).unwrap());
}

// C4
#[tokio::test]
async fn c4_unauthorized_stops_device_queue_until_repaired() {
    let mut rig = Rig::new(small_chunks(), None, true).await;
    let a = write(&rig.src(), "a.mp4", 50 * KB, 1);
    let b = write(&rig.src(), "b.mp4", 50 * KB, 2);
    rig.phone.forget_tokens();
    let (_, ids) = rig.add(&[a, b], "");
    rig.until(|e| matches!(e, TransferEvent::DeviceNeedsRepair { .. })).await;
    rig.until_state(ids[0], |s| matches!(s, UploadState::Paused { reason: PauseReason::NeedsPairing, .. })).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.state(ids[1]), UploadState::Queued, "queue must stop after 401");
    // Ghép lại ⇒ tầng trên gửi `Link` mới có token mới.
    let token = rig.phone.issue_token(CLIENT_ID);
    rig.link.send_replace(Some(link_at(rig.phone.v4(), &token).await));
    assert!(is_done(&rig.until_done(ids[0]).await));
    assert!(is_done(&rig.until_done(ids[1]).await));
}

// C5
#[tokio::test]
async fn c5_source_changed_mid_upload_fails() {
    let mut rig = Rig::new(small_chunks(), Some(2 * MB), true).await;
    let f = write(&rig.src(), "clip.mp4", MB, 6);
    let (_, ids) = rig.add(std::slice::from_ref(&f), "");
    rig.until_state(ids[0], sending_past_zero).await;
    {
        use std::io::Write;
        let mut h = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        h.write_all(b"edited").unwrap();
    }
    let s = rig.until_done(ids[0]).await;
    assert_eq!(s, UploadState::Failed { reason: FailReason::SourceChanged });
    rig.wait_no_sessions().await;
    assert!(rig.phone.files().is_empty());
}

// C7
#[tokio::test]
async fn c7_new_port_mid_upload_continues() {
    let mut rig = Rig::new(small_chunks(), Some(2 * MB), true).await;
    let f = write(&rig.src(), "clip.mp4", MB, 8);
    let (_, ids) = rig.add(std::slice::from_ref(&f), "");
    rig.until_state(ids[0], sending_past_zero).await;
    // Server "đổi cổng": cổng cũ thôi nghe, phiên upload còn nguyên ở cổng mới.
    rig.phone.stop();
    let extra = rig.phone.listen_extra(Family::V4).await.unwrap();
    rig.link.send_replace(Some(link_at(extra.addr, &rig.token).await));
    assert!(is_done(&rig.until_done(ids[0]).await));
    let states = rig.states(ids[0]);
    assert!(no_new_session(&states), "{states:?}");
    assert_eq!(rig.phone_data("clip.mp4"), std::fs::read(&f).unwrap());
}

// C-W3a
#[tokio::test]
async fn cw3a_nested_folder_keeps_dirs() {
    let mut rig = Rig::new(small_chunks(), None, true).await;
    let trip = rig.src().join("Chuyến đi");
    let a = write(&trip, "a.mp4", 40 * KB, 1);
    let b = write(&trip, "Ngày 1/b.mov", 300 * KB, 2);
    write(&trip, "ghi chú.txt", KB, 3);
    let (_, ids) = rig.add(&[trip], "Phim");
    assert_eq!(ids.len(), 2);
    rig.until(|e| matches!(e, TransferEvent::BatchFinished { sent: 2, .. })).await;
    let files = rig.phone.files();
    let dir_of = |n: &str| files.iter().find(|f| f.name == n).unwrap().dir.clone();
    assert_eq!(dir_of("a.mp4").as_deref(), Some("Phim/Chuyến đi"));
    assert_eq!(dir_of("b.mov").as_deref(), Some("Phim/Chuyến đi/Ngày 1"));
    assert_eq!(rig.phone_data("b.mov"), std::fs::read(&b).unwrap());
    assert_eq!(rig.phone_data("a.mp4"), std::fs::read(&a).unwrap());
    assert!(!rig.log.iter().any(|e| matches!(e, TransferEvent::PhoneNeedsUpdate { .. })));
    match rig.state(ids[1]) {
        UploadState::Done { remote_dir, .. } => assert_eq!(remote_dir.as_deref(), Some("Phim/Chuyến đi/Ngày 1")),
        s => panic!("{s:?}"),
    }
}

// W-11 / §6.9 — thư mục ẩn có thật là đích hợp lệ khi điện thoại khai `browse`; không `browse` thì điện thoại bỏ `.` đầu.
#[tokio::test]
async fn hidden_dir_kept_only_with_browse() {
    for (features, want) in [(&["dirs", "browse"][..], ".vdr/media/.cfg"), (&["dirs"][..], "vdr/media/cfg")] {
        let phone = FakePhone::start().await;
        phone.set_features(features);
        let mut rig = Rig::with_phone(phone, small_chunks(), None, true).await;
        let cfg = rig.src().join(".cfg");
        write(&cfg, "a.mp4", 40 * KB, 1);
        rig.add(&[cfg], ".vdr/media");
        rig.until(|e| matches!(e, TransferEvent::BatchFinished { sent: 1, .. })).await;
        assert_eq!(rig.phone.files()[0].dir.as_deref(), Some(want), "{features:?}");
    }
}

// C-W3a — điện thoại cũ không có `dirs`.
#[tokio::test]
async fn cw3a_phone_without_dirs_sends_flat_and_flags() {
    let phone = FakePhone::start().await;
    phone.set_features(&[]);
    let mut rig = Rig::with_phone(phone, small_chunks(), None, true).await;
    let trip = rig.src().join("Trip");
    write(&trip, "a.mp4", 40 * KB, 1);
    write(&trip, "Day 1/b.mp4", 40 * KB, 2);
    let (_, ids) = rig.add(&[trip], "");
    rig.until(|e| matches!(e, TransferEvent::BatchFinished { sent: 2, .. })).await;
    assert!(rig.phone.files().iter().all(|f| f.dir.as_deref() == Some("")));
    let flags = rig.log.iter().filter(|e| matches!(e, TransferEvent::PhoneNeedsUpdate { .. })).count();
    assert_eq!(flags, 1);
    assert!(ids.iter().all(|id| is_done(&rig.state(*id))));
}

// C-W3b
#[tokio::test]
async fn cw3b_switch_v4_to_v6_between_chunks() {
    let phone = FakePhone::start().await;
    let Some(v6) = phone.v6() else {
        eprintln!("skip: no IPv6 loopback");
        return;
    };
    let mut rig = Rig::with_phone(phone, small_chunks(), Some(2 * MB), true).await;
    let f = write(&rig.src(), "clip.mp4", MB, 10);
    let (_, ids) = rig.add(std::slice::from_ref(&f), "");
    rig.until_state(ids[0], sending_past_zero).await;
    rig.link.send_replace(Some(link_at(v6, &rig.token).await));
    assert!(is_done(&rig.until_done(ids[0]).await));
    assert!(no_new_session(&rig.states(ids[0])));
    assert!(!rig.log.iter().any(|e| matches!(e, TransferEvent::ConnectionLost { .. })));
    assert_eq!(rig.phone_data("clip.mp4"), std::fs::read(&f).unwrap());
}

// ---------------------------------------------------------------- lệnh người dùng, Offline

#[tokio::test]
async fn pause_resume_then_cancel_deletes_session() {
    let mut rig = Rig::new(small_chunks(), Some(MB), true).await;
    let a = write(&rig.src(), "a.mp4", MB, 1);
    let b = write(&rig.src(), "b.mp4", MB, 2);
    let (_, ids) = rig.add(&[a.clone(), b], "");
    rig.until_state(ids[0], sending_past_zero).await;
    rig.handle.send(Command::Item { id: ids[0], action: ItemAction::Pause });
    rig.until_state(ids[0], |s| matches!(s, UploadState::Paused { reason: PauseReason::User, .. })).await;
    // Mục sau chạy khi mục đầu tạm dừng; huỷ nó giữa chừng ⇒ `DELETE` phiên.
    rig.until_state(ids[1], sending_past_zero).await;
    rig.handle.send(Command::Item { id: ids[1], action: ItemAction::Cancel });
    rig.until_state(ids[1], |s| *s == UploadState::Cancelled).await;
    // Phiên của `a` (tạm dừng) còn trên điện thoại, phiên của `b` đã xoá.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.phone.sessions(), 1);
    rig.handle.send(Command::Item { id: ids[0], action: ItemAction::Resume });
    assert!(is_done(&rig.until_done(ids[0]).await));
    assert!(no_new_session(&rig.states(ids[0])));
    assert_eq!(rig.phone_data("a.mp4"), std::fs::read(&a).unwrap());
    rig.wait_no_sessions().await;
}

#[tokio::test]
async fn offline_waits_without_work_then_runs_when_online() {
    let mut rig = Rig::new(small_chunks(), None, false).await;
    let f = write(&rig.src(), "a.mp4", 50 * KB, 1);
    let (_, ids) = rig.add(&[f], "");
    let before = rig.phone.connections();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.state(ids[0]), UploadState::Queued);
    assert_eq!(rig.phone.connections(), before);
    rig.link.send_replace(Some(link_at(rig.phone.v4(), &rig.token).await));
    assert!(is_done(&rig.until_done(ids[0]).await));
}

#[tokio::test]
async fn offline_mid_backoff_wakes_immediately_on_new_address() {
    let mut rig = Rig::new(small_chunks(), Some(2 * MB), true).await;
    let f = write(&rig.src(), "clip.mp4", MB, 3);
    let (_, ids) = rig.add(std::slice::from_ref(&f), "");
    rig.until_state(ids[0], sending_past_zero).await;
    // Điện thoại tắt hẳn ở địa chỉ cũ: runner rơi vào backoff, discovery báo Offline rồi thấy lại ở cổng mới.
    rig.phone.stop();
    rig.link.send_replace(None);
    rig.until_state(ids[0], |s| matches!(s, UploadState::Waiting { .. })).await;
    let extra = rig.phone.listen_extra(Family::V4).await.unwrap();
    let t = Instant::now();
    rig.link.send_replace(Some(link_at(extra.addr, &rig.token).await));
    rig.until_state(ids[0], |s| matches!(s, UploadState::Sending { .. })).await;
    assert!(t.elapsed() < Duration::from_millis(1500), "woke after {:?}", t.elapsed());
    assert!(is_done(&rig.until_done(ids[0]).await));
    assert_eq!(rig.phone_data("clip.mp4"), std::fs::read(&f).unwrap());
}

// §6.10 — điện thoại có `batch`: mọi `POST` của lô mang `X-Batch-Id` / `X-Batch-Index`; thân danh sách đủ cả lô (tệp
// đã có ⇒ `skip`) gửi tới khi một `POST` nhận `201`, kể cả lần gửi lại sau `503 awaiting_approval`.
#[tokio::test]
async fn batch_manifest_until_first_created() {
    let phone = FakePhone::start().await;
    phone.set_features(&["dirs", "batch"]);
    phone.add_file("b.mp4", Some("Trip"), pattern(30 * KB, 2));
    phone.set_awaiting(1);
    let mut rig = Rig::with_phone(phone, small_chunks(), None, true).await;
    let trip = rig.src().join("Trip");
    write(&trip, "a.mp4", 40 * KB, 1);
    write(&trip, "b.mp4", 30 * KB, 2);
    write(&trip, "c.mp4", 50 * KB, 3);
    let (batch, ids) = rig.add(&[trip], "");
    assert_eq!(ids.len(), 3);
    rig.until(|e| matches!(e, TransferEvent::BatchFinished { sent: 2, skipped: 1, .. })).await;
    let posts = rig.phone.batch_posts();
    let names: Vec<&str> = posts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["a.mp4", "a.mp4", "c.mp4"], "503 awaiting ⇒ cùng POST gửi lại; b đã có ⇒ không POST");
    for p in &posts {
        assert_eq!(p.id.as_deref(), Some(batch.0.as_str()));
        assert_eq!((p.files, p.bytes), (Some(2), Some(90 * KB)), "không tính tệp skip");
    }
    assert_eq!(posts.iter().map(|p| p.index).collect::<Vec<_>>(), [Some(0), Some(0), Some(2)]);
    let list = posts[0].list.clone().expect("thân danh sách ở POST đầu");
    assert_eq!(posts[1].list.as_ref(), Some(&list), "chưa có 201 ⇒ gửi lại danh sách");
    assert!(posts[2].list.is_none(), "đã có 201 ⇒ thôi gửi thân");
    let rows: Vec<(&str, &str, u64, bool)> =
        list.iter().map(|f| (f.name.as_str(), f.dir.as_str(), f.length, f.skip)).collect();
    assert_eq!(
        rows,
        [("a.mp4", "Trip", 40 * KB, false), ("b.mp4", "Trip", 30 * KB, true), ("c.mp4", "Trip", 50 * KB, false)]
    );
}

// §6.10 — `410 declined`: cả lô Failed "Điện thoại từ chối nhận", không thử lại, không PATCH.
#[tokio::test]
async fn declined_batch_fails_every_item() {
    let phone = FakePhone::start().await;
    phone.set_features(&["dirs", "batch"]);
    phone.set_declined(true);
    let mut rig = Rig::with_phone(phone, small_chunks(), None, true).await;
    let trip = rig.src().join("Trip");
    write(&trip, "a.mp4", 40 * KB, 1);
    write(&trip, "b.mp4", 40 * KB, 2);
    write(&trip, "c.mp4", 40 * KB, 3);
    let (_, ids) = rig.add(&[trip], "");
    rig.until(|e| matches!(e, TransferEvent::BatchFinished { sent: 0, failed: 3, .. })).await;
    for id in &ids {
        assert_eq!(rig.state(*id), UploadState::Failed { reason: FailReason::Declined });
    }
    assert_eq!(rig.phone.batch_posts().len(), 1, "410 ⇒ không POST thêm mục nào của lô");
    assert!(rig.phone.files().is_empty());
}

// Điện thoại chưa có `batch`: chỉ `X-Batch-Files` / `X-Batch-Bytes` (§6.5), không id / index / thân.
#[tokio::test]
async fn phone_without_batch_gets_counts_only() {
    let mut rig = Rig::new(small_chunks(), None, true).await;
    let a = write(&rig.src(), "a.mp4", 40 * KB, 1);
    let b = write(&rig.src(), "b.mp4", 20 * KB, 2);
    rig.add(&[a, b], "Phim");
    rig.until(|e| matches!(e, TransferEvent::BatchFinished { sent: 2, .. })).await;
    let posts = rig.phone.batch_posts();
    assert_eq!(posts.len(), 2);
    for p in &posts {
        assert_eq!((p.files, p.bytes), (Some(2), Some(60 * KB)));
        assert!(p.id.is_none() && p.index.is_none() && p.list.is_none(), "{p:?}");
    }
}
