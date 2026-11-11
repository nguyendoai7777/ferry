#!/usr/bin/env bash
# Dựng `<out>/Ferry.app` + `ferry_<version>_macos_<arm64|x64>[_draft].dmg`, ký ad-hoc (ferry-gd1_14, bê groove). Chạy trên Mac.
# Không Developer ID / notarize: dịch vụ trả phí, chủ dự án quyết; lần đầu mở thì Gatekeeper cảnh báo, chuột phải > Open.
#   bash packaging/macos/build-macos.sh                # bản phát hành: thiếu trang giấy phép là dừng
#   bash packaging/macos/build-macos.sh --draft        # bản thử: thiếu thì cảnh báo, tên `.dmg` có `_draft`
# Thêm: --arch arm64|x86_64 (mặc định kiến trúc máy), --skip-build, --out <thư mục> (mặc định dist/macos-<arch>).
# Cần: `cargo install cargo-about --locked --features cli`, `node` (`packaging/licenses/gen-licenses.mjs`).
#
# Tự chạy cùng máy (`crates/daemon/src/autostart.rs`): LaunchAgent gọi `open -g -a <Ferry.app>` khi exe nằm trong `.app` ⇒ dời
# app sang chỗ khác thì daemon tự ghi lại lúc khởi động. Kéo vào /Applications trước khi bật.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
arch="$(uname -m)"
draft=""
skip_build=""
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) arch="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --draft) draft=1; shift ;;
    --skip-build) skip_build=1; shift ;;
    *) echo "Tham số lạ: $1" >&2; exit 2 ;;
  esac
done
[ "$(uname -s)" = Darwin ] || { echo "Chỉ chạy trên macOS." >&2; exit 1; }
case "$arch" in
  arm64 | aarch64) arch=arm64; triple=aarch64-apple-darwin ;;
  x86_64 | x64) arch=x86_64; triple=x86_64-apple-darwin ;;
  *) echo "Kiến trúc không hỗ trợ: $arch" >&2; exit 2 ;;
esac

# `tr`: checkout từ Windows có thể là CRLF. Dòng version đầu tiên là của [workspace.package].
version="$(tr -d '\r' < "$repo/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)"
[ -n "$version" ] || { echo "Không đọc được version trong Cargo.toml" >&2; exit 1; }
target_dir="${CARGO_TARGET_DIR:-$repo/target}"
release="$target_dir/$triple/release"

if [ -z "$skip_build" ]; then
  rustup target list --installed | grep -qx "$triple" || rustup target add "$triple"
  (cd "$repo" && cargo build --release --locked --target "$triple" -p ferry-daemon -p ferry-ui --bins)
fi

[ -n "$out" ] || out="$repo/dist/macos-$arch"
mkdir -p "$out"
app="$out/Ferry.app"
resources="$app/Contents/Resources"
# Chỉ xoá `.app` cũ: `--out` có thể là thư mục chung chứa bản dựng khác (release/ferry).
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$resources/vi.lproj" "$resources/en.lproj"
cp "$release/ferry" "$release/ferry-ui" "$app/Contents/MacOS/"
cp "$repo/packaging/macos/Info.plist" "$app/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$version" "$app/Contents/Info.plist"
plutil -lint -s "$app/Contents/Info.plist"
cp "$repo/packaging/macos/ferry.icns" "$resources/"
# Lời xin quyền mạng LAN theo ngôn ngữ máy.
printf '"NSLocalNetworkUsageDescription" = "Ferry finds your phone on the home WiFi and sends files to it.";\n' \
  > "$resources/en.lproj/InfoPlist.strings"
printf '"NSLocalNetworkUsageDescription" = "Ferry tìm điện thoại trong WiFi nhà và gửi tệp sang đó.";\n' \
  > "$resources/vi.lproj/InfoPlist.strings"

warnings=""
if command -v node >/dev/null 2>&1 && command -v cargo-about >/dev/null 2>&1; then
  node "$repo/packaging/licenses/gen-licenses.mjs" "$resources/licenses"
elif [ -n "$draft" ]; then
  warnings="⚠ Không có trang giấy phép: cần node + cargo-about."$'\n'
else
  echo "Không có trang giấy phép: cần node + cargo-about (bản thử: --draft)." >&2
  exit 1
fi

# Ký từ trong ra ngoài, không `--deep`.
codesign --force --sign - --identifier com.sandtechs.desktop.ferry.ui "$app/Contents/MacOS/ferry-ui"
codesign --force --sign - --identifier com.sandtechs.desktop.ferry.daemon "$app/Contents/MacOS/ferry"
codesign --force --sign - --identifier com.sandtechs.desktop.ferry "$app"
codesign --verify --deep --strict "$app"

suffix=""
[ -z "$draft" ] || suffix="_draft"
# Tên tệp: {app}_{version}_{os}_{arch}[_draft].dmg, x86_64 viết là x64.
dmg="$out/ferry_${version}_macos_${arch/x86_64/x64}$suffix.dmg"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname "Ferry $version" -srcfolder "$stage" -format UDZO -ov "$dmg"
( cd "$out" && shasum -a 256 "$(basename "$dmg")" > "$(basename "$dmg").sha256" )

printf '\nXong: %s (%s)\n%s\n%s' "$dmg" "$(du -h "$dmg" | cut -f1)" "$app" "$warnings"
