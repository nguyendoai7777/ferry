# media-wireless-sync-gd1_07 — daemon-runtime

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | M — ~700 dòng |
| Phụ thuộc | gd1_04, gd1_05, gd1_06 |
| Quyết định áp dụng | W-1, D-13 |
| Phụ thuộc mới | thông báo hệ thống (`notify-rust` MIT/Apache hoặc API OS trực tiếp — soát Linux không kéo `libdbus`) |
| File đụng tới | MỚI `crates/daemon/src/{engine,tray_state,notify}.rs`; SỬA `crates/core/src/ipc.rs`, `crates/daemon/src/main.rs` |

## 0. Bàn giao từ media-wireless-sync-gd1_05 (11/11/2026)

<!-- handoff: media-wireless-sync-gd1_05 -->

- **Vòng lặp discovery mẫu**: `crates/core/examples/discover.rs` — `mdns::Browser::next()` ⇒ `Devices::announce` /
  `withdraw`; ngủ tới `Devices::next_deadline()` (không poll) ⇒ `tick(now)` (phát `Lost` sau 30 s) + `due_probes(now)` ⇒
  `discovery::connect(id, &devices.addresses(id), token)` ⇒ `info_ok` / `connect_failed`. Bê vào runtime tokio của `mws`.
- **Sự kiện**: mọi hàm của `Devices` trả `Vec<DeviceEvent>` (`Seen`, `Lost`, `AddressChanged`, `StateChanged`) ⇒ gộp vào
  snapshot. Runner (gd1_04): lỗi mạng ở một địa chỉ ⇒ `Devices::address_failed(id, addr)` rồi `connect` lại; không địa chỉ
  nào trả lời ⇒ `connect_failed` (⇒ Offline, thử lại sau `PROBE_INTERVAL` 60 s); `401` ⇒ `unauthorized`.
- **Mạng đổi** ⇒ `Devices::set_local_interfaces(address::local_interfaces())` (đổi thứ tự địa chỉ); ghi
  `config.devices = devices.to_config()` khi địa chỉ / tên của máy đã ghép đổi.
- **Đo C5 gd1_05** (`examples/discover`, release, macOS): mDNS thêm ~0,1 MB footprint, CPU ≈ 0 % — C4 ở đây so lại trên
  `mws` thật (mốc 10 MB). `mdns::IP_CHECK_INTERVAL_SECS` = 15 s (soát interface của `mdns-sd`); thấy đánh thức nhiều thì nâng.
- **Khớp gd1_04**: `discovery::connect` ⇒ `Connected { client, info, addr }` ⇒ `Link::new(client, &info)`; `TransferEvent::
  ConnectionLost{addr}` ⇒ `Devices::address_failed(id, addr)` rồi `connect` lại.
- **UI**: `Device::newer_api`, `Device::features` (`dirs`) cần vào snapshot (gd1_09 hiện chữ "cập nhật app trên máy tính").

## 0. Bàn giao từ media-wireless-sync-gd1_04 (11/11/2026)

<!-- handoff: media-wireless-sync-gd1_04 -->

- **Đã có sẵn** (`mws-core::transfer`): `UploadRunner::new(device, store: SharedStore, link: LinkWatch, Throttle, events,
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
- **Soát lại**: `cargo test -p mws-core --test upload`; mục 9 của `plan/media-wireless-sync-gd1_04_upload-runner.md`.

## 1. Làm gì
- `mws` chạy runtime tokio đa luồng nhỏ (2 luồng) cho discovery + một `Runner` mỗi thiết bị; vòng sự kiện khay ở luồng chính.
- `Snapshot` (thiết bị + trạng thái, hàng đợi từng thiết bị, tiến độ, "Đã gửi") gửi qua IPC khi đổi, gộp tối đa 10 lần / s; UI mới nối nhận ngay bản đầy đủ.
- Lệnh IPC: `AddFiles{device, paths}` (lọc đuôi 6.6, thư mục đệ quy), `ItemAction{id, pause|resume|cancel|retry}`, `PauseAll/ResumeAll`, `Pair/Forget/AddManualAddress`, `ListLibrary`, `Download` — ≤ 4 biến thể mới mỗi lần thêm, tách nếu vượt trần.
- Khay 3 trạng thái (rảnh / đang gửi kèm % / cần chú ý), tooltip "Bật Sync WiFi trên điện thoại để gửi N video" khi có mục chờ mà Offline; menu: Mở cửa sổ, Tạm dừng tất cả, Thoát.
- Một thông báo hệ thống khi cả lô xong; lỗi cần người dùng làm (hết chỗ, cần ghép lại) báo một lần.
- Nợ từ gd1_01: **khay Linux** — bê StatusNotifierItem tự viết của fastcast (`fastcast/crates/daemon/src/platform/linux/sni.rs`,
  không GTK / libappindicator) vào `crates/daemon/src/platform/linux.rs` (hiện chỉ kênh chặn, không icon). Khung đã có:
  `Event`, `Daemon::handle`, `tray::{menu_action, phone_rgba}`, IPC `UiToDaemon` / `DaemonToUi` + `Hello`/`Ok`.
  Mốc RAM gd1_01: `mws` rảnh footprint 10 MB (RSS 44 MB) trên macOS — C4 so với số này.

## 6. Nghiệm thu
- C1. Client IPC giả nhận snapshot đầu đủ, rồi delta theo tiến độ ≤ 10 / s. `[opus-unit]`
- C2. Thả 3 tệp qua IPC với `fake_phone` ⇒ khay chuyển "đang gửi" ⇒ "rảnh" + một thông báo. `[opus-run]`
- C3. Tắt `mws` giữa khúc, mở lại ⇒ gửi tiếp. `[opus-run]`
- C4. RAM / CPU `mws` rảnh sau task, so với mốc gd1_01. `[opus-run]`
- C5. Icon khay ba trạng thái nhìn rõ ở nền sáng / tối ba OS. `[owner-qa]`

## 8. Nợ chuyển task khác
- Icon khay: vẽ tạm; chủ dự án chọn bộ icon sau (backlog nếu cần thiết kế).

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
