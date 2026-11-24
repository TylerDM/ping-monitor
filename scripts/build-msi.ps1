<#
.SYNOPSIS
    Builds a release exe and packages it as target\wix\PingMonitor.msi.

.DESCRIPTION
    The MSI's version comes from Cargo.toml. WiX is a repo-local dotnet tool
    (see dotnet-tools.json), restored here on first use.
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot

Push-Location $root
try {
    $version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value

    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    dotnet tool restore | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'dotnet tool restore failed' }

    $out = Join-Path $root 'target\wix\PingMonitor.msi'
    New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null
    dotnet tool run wix build installer\PingMonitor.wxs `
        -arch x64 `
        -d "Version=$version" `
        -d "ExePath=$(Join-Path $root 'target\release\ping-monitor.exe')" `
        -o $out
    if ($LASTEXITCODE -ne 0) { throw 'wix build failed' }

    Write-Host "Built $out (version $version)"
} finally {
    Pop-Location
}
