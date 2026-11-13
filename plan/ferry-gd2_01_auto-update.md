# ferry-gd2_01 — Tự cập nhật (như fastcast gd8_03)

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ 13/11/2026 (macOS arm64 chạy thật với server giả; Windows / Linux chờ — mục 8) |
| Repo | ferry (+ crate chung `materiant/updater`, server `cloud` app `ferry` — session điều phối làm) |
| Model | opus |
| Cỡ | M — ~650 dòng mới (không tính test) |
| Phụ thuộc | gd1_14 (đóng gói); `materiant/updater` (master materiant 92004bf, 26bf0f2) |
| Quyết định áp dụng | D-2, D-9, D-12, D-13, D-18, D-22, D-24, D-27 |
| Phụ thuộc mới | `updater` (path, crate nội bộ) ⇒ kéo `ureq` 3 + `rustls` (`ring`) + `rustls-platform-verifier` + `ed25519-dalek` + `sha2` vào daemon — MIT / Apache / ISC, `cargo deny` xanh |
| File đụng tới | `Cargo.toml`; core `ipc.rs`; daemon MỚI `update.rs`, `daemon.rs`, `engine.rs`, `ipc_server.rs`, `tray.rs`, `platform/linux/sni.rs`, `main.rs`, `Cargo.toml`; ui MỚI `update.rs`, `main.rs`, `model.rs`, `daemon_link.rs`, `app_menu.rs`, `views/{side_nav,main_window,settings}.rs`, `assets.rs`, MỚI `assets/icons/ferry-update.svg`, `build.rs`, `Cargo.toml`; `i18n/*.toml`; `packaging/macos/build-macos.sh`, `packaging/windows/{ferry.nsi,build-installer.ps1}`; `ROADMAP.md` |

## 1. Làm gì

- Daemon `ferry` soát server `https://cloud.ferry.sandtechs.com` (D-27) 10 s sau khi bật rồi mỗi 6 giờ (sổ `<data_dir>/update/state.json`).
- Có bản mới ⇒ menu khay thêm "Cập nhật lên v<x>" đứng đầu; cửa sổ có hàng "Cập nhật lên v<x> · ⌘D" trên Cài đặt ở cột trái,
  phím Cmd+D / Ctrl+D; Cài đặt có thẻ Giới thiệu (phiên bản, trạng thái, nút Kiểm tra cập nhật / Cập nhật); menu macOS
  "Kiểm tra cập nhật…".
- Bấm ⇒ daemon tải (cỡ + sha256), giữ release note, thay bản cài, thoát (cửa sổ thoát theo); bản mới tự bật với `--updated`
  = chạy tay ⇒ cửa sổ mở, hộp "Có gì mới" hiện (Enter / Esc / OK đóng).
- Bản major / không tự cài được (dev, `.deb`, `_portable.zip`) ⇒ mở `https://ferry.sandtechs.com`.
- Đóng gói: macOS thêm `ferry_<ver>_macos_<arch>.app.zip` + `.sha256`, in lệnh `publish-release.sh`; Windows NSIS nhận `/UPDATE`
  (cài xong bật lại qua Explorer), `build-installer.ps1` in lệnh publish cho `_setup.exe`.

## 2. Không làm

- Linux `.tar.gz` / cài vào thư mục người dùng: bản `.deb` ở `/usr` cần root ⇒ không tự cài (như fastcast, chỉ mở trang tải).
- Đăng bản thật lên server, deploy server, DNS / Caddy cho `cloud.ferry.sandtechs.com` (chủ dự án).

## 3. IPC

- `UiToDaemon::Update { op: UpdateOp::{Check, Install} }` — không trả lời.
- `Snapshot::update: UpdateView` (`#[serde(default)]`, tag `state`): `idle | checking | up_to_date | available{version, opensPage}
  | downloading{version, percent} | installing{version} | failed{version?}`.
- Nội bộ daemon: `Event::Update(update::Msg)`, `Event::UpdateOp`, `engine::Input::Update(UpdateView)`.

## 6. Case test

| Mã | Case | Phủ |
|---|---|---|
| C1 | Có bản mới ⇒ khay có mục "Cập nhật lên v0.1.1" (vi / en) đứng đầu, bấm ra `MenuAction::Update` | `tray::tests::update_item_leads_the_menu_when_a_version_is_available` ✅ |
| C2 | Trạng thái cập nhật đi vào ảnh chụp cho cửa sổ | `engine::tests::update_state_reaches_the_window_snapshot` ✅, `ipc::tests::update_wire_format` ✅ |
| C3 | Soát lỗi tự động ⇒ im lặng; hỏi tay ⇒ báo; tải ⇒ %; cài lỗi ⇒ giữ nút thử lại; cài xong ⇒ daemon thoát | `update::tests::*` ✅ |
| C4 | Bản dev / major ⇒ không tải, mở trang tải | `update::tests::available_update_shows_a_version_and_dev_builds_open_the_page`, `major_always_opens_the_page` ✅ |
| C5 | macOS arm64: `Ferry.app` 0.1.0 + server giả phát 0.1.1 ⇒ tự thay gói, bật lại 0.1.1 với `--updated`, hộp "Có gì mới" | `[opus-run]` ✅ mục 9 |
| C6 | Windows: `_setup.exe /S /UPDATE` cài đè rồi bật lại (UAC vì bản cài cần admin) | `[owner-qa]` — `_backlog.md` |

## 7. Ghi chú UI

Khoá i18n: `tray.update`, `update.{nav,downloading,installing,failed,about,check,checking,up_to_date,check_failed,available}`,
`whats_new.{title,version,ok}`, `menu.check_updates`. Icon `ferry-update.svg` (Lucide `circle-arrow-up`, ISC — đã có Lucide
trong trang giấy phép). Hàng cột trái: icon màu nhấn, phím tắt mờ ở cuối, ẩn khi không có bản mới.

## 8. Nợ chuyển task khác

- C6 Windows chạy thật — `[owner-qa]`.
- Linux tự cài (cần đường cài lần đầu vào thư mục người dùng) — như fastcast, `_backlog.md`.
- DNS + Caddy cho `cloud.ferry.sandtechs.com`, deploy `cloud` (khoá `secrets/ferry/update-prod-key.json`) — chủ dự án; tới lúc
  đó bản phát hành soát lỗi mạng ⇒ chỉ log `update: check failed`, không báo người dùng.

## 9. Bàn giao thực tế (13/11/2026)

**Đã làm** (nhánh `claude/ferry-gd2_01`, gộp `master`): như mục 1. Phần chung (manifest ký, sổ, tải, cài, release note) ở
`materiant/updater`; ferry chỉ có lịch + trạng thái (`daemon/src/update.rs`) và lớp vẽ (`ui/src/update.rs`, cột trái, Cài đặt).

**Opus chọn** (D-9):
- Soát / tải / cài ở **daemon**, không ở `ferry-ui`: luật "UI không gọi mạng"; mục khay cần biết bản mới cả khi cửa sổ đóng.
  Trạng thái đi `Snapshot::update` (engine giữ bản sao, luồng chính báo qua `Input::Update`), lệnh đi `UiToDaemon::Update`.
- Luồng `update-timer` chỉ ngủ (10 s / hạn 6 giờ tính từ sổ), mỗi lần soát / cài một luồng std riêng (hàm chặn của `updater`);
  không đụng runtime tokio của engine.
- Cài xong daemon thoát (`Daemon::handle` trả `false`), cửa sổ thấy IPC đóng thì thoát theo; script của `updater` chờ pid daemon
  thoát rồi `open -a Ferry.app --args --updated`. `--updated` không phải `--autostart` ⇒ daemon mới mở cửa sổ, cửa sổ đọc sổ
  `last-version` ⇒ hộp "Có gì mới" (note của manifest, `updater::notes::save_pending`).
- Bản dev = `cfg!(debug_assertions)` (như groove): tin khoá dev, nhận `http://`, đọc `FERRY_CLOUD_URL` /
  `FERRY_UPDATE_AUTOINSTALL=1`. Gói `.app` thử: `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` (bỏ feature riêng định làm lúc đầu).
  Bản này cũng dùng tên tự chạy / kho khoá `ferry.dev` ⇒ chạy thử không đụng mục của bản cài.
- Hàng cập nhật ở cột trái ngay trên Cài đặt (ferry không có thanh trên chứa chỗ như launcher fastcast), icon màu nhấn + phím mờ;
  Cài đặt thêm thẻ Giới thiệu; menu macOS "Kiểm tra cập nhật…" ⇒ mở Cài đặt + hỏi ngay.
- Tự soát lỗi mạng ⇒ im lặng (giữ bản đang chờ nếu có); hỏi tay lỗi ⇒ "Không kiểm tra được…"; cài lỗi ⇒ giữ hàng để thử lại.
- `Incoming::Message` của `daemon_link` thành `Box<DaemonToUi>` (clippy `large_enum_variant` khi `Snapshot` thêm trường).
- Lỗi có sẵn sửa luôn (D-11): `packaging/licenses/gen-licenses.mjs` đòi crate `notify` (đã gỡ cùng watch-folder 12/11/2026) ⇒
  `build-macos.sh` chết ở trang giấy phép; đổi thành `notify-rust` + thêm `ureq`.

**C5 chạy thật** (macOS arm64, 13/11/2026): `Ferry.app` 0.1.0 và 0.1.1 (version nâng tạm, không commit) build bằng
`build-macos.sh` với debug-assertions, đặt ở scratchpad (ngoài `target/` — `Layout::detect` bỏ đường có `target`); dữ liệu thử
`FERRY_DATA_DIR` / `FERRY_CONFIG_FILE` riêng, sổ `last-version` = 0.1.0 (giả lập đã dùng bản cũ); server giả
`materiant/updater/tools/fake-server.mjs` phát `ferry_0.1.1_macos_arm64.app.zip` (11,3 MB). Log:

```
15:50:49 ferry 0.1.0 starting (pid 23164, autostart: true)
15:50:59 update: 0.1.1 available (Patch) → FERRY_UPDATE_AUTOINSTALL=1, installing now → downloading 0.1.1
15:50:59 update: 0.1.1 installed, quitting so the new version starts → daemon: shutting down → ferry stopped
15:51:00 ferry 0.1.1 starting (pid 23240, autostart: false) → ui: started pid 23243
15:51:00 ferry-ui 0.1.1 starting → whats new: showing 1 note(s) for 0.1.1 → first frame in 12.4 ms
```

Sau đó: `Info.plist` của gói = 0.1.1, `last-version` = 0.1.1, `notes.json` đã xoá, tệp cài đã dọn (còn `state.json`); log của bản
mới vẫn ở thư mục thử (`open --env` chuyển `FERRY_*`). Daemon mới không hỏi lại ngay (sổ vừa soát ⇒ hạn 6 giờ). RSS daemon (bản
release + debug-assertions, macOS): 0.1.0 trước lần soát 48 MB, 0.1.1 sau khi bật lại 46 MB; chưa đo bản không `updater` cùng
cấu hình (ureq + rustls chỉ chạy lúc soát, không có luồng thường trực ngoài `update-timer` đang ngủ).

**Chưa làm / chờ**: mục 8.
