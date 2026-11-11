# ferry-gd1_12 — settings

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | ferry |
| Model | sonnet (autostart theo OS: Opus bê từ fastcast) |
| Cỡ | M — ~500 dòng |
| Phụ thuộc | gd1_07 |
| Quyết định áp dụng | D-13, D-16 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/ui/src/views/settings.rs`, `crates/daemon/src/autostart.rs`; SỬA `crates/core/src/config.rs`, `i18n/{vi,en}.toml` |

## 1. Làm gì (PROTOCOL mục 4.5)
- Thiết bị đã ghép: đổi tên hiển thị, "Quên" (hộp xác nhận nói rõ hàng đợi của máy đó bị xoá).
- Thư mục lưu tệp tải về, thư mục theo dõi (bật / tắt, task 13 dùng).
- Giới hạn băng thông: tắt / số MB/s.
- Khởi động cùng hệ thống (bê `autostart` của fastcast daemon).
- Nâng cao: địa chỉ tay `ip:port` (thêm / xoá, báo lỗi rõ khi `info` không trả lời).
- Ngôn ngữ: theo hệ thống / Tiếng Việt / English.

## 6. Nghiệm thu
- C1. Đổi từng mục ⇒ `config.json` đúng khoá camelCase, mở lại còn. `[sonnet-test]`
- C2. Bật khởi động cùng máy ⇒ đăng nhập lại có khay. `[owner-qa]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
