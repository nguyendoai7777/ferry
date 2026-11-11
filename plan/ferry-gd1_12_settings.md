# ferry-gd1_12 — settings

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | 🔄 (11/11/2026) |
| Repo | ferry |
| Model | opus điều phối: autostart + IPC + engine [opus], màn cài đặt [sonnet] |
| Cỡ | M — ~500 dòng |
| Phụ thuộc | gd1_07 |
| Quyết định áp dụng | D-13, D-16 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/daemon/src/autostart.rs`, `crates/ui/src/settings_link.rs`, `crates/ui/src/views/{mod,settings}.rs`; SỬA `crates/core/src/{config,device,ipc}.rs`, `crates/daemon/src/{main,engine,ipc_server,daemon,tray,tray_state,ui_process}.rs`, `crates/ui/src/{main,daemon_link}.rs`, `i18n/{vi,en}.toml` |

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

## 2b. Phân vai

| Phần việc | Ai |
| --- | --- |
| Khoá config mới, bí danh thiết bị (`device.rs`), IPC `Settings` / `OpenSettings` / `NotWritable`, engine áp + lưu, địa chỉ tay lúc khởi động, đổi ngôn ngữ khay | [opus] |
| `autostart.rs` (Task Scheduler / XDG / LaunchAgent, bê fastcast), `ferry --autostart`, menu khay "Cài đặt…", `ferry-ui --settings`, `settings_link.rs` | [opus] |
| Cửa sổ cài đặt `views/settings.rs` + khoá i18n `settings.*` | [sonnet] |
| C2 đăng nhập lại có khay | [owner-qa] |

## 3. Kiểu dữ liệu · IPC · cấu hình

- `config.json` thêm (mọi trường có mặc định, vắng không ghi): `speedLimitMbPerSec` (1 MB = 1 048 576 byte, 1…10 000),
  `autostart` (bool), `watchFolder: {enabled, path?}` (gd1_13 đọc), `manualAddresses: [..]`, `devices[].alias`.
  `downloadDir` (gd1_10) giữ nguyên.
- IPC (`ferry_core::ipc`, 4 biến thể mới): `UiToDaemon::Settings{req, op: SettingsOp}` — `get`, `set_language`,
  `set_download_dir{path?}`, `set_speed_limit{mbPerSec?}`, `set_autostart{enabled}`, `set_watch_folder{folder}`,
  `rename_device{device, name}`, `add_address{address}`, `remove_address{address}`; mọi lệnh trả
  `Outcome::Settings(SettingsView)` (cài đặt sau khi áp). `DaemonToUi::OpenSettings`. `CommandError::NotWritable`.
  Quên điện thoại dùng `PhoneOp::Forget` có sẵn.
- UI gọi qua `settings_link::SettingsClient` (kết nối vai `Cli` riêng, không dùng kho của cửa sổ chính).

## 6. Nghiệm thu
- C1. Cho daemon chạy; khi đổi từng mục qua IPC; thì `config.json` đúng khoá camelCase, đọc lại còn; đường dẫn tương đối
  ⇒ `InvalidInput`, thư mục không ghi được ⇒ `NotWritable`, thư mục theo dõi không có ⇒ `InvalidInput`, địa chỉ
  không trả lời ⇒ `Unreachable`. `[opus-unit]` (`config::tests::settings_keys_are_camel_case_and_round_trip`,
  `engine::tests::c1_settings_are_applied_and_saved`)
- C1b. Bí danh sống qua `info` (điện thoại đổi tên) và lưu / nạp config. `[opus-unit]`
- C1c. Đổi ngôn ngữ ⇒ khay (menu + tooltip) đổi ngay. `[opus-unit]` + `[opus-run]`
- C1d. Màn cài đặt: mọi mục đọc từ `get`, đổi ⇒ gửi lệnh, lỗi hiện chữ nói việc cần làm; quên điện thoại có hộp xác nhận. `[sonnet-test]` + `[opus-run]`
- C2. Bật khởi động cùng máy ⇒ đăng nhập lại có khay. `[owner-qa]` (bản cài; bản debug ghi mục `ferry.dev`)

## 7. Ghi chú UI

Cửa sổ riêng (`views::settings::open`), 560×640, tối thiểu 480×420, tiêu đề "Cài đặt — Ferry". Mở từ menu khay "Cài đặt…"
(daemon gửi `OpenSettings` hoặc bật `ferry-ui --settings`); gd1_08 thêm nút bánh răng gọi cùng hàm.

```
┌ ScrollArea (D-16: padding ngang trong nội dung) ─────────────┐
│ Điện thoại đã ghép                        (CardGroup)        │
│  [Máy của mẹ · Pixel 8]   [Đổi tên] [Quên]                   │
│ Tệp                                                          │
│  Thư mục tải về    /home/u/Downloads  [Chọn…] [Mặc định]     │
│  Theo dõi thư mục  [switch]  /path   [Chọn…]                 │
│ Mạng                                                         │
│  Giới hạn tốc độ   [switch]  [NumberField] MB/s              │
│ Hệ thống                                                     │
│  Khởi động cùng hệ thống [switch]   (lỗi dưới, màu error)    │
│  Ngôn ngữ  [Select: Theo hệ thống / Tiếng Việt / English]    │
│ Nâng cao                                                     │
│  Địa chỉ tay  [TextField ip:port] [Thêm]                     │
│   192.168.1.9:8080  [Xoá]                                    │
└──────────────────────────────────────────────────────────────┘
```

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
