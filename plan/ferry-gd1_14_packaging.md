# ferry-gd1_14 — packaging

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | ferry (+ gốc: `build.*`, `tag.*`) |
| Model | opus |
| Cỡ | M |
| Phụ thuộc | gd1_08 … gd1_13 |
| Quyết định áp dụng | D-2, D-3, D-21, D-22 |
| Phụ thuộc mới | `cargo-about` (công cụ build) |
| File đụng tới | MỚI `build.sh`, `build.ps1`, `packaging/…`, `release-notes/TEMPLATE.md`; SỬA gốc `build.sh`, `build.ps1`, `tag.sh`, `tag.ps1` |

## 1. Làm gì
- Bản cài theo khuôn tên gốc: Windows NSIS, macOS `.app` + `.dmg`, Linux `.deb` (học `groove/build.*`, `groove/packaging`).
- Hai exe (`ferry`, `ferry-ui`) trong một gói; autostart trỏ `ferry`.
- Windows: luật tường lửa cho mDNS (UDP 5353 vào) lúc cài — không có thì dò không chạy trên mạng "Public".
- macOS: `NSLocalNetworkUsageDescription` + `NSBonjourServices` (`_vpsync._tcp`) trong `Info.plist` (macOS 15 chặn mạng LAN nếu thiếu).
- Trang giấy phép bên thứ ba; thông tin phát hành D-21; release note D-22; nối radio của `build.*` / `tag.*` gốc.

## 6. Nghiệm thu
- C1. Cài trên máy sạch mỗi OS ⇒ dò thấy điện thoại, gửi được 1 tệp. `[owner-qa]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
