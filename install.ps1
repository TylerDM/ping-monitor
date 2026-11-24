<#
.SYNOPSIS
    Builds the MSI from this checkout and installs it, exactly as a release
    would be installed (per user, no admin needed). Run it again to update.

.PARAMETER Uninstall
    Remove Ping Monitor, the same as Settings > Apps. Settings in
    %APPDATA%\PingMonitor are kept.
#>
param(
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'

function Get-InstalledProductCode {
    # Every version shares the UpgradeCode in installer/PingMonitor.wxs.
    $installer = New-Object -ComObject WindowsInstaller.Installer
    @($installer.RelatedProducts('{DBD4F0B6-0A38-4CBB-9FBA-31A7BDA0BE11}')) | Select-Object -First 1
}

function Invoke-Msiexec([string[]]$Arguments) {
    $process = Start-Process msiexec.exe -ArgumentList $Arguments -Wait -PassThru
    # 3010: success, restart required (never expected here).
    if ($process.ExitCode -notin 0, 3010) { throw "msiexec failed with exit code $($process.ExitCode)" }
}

# Leftovers from before there was an installer: a copied exe plus shortcuts.
# The MSI owns the Start Menu shortcut and the folder now; the old Startup
# shortcut is replaced by the installer's run-at-sign-in entry.
Remove-Item (Join-Path ([Environment]::GetFolderPath('Startup')) 'Ping Monitor.lnk') -ErrorAction SilentlyContinue

if ($Uninstall) {
    $code = Get-InstalledProductCode
    if (-not $code) {
        Write-Host 'Ping Monitor is not installed.'
        return
    }
    Invoke-Msiexec '/x', $code, '/passive'
    Write-Host 'Ping Monitor uninstalled.'
    return
}

& (Join-Path $PSScriptRoot 'scripts\build-msi.ps1')
# A running copy is closed by the installer and the new one started after.
Invoke-Msiexec '/i', "`"$(Join-Path $PSScriptRoot 'target\wix\PingMonitor.msi')`"", '/passive', '/norestart'
Write-Host 'Installed and started.'
