# media-wireless-sync-gd1_10 — download-runner

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | S — ~400 dòng |
| Phụ thuộc | gd1_03, gd1_07 |
| Quyết định áp dụng | W-1 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/core/src/transfer/download.rs`, `crates/core/tests/download.rs`; SỬA `crates/core/src/config.rs` (`downloadDir`) |

## 0. Bàn giao từ gd1_03 (11/11/2026)

<!-- handoff: media-wireless-sync-gd1_03 -->

- Máy trạng thái tải về có sẵn: `queue::state::DownloadMachine` (`Queued, Fetching{offset, etag}, Waiting, Done,
  Failed, Cancelled`), effect `Get{offset, etag}` / `Truncate` (nhận `200` khi đang tải tiếp) / `DiscardPartial` /
  `Wait`. Lưu: `QueueStore::add_download(device, &NewDownload, now)`, `next_download`, `apply_download` — cùng bảng
  `items` với upload (`kind = 'download'`), mở lại thì `Fetching` ⇒ `Waiting`. Báo `Progress{offset}` theo chặng
  (vài MB), không mỗi gói.

## 1. Làm gì (PROTOCOL 6.4)
- Mục download trong cùng hàng đợi thiết bị (tuần tự với upload): ghi `<tên>.part` trong `downloadDir` (mặc định thư mục Video / Downloads của OS), lưu `ETag` + offset.
- Tiếp: `Range: bytes=<đã có>-` + `If-Range`; `200` ⇒ ghi lại từ đầu; `416` ⇒ coi như xong nếu cỡ khớp.
- Xong ⇒ đổi tên `.part` thành tên từ `Content-Disposition filename*` (lọc ký tự cấm theo OS, trùng thì thêm ` (2)`).

## 6. Nghiệm thu
- C1. Đứt giữa chừng ⇒ tiếp từ offset, sha256 khớp. `[opus-unit]`
- C2. ETag đổi ⇒ tải lại từ 0. `[opus-unit]`
- C3. Tên có ký tự cấm Windows (`:`, `?`) ⇒ tên hợp lệ. `[opus-unit]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
