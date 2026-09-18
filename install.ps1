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

.PARAMETER Skill
    Also install the Claude Code skill for auriscope-cli, even when there is no
    .claude directory yet. Without this it is installed only when one exists.

.PARAMETER NoSkill
    Never install that skill.

.PARAMETER Uninstall
    Remove everything this script installed.
#>
[CmdletBinding()]
param(
    [string] $Version,
    [switch] $Force,
    [switch] $Skill,
    [switch] $NoSkill,
    [switch] $Uninstall
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo    = 'frdcmp/auriscope'
$AppName = 'Auriscope'
$Root    = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$Exe     = Join-Path $Root 'auriscope.exe'
$Shortcut = Join-Path ([Environment]::GetFolderPath('StartMenu')) "Programs\$AppName.lnk"
# Claude Code looks here, which is outside the install root; the skill is put
# there only when that directory already exists, or when -Skill asks for it.
$ClaudeDir = Join-Path $env:USERPROFILE '.claude'
$SkillDir  = Join-Path $ClaudeDir 'skills\auriscope-cli'

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
    if (Test-Path $SkillDir) { Remove-Item -Recurse -Force $SkillDir }
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
# --version prints "0.1.1", or "0.1.1 (dev v0.1.1-7-g1a2b3c4)" for a build
# from git. Compare the number only, and never treat a development build as
# current: installing a release over it is how you go back to a tested one.
$haveVersion = if ($have) { ($have -split '\s+')[0] } else { $null }
$haveIsDev   = $have -and $have -match '\(dev'
if (-not $Force -and $haveVersion -and "v$haveVersion" -eq $Version -and -not $haveIsDev) {
    Say "$AppName $haveVersion is already installed and current."
    # Still check PATH: a rerun is how someone whose PATH lost the entry
    # gets it back, and the early exit used to skip this entirely.
    Add-ToUserPath $Root
    exit 0
}

# ---- download and verify ---------------------------------------------------

$archive = "auriscope-$Version-x86_64-windows.zip"
$url     = "https://github.com/$Repo/releases/download/$Version/$archive"
$work    = Join-Path ([System.IO.Path]::GetTempPath()) "auriscope-$(Get-Random)"
New-Item -ItemType Directory -Force -Path $work | Out-Null

try {
    if ($have) { Say "Updating $AppName $haveVersion to $Version" } else { Say "Downloading $AppName $Version" }
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

    # The skill, when this release carries one and this machine wants it.
    $skillSrc = Join-Path $payload 'skills\auriscope-cli'
    $skillPut = ''
    if (-not $NoSkill -and (Test-Path $skillSrc) -and ($Skill -or (Test-Path $ClaudeDir))) {
        # Replaced rather than merged, so a reference renamed upstream does
        # not linger beside the new ones.
        if (Test-Path $SkillDir) { Remove-Item -Recurse -Force $SkillDir }
        New-Item -ItemType Directory -Force -Path $SkillDir | Out-Null
        Copy-Item -Path (Join-Path $skillSrc '*') -Destination $SkillDir -Recurse -Force
        $skillPut = $SkillDir
    }

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
    if ($skillPut) { Say "Also: $skillPut (Claude Code skill; -NoSkill to skip)" }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
