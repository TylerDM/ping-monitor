<#
.SYNOPSIS
    Builds Ping Monitor and installs it for the current user (no admin needed).

.PARAMETER Startup
    Also start Ping Monitor when you sign in.

.PARAMETER Uninstall
    Remove the app and its shortcuts. Settings in %APPDATA%\PingMonitor are kept.
#>
param(
    [switch]$Startup,
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'

$installDir = Join-Path $env:LOCALAPPDATA 'Programs\PingMonitor'
$exe = Join-Path $installDir 'ping-monitor.exe'
$startMenuLink = Join-Path ([Environment]::GetFolderPath('Programs')) 'Ping Monitor.lnk'
$startupLink = Join-Path ([Environment]::GetFolderPath('Startup')) 'Ping Monitor.lnk'

function New-Shortcut($path, $target) {
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut($path)
    $link.TargetPath = $target
    $link.WorkingDirectory = Split-Path $target
    $link.Description = 'Shows your current ping in the notification area'
    $link.Save()
}

# The exe can't be replaced or removed while it's running.
Get-Process ping-monitor -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 300

if ($Uninstall) {
    Remove-Item $startMenuLink, $startupLink -ErrorAction SilentlyContinue
    Remove-Item $installDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host 'Ping Monitor uninstalled.'
    return
}

Push-Location $PSScriptRoot
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally {
    Pop-Location
}

New-Item -ItemType Directory -Force $installDir | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'target\release\ping-monitor.exe') $exe -Force
New-Shortcut $startMenuLink $exe
if ($Startup) {
    New-Shortcut $startupLink $exe
} else {
    Remove-Item $startupLink -ErrorAction SilentlyContinue
}

Start-Process $exe
Write-Host "Installed to $installDir and started."
