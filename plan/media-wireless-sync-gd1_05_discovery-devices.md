# media-wireless-sync-gd1_05 — discovery-devices

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | M — ~500 dòng |
| Phụ thuộc | gd1_02 |
| Quyết định áp dụng | W-1, W-3, D-3 |
| Phụ thuộc mới | `mdns-sd` (MIT/Apache) — đo RAM / CPU rảnh của `mws` trước / sau |
| File đụng tới | MỚI `crates/core/src/{discovery,device}.rs` |

## 0. Bàn giao từ media-wireless-sync-gd1_02 (11/11/2026)

<!-- handoff: media-wireless-sync-gd1_02 -->

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

## 6. Nghiệm thu
- C1. Máy trạng thái: mọi mũi tên mục 5 có test. `[opus-unit]`
- C2. Quảng bá `_vpsync._tcp` giả bằng chính `mdns-sd` trong test ⇒ Seen với đúng `id`; dừng quảng bá ⇒ Lost sau 30 s (đồng hồ giả). `[opus-unit]`
- C3. Hai địa chỉ (IPv6 + IPv4 cùng subnet) ⇒ chọn IPv4. `[opus-unit]`
- C4. Điện thoại thật cùng WiFi bật Sync WiFi ⇒ hiện trong ≤ 5 s. `[owner-qa]`
- C5. CPU `mws` rảnh khi có mDNS ≈ 0 %, ghi RAM trước / sau vào mục 9. `[opus-run]`
- C-W3a. Thứ tự chọn địa chỉ (bảng ca: chỉ v4, chỉ v6, link-local, v4 sai subnet) — hàm thuần. `[opus-unit]`
- C-W3b. Địa chỉ đầu không nối được ⇒ địa chỉ kế thắng trong ≤ 300 ms (server giả nghe `[::1]` chỉ). `[opus-unit]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
