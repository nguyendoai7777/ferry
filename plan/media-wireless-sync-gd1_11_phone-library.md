# media-wireless-sync-gd1_11 — phone-library

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | sonnet |
| Cỡ | S — ~350 dòng |
| Phụ thuộc | gd1_08 (`PhoneBrowser`), gd1_10 |
| Quyết định áp dụng | D-13, D-17, W-3, W-4 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/ui/src/views/phone_library.rs`; SỬA `crates/ui/src/views/main_window.rs` (tab), `i18n/{vi,en}.toml` |

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
