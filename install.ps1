<#
.SYNOPSIS
    Install or update Auriscope on Windows, for one user, no admin rights.

.DESCRIPTION
    Downloads the latest GitHub release, checks its SHA-256, and installs it
    to %LOCALAPPDATA%\Programs\Auriscope with a Start Menu shortcut and the
    install directory added to your user PATH. Run it again to update; it
    does nothing if you are already current.

        irm https://raw.githubusercontent.com/frdcmp/auriscope/main/install.ps1 | iex

    To pass options through a piped invocation, fetch it into a block first:

        & ([scriptblock]::Create((irm https://raw.githubusercontent.com/frdcmp/auriscope/main/install.ps1))) -Uninstall

.PARAMETER Version
    Install a specific release, for example v0.1.0, instead of the latest.

.PARAMETER Force
    Reinstall even if that version is already installed.

.PARAMETER Uninstall
    Remove everything this script installed.
#>
[CmdletBinding()]
param(
    [string] $Version,
    [switch] $Force,
    [switch] $Uninstall
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo    = 'frdcmp/auriscope'
$AppName = 'Auriscope'
$Root    = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$Exe     = Join-Path $Root 'auriscope.exe'
$Shortcut = Join-Path ([Environment]::GetFolderPath('StartMenu')) "Programs\$AppName.lnk"

function Say($msg) { Write-Host $msg -ForegroundColor Cyan }
function Die($msg) { Write-Host "error: $msg" -ForegroundColor Red; exit 1 }

if ($PSVersionTable.PSVersion.Major -lt 5) {
    Die 'PowerShell 5 or newer is required.'
}

# ---- user PATH -------------------------------------------------------------

function Add-ToUserPath($dir) {
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($current -split ';' | Where-Object { $_ })
    if ($entries -notcontains $dir) {
        [Environment]::SetEnvironmentVariable('Path', (@($entries) + $dir) -join ';', 'User')
        Say "Added $dir to your PATH (new terminals will see it)."
    }
}

function Remove-FromUserPath($dir) {
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($current -split ';' | Where-Object { $_ -and $_ -ne $dir })
    [Environment]::SetEnvironmentVariable('Path', $entries -join ';', 'User')
}

# ---- uninstall -------------------------------------------------------------

if ($Uninstall) {
    if (Test-Path $Root)     { Remove-Item -Recurse -Force $Root }
    if (Test-Path $Shortcut) { Remove-Item -Force $Shortcut }
    Remove-FromUserPath $Root
    Say "$AppName removed."
    exit 0
}

# ---- which version ---------------------------------------------------------

if (-not $Version) {
    # /releases/latest redirects to /releases/tag/vX.Y.Z; drafts and
    # pre-releases are skipped, the same way the app's own update check works.
    try {
        $resp = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" `
            -MaximumRedirection 0 -ErrorAction SilentlyContinue -UseBasicParsing
        $location = $resp.Headers.Location
    } catch {
        $location = $_.Exception.Response.Headers.Location
    }
    if (-not $location) { Die 'could not resolve the latest release.' }
    $Version = ([string]$location -split '/')[-1]
}

# The version already installed, or $null.
#
# Builds before 0.1.1 have no --version flag: they take the argument as a file
# path and open the window instead of printing anything, so this must never
# wait for the process. It is killed after five seconds and the empty answer
# means "too old to ask, reinstall".
function Get-InstalledVersion {
    if (-not (Test-Path $Exe)) { return $null }
    $out = New-TemporaryFile
    try {
        $p = Start-Process -FilePath $Exe -ArgumentList '--version' -PassThru `
            -WindowStyle Hidden -RedirectStandardOutput $out
        if (-not $p.WaitForExit(5000)) {
            try { $p.Kill() } catch { }
            return $null
        }
        $line = (Get-Content $out -ErrorAction SilentlyContinue | Select-Object -First 1)
        if ($line) { return $line.Trim() }
        return $null
    } catch {
        return $null
    } finally {
        Remove-Item $out -Force -ErrorAction SilentlyContinue
    }
}

$have = Get-InstalledVersion
if (-not $Force -and $have -and "v$have" -eq $Version) {
    Say "$AppName $have is already installed and current."
    exit 0
}

# ---- download and verify ---------------------------------------------------

$archive = "auriscope-$Version-x86_64-windows.zip"
$url     = "https://github.com/$Repo/releases/download/$Version/$archive"
$work    = Join-Path ([System.IO.Path]::GetTempPath()) "auriscope-$(Get-Random)"
New-Item -ItemType Directory -Force -Path $work | Out-Null

try {
    if ($have) { Say "Updating $AppName $have to $Version" } else { Say "Downloading $AppName $Version" }
    $zip = Join-Path $work $archive
    # Invoke-WebRequest's progress bar makes large downloads crawl in PS 5.
    $oldProgress = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    Invoke-WebRequest -Uri "$url.sha256" -OutFile "$zip.sha256" -UseBasicParsing
    $ProgressPreference = $oldProgress

    $expected = ((Get-Content "$zip.sha256" -Raw).Trim() -split '\s+')[0]
    $actual   = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($expected.ToLower() -ne $actual) { Die "checksum mismatch for $archive" }

    Expand-Archive -Path $zip -DestinationPath $work -Force
    $payload = Join-Path $work 'auriscope'
    if (-not (Test-Path $payload)) { Die 'unexpected archive layout' }

    # ---- install -----------------------------------------------------------

    Say "Installing to $Root"
    # A running copy cannot be replaced; ask first rather than failing midway.
    $running = Get-Process -Name 'auriscope' -ErrorAction SilentlyContinue
    if ($running) { Die 'Auriscope is running; close it and run this again.' }

    New-Item -ItemType Directory -Force -Path $Root | Out-Null
    Copy-Item -Path (Join-Path $payload '*') -Destination $Root -Recurse -Force

    $shell = New-Object -ComObject WScript.Shell
    $lnk = $shell.CreateShortcut($Shortcut)
    $lnk.TargetPath = $Exe
    $lnk.WorkingDirectory = $Root
    $lnk.Description = 'Audio player and analyser'
    $lnk.Save()

    Add-ToUserPath $Root
    $now = Get-InstalledVersion
    if (-not $now) { $now = $Version }
    Say "Done: $Exe ($now)"
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
