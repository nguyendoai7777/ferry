# media-wireless-sync-gd1_01 — workspace

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync (+ repo gốc xdesktop: `start.sh`, `clean.*`, `init.*`) |
| Model | opus |
| Cỡ | M — ~500 dòng |
| Phụ thuộc | — |
| Quyết định áp dụng | D-2, D-3, D-6, D-13, D-15, D-18, D-19, D-25, W-1, W-2 |
| Phụ thuộc mới | `materiant` + `devtool` (path), `tray-icon` (MIT/Apache), `serde`/`serde_json`, `log`, `interprocess` (MIT/Apache) — soát `cargo deny` |
| File đụng tới | MỚI `Cargo.toml`, `deny.toml`, `rustfmt.toml`, `.gitignore`, `crates/{core,daemon,ui}/…`, `i18n/{vi,en}.toml`, `start.sh`; SỬA gốc `start.sh`, `clean.sh`, `clean.ps1`, `init.sh`, `init.ps1` |

## 1. Làm gì
- Cargo workspace 3 crate theo `CLAUDE.md` mục Bố cục: `mws-core` (lib), `mws-daemon` (bin `mws`), `mws-ui` (bin `mws-ui`); khai sẵn mọi `mod` của core với tệp rỗng để task sau không giẫm `lib.rs`.
- `core`: `paths` (dữ liệu / cấu hình / log theo OS), `logging::init("<tiến trình>")`, `i18n` (catalog `vi` + `en` cùng bộ khoá, theo ngôn ngữ hệ thống), `config` (JSON camelCase, ghi nguyên tử, sinh `clientId` UUID lần đầu), `ipc` (JSON một dòng qua named pipe / Unix socket — học `fastcast_core::ipc`).
- `mws`: một instance, khay rỗng có menu "Mở cửa sổ" / "Thoát"; "Mở cửa sổ" bật `mws-ui` (đã mở thì đưa lên trước qua IPC).
- `mws-ui`: cửa sổ trống GPUI + materiant (`materiant::init`, `devtool::init`), tiêu đề tên app; đóng cửa sổ ⇒ tiến trình thoát.
- `start.sh` của repo (bản debug, `--plans` in rỗng) và nối vào `start.sh` gốc (radio + `case`), `clean.*` gốc (thêm repo vào vòng), `init.*` gốc (khi đã có remote — xem mục 8).
- Worktree build được (lối nối `.claude/worktrees/materiant` như groove).

## 2. Không làm trong task này
- Không mạng, không mDNS, không kho khoá, không SQLite (task 02–06).
- Không bản cài (task 14).

## 3. Kiểu dữ liệu · IPC · cấu hình
- `config.json`: `{ "clientId": "<uuid>", "clientName": "<hostname>", "language": "system" }` — mọi trường `#[serde(default)]`.
- IPC (khung, task 07 thêm biến thể): `UiToDaemon::{Hello, Focus}`, `DaemonToUi::{Focus}`.

## 6. Nghiệm thu
- C1. `cargo build`, clippy `-D warnings`, `cargo test`, `cargo deny check licenses` xanh. `[opus-run]`
- C2. Hai catalog cùng bộ khoá, không chữ toàn hoa. `[opus-unit]`
- C3. Chạy `mws` hai lần ⇒ lần hai thoát, không có khay thứ hai. `[opus-run]`
- C4. Khay → "Mở cửa sổ" ⇒ `mws-ui` mở; đóng cửa sổ ⇒ tiến trình `mws-ui` hết, `mws` còn. `[opus-run]`
- C5. RAM `mws` lúc rảnh (debug + release) ghi vào mục 9 — mốc cho các task sau. `[opus-run]`

## 8. Nợ chuyển task khác
- `init.*` gốc cần URL remote: repo hiện chưa có remote (khởi tạo 11/11/2026). Chưa có ⇒ thêm dòng `plan/_backlog.md`, không bịa URL.
- Tên hiển thị của app chưa chốt (`plan/_backlog.md`): dùng tạm "Media Wireless Sync" qua i18n, một khoá `app.name`.

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
