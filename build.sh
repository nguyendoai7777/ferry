#!/usr/bin/env bash
# Build bản cài Ferry cho máy đang chạy, ra <gốc>/release/ferry/ (ferry-gd1_14). Gốc workspace gọi sang đây
# (`bash build.sh --project ferry`); chạy thẳng cũng được, kể cả từ worktree (build ở ferry/target/w<N>):
#   bash ferry/build.sh [-f | --force]    # --force: biên dịch lại mã của ta, giữ thư viện đã build
#   bash ferry/build.sh --draft           # thiếu trang giấy phép thì vẫn ra bản thử, tên có `_draft`
#   bash ferry/build.sh --plans           # ferry chưa có plan ⇒ in rỗng
# Dựng thật: packaging/linux/build-deb.sh (ferry_<version>_linux_amd64.deb), packaging/macos/build-macos.sh (Ferry.app +
# ferry_<version>_macos_<arm64|x64>.dmg; --arch arm64|x64 build chéo Apple Silicon ⇄ Intel); Windows (Git Bash) ⇒ build.ps1 (bản cài
# NSIS `_setup.exe` / `_portable.zip`, packaging/windows/build-installer.ps1). Mỗi tệp kèm `.sha256`.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$repo"
until [ -f "$root/tools/script-lib.sh" ]; do
  [ "$root" != / ] || { echo "Không thấy gốc workspace xdesktop (tools/script-lib.sh) phía trên $repo" >&2; exit 1; }
  root="$(dirname "$root")"
done
. "$root/tools/script-lib.sh"
repo="$(xd_path "$repo")"

force="" draft="" arch="" package=""
while [ $# -gt 0 ]; do
  case "$1" in
    --plans) exit 0 ;;
    -f|--force) force=1; shift ;;
    --draft) draft=1; shift ;;
    --arch) arch="${2:-}"; shift 2 || { echo "--arch cần giá trị: arm64, x64" >&2; exit 1; } ;;
    --arch=*) arch="${1#--arch=}"; shift ;;
    --chip) case "${2:-}" in apple) arch=arm64 ;; intel) arch=x64 ;; *) echo "--chip: apple hoặc intel (chỉ macOS)" >&2; exit 1 ;; esac; shift 2 ;;
    --package) package="${2:-}"; shift 2 || { echo "--package cần giá trị: setup,portable" >&2; exit 1; } ;;
    --package=*) package="${1#--package=}"; shift ;;
    --os) echo "ferry chỉ build cho OS đang chạy, không nhận --os." >&2; exit 1 ;;
    *) echo "Không hiểu tham số: $1" >&2; exit 1 ;;
  esac
done

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    exec pwsh -NoProfile -File "$(cygpath -w "$repo/build.ps1")" ${force:+-Force} ${package:+-Package "$package"} ;;
  Darwin) host_os=macos ;;
  *) host_os=linux ;;
esac
[ "$host_os" = macos ] || [ -z "$arch" ] || [ "$arch" = x64 ] \
  || { echo "Windows / Linux chỉ build x64; --arch chỉ dùng trên macOS." >&2; exit 1; }
[ -z "$package" ] || { echo "--package chỉ có trên Windows." >&2; exit 1; }

# Master build ở ferry/target; worktree w<N> ở ferry/target/w<N> (D-18) — không trộn nguồn trong một thư mục build.
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  name="$(xd_target_name "$repo" ferry)"
  if [ "$name" = start ]; then export CARGO_TARGET_DIR="$root/ferry/target"; else export CARGO_TARGET_DIR="$root/ferry/target/$name"; fi
fi
out="$root/release/ferry"
mkdir -p "$out"

if [ "$host_os" = macos ]; then
  case "${arch:-$(uname -m)}" in
    arm64|aarch64|apple) mac_arch=arm64; triple=aarch64-apple-darwin ;;
    x64|x86_64|intel) mac_arch=x86_64; triple=x86_64-apple-darwin ;;
    *) echo "--arch: arm64 hoặc x64" >&2; exit 1 ;;
  esac
fi
if [ -n "$force" ]; then
  clean_target=()
  [ "$host_os" != macos ] || clean_target=(--target "$triple")
  cargo clean --release --manifest-path "$repo/Cargo.toml" ${clean_target[@]+"${clean_target[@]}"} \
    -p ferry-core -p ferry-daemon -p ferry-ui -p materiant -p devtool
fi
case "$host_os" in
  macos) exec bash "$repo/packaging/macos/build-macos.sh" --arch "$mac_arch" --out "$out" ${draft:+--draft} ;;
  *) exec bash "$repo/packaging/linux/build-deb.sh" --out "$out" ${draft:+--draft} ;;
esac
