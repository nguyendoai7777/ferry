# media-wireless-sync-gd1_03 — queue-store

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | M — ~600 dòng |
| Phụ thuộc | gd1_01 (song song với 02) |
| Quyết định áp dụng | W-1, W-3 |
| Phụ thuộc mới | `rusqlite` (`bundled`; MIT, SQLite public domain) |
| File đụng tới | MỚI `crates/core/src/queue/{mod,state,store,backoff}.rs` |

## 1. Làm gì
- Máy trạng thái mục upload đúng PROTOCOL mục 5 dưới dạng hàm thuần `next(state, event) -> (state, effect)`: `Queued, Checking, Creating, Sending{uid,offset}, Waiting{attempt,until}, Paused{reason}, Done{remote_name}, Skipped, Failed{reason}, Cancelled`. Mục download: `Queued, Fetching{offset,etag}, Waiting, Done, Failed`.
- Backoff 2 s, 4 s … trần 60 s; `Retry-After` của server thắng.
- Lưu SQLite ở thư mục dữ liệu: ghi ngay khi có `uid`, sau mỗi khúc lưu offset; WAL; migration có số phiên bản.
- Mục ghi kèm `size` + `mtime` lúc thả vào để phát hiện "File đã thay đổi".
- "Đã gửi" giữ 7 ngày rồi tự dọn (lúc khởi động + mỗi ngày một lần, không timer dày).
- Mỗi thiết bị một hàng đợi tuần tự (`device_id` trên mục).
- Bổ sung W-3: mục upload lưu `dir` đích (đã chuẩn hoá `/`) và `batch_id` (một lần thả / dán = một lô — PROTOCOL 5); truy vấn tổng tiến độ theo lô.

## 2. Không làm trong task này
- Không gọi mạng; effect chỉ là mô tả (`Effect::Create`, `Effect::Patch{offset}`, `Effect::Head`…), task 04 thực thi.

## 6. Nghiệm thu
- C1. Mọi mũi tên của PROTOCOL mục 5 có test một-một (404 ⇒ Creating lại, 409 ⇒ Head, 507 ⇒ Paused, 415/413 ⇒ Failed…). `[opus-unit]`
- C2. Ghi 1000 mục, đóng / mở lại store ⇒ đủ mục, đúng trạng thái + offset; `Sending` mở lại thành `Waiting` (cần `HEAD`). `[opus-unit]`
- C3. Giết tiến trình giữa lúc ghi (test con) ⇒ DB không hỏng. `[opus-unit]`
- C4. Backoff ca biên: lần 1, lần chạm trần, `Retry-After` 0 / lớn. `[opus-unit]`
- C-W3. Lô 3 tệp trong 2 thư mục con: lưu / đọc lại đúng `dir`, tổng tiến độ lô đúng sau khởi động lại. `[opus-unit]`

## 8. Nợ chuyển task khác
## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
