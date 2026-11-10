# media-wireless-sync-gd1_09 — add-phone

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | sonnet |
| Cỡ | S — ~350 dòng |
| Phụ thuộc | gd1_07 |
| Quyết định áp dụng | D-13 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/ui/src/views/add_phone.rs`; SỬA `i18n/{vi,en}.toml` |

## 1. Làm gì (PROTOCOL mục 4.4)
- Bước 1: hướng dẫn "Trên điện thoại: mở app → menu → Sync WiFi → Bật".
- Bước 2: danh sách điện thoại dò thấy chưa ghép (từ snapshot), cập nhật trực tiếp; trống thì gợi ý cùng WiFi + liên kết "Nhập địa chỉ tay" (nâng cao).
- Bước 3: ô PIN 6 số (chỉ số, dán được), gửi `Pair`; sai ⇒ báo lại; `429` ⇒ đếm ngược `retry_after`; xong ⇒ về cửa sổ chính với máy vừa ghép được chọn.

## 6. Nghiệm thu
- C1. Ô PIN nhận 6 số, từ chối chữ, dán "123 456" thành 123456. `[sonnet-test]`
- C2. Ghép với điện thoại thật / emulator theo PROTOCOL mục 7. `[owner-qa]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
