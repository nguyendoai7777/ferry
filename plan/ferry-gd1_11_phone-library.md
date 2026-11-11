# ferry-gd1_11 — phone-library

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | ferry |
| Model | sonnet |
| Cỡ | S — ~350 dòng |
| Phụ thuộc | gd1_08 (`PhoneBrowser`), gd1_10 |
| Quyết định áp dụng | D-13, D-17, W-3, W-4 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/ui/src/views/phone_library.rs`; SỬA `crates/ui/src/views/main_window.rs` (tab), `i18n/{vi,en}.toml` |

## 0. Bàn giao từ ferry-gd1_10 (11/11/2026)

<!-- handoff: ferry-gd1_10 -->

- **Đã có sẵn — IPC (chỉ thêm, biến thể cũ không đổi)**: `UiToDaemon::Library { req, op: LibraryOp::Download { device,
  remote_ids } }` — dây `{"type":"library","req":7,"op":{"op":"download","device":"3f9a","remoteIds":["v1"]}}`;
  `remoteIds` = `LibraryItem.id` lấy từ `LibraryOp::List`. Trả một `Reply` `Outcome::DownloadsAdded { added,
  already_queued, missing }` (`{"kind":"downloads_added","added":1,"alreadyQueued":0,"missing":0}`); lỗi: `Offline`,
  `NotPaired`, `UnknownDevice`, `InvalidInput` (danh sách rỗng), `Unreachable` (`list` hỏng).
- **Snapshot**: `downloads: Vec<DownloadItem>` (`id, deviceId, remoteId, name, remoteDir, size, dest, state, failures,
  created, updated`; `state` là `DownloadState` — `queued / fetching / waiting / paused / done / failed / cancelled`) +
  `downloadsOmitted`; trần `ipc::DOWNLOADS_IN_SNAPSHOT` = 300. `Done` ⇒ `dest` là tệp thật (có thể đã thêm ` (2)`) —
  nút "Mở thư mục" dùng nó. Tiến độ: `progress` / `DaemonToUi::Progress` dùng chung khoá `id` với upload (`sent` = byte
  đã nhận) — `id` không trùng giữa hai chiều.
- **Nút trên mục tải về**: `QueueOp::Item { id, action }` như upload (pause / resume / cancel / retry); `All` gồm cả tải
  về. Huỷ ⇒ xoá `.part`.
- **Đích**: `config.downloadDir` (vắng ⇒ Downloads của OS); tải phẳng, chưa dựng lại `remoteDir` (đề xuất gd1_10 mục 10).
- **Bẫy**: daemon hỏi lại `list` trước khi xếp hàng — mục đã bị xoá trên điện thoại vào `missing`, đừng báo lỗi cả lệnh;
  cùng `remoteId` đang chờ ⇒ `alreadyQueued` (không thêm bản trùng).
- **Soát lại khi làm**: `cargo test -p ferry-core ipc` (mẫu dây), `cargo test -p ferry-daemon download_command`.

## 1. Làm gì (PROTOCOL mục 4.3)
- Tab phụ "Thư viện điện thoại": dùng lại component `PhoneBrowser` của gd1_08 ở **chế độ thư viện** (PROTOCOL 4.6, W-4) —
  không dựng danh sách riêng. Bảng chi tiết dùng `VirtualList` (2000 mục).
- Chọn một / nhiều ⇒ "Tải về máy" ⇒ lệnh `Download`; mục hiện trong hàng đợi chiều ngược lại.
- Không có nút xoá (v1 không có). Offline ⇒ hiện danh sách lần cuối + ghi chú.
- Bổ sung W-3: cây thư mục dựng từ `dir` (`null` ⇒ nhóm "Khác", không làm đích); chọn thư mục rồi thả / dán ⇒ gửi vào đó (PROTOCOL 6.9).

## 6. Nghiệm thu
- C1. Danh sách 2000 mục cuộn mượt (dựng ảo). `[sonnet-test]` + `[owner-qa]`
- C-W3. Danh sách có `dir` lồng + `null` ⇒ cây đúng; dán vào thư mục con ⇒ lô có `dir` đúng. `[sonnet-run]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
