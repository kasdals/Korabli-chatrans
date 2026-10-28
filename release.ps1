<#
.SYNOPSIS
    一键发布 chatrans：刷新 addons 时间戳 → 测试 → 编译 → 提交 → 打 tag → 推送。

.DESCRIPTION
    推上去之后 GitHub Actions 会自动构建 Windows 包并挂到 Release 上，
    这个脚本只负责把一切准备到位并触发。

    为什么需要它：

    - addons/ 是编译期嵌入的，cargo 只看 mtime。游戏数据从客户端复制过来常常
      保留原始时间戳，不刷新就会编出装着旧数据的 exe（详见 build.ps1）。
    - 推送顺序有讲究：tag 必须指向一个已经在远程的提交，否则 tag push 不会
      触发工作流。所以这里先推 main、确认远程收到，再推 tag。

.PARAMETER Version
    目标版本号，跟游戏版本走，如 26.11 或 26.11.1。
    省略时沿用 chatrans/Cargo.toml 里现有的版本。

.PARAMETER SkipTest
    跳过测试。不推荐，除非只改了文档。

.PARAMETER DryRun
    只打印将要做什么，不写文件、不提交、不推送。

.EXAMPLE
    .\release.ps1 -Version 26.11
        发 v26.11：Cargo.toml 写成 26.11.0，tag 用 v26.11（和 v26.10 的命名保持一致）

.EXAMPLE
    .\release.ps1 -Version 26.11.1 -DryRun
        预演一次补丁版发布
#>

param(
    [string]$Version,
    [switch]$SkipTest,
    [switch]$DryRun
)

Set-Location $PSScriptRoot
$ErrorActionPreference = "Continue"

function Fail($msg) {
    Write-Host ""
    Write-Host "错误: $msg" -ForegroundColor Red
    exit 1
}

function Step($msg) {
    Write-Host "`r`n==> $msg" -ForegroundColor Cyan
}

# ── 版本号 ────────────────────────────────────────────────────────────
$cargoToml = Join-Path $PSScriptRoot "chatrans\Cargo.toml"
$content = Get-Content $cargoToml -Raw

if (-not $Version) {
    $inPkg = $false
    foreach ($line in ($content -split "`n")) {
        if ($line -match '^\[') { $inPkg = $line -match '^\[package\]' }
        elseif ($inPkg -and $line -match '^version\s*=\s*"([^"]+)"') { $Version = $matches[1]; break }
    }
    if (-not $Version) { Fail "无法从 Cargo.toml 读出当前版本" }
    Write-Host "未指定 -Version，沿用 Cargo.toml 的 $Version" -ForegroundColor DarkGray
}

$tagName = $Version                                   # v26.11 / v26.11.1，保留你输入的形式
if ($Version -match '^\d+\.\d+$') {
    $cargoVer = "$Version.0"                          # Cargo 需要三段
}
elseif ($Version -match '^\d+\.\d+\.\d+$') {
    $cargoVer = $Version
}
else {
    Fail "版本号格式不对：$Version（应为 26.11 或 26.11.1）"
}

$tag = "v$tagName"
Step "目标版本 $cargoVer（tag $tag）"

# ── 幂等检查：tag 是否已存在 ─────────────────────────────────────────
$localTag = (git tag -l $tag) -join ''
$remoteTag = (git ls-remote --tags origin "refs/tags/$tag")
if ($localTag -or $remoteTag) {
    Fail "tag $tag 已存在。要重发先删掉：`n  git tag -d $tag`n  git push origin :refs/tags/$tag"
}

# ── 同步 Cargo.toml 版本 ─────────────────────────────────────────────
$reVersion = [regex]'(?m)^version = "[^"]+"'
$currentCargoVer = $null
if ($content -match '(?m)^version = "([^"]+)"') { $currentCargoVer = $matches[1] }

if ($currentCargoVer -ne $cargoVer) {
    Step "Cargo.toml 版本 $currentCargoVer -> $cargoVer"
    if (-not $DryRun) {
        $newContent = $reVersion.Replace($content, "version = `"$cargoVer`"", 1)
        [System.IO.File]::WriteAllText($cargoToml, $newContent, (New-Object System.Text.UTF8Encoding($false)))
    }
}

# ── 刷新 addons 时间戳 ───────────────────────────────────────────────
Step "刷新 addons 时间戳"
$files = @(Get-ChildItem -Path addons -Recurse -File)
$stamp = Get-Date
$skipped = 0
foreach ($f in $files) {
    try {
        if (-not $DryRun) { $f.LastWriteTime = $stamp }
    }
    catch {
        $skipped++
    }
}
Write-Host "    $($files.Count - $skipped) 个文件$(if ($skipped -gt 0) { "，跳过 $skipped 个无法修改的" })"

# ── 测试 ─────────────────────────────────────────────────────────────
if (-not $SkipTest) {
    Step "运行测试"
    if (-not $DryRun) {
        cargo test --release
        if ($LASTEXITCODE -ne 0) { Fail "默认特性测试未通过" }
        cargo test --release --features gui
        if ($LASTEXITCODE -ne 0) { Fail "gui 特性测试未通过" }
    }
    else {
        Write-Host "    (DryRun 跳过)"
    }
}

# ── 编译 ─────────────────────────────────────────────────────────────
Step "编译 release"
if (-not $DryRun) {
    cargo build --release -p chatrans --features gui
    if ($LASTEXITCODE -ne 0) { Fail "编译失败" }
}
else {
    Write-Host "    (DryRun 跳过)"
}

# ── 提交 ─────────────────────────────────────────────────────────────
Step "提交改动"
if (-not $DryRun) {
    git add -A
    $pending = git status --porcelain
    if ($pending) {
        Write-Host ($pending -join "`n") -ForegroundColor DarkGray
        git commit -m "release: $tag" -m "sync game data and bump version to $cargoVer"
        if ($LASTEXITCODE -ne 0) { Fail "提交失败" }
    }
    else {
        Write-Host "    没有需要提交的改动" -ForegroundColor DarkGray
    }
}
else {
    $pending = git status --porcelain
    if ($pending) { Write-Host ($pending -join "`n") -ForegroundColor DarkGray }
}

# ── 推送 main ────────────────────────────────────────────────────────
Step "推送 main"
if (-not $DryRun) {
    git push origin main
    if ($LASTEXITCODE -ne 0) { Fail "推送 main 失败" }

    $head = (git rev-parse HEAD).Trim()
    $confirmed = $false
    for ($i = 0; $i -lt 10; $i++) {
        $remoteHead = ((git ls-remote origin refs/heads/main) -split "`t")[0]
        if ($remoteHead -eq $head) { $confirmed = $true; break }
        Start-Sleep -Seconds 1
    }
    if (-not $confirmed) { Fail "远程 main 还没更新到 $head，先别打 tag" }
    Write-Host "    远程 main 已确认: $head" -ForegroundColor DarkGray
}

# ── 打 tag 并推送 ────────────────────────────────────────────────────
Step "打 tag $tag 并推送"
if (-not $DryRun) {
    git tag $tag
    git push origin $tag
    if ($LASTEXITCODE -ne 0) { Fail "推送 tag 失败" }
}

# ── 收尾 ─────────────────────────────────────────────────────────────
if ($DryRun) {
    Write-Host "`nDryRun 结束，没有做任何改动。" -ForegroundColor Yellow
    Write-Host "去掉 -DryRun 再跑一次就会真的发布。" -ForegroundColor Yellow
}
else {
    Write-Host "`n已触发发布。" -ForegroundColor Green
    Write-Host "  构建进度: https://github.com/kasdals/Korabli-chatrans/actions"
    Write-Host "  成品页面: https://github.com/kasdals/Korabli-chatrans/releases/tag/$tag"
    Write-Host "  通常 3 分钟左右出包。"
}
