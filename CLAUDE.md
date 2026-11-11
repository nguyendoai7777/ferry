# ferry — app PC gửi video sang điện thoại qua WiFi

Repo git riêng trong workspace `xdesktop` (luật chung, quyết định D-* / W-*: `../CLAUDE.md`, `../.claude/rules/decisions/`).
App khay hệ thống nối PC với app điện thoại `video-player` qua WiFi nhà: thả video vào là chảy sang điện thoại, đứt thì
nối tiếp từ byte cuối, xong thì im lặng; kéo video từ điện thoại về là phụ. Lộ trình: `ROADMAP.md`. Task:
`plan/ferry-gd<N>_*.md` (skill `run-task`), việc treo `plan/_backlog.md`. Skill nạp ngữ cảnh: `/ferry`.

**`PROTOCOL.md` là hợp đồng với điện thoại** (W-1), không phải tài liệu thường: triết lý (mục 2), màn hình (4), máy trạng
thái (5), giao thức HTTP v1 (6). Mã lõi trỏ về mục của nó (`// PROTOCOL 6.5`). Đổi giao thức ⇒ sửa `PROTOCOL.md` trước,
rồi sửa cả hai phía; không tự thêm route, header, mã trạng thái ở phía PC.

**GĐ1 xong mã** (11/11/2026, gd1_01 → gd1_14; chờ chủ dự án kiểm tay rồi `close-phase`): `ROADMAP.md`.

## Bố cục (cargo workspace)

Tách hai tiến trình như fastcast (`fst` / `fastcast-ui`) vì luật "RAM là tính năng": app chạy nền cả ngày ở khay, cửa sổ
chỉ mở vài lần.

| Crate | Loại | Việc | Không được mang |
| --- | --- | --- | --- |
| `crates/core` — `ferry-core` | lib | `protocol` (kiểu JSON + header §6), `client` (HTTP §6.3–6.5), `discovery` (mDNS §6.2 + địa chỉ tay), `pairing` (ghép + kho khoá), `device` (máy trạng thái thiết bị §5), `queue` (hàng đợi bền + máy trạng thái mục §5), `transfer` (chạy hàng đợi: upload nối tiếp, tải về `Range`), `watch` (thư mục theo dõi), `config`, `paths`, `i18n`, `logging`, `ipc` | UI, GPU |
| `crates/daemon` — bin `ferry` | bin | runtime tokio của lõi, khay (3 trạng thái), IPC server, bật tiến trình UI, một instance, thông báo hệ thống, autostart | GPUI, materiant |
| `crates/ui` — bin `ferry-ui` | bin | cửa sổ chính (vùng thả + hàng đợi), thư viện điện thoại, thêm điện thoại, cài đặt — GPUI + materiant; chỉ vẽ ảnh chụp trạng thái nhận qua IPC và gửi lệnh | mạng, kho khoá, SQLite |
| `crates/core/tests/fake_phone` | test | server giả trong tiến trình đúng hợp đồng §6 (đứt giữa khúc, `409`, `404`, `507`, `401`, `423`, `503`) | — |

- UI không gọi mạng: mọi lệnh đi IPC sang `ferry`, `ferry` gửi ảnh chụp trạng thái (snapshot) khi đổi, không chia sẻ mutex.
- Đóng cửa sổ ⇒ tiến trình UI thoát (nhả GPU / RAM); truyền vẫn chạy ở `ferry`.

## Luật riêng

- **Không đọc cả tệp vào RAM**: upload stream từng khúc từ đĩa (khúc 32 MB, §6.5), tải về ghi thẳng ra tệp `.part`.
- **HTTP mỗi request một kết nối** (§6.1): client tắt pool (`pool_max_idle_per_host(0)`), không TLS. Quyết định theo mã
  trạng thái, thân lỗi chỉ để log.
- **Token chỉ ở kho khoá OS** (§2.2): không ghi vào `config.json`, log, IPC. Log che token và PIN.
- **Thiết bị nhận theo `id`** (TXT `id=`), không theo IP; IP đổi ⇒ cập nhật, không hỏi.
- **Một tệp một lúc cho mỗi điện thoại** (§2.8); nhiều điện thoại thì mỗi máy một hàng đợi tuần tự.
- **Chữ người dùng**: "Gửi sang điện thoại" / "Tải về máy", không "upload / sync"; lỗi nói việc cần làm, không nói mã HTTP
  (§2.6). Qua i18n `vi` + `en`, viết hoa chữ cái đầu (D-13).
- **Rảnh không giữ CPU**: không vòng lặp bận, không poll dày; mDNS và đồng hồ backoff là nguồn đánh thức.
- Phụ thuộc dự kiến (soát `cargo deny` khi thêm): `tokio`, `reqwest` (`default-features = false`, không TLS), `mdns-sd`,
  `keyring`, `rusqlite` (bundled), `notify`, `uuid`, `serde`; khay `tray-icon` như fastcast (Linux: SNI tự viết của fastcast).

## Lệnh

```bash
bash start.sh                      # bản debug: build ferry + ferry-ui, chạy ferry (khay); theo dõi mã, --no-watch chạy một lần
bash ../start.sh ferry   # cùng việc, gọi từ gốc workspace
bash build.sh [-f] [--draft]       # = bash ../build.sh --project ferry: bản cài máy đang chạy vào ../release/ferry (Windows: pwsh build.ps1 [-Package setup,portable]; macOS: --arch arm64|x64)
node packaging/icons/gen-icons.mjs # vẽ lại icon app (png hicolor, .ico, .icns — kết quả đã commit)
```

- **Đóng gói** (`packaging/`, gd1_14): `linux/build-deb.sh` (`/usr/bin/{ferry,ferry-ui}`, `ferry.desktop` khớp `app_id`
  `ferry`), `macos/build-macos.sh` (`Ferry.app` hai exe, `Info.plist` khai `NSBonjourServices` `_vpsync._tcp` +
  `NSLocalNetworkUsageDescription`, ký ad-hoc), `windows/ferry.nsi` (cài cả máy, cần admin vì luật tường lửa mDNS UDP 5353 vào
  cho `ferry.exe`; gỡ cài xoá luật + tác vụ tự chạy `ferry-<user>`). Trang giấy phép `licenses/gen-licenses.mjs` (Node, cả ba
  OS; `cargo deny` đỏ thì dừng; thêm tay SQLite, Lucide, Mona Sans). Test `crates/core/tests/packaging.rs` giữ tệp đóng gói
  khớp mã (tên exe, `app_id`, dịch vụ mDNS, tên tác vụ). Release note theo tag: `release-notes/v<version>.md` (D-22,
  khuôn `release-notes/TEMPLATE.md`).

- Build: `CARGO_TARGET_DIR=<gốc>/ferry/target/w<N>` trong worktree (D-18); `start.sh` tự đặt.
- Dữ liệu dev riêng (`start.sh`): `FERRY_DATA_DIR` (log, khoá một-instance, socket), `FERRY_CONFIG_FILE` — trỏ vào
  `target/dev`. Mức log `FERRY_LOG=debug`, bản release ghi stderr khi có `FERRY_LOG_STDERR`.
- Soát xanh mỗi task: `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo deny check licenses`.
  Đổi mã theo OS ⇒ thêm `cargo clippy -p ferry-core -p ferry-daemon --all-targets --target x86_64-pc-windows-msvc` (và
  `x86_64-unknown-linux-gnu`) — không cần máy kia, chỉ cần `rustup target add`; đặt `LIBSQLITE3_SYS_USE_PKG_CONFIG=1`
  cho lệnh chéo (bẫy SQLite bên dưới).
- `ferry` chạy tay ⇒ mở cửa sổ ngay (`--autostart` thì chỉ nằm ở khay); lần hai ⇒ IPC `Focus` cho daemon đang chạy (mở cửa sổ) rồi thoát. `ferry-ui` chạy thẳng không có daemon vẫn mở cửa sổ.

Thử không cần điện thoại thật: PROTOCOL mục 7 (emulator + `adb forward tcp:18080 tcp:8080`, nhập tay `127.0.0.1:18080`).

## Bẫy

- **Socket Unix ≤ 104 byte** (macOS; Linux 108): `FERRY_DATA_DIR` sâu làm `bind` hỏng ("exceeds capacity of sun_path").
  `ipc::Endpoint::daemon` tự rơi về `<tmp>/ferry-<băm>/daemon.sock` khi đường dẫn quá 100 byte — đừng gỡ.
- **Khay Linux là SNI tự viết** (`platform/linux/sni.rs`, bê từ fastcast, gd1_07): `zbus` chạy trên runtime engine,
  không GTK / `tray-icon`. GNOME cần extension AppIndicator (`ubuntu-appindicators`); thiếu host thì chỉ log, mở cửa sổ
  bằng cách chạy `ferry` lần nữa. Icon Linux gần trắng vì thanh GNOME tối.
- **Engine một task** (`daemon/src/engine.rs`, gd1_07): mọi trạng thái (thiết bị, runner, UI nghe) xử lý tuần tự trong một
  `select!`; việc chậm (ghép, `plan_batch`, dọn sau quên) chạy task con / `spawn_blocking` rồi trả `Internal` — đừng
  `await` mạng trong vòng chính. Đẩy ra UI chỉ qua `Outbox::push`; `Snapshot` có trần mục (`QUEUED_IN_SNAPSHOT`,
  `FINISHED_IN_SNAPSHOT`) để dòng JSON < `MAX_LINE` 1 MiB — thêm trường mảng thì giữ trần.
- **Clippy chéo macOS không chạy từ Linux**: `notify-rust` kéo `mac-notification-sys` biên dịch Objective-C. Soát macOS trên
  máy Mac; Windows chéo vẫn chạy được.
- macOS: thử menu khay tự động bằng AppleScript không được (status item không lộ menu qua AX) ⇒ bấm bằng sự kiện chuột
  `CGEvent` (vị trí icon: `position of menu bar item 1 of menu bar 1` của tiến trình `ferry`).
- **Clippy chéo hỏng ở `libsqlite3-sys`** (`rusqlite` `bundled`, gd1_03): build script biên dịch C SQLite cho target
  kia ⇒ thiếu header (`stdlib.h` not found). Clippy chỉ soát mã Rust nên tắt biên dịch C bằng
  `LIBSQLITE3_SYS_USE_PKG_CONFIG=1 cargo clippy … --target …`. Không đặt biến này cho build thật.
- **Hàng đợi** (`queue`, gd1_03): `<data_dir>/queue.sqlite3`, WAL. Đổi schema ⇒ thêm phần tử cuối vào `MIGRATIONS`
  của `queue/store.rs`, không sửa bản cũ; DB của bản mới hơn ⇒ `StoreError::NewerSchema`, không đụng.
- **Dò mDNS** (`discovery`, gd1_05): test `--test discovery` C2 chạy mDNS thật trong tiến trình (cần interface ngoài
  loopback, không có thì tự bỏ qua). Kiểm điện thoại thật: `cargo run -p ferry-core --example discover`. Link-local cần
  `scope_id` (index interface nhận bản ghi); config lưu dạng `[fe80::x%en0]:port` (tên, không index — index đổi sau khởi động).
- RSS của `ferry` trên macOS ~44 MB là trang AppKit dùng chung; số thật là `footprint -p <pid>` (≈ 10 MB lúc rảnh, gd1_01).
