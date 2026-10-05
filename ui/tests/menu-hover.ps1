# Requires an unlocked Windows desktop and a Debug build.
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class MenuHover {
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
}
'@
$target = "$PSScriptRoot/../target"
$p = Start-Process (Resolve-Path "$target/debug/everything-ui.exe").Path -RedirectStandardOutput "$target/hover-app.out" -RedirectStandardError "$target/hover-app.err" -PassThru
$null = $p.Handle
try {
    Start-Sleep -Seconds 2
    $p.Refresh()
    if ($p.HasExited -or $p.MainWindowHandle -eq 0) { throw 'Window did not open' }
    $shell = New-Object -ComObject WScript.Shell
    $shell.AppActivate($p.Id) | Out-Null
    if (-not [MenuHover]::SetForegroundWindow($p.MainWindowHandle)) {
        throw 'Cannot activate test window; unlock the Windows desktop before testing'
    }
    $scale = [MenuHover]::GetDpiForWindow($p.MainWindowHandle) / 96
    # Move away, then file -> search -> file. Never send a mouse click.
    foreach ($x in @(500, 30, 140, 30)) {
        $y = if ($x -eq 500) { 600 } else { 14 }
        $position = ([int]($y * $scale) -shl 16) -bor [int]($x * $scale)
        [MenuHover]::PostMessage($p.MainWindowHandle, 0x0200, [IntPtr]::Zero, [IntPtr]$position) | Out-Null
        Start-Sleep -Milliseconds 300
    }
    [MenuHover]::PostMessage($p.MainWindowHandle, 0x0100, [IntPtr]0x1B, [IntPtr]::Zero) | Out-Null
    [MenuHover]::PostMessage($p.MainWindowHandle, 0x0101, [IntPtr]0x1B, [IntPtr]::Zero) | Out-Null
    Start-Sleep -Milliseconds 300
    $events = @(Get-Content "$target/hover-app.err" | Where-Object { $_ -like 'menu:*' })
    $expected = @('menu:file-menu', 'menu:search-menu', 'menu:file-menu', 'menu:closed')
    if (($events -join ',') -ne ($expected -join ',')) {
        throw "Unexpected menu transitions: $($events -join ',')"
    }
    if ($p.HasExited) { throw 'Application exited during hover check' }
    Write-Output 'Hover open, adjacent-menu switching and Escape close passed (no clicks).'
} finally {
    if (-not $p.HasExited) { $p.CloseMainWindow() | Out-Null; $p.WaitForExit(10000) | Out-Null }
}
