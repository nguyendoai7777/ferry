# ferry-gd1_09 — add-phone

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | ferry |
| Model | sonnet |
| Cỡ | S — ~350 dòng |
| Phụ thuộc | gd1_07 |
| Quyết định áp dụng | D-13 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/ui/src/views/add_phone.rs`; SỬA `i18n/{vi,en}.toml` |

## 0. Bàn giao từ ferry-gd1_07 (11/11/2026)
<!-- handoff: ferry-gd1_07 -->

- Hợp đồng IPC đã chốt ở `ferry_core::ipc` (doc module + test `command_and_reply_wire_format` là mẫu dây). Mỗi lệnh UI mang `req: u64` tự tăng, daemon trả đúng một `Reply{req, outcome}`; `outcome` = `done` · `added{…}` · `paired{device, name}` · `found{device, name}` · `library{items}` · `failed{error: CommandError}` (`code`).
- Lệnh: `Phone{req, op}` với `pair{device, pin}` (PIN 6 số; `Pin` không lộ ra Debug / log) ⇒ `paired{device, name}`; `add_address{address}` (`ip:port`, `[v6]:port`, `[fe80::x%iface]:port`) ⇒ `found{device, name}` — sau đó điện thoại hiện trong `Snapshot.devices`, chưa ghép thì gọi tiếp `pair`; `forget{device}` ⇒ `done` (gỡ sổ + config ngay, kho khoá + hàng đợi dọn ở nền).
- Lỗi cần chữ người dùng (§2.6): `wrong_pin`, `rate_limited{retryAfterSecs}`, `other_phone` (điện thoại đang ghép với máy khác), `offline`, `unreachable`, `unknown_interface{name}`, `invalid_input`, `keyring`, `unknown_device`, `internal`.
- `DeviceView{id, name, state, visible, newerApi, features, address}` trong `Snapshot.devices`; `state` = máy trạng thái §5 (`needs_repair` ⇒ màn này mở ở bước PIN cho đúng máy).

## 0. Bàn giao từ ferry-gd1_06 (11/11/2026)

<!-- handoff: ferry-gd1_06 -->

- Lỗi ghép (qua IPC của gd1_07, gốc `pairing::PairError`) ⇒ chữ nói việc cần làm: `WrongPin` "PIN không đúng, xem lại trên
  điện thoại"; `RateLimited{retryAfter}` đếm ngược rồi mới cho nhập (thiếu `Retry-After` ⇒ 60 s); `OtherPhone` "Địa chỉ này
  giờ là điện thoại khác"; `Keyring(Unavailable)` "Cần kho khoá của hệ thống (GNOME Keyring / KWallet) để lưu ghép đôi";
  mạng ⇒ "Không nối được điện thoại — bật Sync WiFi". PIN không giữ lại sau khi gửi.

## 1. Làm gì (PROTOCOL mục 4.4)
- Bước 1: hướng dẫn "Trên điện thoại: mở app → menu → Sync WiFi → Bật".
- Bước 2: danh sách điện thoại dò thấy chưa ghép (từ snapshot), cập nhật trực tiếp; trống thì gợi ý cùng WiFi + liên kết "Nhập địa chỉ tay" (nâng cao).
- Bước 3: ô PIN 6 số (chỉ số, dán được), gửi `Pair`; sai ⇒ báo lại; `429` ⇒ đếm ngược `retry_after`; xong ⇒ về cửa sổ chính với máy vừa ghép được chọn.
- (Đề xuất gd1_05) Thiết bị có `Device::newer_api` (TXT / `info.api` lớn hơn bản PC hiểu) ⇒ dòng phụ "Cập nhật app trên
  máy tính để dùng điện thoại này" (i18n), vẫn cho ghép. Ô "Nhập địa chỉ tay" nhận mọi dạng của
  `discovery::address::parse_address` (`1.2.3.4:8080`, `[fd12::5]:8080`, `[fe80::5%en0]:8080`, cả URL màn Sync hiện);
  lỗi `UnknownInterface` ⇒ "Không thấy card mạng <tên> trên máy tính".

## 6. Nghiệm thu
- C1. Ô PIN nhận 6 số, từ chối chữ, dán "123 456" thành 123456. `[sonnet-test]`
- C2. Ghép với điện thoại thật / emulator theo PROTOCOL mục 7. `[owner-qa]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
