<#
.SYNOPSIS
  Dựng bản Windows của Ferry (ferry-gd1_14, bê groove): build release → trang giấy phép (`cargo deny` đỏ thì dừng) → bản cài NSIS
  `<OutDir>/ferry_<version>_win_x64_setup.exe` và / hoặc bản chạy thẳng `ferry_<version>_win_x64_portable.zip`, mỗi tệp kèm
  `.sha256`. Bước nào đỏ thì dừng. Không ký mã: SmartScreen sẽ cảnh báo (ký là dịch vụ trả phí, chủ dự án quyết).

  Bản portable không có luật tường lửa mDNS (cần admin, chỉ bản cài ghi): lần đầu chạy Windows hỏi cho phép mạng — chọn cả
  Private lẫn Public, không thì dò điện thoại không chạy trên WiFi kiểu Public (vẫn nhập tay địa chỉ được).

.EXAMPLE
  winget install NSIS.NSIS; cargo install cargo-deny --locked; cargo install cargo-about --locked --features cli
  pwsh packaging/windows/build-installer.ps1
  pwsh packaging/windows/build-installer.ps1 -OutDir ../release/ferry -Package setup,portable
#>
param(
    [string] $OutDir,
    [string] $Makensis,
    [ValidateSet('setup', 'portable')] [string[]] $Package = @('setup'),
    [switch] $SkipBuild
)
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not $OutDir) { $OutDir = Join-Path $repo 'dist' }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$stage = Join-Path $OutDir 'ferry-stage'
$triple = 'x86_64-pc-windows-msvc'

function Find-Makensis {
    if ($Makensis) { return (Resolve-Path $Makensis).Path }
    $onPath = Get-Command makensis -ErrorAction Ignore
    if ($onPath) { return $onPath.Source }
    # Worktree nằm ở `.claude/worktrees/…`: tìm theo checkout chính của repo (`<…>/ferry/.git`), không theo thư mục đang đứng.
    $main = git -C $repo rev-parse --path-format=absolute --git-common-dir 2>$null
    $known = @(
        (Join-Path $repo '../tools/nsis/makensis.exe'),
        $(if ($main) { Join-Path $main '../../tools/nsis/makensis.exe' }),
        (Join-Path ${env:ProgramFiles(x86)} 'NSIS/makensis.exe'),
        (Join-Path $env:ProgramFiles 'NSIS/makensis.exe')
    )
    foreach ($path in ($known | Where-Object { $_ })) { if (Test-Path -LiteralPath $path) { return (Resolve-Path $path).Path } }
    throw 'makensis not found: winget install NSIS.NSIS, or pass -Makensis <path>'
}
$nsis = if ('setup' -in $Package) { Find-Makensis }

$metadata = cargo metadata --format-version 1 --no-deps --manifest-path (Join-Path $repo 'Cargo.toml') | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$version = ($metadata.packages | Where-Object name -EQ 'ferry-daemon').version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "version $version does not fit VIProductVersion" }
$release = Join-Path $metadata.target_directory "$triple/release"

if (-not $SkipBuild) {
    Push-Location $repo
    try {
        cargo build --release --locked --target $triple -p ferry-daemon -p ferry-ui --bins
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
}
foreach ($exe in 'ferry.exe', 'ferry-ui.exe') {
    $text = [Text.Encoding]::Latin1.GetString([IO.File]::ReadAllBytes((Join-Path $release $exe)))
    # Inspector (D-19) chỉ có ở bản debug: bản cài mà còn chuỗi của nó thì build sai profile.
    if ('inspector-pick', 'Hover to highlight' | Where-Object { $text.Contains($_) }) { throw "$exe còn inspector (bản debug?)" }
}

function Write-Sha([string] $File) {
    $sha = (Get-FileHash -Algorithm SHA256 -LiteralPath $File).Hash.ToLowerInvariant()
    Set-Content -Encoding ascii -LiteralPath "$File.sha256" -Value "$sha  $(Split-Path -Leaf $File)"
    Write-Host ("{0}: {1:N1} MB, sha256 {2}" -f $File, ((Get-Item -LiteralPath $File).Length / 1MB), $sha)
}

if (Test-Path -LiteralPath $stage) { Remove-Item -Recurse -Force -LiteralPath $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
try {
    Copy-Item -LiteralPath (Join-Path $release 'ferry.exe'), (Join-Path $release 'ferry-ui.exe') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repo 'packaging/windows/ferry.ico') -Destination $stage
    node (Join-Path $repo 'packaging/licenses/gen-licenses.mjs') (Join-Path $stage 'licenses')
    if ($LASTEXITCODE -ne 0) { throw "gen-licenses failed (exit $LASTEXITCODE)" }

    # Tên tệp: {app}_{version}_{os}_{arch}_{setup.exe | portable.zip}
    $base = Join-Path $OutDir "ferry_${version}_win_x64"
    if ('setup' -in $Package) {
        $setup = "${base}_setup.exe"
        & $nsis /V2 /WX /INPUTCHARSET UTF8 `
            "/DVERSION=$version" "/DVERSION4=$version.0" "/DSTAGE=$stage" `
            "/DICON=$(Join-Path $repo 'packaging/windows/ferry.ico')" "/DOUTFILE=$setup" `
            (Join-Path $repo 'packaging/windows/ferry.nsi')
        if ($LASTEXITCODE -ne 0) { throw "makensis failed (exit $LASTEXITCODE)" }
        Write-Sha $setup
        # Tệp tự cập nhật (ferry-gd2_01): app chạy bản cài này với `/S /UPDATE`. Đăng khi chủ dự án bảo.
        $rel = [IO.Path]::GetRelativePath((Split-Path -Parent $repo), $setup).Replace([IO.Path]::DirectorySeparatorChar, '/')
        Write-Host "Đăng lên kênh cập nhật (đứng ở gốc xdesktop, Git Bash):"
        Write-Host "  bash cloud/deploy/publish-release.sh ferry $rel --version $version --os windows --arch x64 --plan free --notes ferry/release-notes/v$version.md"
    }
    if ('portable' -in $Package) {
        # Giải nén ra một thư mục `ferry`, không rải tệp ra chỗ giải nén.
        $zip = "${base}_portable.zip"
        $tree = Join-Path $OutDir 'ferry-portable'
        if (Test-Path -LiteralPath $tree) { Remove-Item -Recurse -Force -LiteralPath $tree }
        New-Item -ItemType Directory -Force $tree | Out-Null
        Copy-Item -Recurse -LiteralPath $stage -Destination (Join-Path $tree 'ferry')
        Remove-Item -LiteralPath $zip -ErrorAction Ignore
        Compress-Archive -Path (Join-Path $tree 'ferry') -DestinationPath $zip -CompressionLevel Optimal
        Remove-Item -Recurse -Force -LiteralPath $tree
        Write-Sha $zip
    }
} finally {
    Remove-Item -Recurse -Force -LiteralPath $stage -ErrorAction SilentlyContinue
}
