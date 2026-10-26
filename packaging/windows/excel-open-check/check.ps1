<#
Excel open check: saves workbooks with GridCraft, opens each result in real Excel (COM) and records per
file whether Excel loads it as it is or has to repair it (docs/file-format-parity.md, "Excel open check").

  check.ps1 -Cli C:\gridcraft\gridcraft-cli.exe [-Inputs C:\workbooks] [-Out C:\out] [-Csv result.csv]
            [-TimeoutSec 120]

Files saved:
  - every .xlsx / .xlsm in -Inputs, opened and saved with `gridcraft-cli run --in --out` (the original
    is opened in Excel too, so a file Excel itself dislikes or loads slowly is told apart from a
    GridCraft regression);
  - every *.jsonl scenario next to this script (one feature each), run on the Sales sample.

Each file is opened read-only with CorruptLoad = xlNormalLoad (0), so a part Excel would repair is a
failure instead of a silent fix, with DisplayAlerts off and macros disabled so nothing waits on a prompt.
Excel is a fresh instance per file; a file that hangs or kills Excel is recorded as TIMEOUT or CRASH and
Excel started by this script is stopped. Nothing is written next to the inputs; Excel processes that were
already running are never touched. Exit code 1 when any saved file is not PASS.
#>
param(
    [Parameter(Mandatory = $true)][string]$Cli,
    [string]$Inputs,
    [string]$Out = (Join-Path ([IO.Path]::GetTempPath()) 'gridcraft-excel-open-check'),
    [string]$Csv,
    [int]$TimeoutSec = 120
)
$ErrorActionPreference = 'Stop'

# Excel processes already running are the person's own work: only ones started after this point are ever killed.
$before = @(Get-Process EXCEL -ErrorAction SilentlyContinue | ForEach-Object Id)
function Stop-Excel {
    Get-Process EXCEL -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | Stop-Process -Force -ErrorAction SilentlyContinue
}

# Runs in a child PowerShell so a hang can be killed after $TimeoutSec without taking this script with it.
$probe = {
    param($path)
    $x = $null
    try {
        $x = New-Object -ComObject Excel.Application
        $x.Visible = $false
        $x.DisplayAlerts = $false
        $x.AutomationSecurity = 3 # msoAutomationSecurityForceDisable: never run macros
        # Named arguments: PowerShell can't skip the optional ones in between positionally.
        $wb = [System.__ComObject].InvokeMember('Open', [Reflection.BindingFlags]::InvokeMethod, $null, $x.Workbooks,
            @($path, 0, $true, 0), $null, $null, @('Filename', 'UpdateLinks', 'ReadOnly', 'CorruptLoad'))
        $sheets = $wb.Worksheets.Count + $wb.Chartsheets.Count
        $wb.Close($false)
        "PASS`t$sheets sheets"
    } catch {
        "FAIL`t" + ($_.Exception.Message -replace '\s+', ' ')
    } finally {
        if ($x) { try { $x.Quit() } catch {} }
    }
}

function Test-InExcel([string]$path) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $job = Start-Job -ScriptBlock $probe -ArgumentList $path
    if (Wait-Job $job -Timeout $TimeoutSec) {
        $out = (Receive-Job $job) -join ' '
        if (-not $out) { $out = "CRASH`tno result (Excel died)" }
    } else {
        Stop-Job $job
        $out = "TIMEOUT`tno answer in $TimeoutSec s"
    }
    Remove-Job $job -Force
    # Next file only once this Excel has exited, else its first call can get RPC_E_SERVERCALL_RETRYLATER.
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline -and @(Get-Process EXCEL -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id }).Count) { Start-Sleep -Milliseconds 200 }
    Stop-Excel
    $state, $detail = $out -split "`t", 2
    [pscustomobject]@{ State = $state; Seconds = [math]::Round($clock.Elapsed.TotalSeconds, 1); Detail = $detail }
}

$saved = Join-Path $Out 'saved'
Remove-Item $saved -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $saved -Force | Out-Null

# (name, original or $null, saved) per file
$work = @()
if ($Inputs) {
    foreach ($f in Get-ChildItem -Path $Inputs -File | Where-Object { $_.Extension -in '.xlsx', '.xlsm' } | Sort-Object Name) {
        $work += , @($f.Name, $f.FullName, (Join-Path $saved $f.Name), @('run', '--in', $f.FullName, '--quiet'))
    }
}
foreach ($s in Get-ChildItem -Path $PSScriptRoot -Filter *.jsonl | Sort-Object Name) {
    $work += , @("scenario:$($s.BaseName)", $null, (Join-Path $saved "$($s.BaseName).xlsx"), @('run', '--sample', 'sales', '--script', $s.FullName, '--quiet'))
}

$rows = @()
foreach ($w in $work) {
    $name, $orig, $dest, $cliArgs = $w
    # The CLI reports on stderr, which Stop would turn into an exception.
    $ErrorActionPreference = 'Continue'
    & $Cli @cliArgs --out $dest *> $null
    $ErrorActionPreference = 'Stop'
    $base = if ($orig) { (Test-InExcel $orig) } else { $null }
    $r = if (Test-Path $dest) { Test-InExcel $dest } else { [pscustomobject]@{ State = 'SAVE-FAIL'; Seconds = 0; Detail = "gridcraft-cli exit code $LASTEXITCODE" } }
    $rows += [pscustomobject]@{
        File = $name; Original = if ($base) { $base.State } else { '-' }; Saved = $r.State
        Seconds = $r.Seconds; Detail = $r.Detail
    }
}
$rows | Format-Table -AutoSize -Wrap | Out-String -Width 220 | Write-Output
if ($Csv) { $rows | Export-Csv -NoTypeInformation -Path $Csv }
$bad = @($rows | Where-Object Saved -ne 'PASS').Count
"{0} of {1} saved files pass" -f ($rows.Count - $bad), $rows.Count
if ($bad) { exit 1 }
