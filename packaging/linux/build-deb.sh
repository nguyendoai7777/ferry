#!/usr/bin/env bash
# Dựng `<out>/ferry_<version>_linux_<arch>[_draft].deb` (ferry-gd1_14, bê `groove/packaging/linux/build-deb.sh`). Chạy trên
# Ubuntu / Debian:
#   sudo apt install dpkg-dev
#   bash packaging/linux/build-deb.sh                  # bản phát hành: thiếu trang giấy phép là dừng
#   bash packaging/linux/build-deb.sh --draft          # bản thử: thiếu thì cảnh báo, tên có `_draft`
# Thêm: --skip-build, --out <thư mục> (mặc định dist/linux).
#
# Gói: /usr/bin/{ferry,ferry-ui} (hai exe cạnh nhau: daemon bật `ferry-ui` nằm cạnh nó), mục menu `ferry.desktop` (trùng
# `app_id` `ferry` của cửa sổ ⇒ dock gom đúng), icon hicolor, AppStream, trang giấy phép. Không cài mục tự chạy: đó là lựa chọn
# của từng người dùng (`crates/daemon/src/autostart.rs` ghi `~/.config/autostart/ferry.desktop` trỏ `/usr/bin/ferry --autostart`,
# có `TryExec` nên gỡ gói thì GNOME bỏ qua mục đó). Phụ thuộc thư viện lấy từ `dpkg-shlibdeps`. Không maintainer script: trigger
# của `desktop-file-utils`, `hicolor-icon-theme` tự cập nhật menu, icon.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
draft=""
skip_build=""
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out="$2"; shift 2 ;;
    --draft) draft=1; shift ;;
    --skip-build) skip_build=1; shift ;;
    *) echo "Tham số lạ: $1" >&2; exit 2 ;;
  esac
done
[ "$(uname -s)" = Linux ] || { echo "Chỉ chạy trên Linux." >&2; exit 1; }
for tool in dpkg-deb dpkg-shlibdeps dpkg-architecture; do
  command -v "$tool" >/dev/null 2>&1 || { echo "Thiếu $tool: sudo apt install dpkg-dev" >&2; exit 1; }
done

# `tr`: checkout từ Windows có thể là CRLF, `"$` không khớp. Dòng version đầu tiên là của [workspace.package].
version="$(tr -d '\r' < "$repo/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)"
[ -n "$version" ] || { echo "Không đọc được version trong Cargo.toml" >&2; exit 1; }
arch="$(dpkg-architecture -qDEB_HOST_ARCH)"
target_dir="${CARGO_TARGET_DIR:-$repo/target}"
bins=(ferry ferry-ui)

if [ -z "$skip_build" ]; then
  (cd "$repo" && cargo build --release --locked -p ferry-daemon -p ferry-ui --bins)
fi
for b in "${bins[@]}"; do
  [ -x "$target_dir/release/$b" ] || { echo "Thiếu $target_dir/release/$b" >&2; exit 1; }
  # Bản phát hành không được mang inspector (D-19, chỉ bản debug) hay đường chạy thử.
  if grep -q -a -e 'Hover to highlight' -e 'inspector-pick' "$target_dir/release/$b"; then
    echo "$b còn inspector (bản debug?)" >&2; exit 1
  fi
done

[ -n "$out" ] || out="$repo/dist/linux"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
root="$work/root"
doc="$root/usr/share/doc/ferry"
mkdir -p "$root/DEBIAN" "$root/usr/bin" "$root/usr/share/applications" "$doc"
for b in "${bins[@]}"; do install -m 0755 "$target_dir/release/$b" "$root/usr/bin/$b"; done
# Chuẩn desktop entry cấm CR: checkout từ Windows có thể là CRLF.
tr -d '\r' < "$repo/packaging/linux/ferry.desktop" > "$root/usr/share/applications/ferry.desktop"
chmod 0644 "$root/usr/share/applications/ferry.desktop"
if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$root/usr/share/applications/ferry.desktop"
fi
install -d "$root/usr/share/metainfo"
metainfo="$root/usr/share/metainfo/com.sandtechs.desktop.ferry.metainfo.xml"
tr -d '\r' < "$repo/packaging/linux/ferry.metainfo.xml" \
  | sed -e "s|@VERSION@|$version|" -e "s|@DATE@|$(date +%Y-%m-%d)|" > "$metainfo"
chmod 0644 "$metainfo"
if command -v appstreamcli >/dev/null 2>&1; then
  appstreamcli validate --no-net --explain "$metainfo" || echo "⚠ metainfo chưa qua appstreamcli validate" >&2
fi
# Icon hicolor mọi cỡ (`packaging/icons/gen-icons.mjs` sinh).
for icon in "$repo"/packaging/linux/icons/*x*.png; do
  dim="$(basename "$icon" .png)"
  install -D -m 0644 "$icon" "$root/usr/share/icons/hicolor/$dim/apps/ferry.png"
done

warnings=""
# Trang giấy phép: `gen-licenses.mjs` (Node). Thiếu cargo-about thì cài vào ~/.cargo (không sudo, vài phút lần đầu).
if ! command -v cargo-about >/dev/null 2>&1; then
  echo "==> Cài cargo-about (trang giấy phép)"
  cargo install cargo-about --locked --features cli || echo "⚠ Cài cargo-about không xong." >&2
fi
if command -v cargo-about >/dev/null 2>&1 && command -v node >/dev/null 2>&1; then
  node "$repo/packaging/licenses/gen-licenses.mjs" "$doc"
elif [ -n "$draft" ]; then
  warnings="⚠ Không có trang giấy phép: cần cargo-about + node."$'\n'
else
  echo "Không có trang giấy phép: cần cargo-about + node (bản thử: --draft)." >&2
  exit 1
fi
cat > "$doc/copyright" <<'EOF'
Ferry is proprietary software. Copyright © 2026 Sandtechs (z0 team, sandtechs jsc.), https://sandtechs.com.
Third-party licenses: /usr/share/doc/ferry/third-party.html
EOF
chmod 0644 "$doc"/*

# `dpkg-shlibdeps` đòi `debian/control` ở thư mục đang đứng, nội dung không dùng tới.
mkdir -p "$work/debian"
: > "$work/debian/control"
depends="$(cd "$work" && dpkg-shlibdeps -O root/usr/bin/ferry root/usr/bin/ferry-ui 2>/dev/null | sed -n 's/^shlibs:Depends=//p')"
[ -n "$depends" ] || { echo "dpkg-shlibdeps không ra phụ thuộc" >&2; exit 1; }
# Thư viện GPUI / wgpu mở lúc chạy (`dlopen`), `dpkg-shlibdeps` không thấy: Wayland, Vulkan (rơi về GLES qua EGL).
depends="$depends, libwayland-client0, libvulkan1 | libegl1"
# Libc của máy dựng là sàn: dựng trên Ubuntu 26.04 thì gói đòi libc6 >= 2.43 (bản cũ hơn không cài được).

size="$(du -sk "$root/usr" | cut -f1)"
cat > "$root/DEBIAN/control" <<EOF
Package: ferry
Version: $version
Section: net
Priority: optional
Architecture: $arch
Maintainer: Ferry <nguyendoai7777@gmail.com>
Homepage: https://sandtechs.com
Installed-Size: $size
Depends: $depends
Recommends: desktop-file-utils, hicolor-icon-theme, gnome-shell-extension-appindicator
Description: Send files and folders to your phone over WiFi
 Ferry sits in the system tray and sends files and folders from this computer
 to the phone app over the home WiFi. Sending resumes from the last byte after
 a dropped connection. Videos can also be downloaded back from the phone.
 On GNOME the tray icon needs the AppIndicator extension.
EOF

suffix=""
[ -z "$draft" ] || suffix="_draft"
# Tên tệp: {app}_{version}_{os}_{arch}[_draft].deb (arch theo Debian: amd64).
deb="$out/ferry_${version}_linux_${arch}${suffix}.deb"
dpkg-deb --build --root-owner-group "$root" "$deb"
( cd "$out" && sha256sum "$(basename "$deb")" > "$(basename "$deb").sha256" )
printf '\nXong: %s (%s)\nDepends: %s\n%s' "$deb" "$(du -h "$deb" | cut -f1)" "$depends" "$warnings"
