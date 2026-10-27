param(
    [string]$Executable = 'dist/WifiRadio.exe',
    [string]$LegacyExecutable
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $projectRoot
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class RadioLifecycle {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr window, int command);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
}
'@
function Get-RadioWindow {
    [RadioLifecycle]::FindWindow('WifiRadioWindow', [System.Management.Automation.Language.NullString]::Value)
}
function Wait-Condition([scriptblock]$Condition, [string]$Label) {
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 40
    }
    throw "Timed out: $Label"
}
function Get-ProjectPath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath((Join-Path $projectRoot $Path))
    if (-not $absolute.StartsWith($projectRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Executable must be inside this project.' }
    if (-not (Test-Path -LiteralPath $absolute -PathType Leaf)) { throw "File missing: $absolute" }
    $absolute
}
function Stop-ProjectRadio {
    $window = Get-RadioWindow
    if ($window -eq [IntPtr]::Zero) { return }
    [uint32]$radioProcessId = 0
    [void][RadioLifecycle]::GetWindowThreadProcessId($window, [ref]$radioProcessId)
    $process = Get-Process -Id $radioProcessId
    if (-not $process.Path.StartsWith($projectRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'A radio outside this project is running; leaving it untouched.' }
    [void][RadioLifecycle]::PostMessage($window, 0x1f, [IntPtr]::Zero, [IntPtr]::Zero)
    [void][RadioLifecycle]::PostMessage($window, 0x8004, [IntPtr]::Zero, [IntPtr]::Zero)
    if (-not $process.WaitForExit(4000)) { throw 'Previous project radio did not exit.' }
}
function Start-Radio([string]$Path, [switch]$Legacy) {
    $process = Start-Process -FilePath $Path -ArgumentList '--no-autostart' -PassThru -WindowStyle Hidden
    Wait-Condition { (Get-RadioWindow) -ne [IntPtr]::Zero } 'window creation'
    if ($Legacy) {
        [void][RadioLifecycle]::ShowWindow((Get-RadioWindow), 9)
        [void][RadioLifecycle]::ShowWindow((Get-RadioWindow), 9)
    }
    Wait-Condition { [RadioLifecycle]::IsWindowVisible((Get-RadioWindow)) } 'visible window'
    $process
}
$release = Get-ProjectPath $Executable
$report = [Collections.Generic.List[string]]::new()
Stop-ProjectRadio
if ($LegacyExecutable) {
    $legacy = Start-Radio (Get-ProjectPath $LegacyExecutable) -Legacy
    $legacyWindow = Get-RadioWindow
    [void][RadioLifecycle]::PostMessage($legacyWindow, 0x111, [IntPtr]101, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 250
    [void][RadioLifecycle]::PostMessage($legacyWindow, 0x8004, [IntPtr]::Zero, [IntPtr]::Zero)
    Wait-Condition { (Get-RadioWindow) -eq [IntPtr]::Zero } 'legacy window destruction inside popup'
    if ($legacy.HasExited) { throw 'Legacy did not reproduce the windowless-process bug.' }
    $report.Add('PASS: reproduced legacy windowless process by closing during a native popup.')
    $recovered = Start-Radio $release
    if (-not $legacy.WaitForExit(5000)) { throw 'New release did not recover the windowless legacy process.' }
    $endpoint = Get-NetUDPEndpoint -LocalPort 49735 -ErrorAction Stop
    if ($endpoint.OwningProcess -notcontains $recovered.Id) { throw 'Recovered release does not own its UDP port.' }
    $report.Add('PASS: new release recovered the orphaned UDP owner and opened its window.')
    Stop-ProjectRadio
}
for ($cycle = 1; $cycle -le 3; $cycle++) {
    $process = Start-Radio $release
    $window = Get-RadioWindow
    [void][RadioLifecycle]::ShowWindow($window, 6)
    Wait-Condition { [RadioLifecycle]::IsIconic($window) } 'normal minimize'
    if (-not [RadioLifecycle]::IsWindowVisible($window)) { throw 'Minimize hid the taskbar window.' }
    $duplicate = Start-Process -FilePath $release -ArgumentList '--no-autostart', '--background' -PassThru -WindowStyle Hidden
    if (-not $duplicate.WaitForExit(5000)) { throw 'Duplicate did not activate the existing instance and exit.' }
    Wait-Condition { -not [RadioLifecycle]::IsIconic($window) } 'restore from taskbar'
    if ((Get-RadioWindow) -ne $window) { throw 'Duplicate replaced a healthy window.' }
    [void][RadioLifecycle]::PostMessage($window, 0x111, [IntPtr]205, [IntPtr]::Zero)
    Wait-Condition { -not [RadioLifecycle]::IsWindowVisible($window) } 'explicit tray hide'
    $duplicate = Start-Process -FilePath $release -ArgumentList '--no-autostart' -PassThru -WindowStyle Hidden
    if (-not $duplicate.WaitForExit(5000)) { throw 'Duplicate did not reopen the hidden instance.' }
    Wait-Condition { [RadioLifecycle]::IsWindowVisible($window) } 'restore from tray'
    [void][RadioLifecycle]::PostMessage($window, 0x111, [IntPtr]101, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 150
    [void][RadioLifecycle]::PostMessage($window, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
    if (-not $process.WaitForExit(4000)) { throw 'X during a popup left a windowless process.' }
    if ((Get-RadioWindow) -ne [IntPtr]::Zero) { throw 'Window survived process exit.' }
    $report.Add("PASS cycle ${cycle}: visible launch, taskbar minimize, duplicate restore, explicit tray hide, reopen, and full X exit during a popup.")
}
New-Item -ItemType Directory -Path .local -Force | Out-Null
$report | Set-Content -Encoding UTF8 .local/lifecycle-check.txt
$report
