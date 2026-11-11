# ferry-gd1_12 — settings

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ (11/11/2026) |
| Repo | ferry |
| Model | opus điều phối: autostart + IPC + engine [opus], màn cài đặt [sonnet] |
| Cỡ | M — ~500 dòng |
| Phụ thuộc | gd1_07 |
| Quyết định áp dụng | D-13, D-16 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/daemon/src/autostart.rs`, `crates/ui/src/settings_link.rs`, `crates/ui/src/views/{mod,settings,title_bar}.rs`; SỬA `crates/core/src/{config,device,ipc}.rs`, `crates/daemon/src/{main,engine,ipc_server,daemon,tray,tray_state,ui_process}.rs`, `crates/ui/src/{main,daemon_link}.rs`, `i18n/{vi,en}.toml` |

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

**Kết quả.** Nhánh `claude/gd1_12-settings` (4 + 1 commit `ferry-gd1_12: …`), rebase lên master sau gd1_08…11. Soát xanh
(cwd = worktree `ferry-w1`): `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (core 122, daemon 30,
ui 23 + test tích hợp), `cargo deny check licenses`, clippy chéo `x86_64-pc-windows-msvc` cho core + daemon.

**Case.** C1, C1b, C1c `[opus-unit]` xanh (`engine::tests::c1_settings_are_applied_and_saved`, `menu_follows_the_language`,
`autostart::tests::*`). C1d `[sonnet-test]` xanh (`parse_speed`, `error_key`, `language_index`, mọi khoá `settings.*` có ở vi + en)
+ `[opus-run]`: GNOME Shell headless (màn ảo 1280×800, không chạm màn thật) chụp màn cài đặt vi / en và cửa sổ chính —
bố cục, font, chữ có dấu, thanh tiêu đề đúng; mở rồi đóng không đổi `config.json` (daemon chỉ ghi khi giá trị khác).
**C2 `[owner-qa]` chờ kiểm tay** (ghi `_backlog.md`).

**Quyết định (Opus chọn 11/11/2026).**
- Một thư mục theo dõi: `watchFolder: {enabled, path?}` — PROTOCOL 4.5 chỉ nói "thư mục theo dõi"; nhiều thư mục để `[pro]`.
- 1 MB/s = 1 048 576 byte/s (khớp `Throttle` sẵn có), 1…10 000; tắt = `null`.
- Địa chỉ tay lưu `manualAddresses`, daemon thử `info` lúc khởi động và khi thêm (không trả lời ⇒ `Unreachable`, không lưu).
- Đổi tên = bí danh `devices[].alias` phía PC (không gửi sang điện thoại; `info` đổi `name` vẫn giữ bí danh).
- Cài đặt là **cửa sổ riêng** (không phải trang trong cửa sổ chính): mở từ khay "Cài đặt…" (daemon gửi `OpenSettings` cho UI
  đang chạy, chưa có UI thì bật `ferry-ui --settings`) và từ nút bánh răng ở đầu cửa sổ chính. Kết nối IPC riêng vai `Cli`.
- Autostart bê fastcast: Windows Task Scheduler, Linux `~/.config/autostart/ferry.desktop`, macOS LaunchAgent; bản debug
  ghi mục `ferry.dev` để không đè bản cài; không trễ khởi động trên Windows. `ferry --autostart` khởi động im lặng
  (không mở cửa sổ). Daemon đồng bộ mục hệ thống với `config.autostart` lúc khởi động.
- Kiểm thư mục tải về (tạo + thử ghi) chạy `spawn_blocking`, không chặn vòng engine.
- Đổi ngôn ngữ: khay + thông báo đổi ngay; cửa sổ đang mở (chính, cài đặt) đổi khi mở lại — chưa có kênh đẩy ngôn ngữ vào UI.
- Font nhúng Mona Sans (D-20, feature `font-mona-sans` của materiant): font hệ thống Ubuntu Sans đặt lệch dấu tiếng Việt.
- `MB` thêm vào danh sách viết tắt của test chữ hoa (`i18n.rs` ACRONYMS) cho "MB/s".
- GNOME Wayland không cấp viền ⇒ `views/title_bar.rs` tự vẽ thanh tiêu đề (kéo, nút đóng) khi `Decorations::Client`; dùng
  cho cả cửa sổ chính (gd1_08 trước đó không có thanh tiêu đề trên GNOME — lỗi phát sinh, sửa luôn theo D-11) và cài đặt.
- Sonnet chọn: đổi tên qua hộp thoại (không sửa tại chỗ), số MB/s gửi sau 700 ms ngừng gõ, bật "Theo dõi thư mục" khi
  chưa có đường dẫn ⇒ mở hộp chọn thư mục trước.
- Sau rebase: bộ thử engine của gd1_10 đặt sẵn `downloadDir` ⇒ sửa giả định đầu của test C1.

**Chưa soát.** Theme tối: GNOME headless không có portal giao diện nên GPUI luôn sáng — chưa nhìn bản tối.

## 10. Đề xuất cải tiến

- [ux] Cửa sổ đang mở không đổi ngôn ngữ ngay — `settings.rs` / `main_window.rs` giữ `Rc<I18n>` nạp lúc mở — thêm
  `DaemonToUi::Language` (hoặc trường trong snapshot) để UI nạp lại catalog — đỡ phải mở lại cửa sổ — S — P3.
- [ux] Chữ gợi ý ô thả ở cửa sổ chính còn "Chỉ gửi tệp video." trong khi W-5 cho đẩy tệp bất kỳ — `i18n/vi.toml` khoá của
  `drop_zone` — thuộc làn gd1_08, sửa khi chủ dự án chốt phạm vi tệp — S — P2.
- [pro] Lịch giới hạn tốc độ (giờ làm việc / ban đêm) và nhiều thư mục theo dõi — chỗ hợp đặt huy hiệu Pro (D-4) — M — P3.
- [bảo mật] Đường dẫn thư mục tải về / theo dõi đến từ IPC: daemon chỉ nhận đường tuyệt đối và tự thử ghi; IPC chỉ nghe
  socket của người dùng hiện tại — không cần đổi.
- [ram] Không áp: cửa sổ cài đặt là tiến trình UI, đóng là nhả; daemon chỉ thêm vài trường config.
