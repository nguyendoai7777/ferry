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

## 0. Bàn giao từ ferry-gd1_07 (11/11/2026)
<!-- handoff: ferry-gd1_07 -->

- `EngineConfig` mang `config_path`, `config`, `throttle: Arc<Throttle>` (giới hạn tốc độ dùng chung mọi runner — đổi lúc chạy qua `Throttle` là có hiệu lực ngay) và `i18n`. Engine chỉ ghi `config.json` khi danh sách thiết bị khác (`save_config`); cài đặt cần thêm lệnh IPC thì thêm `SettingsOp` dưới một biến thể `Settings{req, op}` (giống `Queue` / `Phone` / `Library`) và cho engine ghi config tuần tự trong `select!` — tránh hai nơi ghi.
- Đổi ngôn ngữ: chữ khay / thông báo lấy từ `i18n` của engine + `Tray::apply(&status, &i18n)`; cần đẩy `I18n` mới vào engine và luồng khay (hiện nạp một lần lúc khởi động).
- Tự chạy cùng máy (autostart) chưa làm ở gd1_07.

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
