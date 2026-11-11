# ferry-gd1_04 — upload-runner

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ 11/11/2026 |
| Repo | ferry |
| Model | opus |
| Cỡ | M — ~600 dòng |
| Phụ thuộc | gd1_02, gd1_03 |
| Quyết định áp dụng | W-1, W-3 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/core/src/transfer/{mod,upload,throttle}.rs`, `crates/core/tests/upload.rs`; SỬA `queue/store.rs` (`pending_upload`), `tests/fake_phone/mod.rs` (thêm `listen_extra`) |

## 0. Bàn giao từ ferry-gd1_02 (11/11/2026)

<!-- handoff: ferry-gd1_02 -->

- **Đã có sẵn**: `client::PhoneClient` (`upload_create(name, length, dir: Option<&str>)`, `upload_patch(uid, offset, reader,
  len) -> PatchOutcome::{Partial(u64), Done(LibraryItem)}`, `upload_head -> UploadStatus`, `upload_delete`, `list`),
  `client::CHUNK_SIZE` (32 MB), `protocol::{is_video_name, normalize_dir, feature::DIRS}`, `Info::has_feature`.
- **`upload_patch` nhận `AsyncRead` đã seek** tới `offset` (vd `tokio::fs::File` + `seek`) — runner giữ tệp, kiểm
  `size`/`mtime`, bọc token bucket quanh reader. Bộ đọc trong client đọc đúng `len` byte, buffer 64 KB.
- **Lỗi để quyết** (`ClientError`): `Network(_)` ⇒ `upload_head` rồi gửi tiếp; `OffsetMismatch(o)` ⇒ tiếp từ `o`;
  `NotFound` ⇒ tạo lại từ 0; `InsufficientStorage` ⇒ Paused; `Unauthorized` ⇒ `DeviceNeedsRepair`; `Locked` ⇒ chờ + HEAD;
  `Busy { retry_after }`; `Local(_)` (đọc tệp nguồn hỏng / tệp ngắn hơn khúc, `UnexpectedEof`) ⇒ Failed, đừng thử lại.
- **Địa chỉ đổi (C7, C-W3b)**: một `PhoneClient` = một `SocketAddr`; đổi địa chỉ ⇒ dựng client mới (rẻ), cùng `uid`.
- **`fake_phone`** (`tests/fake_phone/mod.rs`, `mod fake_phone;`): `issue_token`, `cut_next_patch_after(n)`, `restart()`
  (mất phiên ⇒ 404), `fail_next(507|503|…, retry_after)`, `set_free_space`, `set_features(&[])` (không `dirs`),
  `set_discard_data(true)` (upload lớn không tốn RAM test), `files()`, `connections()`, `v4()` / `v6()` (cùng cổng). Chưa có
  "đổi cổng giữa chừng": thêm nút vào tệp này (vd nghe thêm một listener cổng khác dùng chung `state`).
- **Soát lại**: `cargo test -p ferry-core --test client`; mục 9 của `plan/ferry-gd1_02_protocol-client.md`.

## 0. Bàn giao từ gd1_03 (11/11/2026)

<!-- handoff: ferry-gd1_03 -->

- **Có sẵn** (`ferry-core::queue`): `QueueStore::open_default(now)` (mở = migrate + đưa mục đang bay về `Waiting` + dọn
  7 ngày), `next_upload(device, now)` (đầu hàng FIFO; `None` khi đầu hàng còn chờ ⇒ ngủ tới `next_wake()`),
  `apply_upload(id, &UploadEvent, now) -> Option<UploadEffect>` (ghi ngay), `apply_batch`, `batch_progress`,
  `retry_upload`. Runner: lấy mục ⇒ `Queued` gửi `Start`, `Waiting` gửi `WakeUp` ⇒ thực thi `UploadEffect`
  (`CheckRemote`, `Create`, `Patch{uid, offset}`, `Head{uid}`, `Delete{uid}`, `Wait{until}`) ⇒ báo kết quả bằng
  `UploadEvent` (bảng ánh xạ mã HTTP ở doc của từng biến thể trong `queue/state.rs`).
- Mục mang `dir` (đã chuẩn hoá `/`, `""` = gốc) cho `X-File-Dir`, `name` cho `X-File-Name`, `size` cho `Upload-Length`.
  `Committed{remote_name, remote_dir}` lưu tên + `dir` thật điện thoại trả.
- Kiểm nguồn trước mỗi khúc: `queue::batch::source_matches(path, size, mtime)` (`Err(NotFound)` ⇒ `SourceMissing`).
- `BatchProgress::is_settled()` ⇒ phát `BatchFinished`. Đuôi video: `queue::batch::VIDEO_EXTENSIONS` (gộp với
  `protocol` nếu gd1_02 cũng khai).
- **Bẫy**: clippy chéo cần `LIBSQLITE3_SYS_USE_PKG_CONFIG=1` (`CLAUDE.md` mục Bẫy).

## 1. Làm gì
- `transfer::Runner` cho một thiết bị: lấy mục kế, chạy effect của `queue` bằng `PhoneClient`, đẩy sự kiện ngược lại vào máy trạng thái.
- Checking: `list` một lần mỗi lô (không mỗi tệp), trùng tên + cỡ ⇒ Skipped.
- Sending: khúc 32 MB stream từ tệp; trước khúc kiểm `size`/`mtime` nguồn; sau khúc lưu offset; tiến độ + tốc độ (trung bình trượt) ra kênh sự kiện, không mỗi gói.
- Thiết bị Offline ⇒ runner ngủ tới khi Online (không backoff vô ích); tạm dừng / huỷ (huỷ ⇒ `DELETE` phiên nếu đang Online).
- Giới hạn băng thông tuỳ chọn (token bucket trên reader), mặc định không giới hạn.
- Thông báo "cả lô xong" là sự kiện `BatchFinished{sent, skipped, failed}` — task 07 hiển thị.
- Bổ sung W-3: Checking so trùng `dir` + tên + cỡ; Creating gửi `X-File-Dir` chỉ khi `info.features` có `dirs`, không có ⇒ gửi phẳng + cờ "cần cập nhật app điện thoại" cho UI. Khúc sau có thể đi địa chỉ khác (IPv4 ↔ IPv6) — phiên theo `uid`.

## 6. Nghiệm thu (với `fake_phone`)
- C1. Đứt giữa khúc ⇒ HEAD ⇒ gửi tiếp, tổng byte gửi ≈ cỡ tệp + phần khúc dở. `[opus-unit]`
- C2. Server tắt (mất phiên) ⇒ `404` ⇒ tạo lại từ 0. `[opus-unit]`
- C3. `507` ⇒ Paused, thông tin số byte thiếu; người dùng "thử lại" ⇒ chạy tiếp. `[opus-unit]`
- C4. `401` ⇒ dừng hàng đợi thiết bị, phát `DeviceNeedsRepair`. `[opus-unit]`
- C5. Tệp nguồn bị sửa giữa chừng ⇒ Failed "File đã thay đổi". `[opus-unit]`
- C6. Giới hạn 10 MB/s ⇒ tốc độ đo trong ±10 %. `[opus-unit]`
- C7. Đổi cổng / IP server giả giữa chừng (runner nhận địa chỉ mới) ⇒ gửi tiếp. `[opus-unit]`
- C-W3a. `fake_phone` có `dirs`: lô thư mục lồng ⇒ đúng `dir` từng tệp; không có `dirs` ⇒ gửi phẳng + cờ. `[opus-unit]`
- C-W3b. Đổi địa chỉ (v4 → v6) giữa hai khúc ⇒ vẫn xong, không tạo phiên mới. `[opus-unit]`

## 9. Bàn giao thực tế

**Đã làm (11/11/2026, Opus)** — nhánh `claude/gd1_04-upload-runner`, worktree `ferry-w1`, build `target/w1`.
- `transfer/mod.rs` — chỗ cắm cho 05 / 07: `Link { client: Arc<PhoneClient>, dirs }` (`Link::new(client, &info)`),
  `link_channel() -> (LinkSender, LinkWatch)` (`tokio::sync::watch<Option<Link>>`: `Some` = Online ở địa chỉ đã chọn,
  `None` = Offline), `SharedStore = Arc<Mutex<QueueStore>>`, `Command::{Wake, Item{id, action}, Batch{batch, action},
  Stop}`, `ItemAction::{Pause, Resume, Cancel, Retry}`, `RunnerHandle::send`, `TransferEvent::{ItemChanged{id, state},
  Progress{id, sent, size, bytesPerSec}, BatchFinished{batch, sent, skipped, failed}, DeviceNeedsRepair{device},
  ConnectionLost{device, addr}, PhoneNeedsUpdate{device}}`. Không trait: `watch` + `mpsc` đủ cho cả discovery thật lẫn test.
- `transfer/upload.rs` — `UploadRunner::new(device, store, link, throttle, events, RunnerConfig) -> (runner, handle)`,
  `tokio::spawn(runner.run())`. Vòng: `QueueStore::pending_upload` (thêm mới, đầu hàng kể cả đang backoff) ⇒ `Start` /
  `WakeUp` ⇒ thực thi effect ⇒ `UploadEvent` ⇒ … Rảnh / Offline / sau `401` ⇒ `select!` chờ lệnh, `Link` đổi, đồng hồ
  `Waiting.until` — không poll. `Link` đổi lúc đang backoff ⇒ thức ngay (thử: < 1,5 s thay vì chờ hết backoff).
  - Checking: `list` một lần mỗi lô (`RemoteIndex` theo `batch_id`, sống 10 phút, bỏ khi lô yên), khoá `dir` chuẩn hoá +
    tên + cỡ; không `dirs` ⇒ so ở gốc (nơi tệp phẳng sẽ nằm). Không phải đuôi video ⇒ `Rejected(Unsupported)` khỏi request.
  - Creating: `X-File-Dir` chỉ khi `link.dirs` và `dir` khác rỗng; không `dirs` mà mục có thư mục ⇒ gửi phẳng +
    `PhoneNeedsUpdate` một lần mỗi `Link`.
  - Sending: kiểm `source_matches` trước mỗi khúc (cả trước Checking / Creating), mở tệp + seek, `MeteredReader` (đếm +
    token bucket) vào `upload_patch`; `409` dùng luôn offset điện thoại trả (`OffsetReported`, khỏi một `HEAD`).
  - Mỗi request đọc `Link` mới nhất ⇒ khúc sau đi địa chỉ khác cùng `uid`. Lỗi mạng ⇒ `ConnectionLost{addr}` (discovery
    chọn lại địa chỉ) + `Transient`.
  - Lệnh: Pause / Cancel chạm mục đang bay ⇒ cắt request rồi áp (Cancel ⇒ `DELETE` phiên, chờ ≤ 10 s); lệnh khác hoãn tới
    hết request (≤ một khúc). Lệnh cho mục của thiết bị khác bị bỏ.
- `transfer/throttle.rs` — `Throttle::new(Option<bytes/s>)` dùng chung (`Arc`) ⇒ giới hạn tổng app, `set_rate` lúc chạy;
  xô bắt đầu rỗng, trần 100 ms dữ liệu; không giới hạn ⇒ đọc thẳng. `SpeedMeter` trung bình trượt 5 s, mẫu theo nhịp
  `progress_interval` (500 ms).
- `tokio` của `ferry-core` thêm `fs, macros, rt, sync, time` (cùng crate, không phụ thuộc mới; `deny` ✓).

**Opus chọn 11/11/2026** (D-9):
- Cắm địa chỉ bằng `watch<Option<Link>>` thay trait: discovery (05) chỉ cần `send_replace`; nhiều địa chỉ / happy
  eyeballs nằm hẳn ở 05, runner không biết tới tập địa chỉ.
- `401` ⇒ mục `Paused{NeedsPairing}`, runner dừng cả hàng của thiết bị; `Link` mới (sau ghép lại) ⇒ tự `Resume` mọi mục
  `NeedsPairing` của thiết bị. Tầng trên không gửi lại `Link` cũ khi chưa có token mới.
- `507` ⇒ `shortBy: None`: `info` không có chỗ trống (PROTOCOL 6.3) nên PC không biết "thiếu bao nhiêu GB" — xem mục 10.
- Mục 2.6 "một thông báo khi cả lô xong": `BatchFinished` chỉ khi mọi mục kết thúc (Paused chưa tính) và có ít nhất một
  mục Done / Skipped / Failed (huỷ cả lô ⇒ im).
- Khúc cuối đứt rồi `HEAD` ⇒ `404` (phản hồi `201` có thể đã mất): hỏi `list`, có tệp trùng `dir` + tên + cỡ ⇒ coi là
  Done, không gửi lại thành `a (1).mp4`. Chưa có test (server giả không tách được "commit mà mất phản hồi").
- Sự kiện máy trạng thái bỏ qua mà mục vẫn đang bay (mã lạ đúng pha) ⇒ ép `Transient` để không quay vòng nóng.
- Lỗi `Local` khi đọc nguồn: `NotFound` ⇒ `SourceMissing`, còn lại ⇒ `SourceChanged` ("File đã thay đổi").
- Store dùng `std::sync::Mutex` (không giữ qua `await`, mỗi lệnh SQLite ngắn) — không đẩy sang `spawn_blocking`.

**Nghiệm thu** (`tests/upload.rs`, 14 ca, ~2 s, chạy 5 lần liền xanh): C1 ✓ (cắt sau 100 KB ⇒ `ConnectionLost`, chờ,
`HEAD`, gửi tiếp; byte đọc ≤ cỡ + 1 khúc; nội dung khớp) · C2 ✓ (`restart` giữa chừng ⇒ Creating lại, gửi lại từ 0) ·
C3 ✓ (`507` ⇒ `Paused{NoSpace}`, `Resume` ⇒ xong) · C4 ✓ (`DeviceNeedsRepair`, mục 2 vẫn Queued; `Link` mới ⇒ cả hai
xong) · C5 ✓ (sửa tệp giữa chừng ⇒ `Failed{SourceChanged}`, phiên bị `DELETE`) · C6 ✓ (10 MB/s, 20 MB: tổng và tốc độ
báo UI trong ±10 %) · C7 ✓ (cổng cũ dừng, cổng mới cùng phiên, không Creating lại) · C-W3a ✓ (`Phim/Chuyến đi/Ngày 1`
đúng từng tệp; không `dirs` ⇒ phẳng + một `PhoneNeedsUpdate`) · C-W3b ✓ (v4 → v6 giữa hai khúc, không `ConnectionLost`,
không phiên mới). Thêm: Checking một `list` cho lô 3 tệp (đếm 5 kết nối), tạm dừng / chạy tiếp / huỷ, Offline không
request nào, backoff thức ngay khi có địa chỉ mới. Test đơn vị `throttle` 4 ca.

**Soát xanh** (cwd = worktree, `target/w1`): `cargo build` ✓, `cargo clippy --all-targets -- -D warnings` ✓, clippy chéo
`-p ferry-core` Windows + Linux (`LIBSQLITE3_SYS_USE_PKG_CONFIG=1`) ✓, `cargo test` ✓ (core lib 80, client 11, upload 14,
daemon 3), `cargo deny check licenses` ✓.

## 10. Đề xuất cải tiến

Soát 4 lăng kính: **bảo mật** — log chỉ có `device` / địa chỉ / uid, không token (token nằm trong `PhoneClient`, `Debug`
che); đường dẫn nguồn lấy từ store (do `plan_batch` dựng), runner không nhận đường dẫn qua IPC. **RAM** — mỗi khúc chỉ
bộ đệm 64 KB của client (+ 64 KB `scratch` khi bật giới hạn); `RemoteIndex` một lô một lúc. **UI/UX**, **Pro** — không áp
(lõi), trừ dòng dưới.
- [ux] `507` không nói được "cần thêm 1,2 GB" (PROTOCOL 2.6 ví dụ) vì `info` không có chỗ trống — PROTOCOL 6.3 / app điện
  thoại thêm `free` (byte) vào `info` hoặc header vào `507`, PC điền `shortBy = size + 16 MB − free` — S — P2. Không tự
  làm: đổi hợp đồng (W-1) cần chủ dự án; ghi `plan/_backlog.md`.
