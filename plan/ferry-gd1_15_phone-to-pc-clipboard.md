# ferry-gd1_15 — Cắt / Sao chép từ điện thoại, dán ở trình quản lý tệp của máy

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ 12/11/2026 (PC + `fake_phone`; phía app điện thoại chờ — `_backlog.md`) |
| Repo | ferry (+ gốc: `.claude/rules/decisions/ferry.md` W-10) |
| Model | opus |
| Cỡ | M |
| Phụ thuộc | W-7 (§6.11), W-8 (§6.12), W-9 (§6.13) |
| Quyết định áp dụng | W-1, W-10, D-2, D-9 |
| Phụ thuộc mới | `x11rb` 0.13 (Linux, MIT/Apache), `clipboard-win` 5.4 (Windows, BSL-1.0), `arboard` 3.7 (macOS, MIT/Apache, không bật ảnh) |
| File đụng tới | `PROTOCOL.md` §6.14; core `protocol.rs`, `client.rs`, MỚI `export.rs`, `ipc.rs`, `tests/fake_phone`, MỚI `tests/export.rs`, `examples/fake_phone.rs`; daemon MỚI `clipboard/`, MỚI `engine/export.rs`, `engine.rs`, `main.rs`, `Cargo.toml`; ui MỚI `phone_browser/export.rs`, `clip.rs`, `edit.rs`, `mod.rs`, `model.rs`, `add_phone.rs`; `i18n/*.toml` |

## 1. Mục tiêu

Trong trình duyệt bộ nhớ điện thoại (trang thư viện, `features: browse`): chọn tệp / thư mục → "Sao chép" hoặc Ctrl+C ⇒
dán được ở Files (Ubuntu), Explorer, Finder; "Cắt" / Ctrl+X ⇒ dán xong thì mục biến khỏi điện thoại. Cắt / Sao chép / Dán
giữa các thư mục điện thoại (W-9) giữ nguyên.

## 2. Cách làm

- Giao thức §6.14: `features: fetch`, `GET|HEAD /api/v1/fetch?path=` (Range / If-Range / ETag như §6.4), `DELETE
  /entries?ifEmpty=1` để xoá thư mục chỉ khi rỗng. Thư mục liệt kê bằng `GET /dirs`.
- `ferry_core::export`: `plan` (liệt kê cây, tên an toàn cho mọi OS, trần 20 000 tệp) → `download` (một tệp mỗi lúc, `Throttle`
  chung, `.ferry-part` + nối tiếp bằng Range, ETag giữ qua lần đứt, không đọc cả tệp vào RAM, đặt mtime theo điện thoại) →
  `moved_tops` / `remove_moved` (Cắt) → `discard` / `clean_stale`.
- Daemon giữ clipboard (cửa sổ `ferry-ui` đóng vẫn dán được): `clipboard/` — Linux tự làm chủ selection X11 qua XWayland
  (`text/uri-list` + `x-special/gnome-copied-files` + chữ), Windows `CF_HDROP` + `Preferred DropEffect`, macOS URL tệp.
  Engine: `LibraryOp::Export` / `CancelExport`, `Snapshot::export`, một việc mỗi lúc (`engine/export.rs`).
- UI: Cắt / Sao chép của W-9 thêm gọi `Export` khi điện thoại có `fetch`; thanh dưới bảng "Đang chuẩn bị để dán… x / y (z%)"
  ⇒ "Sẵn sàng dán" ⇒ "Đã dời N mục sang máy tính", nút Bỏ; phím Ctrl+C / X / V (⌘ trên macOS) trong cây / bảng.

## 6. Case test

| Mã | Case | Kết quả |
|---|---|---|
| C1 | Route `fetch`: thiếu feature ⇒ 403, Range 206, If-Range đổi tệp ⇒ 200, thư mục ⇒ 404, `..` ⇒ 400 | `tests/export.rs` ✅ |
| C2 | Chép thư mục + tệp: giữ cây, đúng byte, bỏ mục ẩn, không còn `.ferry-part` | ✅ |
| C3 | Đứt giữa tệp ⇒ nối tiếp `bytes=700000-` kèm If-Range | ✅ |
| C4 | Cắt: chưa dán ⇒ không xoá; dời mục tạm ⇒ xoá đúng phần đó; tệp đổi sau khi tải / thư mục còn mục ẩn ⇒ giữ | ✅ |
| C5 | Phiên cũ > 24 h bị dọn lúc khởi động | ✅ |
| C6 | Engine: Cắt khi thiếu `edit` ⇒ Forbidden; Sao chép ⇒ Ready + clipboard; Bỏ ⇒ xoá thư mục tạm; Cắt + dời ⇒ `Moved{4,0}` + thông báo | `engine::tests::export_copies_then_moves_after_paste` ✅ |
| C7 | Clipboard X11 thật đưa `TARGETS` + `x-special/gnome-copied-files` (đọc bằng `xclip`) | `cargo test -p ferry-daemon x11_clipboard -- --ignored` ✅ |
| C8 `[owner-qa]` | `fake_phone` + Files (Ubuntu): Sao chép một tệp, một thư mục ⇒ Dán; Cắt ⇒ Dán ⇒ mục biến khỏi điện thoại | ✅ chủ dự án 12/11/2026, điện thoại thật |
| C9 `[owner-qa]` | Explorer (Windows), Finder ⌥⌘V (macOS) | chưa — cần máy |

## 9. Lựa chọn của Claude (D-9)

- **Clipboard ở daemon, không ở `ferry-ui`**: X11 / XWayland phục vụ dữ liệu lúc dán từ tiến trình sở hữu; UI thoát khi
  đóng cửa sổ.
- **Linux tự viết chủ selection bằng `x11rb`**, không `arboard`: `arboard` chỉ đưa `text/uri-list` ⇒ Files không biết "cắt".
  GNOME Wayland đi qua XWayland (Mutter chuyển sang Wayland); không có X (Wayland thuần không XWayland) ⇒ báo lỗi, không dán
  được. Không làm `INCR` (chỉ đặt mục cấp đầu, danh sách nhỏ).
- **Cắt = soát mục tạm biến mất** (mỗi giây, `stat` vài đường): Files / Explorer dời xong thì xoá nguồn (thư mục tạm của ta)
  ⇒ khi đó mới `DELETE` trên điện thoại, mỗi tệp soát lại cỡ + `modified` qua `GET /dirs`; thư mục xoá từ dưới lên với
  `ifEmpty=1`. Không dán ⇒ không xoá gì; thư mục phiên mất cả (dọn tay) ⇒ dừng, không xoá. Daemon khởi động lại giữa
  chừng ⇒ không còn soát ⇒ không xoá (an toàn, không mất dữ liệu).
- **Tải chạy song song với runner gửi** (lệch §2.8 "một tệp mỗi lúc mỗi điện thoại"): người dùng chờ dán ngay; vẫn một tệp
  mỗi lúc trong việc chép và dùng chung trần tốc độ.
- **Dọn**: lần Cắt / Sao chép mới, nút Bỏ, hết 24 h, lúc khởi động (phiên > 24 h). Sao chép giữ thư mục tạm tới lúc đó vì
  người dùng có thể dán nhiều lần.
- **Bỏ clipboard chỉ khi vẫn là của Ferry**: Linux soát chủ selection, Windows `GetClipboardSequenceNumber`, macOS so danh
  sách tệp.
- Dán Cắt trong điện thoại (W-9) ⇒ gửi `CancelExport` cho lần chép sang máy cùng mục.
- `Snapshot::export` là `Box` (clippy `large_enum_variant` của `DaemonToUi`). Thêm `CommandError::TooMany`.
- `fake_phone`: xoá tệp / thư mục con giữ thư mục cha (như máy thật); test cũ `client::edit_makes_renames_and_deletes` sửa
  kỳ vọng theo.

## 10. Còn lại

- C8, C9 tay (backlog). Phía app điện thoại cài §6.14 (backlog, repo `video-player`).
