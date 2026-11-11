# media-wireless-sync — App PC "cầu nối" gửi video sang điện thoại qua WiFi

> Hợp đồng chung với app điện thoại `video-player` (Android đã cài — `video-player/android`, task_09; iOS cài đúng giao
> thức này sau). Tệp này **tự đủ**: triết lý, mô hình, màn hình, và **giao thức v1**. Ngày chốt: 2026-11-10 (W-1);
> bổ sung IPv6 (§6.8) + thư mục kiểu copy/paste (§6.9) 2026-11-11 (W-3).
> **Đổi giao thức = sửa tệp này trước, rồi sửa cả hai phía** (bản bên `video-player` phải giống hệt mục 6).
> Mục 3 là đề xuất gốc; bố cục thật của repo ở `CLAUDE.md`.

---

## 1. App PC là gì (và không là gì)

Điện thoại có thư viện video. Máy tính có video (tải về, quay màn hình, xuất từ trình dựng). App PC là **cái ống** nối hai
đầu qua WiFi nhà: kéo thả video vào là nó chảy sang điện thoại, đứt mạng thì tự nối lại, xong thì im lặng.

- **Là**: cầu nối đẩy (PC → điện thoại) là chính, kéo về (điện thoại → PC) là phụ. Chạy nền ở khay hệ thống.
- **Không là**: trình phát video, trình quản lý thư viện, trình sửa video. Không có "đồng bộ hai chiều" kiểu Dropbox
  (không xoá hộ, không tự kéo mọi thứ về). Điện thoại là chủ thư viện; PC chỉ gửi và nhận khi được bảo.

Trang web (`http://<ip>:<port>` + PIN) trên điện thoại vẫn còn cho máy lạ / dùng một lần. App PC dành cho **máy của
mình, dùng hằng ngày**: ghép một lần, sau đó không phải gõ gì nữa.

## 2. Triết lý thiết kế

1. **Không bắt người dùng nghĩ về mạng.** Không bao giờ hỏi IP/cổng ở luồng chính: dò bằng mDNS, nhớ thiết bị theo
   `id` (không theo IP — IP DHCP đổi hoài). Ô "nhập địa chỉ tay" chỉ nằm trong mục nâng cao (mạng chặn multicast).
2. **Ghép một lần, tin lâu dài.** PIN 6 số chỉ dùng lúc ghép. Sau đó dùng token lâu dài, cất trong kho khoá của hệ
   điều hành (Keychain / Credential Manager / Secret Service) — không bao giờ ghi ra file cấu hình thường.
3. **Hàng đợi là trung tâm, không phải nút "Gửi".** Mọi thứ người dùng thả vào thành một mục trong hàng đợi bền
   (ghi đĩa). Tắt app, sập điện, đổi WiFi, điện thoại tắt sync → mở lại vẫn còn nguyên, tự chạy tiếp từ byte cuối.
4. **Tiếp tục, không làm lại.** Upload luôn dùng giao thức nối tiếp (§6.5). Đứt giữa chừng 3 GB không bao giờ gửi lại từ
   đầu (trừ khi điện thoại đã tắt server — phiên upload mất, §6.5).
5. **Điện thoại là nguồn sự thật.** Trước khi gửi, hỏi danh sách: trùng tên + cỡ thì bỏ qua (báo "đã có"). Tên trên
   điện thoại có thể khác tên gửi (điện thoại tự đổi khi trùng) — tin vào phản hồi, không tự đoán.
6. **Im lặng khi ổn, rõ ràng khi hỏng.** Không popup cho từng file xong. Một thông báo hệ thống khi cả lô xong. Lỗi thì
   nói việc người dùng phải làm ("Bật Sync WiFi trên điện thoại", "Điện thoại hết chỗ — cần thêm 1,2 GB"), không nói mã
   HTTP.
7. **Lõi tách khỏi giao diện.** Một crate lõi không phụ thuộc UI (dò, ghép, hàng đợi, truyền), có test riêng; UI kit
   Rust chỉ vẽ trạng thái và gửi lệnh. Sau này làm CLI hay bản khác chỉ việc dùng lại lõi.
8. **Nhẹ và lịch sự với máy.** Không quét ổ cứng. Không giữ CPU khi rảnh. Băng thông: mặc định hết cỡ, có tuỳ chọn giới
   hạn. Một file một lúc cho mỗi điện thoại (điện thoại ghi đĩa tuần tự nhanh hơn ghi song song).

## 3. Kiến trúc đề xuất (gốc — bố cục thật ở `CLAUDE.md`)

```
pc-app/
  core/            crate thư viện, không UI
    discovery      mDNS browse `_vpsync._tcp`, + địa chỉ tay; phát sự kiện Online/Offline theo device id
    pairing        POST /api/v1/pair, cất/xoá token trong keyring
    client         HTTP thuần: info, list, download (Range), upload nối tiếp
    queue          hàng đợi bền (SQLite hoặc JSON append-only), máy trạng thái từng mục, retry/backoff
    watch          (tuỳ chọn) thư mục "Gửi sang điện thoại": file mới ổn định → vào hàng đợi
  app/             UI kit của chủ dự án: khay hệ thống + cửa sổ
```

- Lõi chạy trên một runtime async (tokio) riêng; UI nhận **ảnh chụp trạng thái** qua channel (không chia sẻ mutex với UI).
- Crate gợi ý (cân nhắc theo UI kit đang dùng): `mdns-sd` (dò), `reqwest` hoặc `hyper` (stream thân request từ file,
  không đọc cả file vào RAM), `keyring` (token), `directories` (đường dữ liệu), `rusqlite` hoặc `serde_json` (hàng đợi),
  `notify` (theo dõi thư mục), `uuid` (client id).
- Client id: UUID sinh một lần lúc cài, lưu cạnh cấu hình. Tên client: tên máy (`hostname`), cho đổi.

## 4. Màn hình

Ít, phẳng, mỗi màn một việc.

1. **Khay hệ thống** (luôn có): biểu tượng có 3 trạng thái — rảnh / đang gửi (kèm %) / cần chú ý. Menu: mở cửa sổ,
   tạm dừng tất cả, thoát.
2. **Cửa sổ chính** = một vùng thả lớn + hàng đợi bên dưới.
   - Đầu cửa sổ: thiết bị đang chọn (tên điện thoại, chấm xanh/xám online). Nhiều điện thoại thì là menu chọn.
   - Hai nút **"Chọn tệp…"** (hộp chọn tệp của hệ điều hành, chọn **nhiều** tệp) và **"Chọn thư mục…"** (hộp chọn thư
     mục, nhiều thư mục nếu OS cho), cạnh vùng thả. Thả / dán (Ctrl+V · Cmd+V sau khi Copy trong Explorer / Finder) cũng
     được. Tệp / thư mục đã chọn vào **khay chờ gửi** (danh sách ngắn, bỏ được từng mục) — chưa gửi ngay.
   - Bấm **"Gửi sang điện thoại…"** ⇒ mở **trình duyệt điện thoại kiểu Explorer** (mục 4.6) để chọn thư mục đích ⇒ "Gửi
     vào đây" ⇒ cả khay thành một lô. Đẩy y như copy/paste giữa hai ổ — thư mục giữ nguyên cây con trên điện thoại (§6.9). Chỉ tệp video được gửi (§6.6); tệp khác bỏ
     qua, báo gộp một dòng "Bỏ qua N tệp không phải video".
   - Hàng đợi: mỗi dòng = tên, cỡ, thanh tiến độ, tốc độ, trạng thái; nút tạm dừng / huỷ / thử lại. Mục xong gom vào
     "Đã gửi" (gập được), giữ 7 ngày rồi tự dọn.
3. **Thư viện điện thoại** (tab phụ): trình duyệt kiểu Explorer (mục 4.6) ở chế độ thư viện. Chọn video → "Tải về máy"
   (vào hàng đợi chiều ngược lại, tải tiếp bằng `Range`). Không có nút xoá (giao thức v1 không có).
4. **Thêm điện thoại** (màn hướng dẫn): bước 1 "Trên điện thoại: mở app → menu → Sync WiFi → Bật". Bước 2 danh sách
   điện thoại dò thấy (chưa ghép). Bước 3 chọn một → ô PIN 6 số (đọc trên màn điện thoại) → xong.
5. **Cài đặt**: thiết bị đã ghép (đổi tên hiển thị, quên), thư mục theo dõi, thư mục lưu file tải về, giới hạn băng
   thông, khởi động cùng hệ thống, địa chỉ tay (nâng cao).

6. **Trình duyệt điện thoại kiểu Explorer** (một component, dùng ở hộp chọn đích khi gửi và ở tab thư viện điện thoại):
   - Thanh trên: nút Lùi / Tiến / Lên, **thanh địa chỉ dạng breadcrumb** (`Pixel 8 › Chuyến đi › Ngày 1`, bấm từng đoạn
     để nhảy), nút làm mới, ô tìm theo tên trong thư mục đang mở.
   - Trái: **cây thư mục** (gốc = nơi lưu của điện thoại; nhóm "Khác" cho video có `dir = null`, chỉ xem / tải, không
     làm đích). Phải: **bảng chi tiết** — cột Tên (icon thư mục / video), Ngày sửa, Loại, Cỡ; thư mục trước, bấm tiêu đề
     cột để sắp; nhấp đúp thư mục để vào; chọn nhiều bằng Ctrl/Cmd + click, Shift + click.
   - Dựng hoàn toàn từ `GET /api/v1/list` (§6.4): thư mục = các tiền tố của `dir`. Thư mục rỗng không tồn tại trên điện
     thoại ⇒ nút **"Thư mục mới"** tạo thư mục ảo phía PC (in nghiêng) cho tới khi có tệp đầu tiên gửi vào.
   - Chế độ **chọn đích** (khi gửi): thanh dưới "Gửi N tệp (x GB) vào: Chuyến đi/Ngày 1" + nút "Gửi vào đây" / "Huỷ";
     mặc định mở ở thư mục đích lần trước của điện thoại đó. Chế độ **thư viện** (tab 3): nút "Tải về máy" cho mục đã
     chọn; thả / dán từ máy tính vào thư mục đang mở ⇒ gửi vào đó (không hỏi lại).
   - Điện thoại offline ⇒ hiện danh sách lần cuối, mờ đi, không cho "Gửi vào đây".

Văn bản: tiếng Việt + tiếng Anh theo ngôn ngữ hệ thống. Dùng chữ "Gửi sang điện thoại" / "Tải về máy", tránh chữ
"upload/sync" với người dùng.

## 5. Máy trạng thái

### Thiết bị

```
Unknown ─(mDNS thấy, chưa có token)→ Discovered ─(PIN đúng)→ Paired
Paired ──(mDNS thấy + GET /api/v1/info → paired:true)→ Online
Online ─(mất mDNS 30 s / lỗi kết nối)→ Offline ─(thấy lại)→ Online
Online/Offline ─(info trả paired:false hoặc 401)→ NeedsRepair   (điện thoại đã "Quên máy tính")
```

- `Offline` là bình thường (điện thoại chưa bật Sync WiFi). Hàng đợi chờ, không báo lỗi; nhắc nhẹ trên khay:
  "Bật Sync WiFi trên điện thoại để gửi N video".
- Khi thấy lại thiết bị ở IP mới: cập nhật địa chỉ, không hỏi gì.

### Mục upload

```
Queued → Checking (list: trùng dir+tên+cỡ? → Skipped)
       → Creating (POST /uploads → upload id, lưu ngay vào hàng đợi)
       → Sending  (PATCH theo khúc; sau mỗi khúc lưu offset)
       → Done (201 + mục thư viện)            ← lưu tên thật trên điện thoại
Lỗi mạng / 5xx / 503 → Waiting (backoff 2 s, 4 s … tối đa 60 s) → HEAD lấy offset → Sending
404 upload id (server đã tắt) → Creating lại từ đầu
409 Upload-Offset lệch → HEAD → tiếp từ offset đúng
415 / 413 → Failed (không thử lại; nói lý do)
507 → Paused (hết chỗ; báo số GB thiếu; người dùng bấm thử lại)
File nguồn đổi (cỡ/mtime khác lúc thả vào) → Failed "File đã thay đổi"
```

Một lần thả / dán = một **lô**: hàng đợi hiện lô như một dòng gộp (tên thư mục hoặc "N tệp", tổng tiến độ), mở ra thấy
từng tệp. Mỗi tệp vẫn là một mục riêng với máy trạng thái trên và mang theo `dir` đích (§6.9).

## 6. Giao thức v1 (điện thoại = server, PC = client)

### 6.1 Chung

- HTTP/1.1 thường (không TLS) trên LAN. **Mỗi kết nối đúng một request** — server luôn trả `Connection: close`. Đừng
  dùng keep-alive pool (hoặc tắt nó: `reqwest::ClientBuilder::pool_max_idle_per_host(0)`).
- Server đọc chờ tối đa 60 s không nhận byte nào thì đóng. Tối đa 16 kết nối đồng thời; quá thì `503` + `Retry-After: 2`.
- Server nghe **cả IPv4 lẫn IPv6** trên cùng một cổng (socket dual-stack). Host header theo RFC: `[fd12::5]:8080`.
- Header tên file / thư mục: URL-encoded UTF-8 (percent-encoding mọi byte ngoài ASCII in được).
- Lỗi: thân `text/plain` ngắn, chỉ để log; client quyết theo **mã trạng thái**.
- Xác thực: `Authorization: Bearer <token>`. Token từ §6.3 (lâu dài) hoặc từ đăng nhập trang web (sống tới khi tắt server).
- Server chỉ chạy khi người dùng bật Sync WiFi trên điện thoại (foreground service, có thông báo). Android 15+ giới hạn
  ~6 giờ/ngày: hết hạn mức thì server tự tắt → client thấy `Offline`.

### 6.2 Dò thiết bị

- mDNS/DNS-SD: kiểu dịch vụ **`_vpsync._tcp`**, tên instance = tên điện thoại (có thể bị hệ thống thêm hậu tố
  ` (2)` khi trùng). TXT:
  - `api=1` — phiên bản giao thức; client bỏ qua thiết bị có `api` lớn hơn bản mình hiểu… nhưng vẫn gọi `info`.
  - `id=<16 ký tự hex>` — id cố định của điện thoại (giữ qua lần bật/tắt, đổi khi cài lại app). **Khoá nhận dạng.**
- Địa chỉ lấy từ bản ghi SRV + **A và AAAA** của mDNS, cộng `addresses` trong `info` (§6.3). Giữ mọi địa chỉ của thiết
  bị; thứ tự thử (IPv4 / IPv6 ngang hàng, mạng nào chạy thì dùng mạng đó — §6.8):
  1. IPv4 cùng subnet với một interface của PC;
  2. IPv6 global / ULA (`fc00::/7`) cùng prefix /64 với PC;
  3. IPv6 link-local `fe80::/10` kèm **scope id = interface PC đã nhận bản ghi mDNS đó**;
  4. mọi địa chỉ còn lại.
- Nhập tay: `1.2.3.4:8080`, `[fd12::5]:8080`, `[fe80::5%en0]:8080` (Windows `%12`) — màn điện thoại hiện sẵn IPv4 và
  IPv6; rồi `GET /api/v1/info` để lấy `id`.

### 6.3 Thông tin + ghép đôi

`GET /api/v1/info` — không cần xác thực.
```json
{"app":"xmobile.videoplayer","api":1,"id":"3f9a0c1d2b4e5a67","name":"Pixel 8","paired":false,
 "port":8080,"addresses":["192.168.1.23","fd12:3456:789a::23"],"features":["ipv6","dirs"]}
```
- `paired` = `true` khi request kèm Bearer token hợp lệ. Dùng nó để kiểm token còn sống.
- `port` + `addresses`: mọi địa chỉ LAN của điện thoại (IPv4 riêng + IPv6 global / ULA; **không** link-local — scope
  phía điện thoại vô nghĩa với PC). Thêm đường khi mDNS chỉ trả một họ địa chỉ hoặc IP đổi.
- `features`: tính năng điện thoại có. Điện thoại cũ không có trường này = `[]`. `dirs` ⇒ nhận `X-File-Dir` (§6.5, §6.9);
  thiếu ⇒ PC gửi phẳng và báo "Cập nhật app trên điện thoại để giữ thư mục". `ipv6` ⇒ server nghe IPv6.

`POST /api/v1/pair` — không cần xác thực.
- Header: `X-Client-Id: <8–64 ký tự [A-Za-z0-9_-]>` (UUID của PC), `X-Client-Name: <URL-encoded, ≤ 64 ký tự>` (tuỳ chọn, thiếu thì "PC"),
  `Content-Type: text/plain`, `Content-Length`.
- Thân: PIN 6 số (đang hiện trên màn điện thoại; PIN mới mỗi lần bật server).
- `200` →
  ```json
  {"token":"<64 hex>","id":"3f9a0c1d2b4e5a67","name":"Pixel 8"}
  ```
  Token lâu dài, sống qua lần bật/tắt server và khởi động lại máy. Ghép lại cùng `X-Client-Id` → token cũ mất hiệu lực.
  Điện thoại nhớ tối đa 16 máy (máy lâu không dùng nhất bị bỏ).
- `400` thiếu/sai header · `403` sai PIN · `429` + `Retry-After: <giây>` khi sai 5 lần liền (khoá 30 s, chung cả trang
  web) · `411`/`413` thân thiếu/quá dài.

`DELETE /api/v1/pair` — Bearer. Máy PC tự "quên điện thoại": huỷ token trên điện thoại. `204`.
Điện thoại cũng có nút "Quên các máy tính đã ghép" → mọi token lâu dài chết → PC nhận `401` → `NeedsRepair`.

### 6.4 Danh sách + tải về

`GET /api/v1/list` — Bearer. `200`:
```json
{"items":[{"id":"…","name":"clip.mp4","size":7340032,"modified":1762761600000,"mime":"video/mp4"}]}
```
- `id` là chuỗi mờ (opaque), dùng nguyên văn trong URL (URL-encode khi ghép đường dẫn). Không suy luận gì từ nó.
- `dir`: thư mục chứa video, **tương đối so với nơi lưu của điện thoại** (gốc nhận upload), `/` phân cách, `""` = gốc
  (vd `"Chuyến đi/Ngày 1"`). `null` (hoặc vắng) = video nằm ngoài nơi lưu (thư mục quét / thư mục người dùng thêm) — PC
  xếp vào nhóm "Khác", không dùng làm đích gửi. Điện thoại cũ không có trường này = coi như `""`.
- `modified`: mili giây Unix. Mới nhất trước. Gồm thư viện của app ở mọi nơi lưu (bộ nhớ riêng, `Movies/VideoPlayer`, thư mục lưu người dùng chọn).

`GET` / `HEAD /api/v1/file/<id>` — Bearer. Tải về; hỗ trợ `Range: bytes=a-b` (một khoảng), `If-Range` với `ETag`,
`206`/`416`; `Content-Disposition` có `filename*`. Tải tiếp: lưu `ETag`, gửi `Range: bytes=<đã có>-` + `If-Range: <etag>`;
nhận `200` (file đã đổi) thì ghi lại từ đầu.

### 6.5 Upload nối tiếp (kiểu tus rút gọn)

1. **Tạo** — `POST /api/v1/uploads`, Bearer, không thân (`Content-Length: 0`).
   Header: `X-File-Name: <URL-encoded>` (chỉ lấy phần tên, không thư mục), `Upload-Length: <tổng byte, > 0>`,
   `X-File-Dir: <URL-encoded>` (tuỳ chọn, chỉ khi `features` có `dirs` — §6.9).
   - `201` + `Location: /api/v1/uploads/<uid>` + thân `{"id":"<uid>","offset":0,"length":<tổng>,"dir":"<đã làm sạch>"}`
   - `415` không phải đuôi video · `507` không đủ chỗ (cần tổng + 16 MB) · `400` thiếu header / `X-File-Dir` sai
     (§6.9) · `503` quá nhiều phiên upload dở (tối đa 8).
2. **Gửi khúc** — `PATCH /api/v1/uploads/<uid>`, Bearer.
   Header: `Upload-Offset: <byte bắt đầu khúc này>`, `Content-Length: <cỡ khúc, > 0>`, `Content-Type:
   application/offset+octet-stream`. Thân: byte thô.
   - `204` + `Upload-Offset: <offset mới>` — chưa đủ, gửi khúc tiếp.
   - `201` + thân mục thư viện (như một phần tử của `list`) — **khúc cuối, file đã vào thư viện**. `name` có thể khác tên
     gửi (trùng thì điện thoại đổi).
   - `409` + `Upload-Offset: <offset đúng>` — offset gửi lên lệch. Tiếp từ offset đúng.
   - `423` — đang có một PATCH khác cho cùng upload. Chờ rồi `HEAD`.
   - `413` — khúc vượt quá `Upload-Length`. `404` — không có upload này (server đã tắt/khởi động lại, hoặc quá hạn) → tạo lại.
   - Đứt kết nối giữa khúc: phần byte đã tới **được giữ**. Client `HEAD` để biết offset thật rồi gửi tiếp.
   - Khúc gợi ý: 32 MB. Mỗi khúc một kết nối mới; đứt giữa khúc không mất byte đã tới, nên khúc lớn chỉ tiết kiệm bắt
     tay TCP — đừng nhỏ hơn 4 MB.
3. **Hỏi offset** — `HEAD /api/v1/uploads/<uid>`, Bearer → `200` + `Upload-Offset`, `Upload-Length`. `404` như trên.
4. **Huỷ** — `DELETE /api/v1/uploads/<uid>`, Bearer → `204`, file tạm bị xoá.

Vòng đời phiên upload: chỉ trong bộ nhớ điện thoại khi server chạy. Tắt Sync WiFi / app bị kill → mọi phiên dở bị huỷ,
file tạm xoá → PC nhận `404` → tạo lại từ 0. Phiên không có byte mới trong 60 phút cũng bị huỷ.

### 6.6 Đuôi video nhận

`mp4 m4v mov mkv webm ts 3gp avi flv` (không phân biệt hoa thường). Lọc phía PC cho khỏi phí
một request; server vẫn kiểm (`415`).

### 6.7 API cũ (trang web, vẫn chạy)

`GET /` trang · `POST /api/login` (thân PIN → cookie) · `GET /api/list` (mảng JSON) · `GET /api/file/<id>` ·
`PUT /api/upload` (một phát, không nối tiếp). App PC **không** dùng các route này.

### 6.8 IPv4 + IPv6

- Điện thoại: một `ServerSocket` dual-stack (`::`, cùng cổng cho cả hai họ); mDNS quảng bá A + AAAA; màn Sync hiện URL
  IPv4 và URL IPv6 (global / ULA, dạng `http://[fd12::5]:8080`); `info.addresses` liệt kê cả hai.
- PC: dò mDNS trên mọi interface (cả hai họ); thiết bị là một `id` với **tập địa chỉ**, không phải một IP.
  - Nối kiểu "happy eyeballs" đơn giản: thử địa chỉ đầu theo thứ tự §6.2, sau 250 ms chưa nối được thì thử song song địa
    chỉ kế; địa chỉ thắng được nhớ cho tới khi lỗi kết nối, lỗi thì chọn lại từ đầu (không đổi giữa một khúc PATCH).
  - Upload nối tiếp không phụ thuộc địa chỉ: khúc sau đi đường khác vẫn đúng (phiên upload theo `uid`, không theo kết nối).
  - **Bẫy**: crate `url` (và `reqwest`) không nhận zone id `%en0` trong URL. Đừng ghép IP vào URL: dùng host giả
    `<id>.vpsync.invalid` + `ClientBuilder::resolve(host, SocketAddr)` (link-local: `SocketAddrV6` có `scope_id`) — một
    đường code cho cả IPv4, IPv6, link-local.
- Mạng chỉ có IPv6 (không DHCPv4) hoặc chỉ IPv4 đều phải chạy. Mạng chặn multicast: nhập tay (§6.2).

### 6.9 Thư mục — đẩy như copy/paste

PC đẩy tệp và thư mục y như copy/paste giữa hai ổ trong Explorer / Finder; điện thoại dựng lại cây trong nơi lưu.

- **Đích**: gốc nơi lưu (mặc định) hoặc thư mục người dùng chọn ở tab thư viện điện thoại (một `dir` có thật trong `list`).
- **Tệp lẻ** thả vào ⇒ `X-File-Dir = <đích>`.
- **Thư mục** thả vào (vd `D:\Phim\Chuyến đi`) ⇒ mỗi video bên trong gửi với
  `X-File-Dir = <đích>/Chuyến đi/<đường con>` (giữ tên thư mục được thả làm gốc, như paste). Thư mục không có video nào
  (ở mọi cấp) thì không tạo. Tệp không phải video bỏ qua.
- **Mã hoá** `X-File-Dir`: URL-encode cả chuỗi UTF-8; sau khi giải mã, `/` là phân cách (PC Windows đổi `\` thành `/`
  trước khi gửi; điện thoại cũng coi `\` là phân cách).
- **Điện thoại làm sạch** từng đoạn như tên tệp (ký tự cấm `\ : * ? " < > |` và ký tự điều khiển → `_`, bỏ `.` đầu,
  bỏ khoảng trắng hai đầu), bỏ đoạn rỗng và `.`; `..` ⇒ `400`; quá 8 cấp hoặc quá 255 ký tự sau làm sạch ⇒ `400`.
  Chuỗi rỗng = gốc. `dir` trong phản hồi (`POST /uploads`, `201` cuối) là bản đã làm sạch — PC lưu lại để đối chiếu.
- **Trùng**: thư mục đã có ⇒ gộp vào (như paste "merge"); tệp trùng tên trong cùng thư mục ⇒ điện thoại đổi tên
  `a (1).mp4`. PC tránh gửi lại bằng bước Checking (trùng `dir` + tên + cỡ ⇒ bỏ qua, §5).
- Thư mục chưa có ⇒ điện thoại tạo khi **commit** (khúc cuối) — upload dở / huỷ không để lại thư mục rỗng.

## 7. Kiểm thử

- **Không cần điện thoại thật**: Android emulator + `adb forward tcp:18080 tcp:8080` → nhập tay `127.0.0.1:18080`
  (mDNS không qua được `adb forward`; để kiểm dò thì dùng điện thoại thật cùng WiFi).
- Bật server trên emulator: mở app → menu → Sync WiFi → Bật (hoặc `adb shell am start -n
  com.xmobile.videoplayer/.sync.ui.SyncActivity` với bản debug). PIN hiện trên màn hình.
- `curl` mẫu:
  ```bash
  curl -s http://127.0.0.1:18080/api/v1/info
  curl -s -X POST -H 'X-Client-Id: 0e5e6d0c-test' -H 'X-Client-Name: My%20PC' --data '123456' http://127.0.0.1:18080/api/v1/pair
  curl -s -i -X POST -H "Authorization: Bearer $T" -H 'X-File-Name: a.mp4' -H 'Upload-Length: 7340032' http://127.0.0.1:18080/api/v1/uploads
  curl -s -i -X PATCH -H "Authorization: Bearer $T" -H 'Upload-Offset: 0' --data-binary @part0 http://127.0.0.1:18080/api/v1/uploads/$U
  ```
- Lõi PC nên có test với một server giả trong tiến trình (cùng hợp đồng §6) cho các ca: đứt giữa khúc, `409`, `404`
  phiên mất, `507`, `401` sau khi bị quên, IP đổi giữa chừng.

## 8. Ngoài phạm vi v1 (đừng làm trước)

Xoá video trên điện thoại từ PC · đồng bộ thư mục hai chiều · mã hoá TLS / ghép bằng QR · đánh thức điện thoại từ xa
(điện thoại phải tự bật Sync WiFi) · nhiều điện thoại gửi song song cùng lúc (cho phép chọn, nhưng hàng đợi mỗi máy chạy
tuần tự) · iOS (chờ phía iOS cài giao thức này).
