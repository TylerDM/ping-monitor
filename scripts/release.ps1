<#
.SYNOPSIS
    Cuts a release: bumps the version, commits, tags and pushes.

.DESCRIPTION
    Pushing the tag starts .github/workflows/release.yml, which builds the MSI
    and publishes the GitHub release. Installed copies pick it up through
    their update check.

.EXAMPLE
    ./scripts/release.ps1 0.2.0
#>
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version
)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot)
try {
    if (git status --porcelain) { throw 'Commit or stash your changes first.' }
    if ((git branch --show-current) -ne 'main') { throw 'Releases are cut from main.' }
    if (git tag --list "v$Version") { throw "v$Version already exists." }

    $toml = [IO.File]::ReadAllText('Cargo.toml')
    $toml = [regex]::Replace($toml, '(?m)^version\s*=\s*"[^"]+"', "version = `"$Version`"", 1)
    [IO.File]::WriteAllText('Cargo.toml', $toml)
    cargo check --quiet # updates Cargo.lock to the new version
    if ($LASTEXITCODE -ne 0) { throw 'cargo check failed' }

    git commit --quiet -am "Release v$Version"
    git tag "v$Version"
    git push --quiet origin main "v$Version"
    Write-Host "Pushed v$Version. The release workflow is building it:"
    Write-Host '  https://github.com/TylerDM/ping-monitor/actions/workflows/release.yml'
} finally {
    Pop-Location
}
