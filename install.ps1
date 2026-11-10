# Installs gecko on Windows: downloads the release binary for this machine, checks its
# SHA-256 against the release's SHA256SUMS, puts it in %LOCALAPPDATA%\gecko\bin and adds
# that directory to the user's PATH.
#
#   irm https://raw.githubusercontent.com/dolusoft/gecko-cli/main/install.ps1 | iex
#
# $env:GECKO_VERSION = 'v0.3.0' installs that release instead of the latest one;
# $env:GECKO_INSTALL_DIR changes the directory.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repo = 'dolusoft/gecko-cli'
$dir = if ($env:GECKO_INSTALL_DIR) { $env:GECKO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'gecko\bin' }

$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$target = switch ($arch) {
    'AMD64' { 'x86_64-pc-windows-msvc' }
    'ARM64' { 'aarch64-pc-windows-msvc' }
    default { throw "gecko install: no build for $arch" }
}
$asset = "gecko-$target.exe"
$base = if ($env:GECKO_VERSION) {
    "https://github.com/$repo/releases/download/$($env:GECKO_VERSION)"
} else {
    "https://github.com/$repo/releases/latest/download"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("gecko-install-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Downloading $asset"
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile (Join-Path $tmp $asset)
    Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS')

    $expected = $null
    foreach ($line in Get-Content (Join-Path $tmp 'SHA256SUMS')) {
        $parts = $line -split '\s+'
        if ($parts.Count -ge 2 -and $parts[1] -eq $asset) { $expected = $parts[0].ToLowerInvariant() }
    }
    if (-not $expected) { throw "gecko install: $asset is not in SHA256SUMS" }
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { throw "gecko install: SHA-256 mismatch for $asset (expected $expected, got $actual)" }

    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $exe = Join-Path $dir 'gecko.exe'
    Move-Item -Force (Join-Path $tmp $asset) $exe
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
Write-Host "Installed gecko $(& $exe --version) to $exe"

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$entries = if ($userPath) { $userPath -split ';' } else { @() }
if ($entries -notcontains $dir) {
    $newPath = (@($entries | Where-Object { $_ }) + $dir) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$dir"
    Write-Host "Added $dir to your PATH; open a new terminal to use gecko."
}
