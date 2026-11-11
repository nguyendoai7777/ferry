# ferry-gd1_07 — daemon-runtime

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ 11/11/2026 |
| Repo | ferry |
| Model | opus |
| Cỡ | M — ~700 dòng |
| Phụ thuộc | gd1_04, gd1_05, gd1_06 |
| Quyết định áp dụng | W-1, D-13 |
| Phụ thuộc mới | thông báo hệ thống (`notify-rust` MIT/Apache hoặc API OS trực tiếp — soát Linux không kéo `libdbus`) |
| File đụng tới | MỚI `crates/daemon/src/{engine,tray_state,notify}.rs`, `crates/daemon/src/platform/linux/sni.rs`; SỬA `crates/core/src/ipc.rs`, `crates/daemon/src/{main,daemon,ipc_server,ui_process,tray}.rs`, `platform/{mod,linux}.rs`, `crates/ui/src/daemon_link.rs`, `i18n/*.toml`, `queue/store.rs` (Deserialize), `i18n.rs` (test) |

## 0. Bàn giao từ ferry-gd1_06 (11/11/2026)

<!-- handoff: ferry-gd1_06 -->

- **Token**: daemon giữ một `pairing::Tokens::os()` (đệm RAM, clone rẻ). Lúc khởi động `Devices::from_config(&config.devices,
  ..)` rồi với mỗi probe: `tokens.get(id).await` ⇒ `None` (người dùng xoá trong kho khoá) ⇒ `devices.unauthorized(id)`
  (NeedsRepair), có ⇒ `discovery::connect(id, &addrs, Some(&token))`. `info.paired=false` đã tự ra `NeedsRepair` qua `info_ok`.
- **Lệnh `Pair{device, pin}`**: `PhoneClient::new(addr, Some(&id), None)` ⇒ `pairing::pair(&mut c, Some(&id), &pin, (&config).into(),
  &tokens)` ⇒ `config.remember_device(..)` + `config::save` + `devices.paired(id, now)` ⇒ gửi `Link` mới cho runner (runner
  đang `NeedsPairing` tự chạy tiếp). Lỗi ⇒ IPC trả loại lỗi (`WrongPin`, `RateLimited{retryAfter giây}`, `OtherPhone`,
  `Keyring`, mạng) — **PIN chỉ đi UI → daemon, không vào snapshot / log**; `Secret` không `Serialize`, IPC nhận chuỗi rồi
  bọc ngay `Secret::new`.
- **Lệnh `Forget{device}`**: dừng runner của thiết bị (`Command::Stop`) ⇒ `pairing::forget(id, client_online, &tokens)` ⇒
  `config.forget_device` + save ⇒ `QueueStore::remove_device` ⇒ `devices.forget(id)`.
- Bản debug dùng dịch vụ kho khoá `ferry.dev` (`pairing::KEYRING_SERVICE`) — không đè bản đã cài.
- C4 (RAM): Linux `zbus` bật luồng `async-io` khi chạm kho khoá lần đầu — đo sau khi ghép.
- **Soát lại**: `cargo test -p ferry-core --test pairing`; app thật `real_phone_keyring` (`tests/real_phone.rs`).

## 0. Bàn giao từ ferry-gd1_05 (11/11/2026)

<!-- handoff: ferry-gd1_05 -->

- **Vòng lặp discovery mẫu**: `crates/core/examples/discover.rs` — `mdns::Browser::next()` ⇒ `Devices::announce` /
  `withdraw`; ngủ tới `Devices::next_deadline()` (không poll) ⇒ `tick(now)` (phát `Lost` sau 30 s) + `due_probes(now)` ⇒
  `discovery::connect(id, &devices.addresses(id), token)` ⇒ `info_ok` / `connect_failed`. Bê vào runtime tokio của `ferry`.
- **Sự kiện**: mọi hàm của `Devices` trả `Vec<DeviceEvent>` (`Seen`, `Lost`, `AddressChanged`, `StateChanged`) ⇒ gộp vào
  snapshot. Runner (gd1_04): lỗi mạng ở một địa chỉ ⇒ `Devices::address_failed(id, addr)` rồi `connect` lại; không địa chỉ
  nào trả lời ⇒ `connect_failed` (⇒ Offline, thử lại sau `PROBE_INTERVAL` 60 s); `401` ⇒ `unauthorized`.
- **Mạng đổi** ⇒ `Devices::set_local_interfaces(address::local_interfaces())` (đổi thứ tự địa chỉ); ghi
  `config.devices = devices.to_config()` khi địa chỉ / tên của máy đã ghép đổi.
- **Đo C5 gd1_05** (`examples/discover`, release, macOS): mDNS thêm ~0,1 MB footprint, CPU ≈ 0 % — C4 ở đây so lại trên
  `ferry` thật (mốc 10 MB). `mdns::IP_CHECK_INTERVAL_SECS` = 15 s (soát interface của `mdns-sd`); thấy đánh thức nhiều thì nâng.
- **Khớp gd1_04**: `discovery::connect` ⇒ `Connected { client, info, addr }` ⇒ `Link::new(client, &info)`; `TransferEvent::
  ConnectionLost{addr}` ⇒ `Devices::address_failed(id, addr)` rồi `connect` lại.
- **UI**: `Device::newer_api`, `Device::features` (`dirs`) cần vào snapshot (gd1_09 hiện chữ "cập nhật app trên máy tính").

## 0. Bàn giao từ ferry-gd1_04 (11/11/2026)

<!-- handoff: ferry-gd1_04 -->

- **Đã có sẵn** (`ferry-core::transfer`): `UploadRunner::new(device, store: SharedStore, link: LinkWatch, Throttle, events,
  RunnerConfig::default()) -> (runner, RunnerHandle)`, chạy `tokio::spawn(runner.run())` — một runner mỗi thiết bị.
  `SharedStore = Arc<Mutex<QueueStore>>` dùng chung với lệnh `AddFiles` (sau `add_batch` gửi `Command::Wake`).
- **Địa chỉ**: giữ `LinkSender` (`link_channel()`) cho mỗi thiết bị; Online / đổi địa chỉ ⇒ `send_replace(Some(Link::new(client,
  &info)))`, Offline ⇒ `None`. `TransferEvent::ConnectionLost{addr}` ⇒ cho discovery chọn lại địa chỉ (§6.8).
- **Lệnh UI**: `ItemAction{id, pause|resume|cancel|retry}` ⇒ `handle.send(Command::Item{..})`; theo lô `Command::Batch`;
  "Tạm dừng tất cả" = `Batch`/`Item` cho mọi mục hoặc ngừng gửi `Link` (runner ngủ). Tắt app ⇒ `Command::Stop`.
- **Sự kiện → snapshot / khay**: `ItemChanged{id, state}` (đã ghi store), `Progress{id, sent, size, bytesPerSec}` mỗi
  500 ms khi đang gửi (gộp ≤ 10 / s là việc ở đây), `BatchFinished{batch, sent, skipped, failed}` ⇒ một thông báo,
  `DeviceNeedsRepair` ⇒ thiết bị `NeedsRepair` + báo một lần (gửi `Link` mới sau ghép lại ⇒ runner tự chạy tiếp mục
  `NeedsPairing`), `PhoneNeedsUpdate` ⇒ "Cập nhật app trên điện thoại để giữ thư mục".
- Giới hạn băng thông: một `Throttle` (`Arc`) chung cho mọi runner, `set_rate` khi cài đặt đổi (gd1_12).
- **Soát lại**: `cargo test -p ferry-core --test upload`; mục 9 của `plan/ferry-gd1_04_upload-runner.md`.

## 1. Làm gì
- `ferry` chạy runtime tokio đa luồng nhỏ (2 luồng) cho discovery + một `Runner` mỗi thiết bị; vòng sự kiện khay ở luồng chính.
- `Snapshot` (thiết bị + trạng thái, hàng đợi từng thiết bị, tiến độ, "Đã gửi") gửi qua IPC khi đổi, gộp tối đa 10 lần / s; UI mới nối nhận ngay bản đầy đủ.
- Lệnh IPC: `AddFiles{device, paths}` (lọc đuôi 6.6, thư mục đệ quy), `ItemAction{id, pause|resume|cancel|retry}`, `PauseAll/ResumeAll`, `Pair/Forget/AddManualAddress`, `ListLibrary`, `Download` — ≤ 4 biến thể mới mỗi lần thêm, tách nếu vượt trần.
- Khay 3 trạng thái (rảnh / đang gửi kèm % / cần chú ý), tooltip "Bật Sync WiFi trên điện thoại để gửi N video" khi có mục chờ mà Offline; menu: Mở cửa sổ, Tạm dừng tất cả, Thoát.
- Một thông báo hệ thống khi cả lô xong; lỗi cần người dùng làm (hết chỗ, cần ghép lại) báo một lần.
- Nợ từ gd1_01: **khay Linux** — bê StatusNotifierItem tự viết của fastcast (`fastcast/crates/daemon/src/platform/linux/sni.rs`,
  không GTK / libappindicator) vào `crates/daemon/src/platform/linux.rs` (hiện chỉ kênh chặn, không icon). Khung đã có:
  `Event`, `Daemon::handle`, `tray::{menu_action, phone_rgba}`, IPC `UiToDaemon` / `DaemonToUi` + `Hello`/`Ok`.
  Mốc RAM gd1_01: `ferry` rảnh footprint 10 MB (RSS 44 MB) trên macOS — C4 so với số này.

## 6. Nghiệm thu
- C1. Client IPC giả nhận snapshot đầu đủ, rồi delta theo tiến độ ≤ 10 / s. `[opus-unit]`
- C2. Thả 3 tệp qua IPC với `fake_phone` ⇒ khay chuyển "đang gửi" ⇒ "rảnh" + một thông báo. `[opus-run]`
- C3. Tắt `ferry` giữa khúc, mở lại ⇒ gửi tiếp. `[opus-run]`
- C4. RAM / CPU `ferry` rảnh sau task, so với mốc gd1_01. `[opus-run]`
- C5. Icon khay ba trạng thái nhìn rõ ở nền sáng / tối ba OS. `[owner-qa]`

## 8. Nợ chuyển task khác
- Icon khay: vẽ tạm; chủ dự án chọn bộ icon sau (backlog nếu cần thiết kế).

## 9. Bàn giao thực tế

- **Bố cục**: `main.rs` dựng runtime tokio đa luồng 2 worker (`ferry-rt`) + `QueueStore` (`<data_dir>/queue.sqlite3`) +
  `Tokens::os()` ⇒ `engine::spawn(&rt, EngineConfig, Sinks)` ⇒ `ipc_server::spawn(listener, waker, engine)` ⇒
  `Daemon::new(i18n, waker, engine, &rt)`. Engine (`engine.rs`) là **một task** giữ `Devices`, một `UploadRunner` mỗi
  điện thoại có token (dựng lúc khởi động từ `config.devices`, kể cả Offline), mDNS, danh sách UI nghe; mọi thứ xử lý
  tuần tự trong một `select!` (lệnh, `TransferEvent`, kết quả task con, mDNS, mốc `Devices::next_deadline`, mốc đẩy,
  dọn 24 h). Luồng chính chỉ giữ khay + `ferry-ui`: engine báo khay qua `Sinks::tray` ⇒ `Event::Tray(TrayStatus)`,
  thông báo qua `Sinks::notice` ⇒ `notify::show` (luồng ngắn, `notify-rust`). Test thay hai sink bằng hàm ghi lại.
- **IPC (chốt, `ferry_core::ipc`, doc module có tóm tắt)**: UI ⇒ daemon `Queue{req, op}` (`add_files{device, paths,
  destDir}` · `item{id, action}` · `batch{batch, action}` · `all{action}` — `action` = `pause|resume|cancel|retry`, `all`
  chỉ nhận pause/resume), `Phone{req, op}` (`pair{device, pin}` · `forget{device}` · `add_address{address}`),
  `Library{req, op}` (`list{device}`). Mỗi lệnh đúng một `Reply{req, outcome}`: `done` · `added{batch?, files,
  skippedNonVideo, unreadable}` · `paired{device, name}` · `found{device, name}` · `library{items}` · `failed{error}` với
  `CommandError` (`code`): `unknown_device`, `not_paired`, `offline`, `invalid_input`, `unknown_interface{name}`,
  `wrong_pin`, `rate_limited{retryAfterSecs}`, `other_phone`, `keyring`, `unreachable`, `internal`. Daemon ⇒ UI:
  `Snapshot` đầy đủ ngay sau `Hello` (vai `ui`), rồi mỗi khi mục / lô / thiết bị đổi; chỉ tiến độ đổi ⇒
  `Progress{items:[{id, sent, size, bytesPerSec}]}`; gộp ≤ `MAX_PUSH_PER_SEC` = 10 / s / client. `Snapshot` = `devices`
  (`DeviceView{id, name, state, visible, newerApi, features, address}`), `batches` (`{info, progress}` của lô có mục
  trong ảnh), `uploads` (≤ `QUEUED_IN_SNAPSHOT` 1000 mục chưa xong đầu hàng + ≤ `FINISHED_IN_SNAPSHOT` 300 mục xong mới
  nhất, xếp theo `id`), `progress`, `finishedOmitted`, `queuedOmitted`, `canPauseAll`, `canResumeAll`. Mẫu dây:
  test `ipc::tests::command_and_reply_wire_format`. Biến thể mới gd1_07: `Queue`, `Phone`, `Library` (UiToDaemon) +
  `Snapshot`, `Progress`, `Reply` (DaemonToUi) — **Download chưa có**, gd1_10 thêm `QueueOp`/`LibraryOp` (≤ 4).
- **Client chậm**: mỗi kết nối một `Outbox` (kênh + đếm chờ ghi) + luồng ghi; chờ ≥ 8 ⇒ engine bỏ lượt ảnh chụp /
  tiến độ của client đó rồi gửi bản đầy đủ khi kịp. Trả lời lệnh luôn đi. `UiProcess` gửi `Focus` qua `Outbox`.
- **Lệnh**: `add_files` chạy `plan_batch` + `add_batch` ở `spawn_blocking` rồi `Command::Wake`; điện thoại chưa có token ⇒
  `not_paired`. `pair` soát PIN 6 số ⇒ `discovery::connect(None)` ⇒ `pairing::pair` ⇒ `Devices::paired` + runner + probe
  ngay (ghép lại sau 401 ⇒ `Link` mới ⇒ runner chạy tiếp). `forget` gỡ sổ + config **ngay** (khỏi probe dựng lại runner),
  rồi task con: chờ runner dừng ⇒ `pairing::forget` ⇒ `remove_device` ⇒ `Reply`. `add_address`: `parse_address`
  (link-local `%iface` lạ ⇒ `unknown_interface`) ⇒ `probe`; điện thoại đã có token ⇒ nối lại **kèm token**, địa chỉ mới
  thử trước (`info` không token luôn `paired:false`, đưa vào `info_ok` sẽ thành NeedsRepair giả).
- **Khay** (`tray_state.rs` thuần, `tray.rs` vẽ icon): Rảnh / Đang gửi {%} (màn đầy dần theo phần tư, icon chỉ đổi khi
  qua bậc) / Cần chú ý (chấm cam góc trên phải: cần ghép lại, hết chỗ). Tooltip: "Đang gửi N tệp — P %", "Bật Sync WiFi
  trên điện thoại để gửi N tệp" (W-5: "tệp", không "video"), "Đã tạm dừng N tệp", "Ghép lại {phone}…", "{phone} hết
  chỗ…". Menu Mở cửa sổ · Tạm dừng / Tiếp tục tất cả (tắt khi không có gì) · Thoát. Thoát ⇒ `EngineHandle::shutdown`
  (Stop mọi runner, chờ ≤ 3 s).
- **Khay Linux** (nợ gd1_01 xong): `platform/linux/sni.rs` bê từ fastcast, chạy trên runtime engine (`zbus` 5
  `async-io`, đã có trong cây qua `keyring`); thêm `NewIcon`, `enabled=false` của DBusMenu, `Activate` ⇒ mở cửa sổ.
  Kiểm thật Ubuntu (GNOME `ubuntu-appindicators`): đăng ký host, `GetLayout` đúng 4 mục, bấm mục tắt bị bỏ, "Quit" qua
  `Event` ⇒ thoát sạch.
- **Thông báo**: một cái mỗi lô (`BatchFinished`): tiêu đề "Đã gửi N tệp sang {phone}", thân thêm "N tệp đã có sẵn…" /
  "N tệp không gửi được…"; lô chỉ toàn huỷ ⇒ không báo. NeedsRepair báo một lần khi thiết bị có mục chờ (xoá cờ khi về
  Online); hết chỗ báo một lần mỗi thiết bị (xoá khi lại Sending). `notify-rust` 4.18: Linux `zbus` (không `libdbus`),
  Windows WinRT, macOS `mac-notification-sys`.
- Opus chọn 11/11/2026: (1) gộp lệnh thành 3 biến thể có `op` con thay vì một biến thể mỗi lệnh — giữ trần "≤ 4 biến thể
  mới / task" mà vẫn mở rộng được (`QueueOp`/`PhoneOp`/`LibraryOp` thêm nhánh không đổi `UiToDaemon`); (2) "Tạm dừng
  tất cả" = `Command::Item{Pause}` cho mọi mục chưa xong (không ngừng `Link`: mục tạm dừng phải hiện "Đã tạm dừng" ở
  UI); "Tiếp tục tất cả" chạy tiếp mọi `Paused` kể cả hết chỗ / cần ghép (người dùng vừa giải phóng chỗ); (3) icon Linux
  gần trắng (`#f2f2f2`) vì thanh trên GNOME luôn tối — KDE nền sáng xem ở C5; (4) i18n test D-13 chấp nhận chuỗi mở đầu
  bằng tham số (`{count} files paused`).
- **Ngoài phạm vi đã sửa (D-11)**: `UploadItem` / `BatchInfo` / `BatchProgress` / `DownloadItem` thêm `Deserialize` (UI
  đọc lại); `config.rs`, `transfer/*`, `download.rs` **không đụng** (gd1_10 đang làm ở w2).
- **Nghiệm thu**: C1 `engine::tests::c1_full_snapshot_then_at_most_ten_pushes_per_second` (ảnh chụp < 500 ms sau nối,
  khoảng cách giữa hai lần đẩy ≥ 80 ms đo phía nhận); C2 `c2_three_files_tray_goes_sending_then_idle_with_one_notice`
  (engine thật + `fake_phone` gộp bằng `#[path]`, khay Idle → Sending → Idle, đúng một thông báo "Sent 3 files to Fake
  Pixel"); C3 `c3_restart_mid_chunk_resumes` (tắt engine khi điện thoại có ≥ 1 MB, mở engine mới cùng hàng đợi ⇒ mục giữ
  offset, tệp trên điện thoại đúng từng byte) — mức engine; `kill -9` thật dựa vào `QueueStore::open` đưa mục bay về
  Waiting (test gd1_03/04). C4 (Linux, debug, chưa ghép điện thoại): RSS 19 MB, RssAnon 1,9 MB, 7 luồng (`ferry`,
  2 `ferry-rt`, `ipc-accept`, `mDNS_daemon`, `async-io`, `zbus`), **0 tick CPU trong 30 s rảnh**. Mốc gd1_01 là macOS
  footprint 10 MB — đo `footprint` macOS ở lượt thử tay. C5 `[owner-qa]` ⇒ `_backlog.md`.
- Soát xanh: `cargo build`, `cargo clippy --all-targets -D warnings` (+ chéo `x86_64-pc-windows-msvc`), `cargo test`
  (daemon 21, core lib 105), `cargo deny check licenses`. **Clippy chéo macOS không chạy được từ Linux**:
  `mac-notification-sys` biên dịch Objective-C (bẫy ghi ở `CLAUDE.md`). Cỡ thật ~2 600 dòng (engine 1 300 gồm 4 test) —
  vượt ước lượng M vì SNI + test engine.

## 10. Đề xuất cải tiến

- Đã làm trong lượt (D-8): trần `QUEUED_IN_SNAPSHOT` — thả cả ổ (vài nghìn tệp) từng làm dòng `Snapshot` vượt
  `MAX_LINE` 1 MiB và UI bỏ thông điệp (`InvalidData`); test `huge_queue_snapshot_stays_under_the_line_limit`.
- Interface mạng chỉ đọc lại khi nhập địa chỉ tay và mỗi 24 h ⇒ đổi WiFi giữa chừng có thể thử địa chỉ theo thứ tự cũ
  (vẫn nối được, chậm hơn ≤ 250 ms mỗi địa chỉ). Theo dõi đổi mạng của OS là việc của task đóng gói / gd2.
