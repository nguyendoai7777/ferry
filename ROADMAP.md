# ferry — lộ trình

App PC (Windows · macOS · Linux, D-3) gửi video sang app điện thoại `video-player` qua WiFi nhà, theo hợp đồng
`PROTOCOL.md` (W-1). Khởi tạo 11/11/2026. Task: `plan/ferry-gd<N>_*.md`, trạng thái ở từng tệp task (một nguồn sự thật).

## GĐ1 — bản dùng được hằng ngày (v1 của PROTOCOL)

Đích: ghép một điện thoại, thả video, tắt app / đổi WiFi / điện thoại tắt Sync giữa chừng mà vẫn gửi tiếp từ byte cuối;
kéo video về máy; khay + thông báo; bản cài ba OS.

| Task | Việc | Phụ thuộc | Model |
| --- | --- | --- | --- |
| `gd1_01` `workspace` | cargo workspace 3 crate, deny, i18n, paths, log, config, khung IPC, khay rỗng + cửa sổ rỗng, `start.sh`, nối script gốc | — | opus |
| `gd1_02` `protocol-client` | kiểu §6 + client HTTP (info, pair, list, uploads, file) + server giả `fake_phone` cho test | 01 | opus |
| `gd1_03` `queue-store` | hàng đợi bền SQLite, máy trạng thái mục upload / download (§5), backoff — hàm thuần + lưu đĩa | 01 | opus |
| `gd1_04` `upload-runner` | chạy hàng đợi gửi: Checking → Creating → Sending nối tiếp, mọi nhánh lỗi §5, giới hạn băng thông (gỡ 12/11/2026) | 02, 03 | opus |
| `gd1_05` `discovery-devices` | mDNS `_vpsync._tcp` + địa chỉ tay, máy trạng thái thiết bị, chọn IPv4 cùng subnet | 02 | opus |
| `gd1_06` `pairing-keyring` | ghép bằng PIN, token vào kho khoá OS, client id, quên điện thoại, `NeedsRepair` | 02, 05 | opus |
| `gd1_07` `daemon-runtime` | runtime tokio trong `ferry`, snapshot + lệnh qua IPC, khay 3 trạng thái + menu, thông báo cả lô xong | 04, 05, 06 | opus |
| `gd1_08` `main-window` | cửa sổ chính: thiết bị, vùng thả + "Chọn tệp…" / "Chọn thư mục…", khay chờ, trình duyệt điện thoại kiểu Explorer (W-4), hàng đợi + "Đã gửi" | 07 | sonnet |
| `gd1_09` `add-phone` | màn thêm điện thoại 3 bước (hướng dẫn, danh sách dò thấy, PIN) | 07 | sonnet |
| `gd1_10` `download-runner` | tải về nối tiếp `Range` + `If-Range` / `ETag`, `.part`, thư mục lưu | 03, 07 | opus |
| `gd1_11` `phone-library` | tab thư viện điện thoại (dùng lại `PhoneBrowser`) + "Tải về…" | 08, 10 | sonnet |
| `gd1_12` `settings` | cài đặt: thiết bị đã ghép, thư mục lưu / theo dõi, giới hạn băng thông (gỡ 12/11/2026), khởi động cùng máy, địa chỉ tay (gỡ 12/11/2026) | 07 | sonnet |
| `gd1_13` `watch-folder` | thư mục "Gửi sang điện thoại": tệp mới ổn định ⇒ vào hàng đợi | 04, 12 | opus |
| `gd1_14` `packaging` | `build.sh` / `build.ps1`, bản cài ba OS, trang giấy phép bên thứ ba, nối `build.*` / `tag.*` gốc | 08–13 | opus |

**Bổ sung W-3 (11/11/2026)**: IPv4 + IPv6 (PROTOCOL 6.8) và đẩy thư mục như copy/paste (6.9) — gộp vào 02, 03, 04, 05,
08, 11, 13 (mục "Bổ sung W-3" trong từng task). Phía Android: `video-player` task_10 (IPv6) + task_11 (thư mục).

**GĐ1 xong mã 11/11/2026** (gd1_14 cuối): bản cài `bash build.sh` / `pwsh ./build.ps1` ⇒ `<gốc>/release/ferry/`
(.deb Linux đã dựng + chạy thử: 13 MB, RSS rảnh 12 MB). Còn kiểm tay `[owner-qa]` ở `plan/_backlog.md` (bản cài Windows / macOS,
C1 máy sạch, ghép + khởi động lại, tự chạy, icon khay) rồi `close-phase`. Bàn giao từng task ở mục 9 của tệp task.

Làn song song sau 01: {02 → 04, 05 → 06} và 03 chạy song song (khác tệp); 08, 09, 12 song song sau 07.

## Ngoài GĐ1

PROTOCOL mục 8: xoá video từ PC, đồng bộ hai chiều, TLS / ghép QR, đánh thức điện thoại, gửi song song nhiều máy, iOS.
