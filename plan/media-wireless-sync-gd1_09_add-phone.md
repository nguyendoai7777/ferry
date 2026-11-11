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

## 0. Bàn giao từ media-wireless-sync-gd1_06 (11/11/2026)

<!-- handoff: media-wireless-sync-gd1_06 -->

- Lỗi ghép (qua IPC của gd1_07, gốc `pairing::PairError`) ⇒ chữ nói việc cần làm: `WrongPin` "PIN không đúng, xem lại trên
  điện thoại"; `RateLimited{retryAfter}` đếm ngược rồi mới cho nhập (thiếu `Retry-After` ⇒ 60 s); `OtherPhone` "Địa chỉ này
  giờ là điện thoại khác"; `Keyring(Unavailable)` "Cần kho khoá của hệ thống (GNOME Keyring / KWallet) để lưu ghép đôi";
  mạng ⇒ "Không nối được điện thoại — bật Sync WiFi". PIN không giữ lại sau khi gửi.

## 1. Làm gì (PROTOCOL mục 4.4)
- Bước 1: hướng dẫn "Trên điện thoại: mở app → menu → Sync WiFi → Bật".
- Bước 2: danh sách điện thoại dò thấy chưa ghép (từ snapshot), cập nhật trực tiếp; trống thì gợi ý cùng WiFi + liên kết "Nhập địa chỉ tay" (nâng cao).
- Bước 3: ô PIN 6 số (chỉ số, dán được), gửi `Pair`; sai ⇒ báo lại; `429` ⇒ đếm ngược `retry_after`; xong ⇒ về cửa sổ chính với máy vừa ghép được chọn.
- (Đề xuất gd1_05) Thiết bị có `Device::newer_api` (TXT / `info.api` lớn hơn bản PC hiểu) ⇒ dòng phụ "Cập nhật app trên
  máy tính để dùng điện thoại này" (i18n), vẫn cho ghép. Ô "Nhập địa chỉ tay" nhận mọi dạng của
  `discovery::address::parse_address` (`1.2.3.4:8080`, `[fd12::5]:8080`, `[fe80::5%en0]:8080`, cả URL màn Sync hiện);
  lỗi `UnknownInterface` ⇒ "Không thấy card mạng <tên> trên máy tính".

## 6. Nghiệm thu
- C1. Ô PIN nhận 6 số, từ chối chữ, dán "123 456" thành 123456. `[sonnet-test]`
- C2. Ghép với điện thoại thật / emulator theo PROTOCOL mục 7. `[owner-qa]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
