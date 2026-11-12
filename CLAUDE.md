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
| `crates/core` — `ferry-core` | lib | `protocol` (kiểu JSON + header §6), `client` (HTTP §6.3–6.5), `discovery` (mDNS §6.2 + quét LAN dự phòng + địa chỉ tay), `pairing` (ghép + kho khoá), `device` (máy trạng thái thiết bị §5), `queue` (hàng đợi bền + máy trạng thái mục §5), `transfer` (chạy hàng đợi: upload nối tiếp, tải về `Range`), `config`, `paths`, `i18n`, `logging`, `ipc` | UI, GPU |
| `crates/daemon` — bin `ferry` | bin | runtime tokio của lõi, khay (3 trạng thái), IPC server, bật tiến trình UI, một instance, thông báo hệ thống, autostart | GPUI, materiant |
| `crates/ui` — bin `ferry-ui` | bin | một cửa sổ vỏ Groovex: cột trái (Gửi tệp, Thư viện điện thoại; Cài đặt ở góc dưới) + trang bên phải (vùng thả + hàng đợi, thư viện, thêm điện thoại, cài đặt) — GPUI + materiant; chỉ vẽ ảnh chụp trạng thái nhận qua IPC và gửi lệnh | mạng, kho khoá, SQLite |
| `crates/core/tests/fake_phone` | test | server giả trong tiến trình đúng hợp đồng §6 (đứt giữa khúc, `409`, `404`, `507`, `401`, `423`, `503`) | — |

- UI không gọi mạng: mọi lệnh đi IPC sang `ferry`, `ferry` gửi ảnh chụp trạng thái (snapshot) khi đổi, không chia sẻ mutex.
- Đóng cửa sổ ⇒ tiến trình UI thoát (nhả GPU / RAM); truyền vẫn chạy ở `ferry`.
- **Vỏ UI bê từ Groovex**: `views/app_bar.rs` (ba nút cửa sổ), `views/side_nav.rs`, `platform/{window_controls,move_loop,
  resize_edges}.rs` chép từ `groove/crates/groove-app` — sửa bên đó thì soát bên này. Cửa sổ `WindowDecorations::Client` mọi OS; Windows / Linux `titlebar: None`, Linux tự vẽ mép đổi cỡ,
  Windows `move_loop::install` sau khi mở. **macOS** (11/11/2026): thanh tiêu đề trong suốt của hệ thống (`window_controls::titlebar`)
  ⇒ đèn giao thông native giữa hàng 36 px đầu cột trái (hàng đó không ghi tên app — thanh menu đã có), ba nút tự vẽ vẫn ở
  góc phải kiểu cửa sổ Cài đặt của fastcast (ô 28 px bo 7, khe 4, chừa 8 px mép phải cho góc bo, Đóng rê xám); thanh menu
  của app ở `app_menu.rs` (chữ `[menu]` của i18n, dựng lại khi đổi ngôn ngữ). Cài đặt là trang
  trong cửa sổ chính (`MainWindow::open_settings`); `ferry-ui --settings` (chỉ còn mục "Cài đặt" của
  `ferry.desktop`, chuột phải icon dock Linux) mở thẳng trang đó. Menu khay chỉ **Mở · Thoát** (12/11/2026). Giao diện:
  `theme` (`light|dark|system`) + `themePreset` (4 bảng của `builtin_presets()`, không tự chỉnh) trong `config.json`,
  `appearance::apply` lúc khởi động và khi đổi.

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
node packaging/icons/gen-icons.mjs # vẽ lại icon app máy bay giấy: png hicolor, .ico, .icns, ferry.svg + bộ Android / iOS ở packaging/icons/mobile (kết quả đã commit)
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

- **Icon dock / taskbar**: Wayland lấy icon theo `app_id` ⇒ `<app_id>.desktop`. Bản cài `ferry` (`packaging/linux/ferry.desktop`); bản debug `ferry.dev` (`APP_ID` trong `crates/ui/src/main.rs`), `start.sh` ghi `~/.local/share/applications/ferry.dev.desktop` "Ferry (dev)". Windows: `crates/ui/build.rs` nhúng `packaging/windows/ferry.ico` vào `ferry-ui.exe`; macOS bản dev nhúng `ferry.icns`.
- **Socket Unix ≤ 104 byte** (macOS; Linux 108): `FERRY_DATA_DIR` sâu làm `bind` hỏng ("exceeds capacity of sun_path").
  `ipc::Endpoint::daemon` tự rơi về `<tmp>/ferry-<băm>/daemon.sock` khi đường dẫn quá 100 byte — đừng gỡ.
- **Khay Linux là SNI tự viết** (`platform/linux/sni.rs`, bê từ fastcast, gd1_07): `zbus` chạy trên runtime engine,
  không GTK / `tray-icon`. GNOME cần extension AppIndicator (`ubuntu-appindicators`); thiếu host thì chỉ log, mở cửa sổ
  bằng cách chạy `ferry` lần nữa. Icon Linux gần trắng vì thanh GNOME tối.
- **Engine một task** (`daemon/src/engine.rs`, gd1_07): mọi trạng thái (thiết bị, runner, UI nghe) xử lý tuần tự trong một
  `select!`; việc chậm (ghép, `plan_batch`, dọn sau quên) chạy task con / `spawn_blocking` rồi trả `Internal` — đừng
  `await` mạng trong vòng chính. Đẩy ra UI chỉ qua `Outbox::push`; `Snapshot` có trần mục (`QUEUED_IN_SNAPSHOT`,
  `FINISHED_IN_SNAPSHOT`) để dòng JSON < `MAX_LINE` 1 MiB — thêm trường mảng thì giữ trần.
- **Mở lại cửa sổ y như lúc đóng** (12/11/2026, `ferry_core::view`): `ferry-ui` vẫn thoát khi đóng (luật RAM); trạng thái
  sống ở daemon, **chỉ trong RAM**. UI gửi `UiToDaemon::SaveView` (`ViewState`: trang + lịch sử, khay, lô mở, trình duyệt
  `BrowserState`, bảng nháp, khung cửa sổ) gộp 0,5 s sau mỗi lần vẽ / trình duyệt đổi / dời cửa sổ, chỉ khi khác bản trước, và
  ngay trước khi đóng (`window_controls::before_close` — mọi đường đóng phải qua nó, nút Đóng Linux / macOS gọi
  `remove_window` không qua `on_window_should_close`). Daemon đẩy `DaemonToUi::Restore` ngay sau `Ok` của `Hello`, trước
  `Snapshot` đầu; `daemon_link::connect` chờ tới 1,5 s cho `Snapshot` đầu rồi mới mở cửa sổ ⇒ khung đầu đã đúng trang,
  không chớp màn rỗng. Thư mục đã đọc (`dirs` / `list`) daemon giữ ở `view::DirCache` (LRU, trần 4 MiB / 512 mục): `Restore`
  chỉ mang thư mục khung đầu cần (trần `RESTORE_BUDGET` 640 KiB), thư mục khác UI hỏi `LibraryOp::Cached` (vẽ ngay) song song
  `Dirs` (làm mới); `Model::storage` chỉ là bản làm việc của phiên. Quên / thay máy ⇒ `DirCache::forget_device` +
  `ViewState::forget_device`; UI tự soát lại (`main_window::plan_restore`). Khung cửa sổ còn ghi `ui-state.json` lúc đóng
  (qua được lần khởi động lại daemon); Wayland không cho biết / đặt vị trí, chỉ cỡ. Thêm trường vào `ViewState` ⇒ kẹp trần ở
  `ViewState::clamp`. Kiểm thử đầu-cuối không đụng màn thật: daemon `--autostart` + `ferry-ui` trong
  `fastcast/tools/headless-test.sh`, đọc `Restore` bằng client IPC vai `ui`.
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
- **App điện thoại chưa có `dirs`** (§6.9; bản `xmobile.videoplayer` 11/2026 vẫn chưa có `features`): không có hộp chọn nơi lưu — `send_tray::lacks_dirs` (chỉ khi online) ⇒ nút "Gửi sang điện thoại" gửi thẳng vào gốc, khay báo cần cập nhật app điện thoại; offline thì vẫn mở hộp chọn. Thử hộp chọn cần điện thoại có `dirs` (hoặc `fake_phone`).
- **Duyệt bộ nhớ trong (`features: browse`, §6.11, W-7)**: `PhoneBrowser` đổi nguồn từ `list` sang `GET /dirs` (`LibraryOp::Dirs`, đọc từng thư mục khi mở / mở rộng cây, gom ở `browse::FsCache`, giữ ở `Model::storage`). Có `browse` thì **mọi `dir` tính từ gốc bộ nhớ dùng chung**, không còn từ nơi lưu của app ⇒ thư mục đích lần trước nhớ theo khoá `<id>#browse` (`Model::dest_key`); điện thoại bật / tắt `browse` lúc trình duyệt đang mở ⇒ về gốc, đọc lại. Tệp ngoài thư viện không có `id` ⇒ không tải về.
- **App điện thoại cài lại đổi `id`** (gặp 11/11/2026: `cdf3…` ⇒ `91b0…`, cùng IP, cùng tên): `discovery::connect` theo mã cũ coi địa chỉ là hỏng. Ghép lại bằng PIN (`PhoneOp::Pair`) ⇒ `discovery::find_replacement` lấy máy đang trả lời ở địa chỉ cũ, PIN đúng mới nhận; engine `replace_device` bỏ mã cũ (sổ, config, kho khoá) như Quên nhưng **chuyển** hàng đợi (`QueueStore::move_device`) + tên đặt trên PC sang mã mới. Ghép như máy mới (màn Thêm điện thoại) mà `Devices::superseded_by` thấy máy cũ cùng tên + chung địa chỉ + không Online ⇒ cũng thay máy cũ (không để hai dòng). Không tự gộp khi chưa ghép bằng PIN. Bẫy: đừng `config.forget_device` trước `save_config` — nó thấy config đã khớp sổ nên không ghi tệp, máy đã quên sống lại lần chạy sau.
- **mDNS im dù HTTP thông ⇒ quét LAN dự phòng** (`discovery::scan`, PROTOCOL §6.2; gặp 11/11/2026 trên Mac, WiFi nhà
  `192.168.4.0/24`): hai lỗi chồng nhau — (a) router không chuyển multicast từ máy WiFi khác tới Mac (`dns-sd -B
  _services._dns-sd._udp` chỉ thấy dịch vụ của chính Mac; gói mDNS nhóm `224.0.0.251` từ máy khác không tới, còn trả lời
  **unicast** thì tới), (b) điện thoại chỉ quảng bá `_adb._tcp`, không có `_vpsync._tcp` (lỗi phía app, backlog). Cách
  phân biệt không cần sudo / adb: gửi truy vấn legacy unicast (socket UDP cổng ngẫu nhiên → `224.0.0.251:5353` và thẳng IP
  điện thoại, hỏi PTR `_services._dns-sd._udp.local`) ⇒ máy nào trả lời là còn thấy được dịch vụ nó khai; `tcpdump` cần
  root. **Khác subnet** (WiFi công ty 11/11/2026: `172.16.0.x` ↔ `172.16.4.x`, mỗi bên `/23` — nối thẳng IP + PIN được
  vì qua router, mDNS thì không bao giờ qua router) ⇒ quét subnet của PC cũng không thấy: phải quét `/24` của IP điện thoại
  từng dùng. Quét đầy đủ: `PhoneOp::ScanLan` do màn Thêm điện thoại gửi lúc mở rồi 15 s một lần suốt bước chọn máy, kể cả khi đã thấy máy khác (thêm máy thứ hai; 12/11/2026)
  (`RESCAN_EVERY`), daemon gộp lệnh trùng + cách nhau ≥ 10 s; subnet của PC trọn tới `/22` + `/24` quanh IP điện thoại từng
  dùng (`Engine::phone_hints`: sổ thiết bị, gồm địa chỉ nhập tay ở màn Thêm điện thoại). Quét thưa: máy đã ghép nối hỏng mọi địa chỉ ⇒ `maybe_offline_scan`
  chỉ dải `/24` của máy đó, ≥ 5 phút một lần (`OFFLINE_SCAN_EVERY`). Nối TCP 600 ms, 128 cùng lúc (macOS trần 256 fd),
  chỉ nhận `info.app` = `protocol::APP` (/24 ~1–2,4 s). Máy quét thấy đi đường như địa chỉ tay (`apply_untrusted_info`: máy có token thì
  nối lại kèm token, không thành NeedsRepair). Thử tay: `FERRY_SCAN=1 cargo run -p ferry-core --example discover`.
  Điện thoại sang subnet chưa từng thấy thì chỉ còn nhập tay (không quét cả `172.16/12`).
- **Vòng "đang dò" có vòng nền** (`add_phone::searching_spinner`): cung không xác định của `ProgressCircular` có khung chỉ
  ~10° — ở 24 px (hoặc lúc cửa sổ không vẽ lại) trông như gạch hồng lệch; chồng một `ProgressCircular` `value(0)` làm track.
- **Sửa bộ nhớ điện thoại (W-8, `features: edit`, §6.12)**: `LibraryOp::{MakeDir, Rename, Delete}` ⇒ `ClientError`
  `409` không `Upload-Offset` = `Conflict` ⇒ `CommandError::Exists`, `404` ⇒ `Missing`, `403` ⇒ `Forbidden` (+ đọc lại
  `info`). Daemon chặn sớm khi máy chưa khai `edit`. UI: `phone_browser/edit.rs` — menu chuột phải gắn ở khung NGOÀI của
  dòng (`context_menu` dừng lan sự kiện), dòng bên trong bắt chuột phải trước để chọn dòng (con nhận trước cha); một hộp
  thoại chung cho thư mục mới / đổi tên / xoá; xoá nhiều mục gửi từng lệnh, đủ trả lời mới đọc lại thư mục cha. Không có
  `edit`: thư mục mới là thư mục ảo như cũ, đổi tên / xoá chỉ cho thư mục ảo. Thử UI không cần điện thoại thật:
  `cargo run -p ferry-core --example fake_phone` (khai `browse` + `edit` + `copy`, có sẵn thư mục) rồi nhập tay địa chỉ nó in, PIN
  `123456` — chạy một bản `ferry` riêng với `FERRY_DATA_DIR` / `FERRY_CONFIG_FILE` khác để không đụng bản dev.
- **Cắt / Sao chép / Dán (W-9, `features: copy`, §6.13)**: `LibraryOp::{Copy, Move}` đi qua cùng `edit_op` (cần cả `edit` lẫn
  `copy`). Bảng nháp là `Model::clip` (chỉ đường, không gọi mạng; sống qua lần mở lại trình duyệt), UI ở
  `phone_browser/clip.rs`; Dán gửi từng mục, đủ trả lời mới đọc lại thư mục đích (+ nguồn khi dời). Mục ảo không chép.
  App điện thoại cài §6.13 sau (12/11/2026 mới có ở PC + `fake_phone`) ⇒ máy thật chưa khai `copy` thì Dán xám.
- **Cắt / Sao chép sang máy tính (W-10, `features: fetch`, §6.14)**: daemon (không phải UI) tải vào
  `<data_dir>/clipboard/<phiên>` (`ferry_core::export`) rồi đặt clipboard (`daemon/src/clipboard/`); engine ở
  `engine/export.rs`, UI `phone_browser/export.rs`. Linux: chủ selection X11 tự viết (`x11rb`) chạy qua XWayland — máy
  Wayland không có XWayland thì không dán được. Cắt chỉ xoá trên điện thoại khi mục cấp đầu trong thư mục phiên biến mất
  (trình quản lý tệp đã dời) — đừng đổi sang "xoá khi tải xong". Test engine dùng clipboard giả (`Clipboard` hai hàm);
  test clipboard thật `--ignored`. Thư mục tên `Con`, `Nul`… bị đặt lại `_Con` (tên cấm của Windows) ở mọi OS.
- **Mục ẩn (W-11, 12/11/2026)**: `dirs` trả cả tên bắt đầu bằng `.`; PC nhận ra theo tên (`browse::is_hidden_name`) và vẽ icon
  nhạt (`table::HIDDEN_ICON_OPACITY`, bảng + cây), cho gõ tên bắt đầu bằng `.` (`edit::sanitize_name`). Chép sang máy
  (`export::keep_hidden`) giữ dấu chấm đầu; "Tải về máy" (`sanitize_file_name`) vẫn cắt nó. App điện thoại bản cũ còn
  giấu / cấm mục ẩn ⇒ tạo thư mục ẩn nhận `403` / `400` tới khi app cập nhật.
- **`features` của máy Online không tự mới** (gặp 11/11/2026: ghép xong mới bật "Cho máy tính duyệt bộ nhớ" ⇒ PC
  vẫn "Chỉ thấy thư mục đã có tệp gửi qua Ferry"): daemon chỉ đọc `info` lúc nối (`Internal::Connected`), probe định kỳ
  chỉ chạy cho máy chưa Online. Màn duyệt điện thoại (`LibraryOp::List` / `Dirs`) đọc lại `info` kèm token (`refresh_info`,
  cách nhau ≥ 3 s mỗi máy; `Dirs` nhận `403` thì đọc ngay) ⇒ `InfoRefreshed` cập nhật `features` + `Link::dirs`; UI đổi
  chế độ khi `browses_storage` đổi. Thêm `feature` mới cần PC phản ứng ⇒ đọc theo đường này, đừng tin `features` lúc ghép.
- **Tường lửa chặn mDNS** (W-6, `ferry_core::firewall`): Linux `ufw` bật (`/etc/ufw/ufw.conf` `ENABLED=yes`, đọc không cần root)
  và chưa có dấu đã cho phép ⇒ `Snapshot::mdnsFirewall` ngay từ ảnh chụp đầu (tắt khi mDNS thấy điện thoại) ⇒ vào màn Thêm điện
  thoại là tự mở hộp thoại xin quyền (một lần mỗi lần mở màn; "Để sau" còn nút mở lại) ⇒ `PhoneOp::AllowMdnsFirewall` ⇒ `pkexec /bin/sh -c "<ufw> allow in proto udp to 224.0.0.251 port 5353 comment 'ferry mDNS'
  && { <ufw> … ff02::fb … || true; }"` (một hộp mật khẩu cho hai luật; IPv6 tắt trong ufw thì luật v6 lỗi, bỏ qua). Mã thoát
  `pkexec`: 126 = người dùng huỷ, 127 = không có agent polkit ⇒ UI hiện câu lệnh để chép. Xong ⇒ dấu `mdns-firewall-allowed` cạnh
  `config.json` + dò lại mDNS (dấu có rồi thì không hỏi nữa, kể cả khởi động lại). Postinst `.deb` thêm cùng luật + dấu `/var/lib/ferry/ufw-mdns-allowed`; postrm xoá luật theo
  comment (`ufw show added`) — đổi comment phải sửa cả hai (test `deb_scripts_use_the_same_comment`). Ubuntu mặc định đã có luật
  mDNS multicast trong `/etc/ufw/before.rules` (đọc phải root) ⇒ ufw bật chưa chắc là thủ phạm: WiFi công ty / AP isolation chặn
  multicast thì cho phép xong vẫn không thấy, còn đường nhập địa chỉ tay. Windows: luật `netsh` của NSIS giữ `profile=any` (WiFi
  mới mặc định là Public). macOS: tường lửa ứng dụng tự hỏi, không thêm bước. Không bao giờ tắt tường lửa / đổi luật khác.
- **macOS: `titlebar: None` làm nút cửa sổ chết** — gpui-pre-macos 0.3.10 tạo NSWindow chỉ `Titled | FullSizeContentView`,
  thiếu `Miniaturizable` / `Resizable` ⇒ `miniaturize:` / `zoom:` bị AppKit bỏ qua. Cần `TitlebarOptions` (trong suốt +
  `traffic_light_position`), không quay lại `None` trên macOS. Cửa sổ chưa focus thì lần bấm đầu chỉ kích hoạt nó (chuẩn macOS).
- **macOS: không `set_menus` thì thanh menu giữ tên + menu của app trước** (thấy "Claude"). Tên in đậm là `CFBundleName`:
  `Ferry.app` lấy từ `packaging/macos/Info.plist`; exe trần bản dev lấy `Info.plist` nhúng vào `__TEXT,__info_plist` bởi
  `crates/ui/build.rs` (chỉ debug) — thiếu thì hiện "ferry-ui". Thử tự động: `osascript` đọc `menu bar 1` của tiến trình, bấm
  nút native qua subrole `AXCloseButton` / `AXMinimizeButton` (nút xanh là `AXFullScreenButton`).
- RSS của `ferry` trên macOS ~44 MB là trang AppKit dùng chung; số thật là `footprint -p <pid>` (≈ 10 MB lúc rảnh, gd1_01).
