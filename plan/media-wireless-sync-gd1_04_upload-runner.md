# media-wireless-sync-gd1_04 — upload-runner

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | M — ~600 dòng |
| Phụ thuộc | gd1_02, gd1_03 |
| Quyết định áp dụng | W-1, W-3 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/core/src/transfer/{mod,upload,throttle}.rs`, `crates/core/tests/upload.rs` |

## 0. Bàn giao từ media-wireless-sync-gd1_02 (11/11/2026)

<!-- handoff: media-wireless-sync-gd1_02 -->

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
- **Soát lại**: `cargo test -p mws-core --test client`; mục 9 của `plan/media-wireless-sync-gd1_02_protocol-client.md`.

## 0. Bàn giao từ gd1_03 (11/11/2026)

<!-- handoff: media-wireless-sync-gd1_03 -->

- **Có sẵn** (`mws-core::queue`): `QueueStore::open_default(now)` (mở = migrate + đưa mục đang bay về `Waiting` + dọn
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
## 10. Đề xuất cải tiến
