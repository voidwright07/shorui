# Builds the Windows installer: dist\Shorui-<version>-x64.msi
#
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1
#
# Needs the WiX Toolset 5 command line, once per machine (no administrator rights):
#   dotnet tool install --global wix --version 5.0.2
#   wix extension add -g WixToolset.UI.wixext/5.0.2
#   wix extension add -g WixToolset.Util.wixext/5.0.2

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$root = Split-Path -Parent $here
Push-Location $root
try {
    $wix = Get-Command wix -ErrorAction SilentlyContinue
    if (-not $wix) {
        $local = Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe'
        if (Test-Path $local) { $wix = $local } else { throw 'The WiX command line is missing. See the top of this script for how to install it.' }
    } else {
        $wix = $wix.Source
    }

    Write-Host 'Building the release binaries...'
    cargo build --release -p shorui
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed (is Shorui still running? Close it and try again).' }
    cargo build --release -p shorui-core --bin shorui-cli
    if ($LASTEXITCODE -ne 0) { throw 'cargo build of shorui-cli failed.' }

    $version = ((cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages | Where-Object { $_.name -eq 'shorui' }).version
    $out = Join-Path $root "dist\Shorui-$version-x64.msi"
    New-Item -ItemType Directory -Force (Join-Path $root 'dist') | Out-Null

    Write-Host "Packaging Shorui $version..."
    & $wix build (Join-Path $here 'shorui.wxs') `
        -arch x64 `
        -pdbtype none `
        -ext WixToolset.UI.wixext `
        -ext WixToolset.Util.wixext `
        -d "Version=$version" `
        -d "BinDir=$(Join-Path $root 'target\release')" `
        -d "Assets=$(Join-Path $root 'crates\shorui\assets')" `
        -d "Here=$here" `
        -o $out
    if ($LASTEXITCODE -ne 0) { throw 'wix build failed.' }
    Write-Host "Wrote $out"
}
finally {
    Pop-Location
}
