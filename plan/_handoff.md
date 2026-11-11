# Bàn giao hiện trạng — sync WiFi điện thoại ↔ PC (11/11/2026)

Tài liệu cho session điều phối mới (máy Linux) làm tiếp tới khi xong app PC. Bàn giao từng task đã nằm trong khối
`## 0. Bàn giao từ …` của task nhận — tệp này chỉ là bản đồ tổng + cách chạy tiếp. Xong GĐ1 thì xoá tệp này.

## 1. Mục tiêu

App khay PC `ferry` (Rust + GPUI + materiant; Windows · macOS · Linux) gửi tệp **và thư mục** video sang app
điện thoại `video-player` (Android) qua WiFi nhà, y như copy/paste trong Explorer / Finder; chạy trên cả IPv4 lẫn IPv6;
đứt thì gửi tiếp từ byte cuối; kéo video về máy là phụ. Đích GĐ1: `ROADMAP.md` (14 task).

Hợp đồng: `PROTOCOL.md` (W-1). Bản sao mục 6 ở repo điện thoại `xmobile/video-player/PC-implement.md` — **hai bản mục 6
phải giống hệt**; đổi giao thức ⇒ sửa cả hai trước, rồi code hai phía. Quyết định: `../.claude/rules/decisions/ferry.md`
— W-1 hợp đồng, W-2 phạm vi một chiều, **W-3** IPv4 + IPv6 + đẩy thư mục (`X-File-Dir`, `dir`, `addresses`, `features`),
**W-4** nút "Chọn tệp…" (nhiều) / "Chọn thư mục…" + khay chờ + trình duyệt điện thoại kiểu Windows Explorer (`PhoneBrowser`,
PROTOCOL 4.2, 4.6).

## 2. Hiện trạng

| Repo (remote) | Commit cuối lúc bàn giao | Trạng thái |
|---|---|---|
| `xmobile` (`nguyendoai7777/xmobile`) | `2ba2de3` | Android xong phía điện thoại: task_09 (giao thức v1), task_10 (IPv6), task_11 (thư mục) — `video-player/android/plan/` |
| `xdesktop` (`nguyendoai7777/xdesktop`) | xem `git log` | quyết định W-3/W-4, `init.*` biết repo này |
| `ferry` (`nguyendoai7777/ferry`) | `b46f3b9` + commit bàn giao này | gd1_01 → gd1_06 ✅ |
| `materiant` (`nguyendoai7777/materiant-rust`) | `22fefca` | không đổi trong đợt này |

Task PC đã xong (chi tiết ở mục 9 từng tệp): 01 workspace · 02 protocol-client + `fake_phone` · 03 hàng đợi SQLite (lô +
`dir`, `plan_batch`) · 04 upload runner (Link qua watch, v4↔v6 giữa khúc, `X-File-Dir` theo `features`) · 05 dò mDNS +
sổ thiết bị (thứ tự địa chỉ, happy eyeballs 250 ms, nhập tay `[v6%iface]:port`) · 06 ghép PIN + kho khoá OS
(Linux: `keyring` qua `zbus` thuần Rust).

### Chờ chủ dự án kiểm tay (`[owner-qa]`)

- gd1_01: chạy khay + cửa sổ trên Windows và Ubuntu (mới thử macOS). Khay Linux (SNI) có từ gd1_07.
- gd1_05 C4: điện thoại thật cùng WiFi hiện trong ≤ 5 s (emulator không phát mDNS ra LAN).
- gd1_06 C3: khởi động lại máy vẫn Online (Windows, GNOME).
- Android: URL IPv6 trên màn Sync (emulator không có IPv6 ULA/global — cần điện thoại thật + WiFi có IPv6); đẩy thư mục
  trên Android ≤ 9 (API < 29) mới build, chưa chạy.

## 3. Việc kế tiếp (theo `ROADMAP.md`, mỗi đợt một nhóm chạy song song)

| Đợt | Task | Ghi chú |
|---|---|---|
| 5 | **gd1_07** daemon-runtime (opus) | một mình; gom 04/05/06 vào runtime tokio của `ferry`, snapshot + lệnh qua IPC, khay 3 trạng thái, thông báo. **Bê khay Linux SNI của fastcast** (nợ gd1_01). Đọc các khối `## 0` trong tệp task |
| 6 | gd1_08 main-window (sonnet, cỡ L — có `PhoneBrowser` W-4) · gd1_09 add-phone · gd1_12 settings · gd1_10 download-runner (opus) | song song sau 07 |
| 7 | gd1_11 phone-library (cần 08 + 10, dùng lại `PhoneBrowser`) · gd1_13 watch-folder (cần 04 + 12) | |
| 8 | gd1_14 packaging (opus) | bản cài ba OS, rồi `close-phase` |

## 4. Chạy tiếp trên máy Linux

1. Kéo về: `git -C <xdesktop> pull --rebase`; repo app chưa có ⇒ `bash init.sh ferry` (clone repo này +
   materiant, soát gói apt). Có rồi ⇒ `git -C ferry pull --rebase`, tương tự `materiant`, `xmobile`.
2. Worktree cố định (D-18, gitignore nên mỗi máy tạo một lần):
   `for i in 1 2 3; do git -C ferry worktree add --detach ../.claude/worktrees/ferry-w$i master; done`
3. Session điều phối mở ở gốc `xdesktop`, gọi `/ferry`. Mỗi task một session con: `cwd` = worktree rảnh,
   không tạo worktree mới, model theo cột Model của `ROADMAP.md`. Brief mẫu (đã dùng cho 02–06):
   - task `plan/ferry-gd1_NN_*.md`, worktree `…/ferry-wK`, build `target/wK`;
   - `git -C <worktree> switch --detach master`, nhánh `claude/gd1_NN-<tên>`;
   - skill `run-task` + `/ferry`; đọc `CLAUDE.md`, `PROTOCOL.md` mục liên quan, W-1…W-4, khối `## 0` của task;
   - session song song sửa tệp nào thì không đụng; `tests/fake_phone/mod.rs` chỉ thêm hàm mới;
   - soát xanh 4 lệnh → commit → `rebase master` → soát lại → `merge --ff-only` → worktree về HEAD tách → `branch -d`;
     ghi mục 9; không push; báo cuối vài dòng.
4. Thử với điện thoại: emulator Linux `Medium_Phone_API_37.0` (SDK `~/Android/Sdk`), cài app từ `xmobile`
   (`shared/tools/agradle.sh video-player :app:installDebug`), bật Sync WiFi, `adb forward tcp:18080 tcp:8080`, nhập tay
   `127.0.0.1:18080` (PROTOCOL mục 7). `adb forward` chỉ IPv4; IPv6 thật cần điện thoại thật.
5. Push khi chủ dự án bảo (luật mặc định: không push).

## 5. Skill nên gọi

`/ferry` (ngữ cảnh) → `run-task ferry-gd1_07` (hoặc giao session con) → `handoff` khi xong
từng task (run-task tự gọi) → `close-phase` khi gd1_14 ✅. Phía điện thoại: `/app-video-android` trong `xmobile`.
