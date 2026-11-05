# Installs stirphp, the StirPHP type checker and language server, on Windows:
#
#   irm https://stirphp.github.io/core/install.ps1 | iex
#
# Environment:
#   STIRPHP_VERSION        a version such as 0.2.0, or "nightly" (default: the latest release)
#   STIRPHP_INSTALL_DIR    where to put stirphp.exe (default: %LOCALAPPDATA%\Programs\stirphp\bin)
#   STIRPHP_NO_MODIFY_PATH set to leave your PATH alone
#
# Everything is inside a function, so a download cut short can't run half a
# script. Errors throw rather than exit, which would close the user's window.

function Install-StirPHP {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    if ($env:OS -ne 'Windows_NT') {
        throw 'stirphp install: install.ps1 is for Windows; on Linux and macOS, use install.sh'
    }

    # the machine's architecture, also from an emulated 32-bit or x64 PowerShell
    $machine = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    $arch = switch ($machine) {
        'AMD64' { 'x86_64' }
        'ARM64' { 'aarch64' }
        default { throw "stirphp install: there is no build for $machine; see https://github.com/StirPHP/core#readme" }
    }
    $target = "$arch-pc-windows-msvc"

    $repo = 'https://github.com/StirPHP/core/releases'
    $version = if ($env:STIRPHP_VERSION) { $env:STIRPHP_VERSION -replace '^v', '' } else { 'latest' }
    switch ($version) {
        'latest' { $base = "$repo/latest/download"; $archive = "stirphp-$target.zip" }
        'nightly' { $base = "$repo/download/nightly"; $archive = "stirphp-nightly-$target.zip" }
        default { $base = "$repo/download/v$version"; $archive = "stirphp-$version-$target.zip" }
    }
    $dir = if ($env:STIRPHP_INSTALL_DIR) { $env:STIRPHP_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\stirphp\bin' }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("stirphp-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Host "Downloading $archive"
        $zip = Join-Path $tmp $archive
        $sums = Join-Path $tmp 'SHA256SUMS'
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$archive" -OutFile $zip
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums

        $want = $null
        foreach ($line in Get-Content $sums) {
            $parts = $line -split '\s+'
            if ($parts.Count -ge 2 -and $parts[1] -eq $archive) { $want = $parts[0] }
        }
        if (-not $want) { throw "stirphp install: SHA256SUMS doesn't list $archive" }
        if ((Get-FileHash -Algorithm SHA256 -Path $zip).Hash -ne $want.ToUpperInvariant()) {
            throw "stirphp install: checksum mismatch for $archive"
        }

        $unpacked = Join-Path $tmp 'unpacked'
        Expand-Archive -Path $zip -DestinationPath $unpacked
        $binary = Get-ChildItem -Path $unpacked -Recurse -Filter 'stirphp.exe' | Select-Object -First 1
        if (-not $binary) { throw "stirphp install: $archive has no stirphp.exe" }

        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $installed = Join-Path $dir 'stirphp.exe'
        Copy-Item -Path $binary.FullName -Destination $installed -Force
        Write-Host "Installed $(& $installed --version) to $installed"
    }
    finally {
        Remove-Item -Recurse -Force -Path $tmp -ErrorAction SilentlyContinue
    }

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = if ($userPath) { $userPath -split ';' } else { @() }
    if ($entries -contains $dir) { return }
    if ($env:STIRPHP_NO_MODIFY_PATH) {
        Write-Host "$dir is not on your PATH."
        return
    }
    $newPath = if ($userPath) { "$($userPath.TrimEnd(';'));$dir" } else { $dir }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$dir"
    Write-Host "Added $dir to your PATH. Open a new terminal to use stirphp there."
}

Install-StirPHP
