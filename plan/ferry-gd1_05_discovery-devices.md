# ferry-gd1_05 — discovery-devices

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ 11/11/2026 |
| Repo | ferry |
| Model | opus |
| Cỡ | M — ~500 dòng |
| Phụ thuộc | gd1_02 |
| Quyết định áp dụng | W-1, W-3, D-3 |
| Phụ thuộc mới | `mdns-sd` (MIT/Apache) — đo RAM / CPU rảnh của `ferry` trước / sau |
| File đụng tới | MỚI `crates/core/src/discovery/{mod,address,mdns}.rs`, `crates/core/tests/discovery.rs`, `crates/core/examples/discover.rs`; SỬA `crates/core/src/device.rs`, `tests/fake_phone/mod.rs` (chỉ thêm `start_v6_only`) |

## 0. Bàn giao từ ferry-gd1_02 (11/11/2026)

<!-- handoff: ferry-gd1_02 -->

- **Đã có sẵn**: `client::PhoneClient::new(addr: SocketAddr, device: Option<&DeviceId>, token)` — một client một địa chỉ,
  host giả `<id>.vpsync.invalid` + `resolve` (v4 / v6 / v6 + `scope_id` đều chạy, đã test). Happy eyeballs = đua nhiều
  `PhoneClient` (`info()`), client tự không đổi địa chỉ. `connect_timeout` 10 s (`client::CONNECT_TIMEOUT`) — đua thì tự
  `tokio::time::timeout`.
- **`protocol::Info`**: `id: DeviceId`, `api`, `port: Option<u16>` (điện thoại cũ `None` ⇒ cổng đang nối), `addresses:
  Vec<String>` (IP không cổng, không link-local), `features`, `paired`; `protocol::API_VERSION = 1`.
- Nhập tay chưa biết `id` ⇒ `PhoneClient::new(addr, None, None).info()` rồi dựng lại với `Some(&info.id)`.
- `ClientError::Network(e)`: `e.kind()` = `ConnectionRefused` khi nối hỏng (địa chỉ chết), `TimedOut`, `Other` đứt giữa chừng.
- **`fake_phone`** (`tests/fake_phone/mod.rs`) nghe cả `127.0.0.1` và `[::1]` cùng cổng, `info.addresses = ["127.0.0.1",
  "::1"]`; C-W3b ("chỉ nghe `[::1]`") cần thêm nút vào tệp này (vd `start_v6_only`).
- **Bẫy**: `adb forward` chỉ nghe `127.0.0.1` ⇒ IPv6 / mDNS thật phải dùng điện thoại thật cùng WiFi.

## 1. Làm gì
- Browse `_vpsync._tcp`; đọc TXT `api`, `id`; thứ tự địa chỉ theo PROTOCOL 6.2 (xem bổ sung W-3 dưới). `api` lớn hơn bản hiểu ⇒ vẫn gọi `info`, đánh dấu "cần cập nhật app".
- Địa chỉ tay `ip:port` ⇒ `info` lấy `id`; lưu vào config là danh sách địa chỉ thử định kỳ (thưa, không poll dày).
- Máy trạng thái thiết bị PROTOCOL mục 5 (`Discovered, Paired, Online, Offline, NeedsRepair`) — hàm thuần; mất mDNS 30 s ⇒ Offline; IP mới ⇒ cập nhật, không hỏi.
- Sự kiện `DeviceEvent::{Seen, Lost, AddressChanged, StateChanged}` ra kênh.
- Bổ sung W-3 (PROTOCOL 6.2, 6.8): thiết bị = `id` + **tập địa chỉ** từ A + AAAA + `info.addresses`; thứ tự thử IPv4 cùng subnet → IPv6 global/ULA cùng /64 → link-local + `scope_id` của interface nhận bản ghi → còn lại; nối kiểu happy eyeballs (thử kế sau 250 ms), nhớ địa chỉ thắng tới khi lỗi. Địa chỉ tay nhận `[v6]:port` và `[fe80::x%iface]:port`. Mạng chỉ IPv6 hoặc chỉ IPv4 đều chạy.

## 2b. Phân vai

| Phần | Ai |
|---|---|
| Máy trạng thái + sổ `Devices` (`device.rs`), thứ tự địa chỉ + nhập tay (`discovery/address.rs`), happy eyeballs + `connect` / `probe` (`discovery/mod.rs`), `Browser` mDNS (`discovery/mdns.rs`), test | [opus] |
| Điện thoại thật cùng WiFi hiện ≤ 5 s (C4) | [owner-qa] |

## 6. Nghiệm thu
- C1. Máy trạng thái: mọi mũi tên mục 5 có test. `[opus-unit]`
- C2. Quảng bá `_vpsync._tcp` giả bằng chính `mdns-sd` trong test ⇒ Seen với đúng `id`; dừng quảng bá ⇒ Lost sau 30 s (đồng hồ giả). `[opus-unit]`
- C3. Hai địa chỉ (IPv6 + IPv4 cùng subnet) ⇒ chọn IPv4. `[opus-unit]`
- C4. Điện thoại thật cùng WiFi bật Sync WiFi ⇒ hiện trong ≤ 5 s. `[owner-qa]` — **chờ kiểm tay** (lệnh ở mục 9).
- C5. CPU `ferry` rảnh khi có mDNS ≈ 0 %, ghi RAM trước / sau vào mục 9. `[opus-run]`
- C-W3a. Thứ tự chọn địa chỉ (bảng ca: chỉ v4, chỉ v6, link-local, v4 sai subnet) — hàm thuần. `[opus-unit]`
- C-W3b. Địa chỉ đầu không nối được ⇒ địa chỉ kế thắng trong ≤ 300 ms (server giả nghe `[::1]` chỉ). `[opus-unit]`

## 9. Bàn giao thực tế

**Đã làm** (11/11/2026, Opus):
- `device.rs`: `DeviceState` (6 trạng thái §5), `DeviceInput`, `next_state` (hàm thuần), `DeviceEvent::{Seen, Lost,
  AddressChanged, StateChanged}`, `Announcement`, sổ `Devices` — mọi hàm nhận `now` (đồng hồ giả trong test),
  `next_deadline()` là mốc duy nhất để đánh thức (không poll): `withdraw` + 30 s ⇒ `tick` phát `Lost`, `due_probes` cho
  thiết bị có token chưa Online (ngay khi thấy lại / nạp config, rồi mỗi `PROBE_INTERVAL` 60 s). `info_ok` gộp
  `info.addresses` + `port`, nhớ địa chỉ thắng (lên đầu `addresses()`), `address_failed` bỏ địa chỉ thắng, `connect_failed`
  ⇒ Offline. `from_config` / `to_config` đọc / ghi `PairedDevice.addresses` (link-local ghi tên interface `%en0`).
- `discovery/address.rs`: `local_interfaces()` (`if-addrs`), `rank` / `order_addresses` theo §6.2 (v4 cùng subnet → v6
  global/ULA cùng /64 → link-local có `scope_id` → còn lại; cùng hạng giữ thứ tự, bỏ trùng), `parse_address` (nhập tay:
  `ip`, `ip:port`, `[v6]:port`, `v6` trần, `[fe80::x%en0]:port`, `%12` Windows, cả URL `http://…/` màn Sync hiện; thiếu
  cổng ⇒ 8080), `format_address`, `from_info` (bỏ link-local của `info`).
- `discovery/mdns.rs`: `Browser::start()` (`mdns-sd`, browse `_vpsync._tcp.local.`, soát interface 15 s thay 5 s mặc định),
  `next().await` ⇒ `MdnsEvent::{Announced, Withdrawn}`; TXT `id` bắt buộc, `api` thiếu ⇒ 1, `api` lớn hơn ⇒ `newer_api`
  (vẫn gọi `info`); link-local mang `scope_id` = index interface nhận bản ghi.
- `discovery/mod.rs`: `race(addrs, 250 ms, f)` (happy eyeballs: thử kế sau 250 ms hoặc ngay khi lần đang chạy hỏng; thắng
  đầu tiên huỷ phần còn lại), `connect(id, addrs, token) -> Connected { client, info, addr }` (soát `info.id` — IP cấp lại
  cho máy khác không bị nhận nhầm; trần cả cuộc đua 12 s), `probe(addr)` cho nhập tay.
- `examples/discover.rs`: vòng lặp kiểu daemon (chờ mDNS / `next_deadline`), in sự kiện + `info` — để kiểm C4 và đo.

**Opus chọn (11/11/2026)**:
- "thấy lại → Online" đi qua `info`: `Seen` ở Offline không đổi trạng thái, chỉ hẹn `info` ngay — để token chết ra
  `NeedsRepair` chứ không báo Online giả. `Paired` (nạp config, chưa xác nhận) đi Offline / NeedsRepair như Online.
  `NeedsRepair` chỉ thoát bằng ghép lại / quên. Discovered mất mDNS ⇒ ra khỏi sổ (Unknown).
- Cùng tên instance mà `id` khác (cài lại app) ⇒ máy cũ `Lost` ngay.
- Daemon chưa chạy discovery (runtime tokio là gd1_07) ⇒ C5 đo bằng `examples/discover` (cùng vòng lặp daemon sẽ chạy).
- `mdns-sd` giữ feature mặc định (`async` = `flume/async`, để `recv_async` trong tokio; không thêm luồng chuyển tiếp).

**Đo C5** (macOS, bản release, sau 80 s rảnh, không điện thoại trên LAN): `FERRY_NO_MDNS=1` (chỉ runtime) footprint
1,66 MB / RSS 6,4 MB / CPU 0:00.01; có mDNS 1,78 MB / 6,99 MB / 0:00.02 ⇒ **mDNS thêm ~0,1 MB footprint, CPU ≈ 0 %**.
Mốc `ferry` gd1_01 = 10 MB footprint ⇒ dự kiến ~10,1 MB sau khi gd1_07 nối vào; gd1_07 C4 đo lại trên `ferry` thật.
Phụ thuộc mới: `mdns-sd` 0.21.7 (MIT/Apache; kéo `flume`, `mio`, `socket2`, `socket-pktinfo`, `fastrand`), `if-addrs`
0.15 (MIT/BSD-3, đã có qua `mdns-sd`); `cargo deny check licenses` ok.

**Case**: C1 `device::tests::state_machine_arrows` (+ `stays`); C2 `tests/discovery.rs::mdns_announce_then_goodbye` (mDNS thật
trong tiến trình, thấy sau ~0,9 s; goodbye ⇒ `Lost` đúng mốc 30 s đồng hồ giả; máy không có interface ngoài loopback thì
bỏ qua); C3 `address::tests::same_subnet_v4_beats_v6`; C-W3a `address::tests::order_table` (7 ca: dual stack, chỉ v4,
chỉ v6, chỉ link-local, v4 sai subnet, link-local PC không tính /64, không rõ interface); C-W3b
`dead_v4_then_v6_only_phone_wins_fast` (v4 từ chối ⇒ v6 thắng ≤ 300 ms; v4 nhận kết nối mà im ⇒ v6 thắng ở 250–400 ms)
+ `race` với đồng hồ giả (4 test). Thêm: nhập tay `[::1]:port`, IP của máy khác bị loại, đổi interface ⇒ đổi thứ tự.
Soát: `cargo build`, `cargo clippy --all-targets -- -D warnings` (+ `-p ferry-core` chéo `x86_64-pc-windows-msvc`,
`x86_64-unknown-linux-gnu`), `cargo test`, `cargo deny check licenses` — xanh.

**Chờ kiểm tay (C4)**: điện thoại thật cùng WiFi với PC, app `video-player` bật Sync WiFi, rồi
`cargo run -p ferry-core --example discover` ⇒ dòng `Seen { … }` + `info via …` trong ≤ 5 s (emulator không quảng bá
mDNS ra LAN của Mac). Ghi ở `plan/_backlog.md`.

## 10. Đề xuất cải tiến

- [ram] `mdns-sd` soát interface mỗi 5 s mặc định — đã hạ 15 s (`IP_CHECK_INTERVAL_SECS`); có thể nâng 30 s nếu đo
  `ferry` thật (gd1_07) thấy đánh thức đáng kể — S — P3. Chưa làm: chờ số đo của daemon thật.
- [ux] `Browser` báo `newer_api` — UI (gd1_09) cần chữ "Cập nhật app trên máy tính để dùng điện thoại này" — S — P2.
  Đã ghi thẳng vào mục 1 của gd1_09 (kèm thông báo lỗi nhập tay `UnknownInterface`).
