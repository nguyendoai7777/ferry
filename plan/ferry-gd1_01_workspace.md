# ferry-gd1_01 — workspace

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ (11/11/2026) |
| Repo | ferry (+ repo gốc xdesktop: `start.sh`, `clean.*`, `init.*`) |
| Model | opus |
| Cỡ | M — ~500 dòng |
| Phụ thuộc | — |
| Quyết định áp dụng | D-2, D-3, D-6, D-13, D-15, D-18, D-19, D-25, W-1, W-2 |
| Phụ thuộc mới | `materiant` + `devtool` (path), `tray-icon` (MIT/Apache), `serde`/`serde_json`, `log`, `interprocess` (MIT/Apache) — soát `cargo deny` |
| File đụng tới | MỚI `Cargo.toml`, `deny.toml`, `rustfmt.toml`, `.gitignore`, `crates/{core,daemon,ui}/…`, `i18n/{vi,en}.toml`, `start.sh`; SỬA gốc `start.sh`, `clean.sh`, `clean.ps1`, `init.sh`, `init.ps1` |

## 1. Làm gì
- Cargo workspace 3 crate theo `CLAUDE.md` mục Bố cục: `ferry-core` (lib), `ferry-daemon` (bin `ferry`), `ferry-ui` (bin `ferry-ui`); khai sẵn mọi `mod` của core với tệp rỗng để task sau không giẫm `lib.rs`.
- `core`: `paths` (dữ liệu / cấu hình / log theo OS), `logging::init("<tiến trình>")`, `i18n` (catalog `vi` + `en` cùng bộ khoá, theo ngôn ngữ hệ thống), `config` (JSON camelCase, ghi nguyên tử, sinh `clientId` UUID lần đầu), `ipc` (JSON một dòng qua named pipe / Unix socket — học `fastcast_core::ipc`).
- `ferry`: một instance, khay rỗng có menu "Mở cửa sổ" / "Thoát"; "Mở cửa sổ" bật `ferry-ui` (đã mở thì đưa lên trước qua IPC).
- `ferry-ui`: cửa sổ trống GPUI + materiant (`materiant::init`, `devtool::init`), tiêu đề tên app; đóng cửa sổ ⇒ tiến trình thoát.
- `start.sh` của repo (bản debug, `--plans` in rỗng) và nối vào `start.sh` gốc (radio + `case`), `clean.*` gốc (thêm repo vào vòng), `init.*` gốc (khi đã có remote — xem mục 8).
- Worktree build được (lối nối `.claude/worktrees/materiant` như groove).

## 2. Không làm trong task này
- Không mạng, không mDNS, không kho khoá, không SQLite (task 02–06).
- Không bản cài (task 14).

## 3. Kiểu dữ liệu · IPC · cấu hình
- `config.json`: `{ "clientId": "<uuid>", "clientName": "<hostname>", "language": "system" }` — mọi trường `#[serde(default)]`.
- IPC (khung, task 07 thêm biến thể): `UiToDaemon::{Hello, Focus}`, `DaemonToUi::{Focus}`.

## 2b. Phân vai
| Phần | Vai |
|---|---|
| Toàn bộ (IPC, vòng sự kiện theo OS, interface giữa crate — mặc định `[opus]`, task nhỏ nên không giao Sonnet) | `[opus]` |

## 6. Nghiệm thu
- C1. `cargo build`, clippy `-D warnings`, `cargo test`, `cargo deny check licenses` xanh. `[opus-run]` — ✅ (+ clippy chéo `--target` Windows / Linux cho `ferry-core` + `ferry-daemon`).
- C2. Hai catalog cùng bộ khoá, không chữ toàn hoa. `[opus-unit]` — ✅ `i18n::tests::{catalogs_have_same_keys_and_placeholders, no_all_caps_words}`.
- C3. Chạy `ferry` hai lần ⇒ lần hai thoát, không có khay thứ hai. `[opus-run]` — ✅ macOS: lần hai gửi `Focus`, thoát 0, daemon mở cửa sổ.
- C4. Khay → "Mở cửa sổ" ⇒ `ferry-ui` mở; đóng cửa sổ ⇒ tiến trình `ferry-ui` hết, `ferry` còn. `[opus-run]` — ✅ macOS (bấm menu khay bằng CGEvent, đóng bằng nút đỏ); khay → "Thoát" ⇒ cả hai tiến trình hết.
- C5. RAM `ferry` lúc rảnh (debug + release) ghi vào mục 9 — mốc cho các task sau. `[opus-run]` — ✅ mục 9.

## 8. Nợ chuyển task khác
- `init.*` gốc cần URL remote: repo hiện chưa có remote (khởi tạo 11/11/2026). Chưa có ⇒ thêm dòng `plan/_backlog.md`, không bịa URL.
- Tên hiển thị của app chưa chốt (`plan/_backlog.md`): dùng tạm "Ferry" qua i18n, một khoá `app.name`.

## 9. Bàn giao thực tế

**Đã làm (11/11/2026, Opus)** — nhánh `claude/gd1_01-workspace`, worktree `ferry-w1`, build `target/w1`.
- Workspace `Cargo.toml` (patch `gpui-pre` của materiant, profile như groove), `deny.toml` / `rustfmt.toml` / `Cargo.lock` lấy
  từ groove làm hạt giống (cùng bản GPUI đã thử).
- `ferry-core`: `paths` (`FERRY_DATA_DIR`, `FERRY_CONFIG_FILE` ghi đè), `logging` (bê fastcast, `FERRY_LOG`), `i18n` (`Language`,
  `LanguagePref::{System,Vi,En}`), `config` (`clientId` UUID v4, `clientName` = tên máy bỏ `.local`, `language`, `devices`;
  ghi nguyên tử tmp + `sync` + rename; tệp hỏng ⇒ `config.json.bad`), `ipc` (bê fastcast, có kiểu theo chiều:
  `ClientConnection` / `ServerConnection`). Mọi module khác khai sẵn tệp rỗng có ghi task chủ: `protocol`, `client`,
  `discovery`, `pairing`, `watch`, `device`, `queue/mod.rs`, `transfer/mod.rs` (thư mục vì task 03 / 10 thêm tệp con).
- Chừa chỗ W-3 (kiểu, không logic): `device::DeviceId`; `config::PairedDevice { id, name, addresses: Vec<String> }`
  (chuỗi để giữ `%iface` của link-local); `queue::BatchId` (UUID v4).
- `ferry`: khoá một-instance, IPC server (luồng accept + mỗi kết nối một luồng ⇒ `Event`), `UiProcess` (bật `ferry-ui` cạnh exe,
  luồng chờ gặt tiến trình), khay `tray-icon` (icon khung điện thoại vẽ bằng mã, macOS template), vòng sự kiện macOS
  (NSApplication Accessory + dispatch main) / Windows (cửa sổ message-only) bê từ fastcast; Linux: kênh chặn, chưa khay.
- `ferry-ui`: GPUI + materiant + devtool, cửa sổ có thanh tiêu đề OS tên app, chữ giữ chỗ `window.placeholder`; luồng IPC
  chuyển vào vòng GPUI bằng `futures::mpsc`; daemon đóng kết nối ⇒ UI thoát.
- `start.sh` của repo (dữ liệu dev riêng `target/dev`); gốc: `start.sh` (radio + `case`), `clean.sh` (`clean.ps1` chỉ bọc
  `clean.sh`, không cần sửa), `CLAUDE.md` gốc. `init.*` gốc: chưa có remote ⇒ dòng backlog sẵn có giữ nguyên.

**Opus chọn 11/11/2026** (không hỏi, D-9):
- IPC thêm `DaemonToUi::{Ok, Error}` ngoài `Focus` của spec: cần để trả lời `Hello` / lệnh; `Hello` mang `role` (`ui` / `cli`).
  `UiToDaemon::Focus` = "mở / đưa cửa sổ lên trước", lần chạy `ferry` thứ hai gửi nó (vai `cli`), daemon trả `Ok`.
- `Language` tiếng Anh khi không đọc được locale; `language: "system"` mặc định như spec.
- `UI` đọc `config.json` (chỉ đọc, `config::load`) để lấy ngôn ngữ; chỉ daemon ghi.
- Thêm `gethostname` (Apache-2.0) cho `clientName`, `uuid` (Apache/MIT) — đã có trong danh sách dự kiến của `CLAUDE.md`.
- Linux chưa khay: không dùng `tray-icon` (GTK + libappindicator LGPL); nợ chuyển gd1_07 (bê SNI của fastcast).
- Không bê mẹo `MallocSpaceEfficient` + `exec` lại của fastcast: footprint đã ~10 MB, để gd1_07 đo lại khi có tokio.

**Lỗi phát sinh đã sửa (D-11)**: socket Unix quá 104 byte khi `FERRY_DATA_DIR` sâu ⇒ `bind` hỏng → `ipc::short_socket_path` rơi về
`<tmp>/ferry-<băm>/daemon.sock` (test `long_socket_path_falls_back_to_temp_dir`). Ghi vào Bẫy của `CLAUDE.md`.

**Soát xanh** (cwd = worktree, `CARGO_TARGET_DIR=…/target/w1`): `cargo build` ✓, `cargo clippy --all-targets -- -D warnings` ✓,
`cargo clippy -p ferry-core -p ferry-daemon --all-targets --target {x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu} -- -D warnings` ✓,
`cargo test` ✓ (core 19, daemon 3), `cargo deny check licenses` ✓.

**Số đo C5 (macOS arm64, rảnh 8 s sau khởi động, chưa mở cửa sổ)**: `ferry` debug — RSS 44,6 MB, `phys_footprint` 10 MB, CPU 0,0 %;
release — RSS 44,7 MB, `phys_footprint` 10 MB, CPU 0,0 %; exe release 1,1 MB. RSS phần lớn là trang AppKit dùng chung — so
bằng `footprint -p <pid>`. Windows / Linux chưa chạy thật (chỉ clippy chéo).

**Chờ kiểm tay** `[owner-qa]`: chạy `bash start.sh ferry` trên Windows (khay chuột trái mở cửa sổ, chuột phải
menu) và Ubuntu (không khay: chạy `ferry` lần hai để mở cửa sổ).

## 10. Đề xuất cải tiến
- [ram] RSS của `ferry` 44 MB trên macOS dù footprint 10 MB — bằng chứng: số đo C5 — đo lại khi có tokio + mDNS (gd1_07 C4), chỉ
  cân mẹo malloc của fastcast nếu footprint vượt ~20 MB — S — P3 (ghi ở gd1_07, không làm bây giờ).
- [bảo mật] IPC: socket 0600 trong thư mục 0700 (Unix), pipe DACL một người + soát exe server (Windows) — đã có, không đề xuất thêm.
- [ux] Cửa sổ trống chỉ có chữ giữ chỗ — màn thật là gd1_08, không áp ở đây.
- [pro] không áp (app chưa có Pro).
