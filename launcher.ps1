# DeepDark_Oracle 启动器
# 中文界面，纯 PowerShell 实现（不用 bat 承载中文，避免 cmd 代码页问题）
# 该文件必须保存为 UTF-8 with BOM，否则 Windows PowerShell 5.1 会按 GBK 解析出错。

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch { }

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$Exe  = Join-Path $Root 'DeepDark_Oracle.exe'

# 数据目录：便携版在 exe 同级 data\；安装版装到受保护目录时，程序会回退到
# %LOCALAPPDATA%\DeepDark_Oracle\data。这里按同样的顺序探测，保证「打开数据目录」
# 和「索引库统计」指向真正在用的那一份。
function Resolve-DataDir {
    param([string]$AppRoot)
    $candidates = @(
        (Join-Path $AppRoot 'data'),
        (Join-Path $env:LOCALAPPDATA 'DeepDark_Oracle\data'),
        (Join-Path $env:APPDATA 'DeepDark_Oracle\data')
    )
    foreach ($c in $candidates) {
        if (Test-Path (Join-Path $c 'index.db')) { return $c }
        if (Test-Path (Join-Path $c 'config.json')) { return $c }
    }
    return $candidates[0]
}

$Data = Resolve-DataDir $Root

$Version = 'dev'
try {
    $info = (Get-Item $Exe).VersionInfo
    if ($info.ProductVersion) {
        # exe 里是 0.0.22，界面显示 0.22；这里统一成后者
        $v = $info.ProductVersion.Trim()
        if ($v -match '^0\.(\d+\.\d+)$') { $Version = $Matches[1] } else { $Version = $v }
    }
} catch { }

function Write-Head {
    Clear-Host
    Write-Host ''
    # 框宽按「显示宽度」算：中文/全角算 2 列，ASCII 算 1 列，避免换名后框线错位。
    $inner = 46
    function Get-DisplayWidth([string]$s) {
        $w = 0
        foreach ($ch in $s.ToCharArray()) {
            if ([int]$ch -gt 0x2E7F) { $w += 2 } else { $w += 1 }
        }
        return $w
    }
    function Write-BoxLine([string]$text, [string]$color) {
        $pad = [Math]::Max(0, $inner - 3 - (Get-DisplayWidth $text))
        Write-Host ('  │   ' + $text + (' ' * $pad) + '│') -ForegroundColor $color
    }
    Write-Host ('  ┌' + ('─' * $inner) + '┐') -ForegroundColor DarkCyan
    Write-BoxLine 'DeepDark_Oracle · AI 磁盘智搜' 'Cyan'
    Write-BoxLine "启动器  v$Version" 'DarkCyan'
    Write-Host ('  └' + ('─' * $inner) + '┘') -ForegroundColor DarkCyan
    Write-Host ''
}

function Pause-Key {
    # 输入被重定向（管道/脚本自动化）时 ReadKey 会抛异常或卡住，直接跳过
    if ([Console]::IsInputRedirected) { return }
    Write-Host ''
    Write-Host '  按任意键返回菜单…' -ForegroundColor DarkGray
    try { [void]$Host.UI.RawUI.ReadKey('NoEcho,IncludeKeyDown') } catch { [void](Read-Host) }
}

function Invoke-Exe {
    param([string[]]$Arguments)

    if (-not (Test-Path $Exe)) {
        Write-Host "  找不到 DeepDark_Oracle.exe，请确认启动器与程序在同一目录。" -ForegroundColor Red
        return
    }
    # 用 cmd 起一个有控制台的进程，让 windowed 程序的 --cli 输出可以附着上来
    $argLine = ($Arguments | ForEach-Object {
        if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
    }) -join ' '
    $cmd = '"' + $Exe + '" ' + $argLine
    & cmd.exe /c $cmd
}

function Show-ModelState {
    $log = Join-Path $Data 'autoload.log'
    if (Test-Path $log) {
        $line = Get-Content -LiteralPath $log -Tail 3 -Encoding UTF8 -ErrorAction SilentlyContinue
        if ($line) {
            Write-Host '  最近一次模型加载：' -ForegroundColor DarkGray
            $line | ForEach-Object { Write-Host ("    " + $_) -ForegroundColor DarkGray }
            Write-Host ''
        }
    }
}

while ($true) {
    Write-Head

    $dbFile = Join-Path $Data 'index.db'
    if (Test-Path $dbFile) {
        $mb  = [math]::Round((Get-Item $dbFile).Length / 1MB, 1)
        $th  = Join-Path $Data 'thumbs'
        $nth = 0
        if (Test-Path $th) { $nth = (Get-ChildItem $th -Filter *.jpg -ErrorAction SilentlyContinue | Measure-Object).Count }
        Write-Host ("  索引库 {0} MB · 缩略图 {1} 张" -f $mb, $nth) -ForegroundColor DarkGray
    } else {
        Write-Host '  还没有索引库，首次使用请先「正常启动」→ 扫描/索引页' -ForegroundColor DarkYellow
    }
    Write-Host ''

    Write-Host '   [1]  正常启动' -ForegroundColor White
    Write-Host '        打开主界面，自动加载模型' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '   [2]  自检' -ForegroundColor White
    Write-Host '        跑一遍端到端检查，结果写入 selftest.txt' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '   [3]  重建索引' -ForegroundColor White
    Write-Host '        全量重新抽取正文 + 重算向量 + 重打画面标签' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '   [4]  扫描目录' -ForegroundColor White
    Write-Host '        输入一个目录，扫描并入库（命令行方式，带进度）' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '   [5]  命令行' -ForegroundColor White
    Write-Host '        查看 --cli 的全部子命令（search / sem / similar / stats …）' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '   [6]  打开数据目录' -ForegroundColor White
    Write-Host '        索引库、向量库、缩略图、日志都在这里' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '   [0]  退出' -ForegroundColor DarkGray
    Write-Host ''

    $choice = Read-Host '  请选择'

    switch ($choice.Trim()) {
        '1' {
            if (-not (Test-Path $Exe)) {
                Write-Host '  找不到 DeepDark_Oracle.exe' -ForegroundColor Red
                Pause-Key; continue
            }
            Write-Host ''
            Write-Host '  正在启动…' -ForegroundColor Green
            Show-ModelState
            Start-Process -FilePath $Exe -WorkingDirectory $Root
            Start-Sleep -Milliseconds 900
            Write-Host '  已启动。本窗口 3 秒后自动关闭。' -ForegroundColor DarkGray
            Start-Sleep -Seconds 3
            return
        }
        '2' {
            Write-Host ''
            Write-Host '  正在自检（约 20 秒，期间不要关窗口）…' -ForegroundColor Yellow
            Write-Host ''
            # 自检输出会同时写进 selftest.txt，这里屏蔽程序和屏幕上的重复输出，
            # 只按颜色回读文件，读起来更清楚。
            Invoke-Exe @('--selftest') | Out-Null
            $st = Join-Path $Root 'selftest.txt'
            if (Test-Path $st) {
                Write-Host ''
                Write-Host '  ── 自检结果 ──' -ForegroundColor DarkCyan
                Get-Content -LiteralPath $st -Encoding UTF8 | ForEach-Object {
                    if ($_ -match '\[FAIL\]|失败 [1-9]') { Write-Host $_ -ForegroundColor Red }
                    elseif ($_ -match '\[OK\]')      { Write-Host $_ -ForegroundColor Green }
                    elseif ($_ -match '\[SKIP\]')    { Write-Host $_ -ForegroundColor DarkYellow }
                    else                              { Write-Host $_ }
                }
            }
            Pause-Key
        }
        '3' {
            Write-Host ''
            Write-Host '  全量重建索引：正文抽取 + 向量重算 + 画面标签。' -ForegroundColor Yellow
            Write-Host '  大目录可能要几分钟，期间请勿关闭窗口。' -ForegroundColor DarkYellow
            $ok = Read-Host '  确认开始？(y/N)'
            if ($ok -match '^[yY]') {
                Write-Host ''
                Invoke-Exe @('--cli', 'index', '--what', 'auto', '--force', '--wait')
                Write-Host ''
                Write-Host '  完成。' -ForegroundColor Green
            } else {
                Write-Host '  已取消。' -ForegroundColor DarkGray
            }
            Pause-Key
        }
        '4' {
            $dir = Read-Host '  要扫描的目录（例如 D:\Photos 或 E:\）'
            if ([string]::IsNullOrWhiteSpace($dir)) { Pause-Key; continue }
            $dir = $dir.Trim().Trim('"')
            if (-not (Test-Path $dir)) {
                Write-Host "  目录不存在：$dir" -ForegroundColor Red
                Pause-Key; continue
            }
            Write-Host ''
            Write-Host "  正在扫描 $dir …" -ForegroundColor Yellow
            Invoke-Exe @('--cli', 'scan', $dir)
            Pause-Key
        }
        '5' {
            Write-Host ''
            Invoke-Exe @('--cli', 'help')
            Pause-Key
        }
        '6' {
            if (Test-Path $Data) { Start-Process explorer.exe $Data }
            else { Start-Process explorer.exe $Root }
            Start-Sleep -Milliseconds 600
        }
        '0' { return }
        default {
            Write-Host '  没有这个选项。' -ForegroundColor DarkYellow
            Start-Sleep -Milliseconds 700
        }
    }
}
