# ferry-gd1_03 — queue-store

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ (11/11/2026) |
| Repo | ferry |
| Model | opus |
| Cỡ | M — ~600 dòng |
| Phụ thuộc | gd1_01 (song song với 02) |
| Quyết định áp dụng | W-1, W-3 |
| Phụ thuộc mới | `rusqlite` (`bundled`; MIT, SQLite public domain) |
| File đụng tới | MỚI `crates/core/src/queue/{mod,state,store,backoff,batch}.rs` |

## 0. Bàn giao từ gd1_01 (11/11/2026)

<!-- handoff: ferry-gd1_01 -->

- **Đã có sẵn**: `ferry-core` khai sẵn mọi module (tệp rỗng ghi task chủ) — chỉ thêm mã vào tệp của mình, không sửa `lib.rs`.
  `queue` và `transfer` là thư mục (`queue/mod.rs`, `transfer/mod.rs`).
- **Kiểu dùng chung W-3**: `device::DeviceId` (chuỗi trong suốt), `config::PairedDevice { id, name, addresses: Vec<String> }`
  (địa chỉ dạng chuỗi giữ `%iface`), `queue::BatchId::new()` (UUID v4). Dùng lại, đừng định nghĩa song song.
- **Phụ thuộc đã có trong `ferry-core`**: `serde`, `serde_json`, `uuid` (v4), `log`, `dirs`, `toml`, `interprocess`,
  `gethostname`, `sys-locale`; dev: `tempfile`. Thêm crate ⇒ khai ở `crates/core/Cargo.toml` (crate dùng chung nhiều
  crate thì vào `[workspace.dependencies]`), soát `cargo deny check licenses`.
- **Đường dẫn**: `paths::data_dir()` (ghi đè `FERRY_DATA_DIR`) cho SQLite / tệp tạm; test dùng `tempfile`, không đụng thư mục thật.
- **Soát lại khi làm**: build ở `<gốc>/ferry/target/w<N>`; đổi mã theo OS ⇒ thêm clippy `--target
  x86_64-pc-windows-msvc` / `x86_64-unknown-linux-gnu` (lệnh ở `CLAUDE.md` mục Lệnh).
- **Bẫy**: socket Unix ≤ 104 byte (đã tự xử lý trong `ipc`); số RAM trên macOS đo bằng `footprint`, không RSS.

## 1. Làm gì
- Máy trạng thái mục upload đúng PROTOCOL mục 5 dưới dạng hàm thuần `next(state, event) -> (state, effect)`: `Queued, Checking, Creating, Sending{uid,offset}, Waiting{attempt,until}, Paused{reason}, Done{remote_name}, Skipped, Failed{reason}, Cancelled`. Mục download: `Queued, Fetching{offset,etag}, Waiting, Done, Failed`.
- Backoff 2 s, 4 s … trần 60 s; `Retry-After` của server thắng.
- Lưu SQLite ở thư mục dữ liệu: ghi ngay khi có `uid`, sau mỗi khúc lưu offset; WAL; migration có số phiên bản.
- Mục ghi kèm `size` + `mtime` lúc thả vào để phát hiện "File đã thay đổi".
- "Đã gửi" giữ 7 ngày rồi tự dọn (lúc khởi động + mỗi ngày một lần, không timer dày).
- Mỗi thiết bị một hàng đợi tuần tự (`device_id` trên mục).
- Bổ sung W-3: mục upload lưu `dir` đích (đã chuẩn hoá `/`) và `batch_id` (một lần thả / dán = một lô — PROTOCOL 5); truy vấn tổng tiến độ theo lô.

## 2. Không làm trong task này
- Không gọi mạng; effect chỉ là mô tả (`Effect::Create`, `Effect::Patch{offset}`, `Effect::Head`…), task 04 thực thi.

## 6. Nghiệm thu
- C1. Mọi mũi tên của PROTOCOL mục 5 có test một-một (404 ⇒ Creating lại, 409 ⇒ Head, 507 ⇒ Paused, 415/413 ⇒ Failed…). `[opus-unit]`
- C2. Ghi 1000 mục, đóng / mở lại store ⇒ đủ mục, đúng trạng thái + offset; `Sending` mở lại thành `Waiting` (cần `HEAD`). `[opus-unit]`
- C3. Giết tiến trình giữa lúc ghi (test con) ⇒ DB không hỏng. `[opus-unit]`
- C4. Backoff ca biên: lần 1, lần chạm trần, `Retry-After` 0 / lớn. `[opus-unit]`
- C-W3. Lô 3 tệp trong 2 thư mục con: lưu / đọc lại đúng `dir`, tổng tiến độ lô đúng sau khởi động lại. `[opus-unit]`

## 8. Nợ chuyển task khác

- gd1_04: Checking gom `list` một lần mỗi lô là việc của runner — máy trạng thái vẫn nhận `Checked{exists}` từng mục.
  Kiểm `size`/`mtime` nguồn trước mỗi khúc bằng `queue::batch::source_matches`, sai ⇒ `SourceChanged` / `SourceMissing`.
- gd1_02 / gd1_04: `queue::batch::VIDEO_EXTENSIONS` + `is_video` (§6.6) đặt ở đây vì gd1_02 chạy song song; nếu
  `protocol` cũng khai danh sách đuôi thì giữ một chỗ (`protocol`) và cho `batch` dùng lại.
- gd1_07: gọi `QueueStore::purge_finished` mỗi `PURGE_INTERVAL_MS` (mở store đã dọn một lần); đồng hồ runner theo
  `QueueStore::next_wake`.
- gd1_07 / gd1_08: lệnh IPC `AddFiles { paths, dest_dir }` ⇒ `queue::plan_batch(&paths, &dest_dir)` rồi
  `QueueStore::add_batch(device, &plan)`; báo "Bỏ qua N tệp không phải video" từ `plan.skipped_non_video`.

## 9. Bàn giao thực tế

- **Mã**: `queue/state.rs` (máy trạng thái upload + download, hàm thuần `UploadMachine::next(&event, now) ->
  (máy, UploadEffect)`), `backoff.rs`, `batch.rs` (dựng lô §6.9: thư mục thả giữ tên làm gốc, chỉ video, không theo
  symlink thư mục, tệp trước thư mục con), `store.rs` (SQLite), `mod.rs` (`UnixMs = i64` mili giây, `now_ms()`).
  ~2 170 dòng, quá nửa là test (46 test `queue::*`). Phụ thuộc mới `rusqlite 0.41` `bundled` (MIT; SQLite public domain).
- **Soát xanh** (worktree w2, `target/w2`): `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
  (65 + 3 test), `cargo deny check licenses` ok; clippy chéo Windows / Linux ok với `LIBSQLITE3_SYS_USE_PKG_CONFIG=1`
  (bẫy mới ghi vào `CLAUDE.md`).
- **Nghiệm thu**: C1 — mỗi mũi tên §5 một test trong `state.rs` (Start, Checked trùng / mới, Created, ChunkAccepted,
  Committed, lỗi tạm ⇒ Waiting ⇒ HEAD, 404 ⇒ Creating, 409 ⇒ HEAD, 423 ⇒ chờ rồi HEAD, 415 / 413 / 400 ⇒ Failed,
  507 ⇒ Paused, 401 ⇒ Paused, nguồn đổi ⇒ Failed, Restore). C2 `thousand_items_survive_reopen`. C3
  `killed_writer_leaves_db_intact` (tiến trình con = chính exe test, SIGKILL 3 vòng, `integrity_check` ok, lô 20 mục
  nguyên vẹn hoặc không có). C4 `backoff::tests`. C-W3 `batch_dirs_and_progress_survive_restart`.
- **Opus chọn 11/11/2026** (D-9):
  - `next` nhận thêm `now` (giờ là tham số, không đọc đồng hồ) để `Waiting.until` tuyệt đối, lưu được và test được.
  - `Waiting` / `Paused` mang `resume: Check | Create | Head{uid, offset}` — chạy tiếp đúng pha, không mất `uid`.
    `failures` (lỗi tạm liên tiếp) nằm cạnh trạng thái trong `UploadMachine`, về 0 khi có tiến triển thật.
  - `Retry-After` thắng nhưng kẹp [1 s, 1 h]: `0` lặp lại không thành vòng bận, số lạ không treo hàng cả ngày.
  - `401` ⇒ `Paused{NeedsPairing}` (mục chờ ghép lại); `400` (`X-File-Dir` sai) ⇒ `Failed{BadRequest}`.
  - Thêm `Cancelled` cho download (UI có nút huỷ); `Done` upload giữ cả `remoteDir` đã làm sạch (§6.9).
  - Một bảng `items` cho cả hai chiều (`kind`), trạng thái là JSON + cột bản sao (`status`, `done_bytes`, `wake_ms`)
    cho truy vấn; `batches` giữ `dest_dir`, nhãn, số tệp bỏ qua. `synchronous=NORMAL` + WAL (lý do ở đầu `store.rs`).
  - Đầu hàng mỗi thiết bị nghiêm FIFO: mục đầu đang chờ backoff thì không ai chen; mục Paused / Failed bị bỏ qua.
  - Dọn 7 ngày chỉ Done / Skipped / Cancelled; Failed giữ tới khi người dùng thử lại / xoá.
  - "Thử lại" sau `SourceChanged` (`retry_upload`) đọc lại cỡ + mtime: người dùng sửa tệp rồi muốn gửi bản mới.
- **Đề xuất đã làm**: `cache_size` SQLite 512 KB thay mặc định 2 MB (daemon chạy nền cả ngày).

## 10. Đề xuất cải tiến

- [ram] `rusqlite` thêm ~1,5 MB vào exe `ferry`; RAM lúc rảnh chưa đo lại vì daemon chưa mở store (gd1_07 nối) — đo
  `footprint` khi gd1_07 xong — S — P3.
