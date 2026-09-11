<#
.SYNOPSIS
    Installs the usage tracker dock for the current user.

.DESCRIPTION
    Builds a release binary, copies it to %LOCALAPPDATA%\Programs\usage-tracker,
    adds a Start Menu shortcut, and optionally starts it at login.

    Per-user only: nothing is written outside your profile and no admin rights
    are needed.

.PARAMETER NoStartup
    Skip registering the dock to run at login.

.PARAMETER Uninstall
    Remove the install directory, shortcut and login entry.

.EXAMPLE
    .\install.ps1
    .\install.ps1 -NoStartup
    .\install.ps1 -Uninstall
#>
[CmdletBinding()]
param(
    [switch]$NoStartup,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'

$AppName   = 'usage-tracker'
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$ExePath   = Join-Path $InstallDir "$AppName.exe"
$RunKey    = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$StartMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$Shortcut  = Join-Path $StartMenu 'Usage Tracker.lnk'

function Stop-Dock {
    Get-Process $AppName -ErrorAction SilentlyContinue | ForEach-Object {
        $_ | Stop-Process -Force
    }
    # The binary cannot be replaced while the old process still holds it.
    Start-Sleep -Milliseconds 700
}

if ($Uninstall) {
    Stop-Dock
    if (Test-Path $Shortcut) { Remove-Item $Shortcut -Force }
    Remove-ItemProperty -Path $RunKey -Name $AppName -ErrorAction SilentlyContinue
    if (Test-Path $InstallDir) { Remove-Item $InstallDir -Recurse -Force }
    Write-Host "Uninstalled. Settings and cached readings remain in:"
    Write-Host "  $(Join-Path $env:LOCALAPPDATA $AppName)"
    return
}

Write-Host 'Building release binary...'
Push-Location $PSScriptRoot
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
}
finally {
    Pop-Location
}

$built = Join-Path $PSScriptRoot 'target\release\usage-tracker.exe'
if (-not (Test-Path $built)) { throw "build produced no binary at $built" }

Stop-Dock
New-Item -ItemType Directory -Force $InstallDir | Out-Null
Copy-Item $built $ExePath -Force
Write-Host "Installed to $ExePath"

$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut($Shortcut)
$lnk.TargetPath = $ExePath
$lnk.WorkingDirectory = $InstallDir
$lnk.Description = 'Claude and Codex quota dock'
$lnk.Save()
Write-Host "Start Menu shortcut: $Shortcut"

if ($NoStartup) {
    Remove-ItemProperty -Path $RunKey -Name $AppName -ErrorAction SilentlyContinue
    Write-Host 'Run at login: disabled'
}
else {
    Set-ItemProperty -Path $RunKey -Name $AppName -Value "`"$ExePath`""
    Write-Host 'Run at login: enabled'
}

Start-Process $ExePath
Write-Host ''
Write-Host 'Running. Look for the dock at the right edge of your screen,'
Write-Host 'and the gauge icon in the system tray.'
