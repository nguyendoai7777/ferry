# Build bản Windows của Ferry, ra <gốc>/release/ferry/ (ferry-gd1_14). Gốc workspace gọi sang đây (`pwsh ./build.ps1 -Project ferry`);
# chạy thẳng cũng được:
#   pwsh ferry/build.ps1 [-f]                         # -Force: biên dịch lại mã của ta, giữ thư viện đã build
#   pwsh ferry/build.ps1 -Package setup,portable      # không hỏi dạng đóng gói
# Hỏi dạng đóng gói (ô tích): bản cài NSIS `ferry_<version>_win_x64_setup.exe` (tích sẵn; cần admin lúc cài — luật tường lửa mDNS)
# và / hoặc bản chạy thẳng `ferry_<version>_win_x64_portable.zip`; mỗi tệp kèm `.sha256` (packaging/windows/build-installer.ps1).
# Chỉ x64. macOS / Linux: chuyển sang build.sh (.app + .dmg / .deb).
param(
    [Alias('f')] [switch] $Force,
    [string[]] $Package
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$repo = $PSScriptRoot
$root = $repo
while (-not (Test-Path (Join-Path $root 'tools/script-lib.ps1'))) {
    $root = Split-Path $root
    if (-not $root) { throw "Không thấy gốc workspace xdesktop (tools/script-lib.ps1) phía trên $repo" }
}
. (Join-Path $root 'tools/script-lib.ps1')

if (-not $IsWindows) {
    if ($Package) { throw '-Package chỉ có trên Windows.' }
    bash (Join-Path $repo 'build.sh') @(if ($Force) { '--force' })
    exit $LASTEXITCODE
}
# `-Package setup,portable` từ dòng lệnh đến dạng một chuỗi.
$Package = @($Package | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if (-not $Package) {
    $Package = Select-Many 'Dạng đóng gói:' @(
        (Option 'setup' 'Bản cài (_setup.exe)'), (Option 'portable' 'Bản chạy thẳng (_portable.zip)')) @('setup')
}
foreach ($p in $Package) { if ($p -notin 'setup', 'portable') { throw "-Package: setup, portable (không phải '$p')" } }

# Master build ở ferry/target; worktree <repo>-w<N> ở ferry/target/w<N> (D-18).
if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $root 'ferry/target'
    if ((Resolve-Path $repo).Path -ne (Resolve-Path (Join-Path $root 'ferry')).Path) {
        $env:CARGO_TARGET_DIR = Join-Path $env:CARGO_TARGET_DIR ((Split-Path $repo -Leaf) -replace '^ferry-', '')
    }
}
if ($Force) {
    cargo clean --release --target x86_64-pc-windows-msvc --manifest-path (Join-Path $repo 'Cargo.toml') -p ferry-core -p ferry-daemon -p ferry-ui -p materiant -p devtool
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
& (Join-Path $repo 'packaging/windows/build-installer.ps1') -OutDir (Join-Path $root 'release/ferry') -Package $Package
exit $LASTEXITCODE
