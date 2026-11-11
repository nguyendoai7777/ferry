# ferry-gd1_10 — download-runner

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ (11/11/2026) |
| Repo | ferry |
| Model | opus |
| Cỡ | S — ~400 dòng |
| Phụ thuộc | gd1_03, gd1_07 |
| Quyết định áp dụng | W-1 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/core/src/transfer/download.rs`, `crates/core/tests/download.rs`; SỬA `crates/core/src/config.rs` (`downloadDir`), `transfer/{mod,upload,throttle}.rs`, `queue/{state,store}.rs`, `client.rs` (`FileDownload.file_name`), `tests/fake_phone/mod.rs` (thêm); sau gd1_07: `crates/core/src/ipc.rs` (chỉ thêm), `crates/daemon/src/engine.rs` (lệnh `Download`, snapshot) |

## 0. Bàn giao từ ferry-gd1_07 (11/11/2026)
<!-- handoff: ferry-gd1_07 -->

- Hợp đồng IPC đã chốt ở `ferry_core::ipc` (doc module + test `command_and_reply_wire_format` là mẫu dây). Mỗi lệnh UI mang `req: u64` tự tăng, daemon trả đúng một `Reply{req, outcome}`; `outcome` = `done` · `added{…}` · `paired{device, name}` · `found{device, name}` · `library{items}` · `failed{error: CommandError}` (`code`).
- Download **chưa có** trên IPC: gd1_10 thêm nhánh vào `QueueOp` / `LibraryOp` (không thêm biến thể `UiToDaemon`; trần ≤ 4 biến thể mới). `Library{req, op: list{device}}` ⇒ `library{items}` đã chạy (engine gọi `client::list` với token).
- Móc vào engine (`crates/daemon/src/engine.rs`): `Op` ⇒ `handle_request`; việc chậm chạy task con trả `Internal` về `select!`; runner theo thiết bị nằm ở `runners`, nhận `Link` khi thiết bị Online. Ảnh chụp dựng ở `snapshot_parts` — thêm `downloads` vào `Snapshot` ở cuối (trường mới `#[serde(default)]`). Đẩy chỉ qua `Outbox::push`, không chặn engine.
- `config.rs`, `transfer/*` không bị gd1_07 đụng.
- Sau rebase lên lõi gd1_10: `on_transfer` có nhánh tạm `DownloadChanged` ⇒ `dirty = true` (chỉ log). `Progress` của mục tải về hiện rơi vào cùng bảng `progress` theo `id` với upload — hai bảng SQLite có thể trùng `id`, nên khi nối phải tách khoá (vd `Progress` mang chiều, hoặc `ItemProgress` thêm `kind`).

## 0. Bàn giao từ ferry-gd1_04 (11/11/2026)

<!-- handoff: ferry-gd1_04 -->

- "Tuần tự với upload" trên cùng thiết bị: `transfer::UploadRunner` hiện chỉ chạy upload (`QueueStore::pending_upload`).
  Gắn tải về vào **cùng vòng** (mở rộng runner, vd chọn đầu hàng chung theo `id` của `items`), đừng chạy runner tải về song
  song cho cùng điện thoại. Mẫu dùng lại: `Link` / `LinkWatch` (địa chỉ đổi giữa hai `Range`), `Command`, `TransferEvent`
  (thêm biến thể cho download), `Throttle` + `SpeedMeter` (`transfer/throttle.rs`), map lỗi theo pha (`Exec::map_error`).
- **Soát lại**: `cargo test -p ferry-core --test upload` vẫn xanh sau khi sửa vòng.

## 0. Bàn giao từ gd1_03 (11/11/2026)

<!-- handoff: ferry-gd1_03 -->

- Máy trạng thái tải về có sẵn: `queue::state::DownloadMachine` (`Queued, Fetching{offset, etag}, Waiting, Done,
  Failed, Cancelled`), effect `Get{offset, etag}` / `Truncate` (nhận `200` khi đang tải tiếp) / `DiscardPartial` /
  `Wait`. Lưu: `QueueStore::add_download(device, &NewDownload, now)`, `next_download`, `apply_download` — cùng bảng
  `items` với upload (`kind = 'download'`), mở lại thì `Fetching` ⇒ `Waiting`. Báo `Progress{offset}` theo chặng
  (vài MB), không mỗi gói.

## 1. Làm gì (PROTOCOL 6.4)
- Mục download trong cùng hàng đợi thiết bị (tuần tự với upload): ghi `<tên>.part` trong `downloadDir` (mặc định thư mục Video / Downloads của OS), lưu `ETag` + offset.
- Tiếp: `Range: bytes=<đã có>-` + `If-Range`; `200` ⇒ ghi lại từ đầu; `416` ⇒ coi như xong nếu cỡ khớp.
- Xong ⇒ đổi tên `.part` thành tên từ `Content-Disposition filename*` (lọc ký tự cấm theo OS, trùng thì thêm ` (2)`).

## 2b. Phân vai
| Phần | Vai |
|---|---|
| `transfer/download.rs` (GET nối tiếp, `.part`, tên đích), gắn vào vòng runner thiết bị, Pause/Resume của máy tải về | `[opus]` |
| Test `tests/download.rs` + nút điện thoại giả | `[opus-unit]` |
| Nối lệnh IPC `Download` + snapshot hàng đợi chiều về ở daemon (sau gd1_07) | `[opus]` — còn lại |

## 6. Nghiệm thu
- C1. Cho tệp 2 MB, đứt sau 700 KB · Khi runner tải tiếp · Thì `GET` thứ hai có `Range: bytes=716800-` + `If-Range: <etag>`, tệp khớp từng byte (mạnh hơn so sha256). `[opus-unit]` ✅ `c1_cut_mid_body_resumes_from_offset`; tắt app giữa chừng ⇒ mở lại tiếp từ cỡ `.part` ✅ `c1b_resume_after_app_restart`
- C2. Cho đứt giữa chừng rồi tệp trên điện thoại đổi (ETag mới) · Khi tải tiếp với `If-Range` cũ · Thì nhận `200`, ghi lại từ 0, ra đúng bản mới. `[opus-unit]` ✅ `c2_etag_change_restarts_from_zero`
- C3. Cho tên `Chuyến đi: ngày 1?.mp4` và tệp trùng sẵn có · Thì ra `Chuyến đi_ ngày 1_ (2).mp4`, tệp cũ còn nguyên. `[opus-unit]` ✅ `c3_forbidden_chars_and_duplicate_name` (+ unit `sanitize_*`: `\ / * " < > |`, điều khiển, `.` đầu / cuối, `CON` / `COM1`, cắt 255 byte giữ đuôi)
- Nhánh lỗi thêm `[opus-unit]` ✅: `404` ⇒ Failed + xoá `.part`; `503` + `Retry-After` ⇒ Waiting rồi xong; `401` ⇒ `DeviceNeedsRepair`, hàng dừng tới `Link` mới; `416` với `.part` đủ cỡ ⇒ xong không tải lại; tạm dừng / chạy tiếp (`Range`) / huỷ (xoá `.part`); upload + tải về chung hàng, theo thứ tự thả.
- C4. Lệnh IPC `Download` ⇒ mục vào hàng, snapshot thấy tiến độ. `[opus-unit]` mức engine ✅ `download_command_queues_and_fetches_into_download_dir` (daemon + fake_phone: `DownloadsAdded{added 1, alreadyQueued 1, missing 1}`, tệp ra `downloadDir` khớp byte, `downloads[0]` tới `Done`); trần dòng ✅ `huge_queue_snapshot_stays_under_the_line_limit` (+1000 mục tải về). Bấm qua UI: gd1_11.

## 8. Nợ chuyển task khác

- gd1_11: nút "Tải về máy" + hàng chiều về trong UI (hợp đồng IPC ở khối bàn giao của gd1_11).
- Khay chỉ tính upload (`tray_state::summarize`): đang tải về thì khay vẫn Idle — chữ khay "Đang gửi" không hợp với tải về; quyết khi làm gd1_13 hoặc sau.

### Đã xong (11/11/2026) — nối engine sau gd1_07

- `crates/daemon` (engine của gd1_07): lệnh IPC `Download { device, remoteIds }` (gd1_07 mục 1 đã liệt kê) ⇒ với mỗi
  `LibraryItem` từ `list`: `QueueStore::add_download(device, &NewDownload { remote_id, name, remote_dir, size, dest:
  transfer::download::default_dest(&config.download_dir(), &name) }, now)` rồi `RunnerHandle::send(Command::Wake)`.
  Không runner riêng: mục tải về chạy trong `UploadRunner` sẵn có của điện thoại.
- Snapshot hàng đợi: thêm `QueueStore::downloads()` (chiều về); xử lý `TransferEvent::DownloadChanged { id, state }`
  (biến thể mới — `match` không `_` trong engine phải thêm nhánh) và `Progress` (cùng biến thể với upload, `sent` = byte
  đã nhận). `ItemAction{id, …}` đi qua `Command::Item` như upload — runner tự nhận ra mục tải về.
- Soát lại `cargo test -p ferry-core --test upload --test download` sau khi nối.

## 9. Bàn giao thực tế

- Lệnh soát (worktree `ferry-w2`, 11/11/2026): `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
  (core 110 unit + `download` 10 + `upload` 14 …), `cargo deny check licenses` — xanh. `--test download` chạy 3 lần liền
  xanh (~2 s).
- **Gắn vào vòng chung** (bàn giao gd1_04): `UploadRunner::next_work` chọn mục sẵn sàng có `id` nhỏ nhất giữa
  `pending_upload` và `pending_download` (mới), không có thì ngủ tới đồng hồ sớm nhất. Giữ tên `UploadRunner` để không
  vỡ mã gd1_07 đang viết song song (Opus chọn 11/11: đổi tên sau, khi không còn làn mở).
- **Offset thật = cỡ `.part`**, store chỉ lưu theo nhịp tiến độ (500 ms; sập app chỉ mất phần chưa lưu, vẫn tiếp từ cỡ
  tệp). `Get{offset: 0}` ⇒ cắt `.part` (mục mới / `416` / tệp đổi).
- **`.part` = `<đích>.<id>.part`** thay vì `<tên>.part` (Opus chọn 11/11: hai mục cùng tên — kể cả hai điện thoại — không
  giẫm nhau; cùng thư mục với đích nên đổi tên nguyên tử).
- **Tên đích**: `Content-Disposition filename*` (thêm `FileDownload::file_name` ở `client.rs`), thiếu thì tên lúc xếp hàng;
  một luật lọc cho cả ba OS (Opus chọn 11/11: tệp hay bị chép sang ổ Windows / exFAT; Linux / macOS chỉ cấm `/` thì quá lỏng).
  `/` và `\` thành `_` ⇒ tên từ điện thoại không đi được ra ngoài `downloadDir`. Trùng ⇒ ` (2)`, ` (3)` …; đường dẫn
  thật ghi lại bằng `QueueStore::set_download_dest` (mới).
- **`downloadDir`** (`config.json`, vắng ⇒ không ghi): mặc định thư mục Downloads của OS, thiếu thì Video, rồi thư mục nhà
  (Opus chọn 11/11: app gửi mọi loại tệp — W-5 — nên không mặc định Video). `Config::download_dir()`.
- **Máy trạng thái thêm `Paused { offset, etag }`** + sự kiện `Pause` / `Resume` (gd1_03 chưa có — nút tạm dừng của hàng
  đợi cần nó); `Resume` ⇒ `Waiting` tới hạn ngay ⇒ `Get` từ offset đã có.
- **Lỗi**: mạng / `5xx` / `503` / `429` / thân ngắn / phản hồi sai ⇒ backoff; `401` ⇒ như upload (`repair_blocked`,
  `DeviceNeedsRepair`); `404` và `4xx` lạ ⇒ Failed RemoteGone + xoá `.part` (không vòng thử lại mãi); lỗi ghi đĩa ⇒ Failed
  LocalWrite, giữ `.part` cho "Thử lại".
- **Băng thông**: `MeteredWriter` (throttle.rs) dùng chung xô `Throttle` với upload — ghi chậm ⇒ đọc socket chậm, không gom RAM.
- `fake_phone`: thêm `cut_next_get_after`, `file_gets` (ghi `Range` / `If-Range`), `etag_of`; route `GET /file` đi qua
  hàm mới `get_file` (thêm một nhánh `match` + 2 trường `State`; hàm cũ không đổi hành vi).
- Chưa kiểm với điện thoại thật (cần app Android + mạng; `real_phone.rs` chưa có ca tải về).
- **Nối engine (phần 2, sau gd1_07)** — `ipc.rs` chỉ THÊM: `LibraryOp::Download { device, remoteIds }`,
  `Outcome::DownloadsAdded { added, alreadyQueued, missing }`, `Snapshot.downloads: Vec<DownloadItem>` +
  `downloadsOmitted` (cả hai `#[serde(default)]` — ảnh chụp kiểu cũ vẫn đọc được, có test), hằng `DOWNLOADS_IN_SNAPSHOT`
  = 300 (chưa xong trước, rồi xong mới nhất; xếp theo `id`). `DownloadItem` thêm `Deserialize`.
- Engine: `Download` ⇒ `online_client` (Offline ⇒ `Offline`, rỗng ⇒ `InvalidInput`) ⇒ task con hỏi lại `list` (tên / cỡ /
  `dir` mới nhất; UI có thể cầm danh sách cũ) ⇒ `spawn_blocking` `add_downloads`: trùng trong lệnh hoặc cùng `remoteId`
  còn trong hàng (chưa xong) ⇒ `alreadyQueued`; không còn trong `list` ⇒ `missing`; mục cũ đã xong / lỗi / huỷ thì tải
  lại (Opus chọn: người dùng bấm lần nữa là muốn bản nữa) ⇒ `Internal::Added` ⇒ `Wake` khi `added > 0`.
- `item()` / `item_device()` tìm upload rồi tới download (`id` chung dãy AUTOINCREMENT của bảng `items` ⇒ không trùng
  khoá `progress` — lo ngại của gd1_07 không xảy ra). `All{Pause|Resume}` gồm cả tải về. `canPauseAll` / `canResumeAll`
  của snapshot tính thêm tải về; khay (`Summary`) vẫn chỉ upload (mục 8).
- `DownloadChanged`: `dirty`; `Fetching` giữ tiến độ (nâng `sent` theo offset), trạng thái khác bỏ khỏi `progress`.
- Lệnh soát phần 2 (11/11/2026, `ferry-w2`): 4 lệnh xanh (core 112 unit, daemon 22).
## 10. Đề xuất cải tiến
- [bảo mật] Đổi tên `.part` ⇒ đích dùng `rename` (đè nếu tệp trùng vừa xuất hiện giữa `unique_path` và `rename`) — cửa sổ
  rất hẹp, chỉ tệp của chính người dùng — có thể dùng `hard_link` + `remove` để không bao giờ đè — S — P3.
- [ux] Thư mục điện thoại (`remote_dir`) chưa dựng lại dưới `downloadDir` (tải phẳng) — gd1_11 quyết khi chọn nhiều mục
  trong một thư mục — S — P3.
