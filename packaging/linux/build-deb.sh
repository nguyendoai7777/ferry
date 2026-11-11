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
# có `TryExec` nên gỡ gói thì GNOME bỏ qua mục đó). Phụ thuộc thư viện lấy từ `dpkg-shlibdeps`. Trigger của `desktop-file-utils`,
# `hicolor-icon-theme` tự cập nhật menu, icon.
#
# Maintainer script chỉ lo tường lửa cho dò mDNS (W-6, cài gói đã là bước người dùng xác nhận quyền root): postinst — `ufw` có
# và đang bật ⇒ cho phép chiều vào UDP 5353 tới 224.0.0.251 + ff02::fb, comment `ferry mDNS`, ghi dấu
# `/var/lib/ferry/ufw-mdns-allowed` (daemon thấy dấu thì không gợi ý nữa — `ferry_core::firewall`); postrm `remove` / `purge` —
# xoá đúng các luật mang comment đó (cả luật app thêm qua `pkexec`) + dấu. Không ufw / không bật / ufw lỗi ⇒ bỏ qua im lặng,
# không bao giờ làm cài / gỡ gói lỗi. Không tắt tường lửa, không đụng luật khác.
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

# W-6: comment phải khớp `ferry_core::firewall::RULE_COMMENT` (test `deb_scripts_use_the_same_comment`).
cat > "$root/DEBIAN/postinst" <<'EOF'
#!/bin/sh
# ferry: cho phép dò điện thoại qua mDNS khi ufw đang bật (W-6). Lỗi ở đây không bao giờ làm hỏng cài gói.
set -e
comment='ferry mDNS'
marker=/var/lib/ferry/ufw-mdns-allowed
if [ "$1" = configure ] && command -v ufw >/dev/null 2>&1; then
  if ufw status 2>/dev/null | grep -q "^Status: active"; then
    # IPv6 tắt trong /etc/default/ufw ⇒ luật ff02::fb báo lỗi: không tính là hỏng.
    ufw allow in proto udp to ff02::fb port 5353 comment "$comment" >/dev/null 2>&1 || true
    if ufw allow in proto udp to 224.0.0.251 port 5353 comment "$comment" >/dev/null 2>&1; then
      mkdir -p /var/lib/ferry && echo "added by the ferry package" > "$marker" || true
    else
      echo "ferry: could not add the ufw rule for mDNS; the app will ask again" >&2
    fi
  fi
fi
exit 0
EOF
cat > "$root/DEBIAN/postrm" <<'EOF'
#!/bin/sh
# ferry: gỡ đúng các luật ufw mang comment của ferry (W-6). Lỗi ở đây không bao giờ làm hỏng gỡ gói.
set -e
comment='ferry mDNS'
case "$1" in
  remove|purge)
    if command -v ufw >/dev/null 2>&1; then
      # "ufw show added" in mỗi luật dạng: ufw allow in proto udp to 224.0.0.251 port 5353 comment 'ferry mDNS'
      ufw show added 2>/dev/null | grep -F "comment '$comment'" | while read -r line; do
        rule=${line#ufw }
        rule=${rule% comment *}
        # shellcheck disable=SC2086 # tách từ có chủ ý: luật không chứa khoảng trắng nào ngoài comment đã bỏ
        ufw delete $rule >/dev/null 2>&1 || echo "ferry: could not delete ufw rule: $rule" >&2
      done || true
    fi
    rm -f /var/lib/ferry/ufw-mdns-allowed
    rmdir /var/lib/ferry 2>/dev/null || true
    ;;
esac
exit 0
EOF
chmod 0755 "$root/DEBIAN/postinst" "$root/DEBIAN/postrm"

suffix=""
[ -z "$draft" ] || suffix="_draft"
# Tên tệp: {app}_{version}_{os}_{arch}[_draft].deb (arch theo Debian: amd64).
deb="$out/ferry_${version}_linux_${arch}${suffix}.deb"
dpkg-deb --build --root-owner-group "$root" "$deb"
( cd "$out" && sha256sum "$(basename "$deb")" > "$(basename "$deb").sha256" )
printf '\nXong: %s (%s)\nDepends: %s\n%s' "$deb" "$(du -h "$deb" | cut -f1)" "$depends" "$warnings"
