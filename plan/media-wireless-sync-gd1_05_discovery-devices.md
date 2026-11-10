# media-wireless-sync-gd1_05 — discovery-devices

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | M — ~500 dòng |
| Phụ thuộc | gd1_02 |
| Quyết định áp dụng | W-1, D-3 |
| Phụ thuộc mới | `mdns-sd` (MIT/Apache) — đo RAM / CPU rảnh của `mws` trước / sau |
| File đụng tới | MỚI `crates/core/src/{discovery,device}.rs` |

## 1. Làm gì
- Browse `_vpsync._tcp`; đọc TXT `api`, `id`; chọn IPv4 cùng subnet PC trước (PROTOCOL 6.2). `api` lớn hơn bản hiểu ⇒ vẫn gọi `info`, đánh dấu "cần cập nhật app".
- Địa chỉ tay `ip:port` ⇒ `info` lấy `id`; lưu vào config là danh sách địa chỉ thử định kỳ (thưa, không poll dày).
- Máy trạng thái thiết bị PROTOCOL mục 5 (`Discovered, Paired, Online, Offline, NeedsRepair`) — hàm thuần; mất mDNS 30 s ⇒ Offline; IP mới ⇒ cập nhật, không hỏi.
- Sự kiện `DeviceEvent::{Seen, Lost, AddressChanged, StateChanged}` ra kênh.

## 6. Nghiệm thu
- C1. Máy trạng thái: mọi mũi tên mục 5 có test. `[opus-unit]`
- C2. Quảng bá `_vpsync._tcp` giả bằng chính `mdns-sd` trong test ⇒ Seen với đúng `id`; dừng quảng bá ⇒ Lost sau 30 s (đồng hồ giả). `[opus-unit]`
- C3. Hai địa chỉ (IPv6 + IPv4 cùng subnet) ⇒ chọn IPv4. `[opus-unit]`
- C4. Điện thoại thật cùng WiFi bật Sync WiFi ⇒ hiện trong ≤ 5 s. `[owner-qa]`
- C5. CPU `mws` rảnh khi có mDNS ≈ 0 %, ghi RAM trước / sau vào mục 9. `[opus-run]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
