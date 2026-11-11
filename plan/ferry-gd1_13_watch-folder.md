# ferry-gd1_13 — watch-folder

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ (11/11/2026) |
| Repo | ferry |
| Model | opus |
| Cỡ | S — ~300 dòng |
| Phụ thuộc | gd1_04, gd1_12 |
| Quyết định áp dụng | W-1, W-3 |
| Phụ thuộc mới | `notify` (CC0-1.0, groove đã dùng; soát `cargo deny`) |
| File đụng tới | MỚI `crates/core/src/watch.rs`; SỬA `crates/daemon/src/engine.rs`, `crates/daemon/src/main.rs`, `crates/core/Cargo.toml`, `crates/core/src/ipc.rs`, `crates/ui/src/views/settings.rs`, `i18n/{vi,en}.toml` |

## 0. Bàn giao từ ferry-gd1_12 (11/11/2026)
<!-- handoff: ferry-gd1_12 -->

- Cấu hình: `Config.watch_folder: WatchFolder { enabled, path: Option<PathBuf> }` (khoá `watchFolder`), một thư mục. Màn cài
  đặt gửi `SettingsOp::SetWatchFolder { folder }`; engine (`engine.rs`, `on_settings`) kiểm `path` tuyệt đối + là thư mục
  (không ⇒ `InvalidInput`), lưu config rồi trả `Outcome::Settings`. Bộ theo dõi chỉ cần bám vào chỗ này: khởi động theo
  `config.watch_folder` lúc daemon chạy và dựng lại / dừng mỗi khi `SetWatchFolder` áp xong (một nơi ghi config là engine).
- Thư mục tải về: `config.download_dir()` (mặc định thư mục Downloads của hệ thống, `downloadDir` khi người dùng chọn).
- Giới hạn tốc độ đổi lúc chạy qua `Throttle` dùng chung — tệp từ thư mục theo dõi đi cùng hàng đợi nên tự chịu giới hạn.
- Cửa sổ cài đặt hiện chữ "Theo dõi thư mục" + đường dẫn; gd1_13 thêm gì cho người dùng thấy (tệp đã gửi tự động) thì dùng
  hàng đợi / thông báo sẵn có.

## 1. Làm gì
- Theo dõi một thư mục (không đệ quy sâu tuỳ ý, không quét ổ — §2.8): tệp video mới **ổn định** (cỡ + mtime không đổi 5 s, mở ghi được) ⇒ `AddFiles` cho thiết bị mặc định.
- Tệp đã có trong hàng đợi / "Đã gửi" không thêm lại (khoá: đường dẫn + cỡ + mtime).
- Không giữ CPU khi rảnh: chỉ sự kiện OS, đồng hồ ổn định chạy khi có tệp chờ.
- Bổ sung W-3: tệp mới trong thư mục theo dõi gửi với `dir = <tên thư mục theo dõi>/<đường con>` (giữ cây như copy/paste).

## 2b. Phân vai
| Phần | Vai |
|---|---|
| `watch.rs`: luồng `notify`, bộ đếm ổn định, chống trùng, `dir` | [opus] |
| Nối engine (`restart_watch`, `on_watch_ready`, `add_watched`) | [opus] |
| Thử trên Windows / macOS thật (đổi tên, khoá ghi) | [owner-qa] |

## 5. Đo
RSS daemon `ferry` bản release, Linux, thư mục theo dõi bật, chưa ghép điện thoại: trước 10,7 MB / 7 luồng ⇒ sau 11,9 MB /
9 luồng (`ferry-watch` + luồng inotify của `notify`); sau khi chép 20 tệp 1 MB vẫn 11,9 MB. Tắt thư mục theo dõi ⇒ không có
hai luồng đó. Rảnh: luồng chặn ở `recv`, không đồng hồ.

## 6. Nghiệm thu
- C1. Cho tệp đang chép (cỡ tăng, sự kiện ghi liên tục, 2 GB / 20 s) · Khi soát · Thì chỉ ra sau 5 s đứng yên kể từ sự kiện
  cuối; Windows: còn handle ghi ⇒ chờ tiếp. `[opus-unit]` `c1_growing_file_waits_until_copy_finishes`, `c1_locked_file_waits`,
  `watcher_end_to_end` (chép thật từng khúc) ✅
- C2. Cho tệp cũ / tệp đã gửi · Khi đổi tên trong thư mục · Thì không thêm mục mới. `[opus-unit]` `c2_rename_does_not_duplicate`,
  `c2_known_after_rename_or_already_queued`, `watcher_end_to_end`, engine `gd1_13_watch_folder_queues_new_files` ✅
- C-W3. Cho tệp mới ở `<thư mục>/Phim/a.mp4` · Thì `dir = "<tên thư mục>/Phim"`, quá 7 cấp con bỏ. `[opus-unit]`
  `cw3_remote_dir_keeps_tree` + engine test (gửi tới điện thoại giả) ✅
- C4. Chép một loạt 20 tệp ⇒ một lô (đo thật ở daemon release: log "20 new file(s) settled"). `[opus-run]` ✅
- C5. Kiểm tay Windows / macOS: đổi tên tệp CŨ (chưa gửi) trong thư mục ⇒ hiện sẽ gửi (xem mục 9). `[owner-qa]`

## 9. Bàn giao thực tế
- `ferry_core::watch`: `FolderWatcher::start(root, generation, stable_for, tx)` chạy một luồng OS (`ferry-watch`) giữ `notify`
  8 (đệ quy) + `Stability`; drop ⇒ dừng, không chờ luồng. Ra `Ready { generation, root, remote_root, files }` qua kênh tokio.
  `watch::plan(ready, uploads, downloads)` dựng `BatchPlan` (nhãn = tên thư mục), bỏ tệp đã biết (`is_known`).
- Engine: `restart_watch()` lúc khởi động và khi `SetWatchFolder` áp xong mà khác cũ; `on_watch_ready` bỏ lô của thế hệ cũ,
  ghi store trong `spawn_blocking` (`add_watched`) rồi `Internal::Watched` ⇒ `Wake` runner. `EngineConfig.watch_stable_for`
  (main: `watch::STABLE_FOR` 5 s; test 300 ms).
- Opus chọn 11/11/2026 (D-9):
  - **Điện thoại mặc định** = điện thoại ghép đầu tiên trong `config.devices` (chưa có khái niệm mặc định; W-2 không gửi song
    song nhiều máy). Chưa ghép ⇒ chỉ log, bỏ lô.
  - **Tệp có sẵn lúc bật theo dõi không gửi** — chỉ tệp mới từ sự kiện OS (bật vào thư mục 100 video cũ mà tự gửi hết là bất
    ngờ). Tệp chép vào lúc daemon tắt cũng không gửi.
  - **Chống trùng**: cùng cỡ + mtime với mục upload còn trong store mà cùng đường dẫn hoặc đường cũ đã mất (đổi tên); thêm:
    tệp là đích của mục tải về (thư mục tải về trùng thư mục theo dõi ⇒ không đẩy ngược). Store dọn mục xong sau 7 ngày ⇒ sau đó
    đổi tên tệp đã gửi sẽ gửi lại.
  - **Đổi tên tệp cũ (chưa gửi)**: Linux bỏ qua (inotify phát `From` → `To` → `Both`, nhớ `From` cuối để nhận ra). Windows
    (`From` / `To` rời) và macOS (`Name(Any)`) không phân biệt được với "dời tệp vào" ⇒ tệp đổi tên coi là mới (C5 chờ kiểm tay).
  - **Gom lô**: tệp ổn định giữ lại tới khi không còn tệp chờ hoặc giữ quá 5 s ⇒ chép một loạt = một lô.
  - Không gửi thông báo hệ thống (PROTOCOL 2: im lặng); người dùng thấy lô trong hàng đợi.
  - Độ sâu: tối đa 7 cấp con (+ tên thư mục = 8 cấp `dir`, §6.9); thư mục dời vào ⇒ duyệt có trần; symlink tới tệp theo, tới
    thư mục bỏ (như `plan_batch`). Danh sách chờ trần 4096 tệp.
- Lỗi bật theo dõi (thư mục mất, không quyền, hết inotify watch): `FolderWatcher::start` đăng ký trong `spawn_blocking`, trả `Err(chữ OS)` ⇒ `SettingsView.watchError` (khoá IPC mới, vắng khi không lỗi) ⇒ màn cài đặt hiện dòng đỏ `settings.files.watch_folder_error` dưới công tắc. Lệnh `SetWatchFolder` trả lời sau khi biết bật được không (`Internal::WatchStarted`). Lỗi lúc đang chạy (thư mục bị xoá sau khi bật) vẫn chỉ log.
- Soát xanh: `cargo build`, `clippy --all-targets -D warnings` (+ chéo `x86_64-pc-windows-msvc`), `cargo test` (core 131,
  daemon 31), `cargo deny check licenses` (notify CC0-1.0 đã có trong `allow`).

## 10. Đề xuất cải tiến
- [ux] Lỗi bật theo dõi không hiện cho người dùng — `watch.rs` chỉ log — đưa vào `SettingsView` + dòng đỏ ở màn cài đặt — M — P2. **Đã làm** trong lượt (D-8), xem mục 9; test engine `gd1_13_watch_folder_queues_new_files` (thư mục `0o000`).
- [ux] Chọn điện thoại nhận cho thư mục theo dõi khi ghép nhiều máy (hiện: máy ghép đầu) — chỉ đáng khi có người dùng nhiều
  điện thoại — S — P3.
- [pro] Nhiều thư mục theo dõi, mỗi thư mục một điện thoại / thư mục đích — hợp huy hiệu "Pro" về sau.
