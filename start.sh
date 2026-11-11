#!/usr/bin/env bash
# Mở media-wireless-sync bản debug để xem tay: build `mws` (khay) + `mws-ui` (cửa sổ) rồi chạy `mws`; chọn "Mở cửa sổ" ở khay
# (hoặc chạy `mws` lần nữa) để mở cửa sổ. Gốc workspace gọi sang đây (`bash start.sh media-wireless-sync`); chạy thẳng cũng
# được, kể cả từ worktree (build ở target/w<N>):
#   bash media-wireless-sync/start.sh [--no-watch]
# Dữ liệu dev riêng, không đụng bản đã cài: cấu hình + log + khoá + socket ở media-wireless-sync/target/dev (dùng chung giữa
# master và mọi worktree ⇒ mỗi lúc một daemon dev, như một máy thật). Log: target/dev/data/logs/{daemon,ui}.log.
# Mặc định theo dõi mã (tools/watch-run.mjs của gốc): lưu tệp ⇒ build lại, mở lại `mws` (cửa sổ đang mở thoát theo).
# --no-watch hoặc XDESKTOP_NO_WATCH=1: build rồi chạy một lần. Thoát: menu khay → Thoát (hoặc Ctrl+C).
#   bash media-wireless-sync/start.sh --plans   chưa có plan ⇒ in rỗng (start.sh gốc bỏ bước hỏi plan)
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

# Master build ở media-wireless-sync/target/start; worktree w<N> build ở media-wireless-sync/target/w<N> (D-18).
export CARGO_TARGET_DIR="$root/media-wireless-sync/target/$(xd_target_name "$repo" media-wireless-sync)"
dev="$root/media-wireless-sync/target/dev"
export MWS_DATA_DIR="$dev/data" MWS_CONFIG_FILE="$dev/config.json"
echo "media-wireless-sync từ $repo (dữ liệu dev: $dev)"
m="$root/materiant"
xd_run_app --scope media-wireless-sync.dev "$CARGO_TARGET_DIR/debug/mws" \
  "$repo/crates|$repo/i18n|$repo/Cargo.toml|$m/src|$m/assets|$m/vendor|$m/Cargo.toml" \
  -- --manifest-path "$repo/Cargo.toml" -p mws-daemon -p mws-ui
