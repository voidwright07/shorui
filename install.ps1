# Installs Shorui on Windows, for the current user (no administrator rights):
#
#   irm https://raw.githubusercontent.com/voidwright07/shorui/main/install.ps1 | iex
#
# Options, set before running the line above:
#   $env:SHORUI_VERSION = 'v0.2.0'   install that release instead of the latest
#   $env:SHORUI_UNINSTALL = '1'      remove Shorui (your settings are kept)

& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'   # the progress bar makes downloads very slow in Windows PowerShell
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $repo = 'voidwright07/shorui'
    $version = if ($env:SHORUI_VERSION) { $env:SHORUI_VERSION } else { 'latest' }
    if ($version -ne 'latest' -and -not $version.StartsWith('v')) { $version = "v$version" }
    $base = if ($version -eq 'latest') { "https://github.com/$repo/releases/latest/download" } else { "https://github.com/$repo/releases/download/$version" }

    function Find-Shorui {
        $sid = ([Security.Principal.WindowsIdentity]::GetCurrent()).User.Value
        $products = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\$sid\Products"
        if (-not (Test-Path $products)) { return $null }
        Get-ChildItem $products | ForEach-Object { Get-ItemProperty (Join-Path $_.PSPath 'InstallProperties') -ErrorAction SilentlyContinue } |
            Where-Object { $_.DisplayName -eq 'Shorui' } | Select-Object -First 1
    }

    if ($env:SHORUI_UNINSTALL -eq '1') {
        $installed = Find-Shorui
        if (-not $installed) { Write-Host 'Shorui is not installed.'; return }
        $code = ($installed.UninstallString -replace '(?i)^.*?(\{[0-9A-F-]+\}).*$', '$1')
        $p = Start-Process msiexec.exe -ArgumentList "/x $code /passive /norestart" -Wait -PassThru
        if ($p.ExitCode -ne 0) { throw "Uninstalling failed (msiexec exit code $($p.ExitCode))." }
        Write-Host "Shorui is removed. Its settings stay in $env:APPDATA\Shorui."
        return
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("shorui-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $asset = 'Shorui-windows-x64.msi'
        $msi = Join-Path $tmp $asset
        Write-Host "Downloading Shorui ($version) for Windows..."
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $msi

        # Check the download against the release's SHA256SUMS.txt.
        try {
            $sums = (Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS.txt").Content
            if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }
            $line = ($sums -split "`n") | Where-Object { $_ -match "\s\*?$([regex]::Escape($asset))\s*$" } | Select-Object -First 1
            if (-not $line) { throw "$asset is not in the release's checksum list." }
            $want = ($line -split '\s+')[0].ToLower()
            $got = (Get-FileHash -Algorithm SHA256 $msi).Hash.ToLower()
            if ($want -ne $got) { throw "The download of $asset is damaged (checksum mismatch). Please try again." }
        }
        catch [System.Net.WebException] {
            Write-Host '  (no checksum list in this release; skipping the check)'
        }

        Write-Host 'Installing...'
        $p = Start-Process msiexec.exe -ArgumentList "/i `"$msi`" /passive /norestart" -Wait -PassThru
        switch ($p.ExitCode) {
            0 { }
            3010 { }
            1602 { throw 'The installation was cancelled.' }
            default { throw "The installer failed (msiexec exit code $($p.ExitCode))." }
        }
        Write-Host ''
        Write-Host 'Shorui is installed. Open it from the Start menu.'
    }
    finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}
