# media-wireless-sync-gd1_06 — pairing-keyring

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | S — ~350 dòng |
| Phụ thuộc | gd1_02, gd1_05 |
| Quyết định áp dụng | W-1 |
| Phụ thuộc mới | `keyring` (MIT/Apache; Linux qua Secret Service — soát cây phụ thuộc không kéo `libdbus` LGPL, dùng backend thuần Rust nếu có) |
| File đụng tới | MỚI `crates/core/src/pairing.rs`; SỬA `crates/core/src/config.rs` (danh sách thiết bị đã ghép: `id`, tên hiển thị, địa chỉ cuối) |

## 1. Làm gì
- `pair(device, pin)` ⇒ `POST /api/v1/pair` với `clientId` / `clientName` ⇒ token vào kho khoá OS (khoá `media-wireless-sync/<device id>`), thiết bị vào config (không token).
- Sai PIN / `429` trả lỗi có `retry_after` cho UI đếm ngược.
- `forget(device)` ⇒ `DELETE /api/v1/pair` nếu Online (lỗi mạng thì vẫn quên phía PC), xoá token + mục config + hàng đợi của thiết bị (hỏi ở UI).
- `401` / `info.paired == false` ⇒ `NeedsRepair`; ghép lại ⇒ token mới thay token cũ.
- Token kiểu `Secret` (không `Debug` / `Serialize` lộ giá trị), log che.

## 6. Nghiệm thu
- C1. Ghép với `fake_phone` ⇒ token có trong kho khoá (backend giả trong test), không có trong `config.json`, log. `[opus-unit]`
- C2. Server "quên mọi máy" ⇒ `NeedsRepair`; ghép lại ⇒ Online. `[opus-unit]`
- C3. Kho khoá thật trên Windows / macOS / GNOME: ghép, khởi động lại máy, vẫn Online. `[owner-qa]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
