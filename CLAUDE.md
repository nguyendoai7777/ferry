# media-wireless-sync — app PC gửi video sang điện thoại qua WiFi

Repo git riêng trong workspace `xdesktop` (luật chung, quyết định D-* / W-*: `../CLAUDE.md`, `../.claude/rules/decisions/`).
App khay hệ thống nối PC với app điện thoại `video-player` qua WiFi nhà: thả video vào là chảy sang điện thoại, đứt thì
nối tiếp từ byte cuối, xong thì im lặng; kéo video từ điện thoại về là phụ. Lộ trình: `ROADMAP.md`. Task:
`plan/media-wireless-sync-gd<N>_*.md` (skill `run-task`), việc treo `plan/_backlog.md`. Skill nạp ngữ cảnh: `/media-wireless-sync`.

**`PROTOCOL.md` là hợp đồng với điện thoại** (W-1), không phải tài liệu thường: triết lý (mục 2), màn hình (4), máy trạng
thái (5), giao thức HTTP v1 (6). Mã lõi trỏ về mục của nó (`// PROTOCOL 6.5`). Đổi giao thức ⇒ sửa `PROTOCOL.md` trước,
rồi sửa cả hai phía; không tự thêm route, header, mã trạng thái ở phía PC.

**Đang ở giai đoạn khởi tạo** (11/11/2026): repo mới có tài liệu + plan, chưa có mã. Task đầu là
`plan/media-wireless-sync-gd1_01_workspace.md`.

## Bố cục (cargo workspace — dự kiến, task 01 dựng)

Tách hai tiến trình như fastcast (`fst` / `fastcast-ui`) vì luật "RAM là tính năng": app chạy nền cả ngày ở khay, cửa sổ
chỉ mở vài lần.

| Crate | Loại | Việc | Không được mang |
| --- | --- | --- | --- |
| `crates/core` — `mws-core` | lib | `protocol` (kiểu JSON + header §6), `client` (HTTP §6.3–6.5), `discovery` (mDNS §6.2 + địa chỉ tay), `pairing` (ghép + kho khoá), `device` (máy trạng thái thiết bị §5), `queue` (hàng đợi bền + máy trạng thái mục §5), `transfer` (chạy hàng đợi: upload nối tiếp, tải về `Range`), `watch` (thư mục theo dõi), `config`, `paths`, `i18n`, `logging`, `ipc` | UI, GPU |
| `crates/daemon` — bin `mws` | bin | runtime tokio của lõi, khay (3 trạng thái), IPC server, bật tiến trình UI, một instance, thông báo hệ thống, autostart | GPUI, materiant |
| `crates/ui` — bin `mws-ui` | bin | cửa sổ chính (vùng thả + hàng đợi), thư viện điện thoại, thêm điện thoại, cài đặt — GPUI + materiant; chỉ vẽ ảnh chụp trạng thái nhận qua IPC và gửi lệnh | mạng, kho khoá, SQLite |
| `crates/core/tests/fake_phone` | test | server giả trong tiến trình đúng hợp đồng §6 (đứt giữa khúc, `409`, `404`, `507`, `401`, `423`, `503`) | — |

- UI không gọi mạng: mọi lệnh đi IPC sang `mws`, `mws` gửi ảnh chụp trạng thái (snapshot) khi đổi, không chia sẻ mutex.
- Đóng cửa sổ ⇒ tiến trình UI thoát (nhả GPU / RAM); truyền vẫn chạy ở `mws`.

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

Chưa có (task 01 thêm `start.sh`, soát xanh). Soát xanh mỗi task: `cargo build`, `cargo clippy --all-targets -- -D warnings`,
`cargo test`, `cargo deny check licenses`.

Thử không cần điện thoại thật: PROTOCOL mục 7 (emulator + `adb forward tcp:18080 tcp:8080`, nhập tay `127.0.0.1:18080`).

## Bẫy

(chưa có — ghi khi gặp)
