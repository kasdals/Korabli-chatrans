<#
.SYNOPSIS
    编译 chatrans 的 GUI 版（release）。

.DESCRIPTION
    为什么需要这个脚本：

    addons/ 下的 xml / def 由 rust-embed 在【编译期】嵌进二进制。cargo 判断
    "要不要重编" 只认文件 mtime，而 addons 里的数据通常是从游戏客户端复制或
    解压过来的 —— Windows 资源管理器复制、7-Zip 解压、robocopy 都会保留源文件
    的 LastWriteTime。结果就是：内容已经变了，时间戳还停在几周前，cargo 认为
    什么都没发生，直接跳过编译，exe 里跑的仍是旧数据。

    parser/build.rs 已经覆盖了「新增文件 / 删除文件 / 修改并更新时间戳」三种情况，
    唯独「内容变了但时间戳没变」cargo 看不见。这个脚本在编译前把 addons/ 下所有
    文件的时间戳统一刷成当前时间，把最后这个缺口补上。

.EXAMPLE
    .\build.ps1
        普通编译

.EXAMPLE
    .\build.ps1 -Test
        先跑两套测试（默认 + gui），再编译

.EXAMPLE
    .\build.ps1 -Clean
        先 cargo clean -p replay-parser 再编译，最彻底
#>

param(
    [switch]$Test,
    [switch]$Clean
)

# 刻意不设 $ErrorActionPreference = "Stop"：cargo 的编译进度是写 stderr 的，
# PowerShell 会把原生命令的 stderr 包成 ErrorRecord，Stop 模式下会被误判成
# 构建失败而中断。成功与否统一看 $LASTEXITCODE。
Set-Location $PSScriptRoot

$files = @(Get-ChildItem -Path addons -Recurse -File)
$stamp = Get-Date
$skipped = @()
foreach ($f in $files) {
    try {
        $f.LastWriteTime = $stamp
    }
    catch {
        # 个别文件（比如混进来的 .lnk 快捷方式）会拒绝修改，跳过但不中断构建
        $skipped += $f.Name
    }
}
Write-Host "刷新 addons 时间戳: $($files.Count - $skipped.Count) 个文件" -ForegroundColor Cyan
if ($skipped.Count -gt 0) {
    Write-Host "  跳过 $($skipped.Count) 个无法修改的文件: $($skipped -join ', ')" -ForegroundColor Yellow
}

if ($Clean) {
    Write-Host "清理 replay-parser 产物..." -ForegroundColor Cyan
    cargo clean -p replay-parser
}

if ($Test) {
    Write-Host "运行测试..." -ForegroundColor Cyan
    cargo test --release
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo test --release --features gui
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Write-Host "编译 chatrans (release, gui)..." -ForegroundColor Cyan
cargo build --release -p chatrans --features gui
$code = $LASTEXITCODE
if ($code -eq 0) {
    Write-Host "构建成功" -ForegroundColor Green
}
else {
    Write-Host "构建失败 (exit code $code)" -ForegroundColor Red
}
exit $code
