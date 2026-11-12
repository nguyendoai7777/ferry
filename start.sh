#!/usr/bin/env bash
# Mở ferry bản debug để xem tay: build `ferry` (khay) + `ferry-ui` (cửa sổ) rồi chạy `ferry`; chọn "Mở cửa sổ" ở khay
# (hoặc chạy `ferry` lần nữa) để mở cửa sổ. Gốc workspace gọi sang đây (`bash start.sh ferry`); chạy thẳng cũng
# được, kể cả từ worktree (build ở target/w<N>):
#   bash ferry/start.sh [--no-watch]
# Dữ liệu dev riêng, không đụng bản đã cài: cấu hình + log + khoá + socket ở ferry/target/dev (dùng chung giữa
# master và mọi worktree ⇒ mỗi lúc một daemon dev, như một máy thật). Log: target/dev/data/logs/{daemon,ui}.log.
# Mặc định theo dõi mã (tools/watch-run.mjs của gốc): lưu tệp ⇒ build lại, mở lại `ferry` (cửa sổ đang mở thoát theo).
# --no-watch hoặc XDESKTOP_NO_WATCH=1: build rồi chạy một lần. Thoát: menu khay → Thoát (hoặc Ctrl+C).
#   bash ferry/start.sh --plans   chưa có plan ⇒ in rỗng (start.sh gốc bỏ bước hỏi plan)
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$repo"
until [ -f "$root/tools/script-lib.sh" ]; do
  [ "$root" != / ] || { echo "Không thấy gốc workspace xdesktop (tools/script-lib.sh) phía trên $repo" >&2; exit 1; }
  root="$(dirname "$root")"
done
. "$root/tools/script-lib.sh"
repo="$(xd_path "$repo")"

xd_parse_watch "$@"
for a in "${xd_args[@]+"${xd_args[@]}"}"; do
  case "$a" in
    --plans) exit 0 ;;
    -h|--help|help) sed -n '2,/^set -euo/p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Không hiểu tham số: $a" >&2; exit 1 ;;
  esac
done

# Master build ở ferry/target/start; worktree w<N> build ở ferry/target/w<N> (D-18).
export CARGO_TARGET_DIR="$root/ferry/target/$(xd_target_name "$repo" ferry)"
dev="$root/ferry/target/dev"
export FERRY_DATA_DIR="$dev/data" FERRY_CONFIG_FILE="$dev/config.json"
echo "ferry từ $repo (dữ liệu dev: $dev)"
# Wayland không cho app tự gửi icon: GNOME ghép cửa sổ theo `app_id` với `<app_id>.desktop` để lấy icon cho dock. Cửa sổ
# bản debug có mã riêng `ferry.dev` (crates/ui/src/main.rs `APP_ID`) ⇒ mục "Ferry (dev)" cạnh "Ferry" của gói, không đè.
# Tệp tự ghi có dấu `X-Xdesktop-Dev=true`; tệp cùng tên không có dấu (người dùng tự viết) thì không đụng.
ferry_dev_desktop_entry() {
  local dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications" marker="X-Xdesktop-Dev=true"
  local file="$dir/ferry.dev.desktop"
  if [ -f "$file" ] && ! grep -qx "$marker" "$file"; then return 0; fi
  mkdir -p "$dir"
  cat > "$file.tmp" <<ENTRY
[Desktop Entry]
Type=Application
Name=Ferry (dev)
Exec=$CARGO_TARGET_DIR/debug/ferry
Icon=$repo/packaging/linux/icons/256x256.png
Terminal=false
Categories=Network;FileTransfer;
StartupWMClass=ferry.dev
$marker
ENTRY
  mv "$file.tmp" "$file"
}
[ "$(uname -s)" = Linux ] && ferry_dev_desktop_entry

m="$root/materiant"
xd_run_app --scope ferry.dev "$CARGO_TARGET_DIR/debug/ferry" \
  "$repo/crates|$repo/i18n|$repo/Cargo.toml|$m/src|$m/assets|$m/vendor|$m/Cargo.toml" \
  -- --manifest-path "$repo/Cargo.toml" -p ferry-daemon -p ferry-ui
