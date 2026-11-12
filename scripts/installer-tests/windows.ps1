# Install, launch and uninstall the Windows bundles (Phase 7, L8).
# Usage: powershell -File scripts/installer-tests/windows.ps1 -ArtifactDir <dir> [-PreviousMsi <msi>]
# The MSI steps need an elevated shell (GitHub's Windows runners are).
param(
    [Parameter(Mandatory = $true)][string]$ArtifactDir,
    # The MSI of the last release, to check an upgrade (INS-WIN-006).
    [string]$PreviousMsi = ""
)
$ErrorActionPreference = "Stop"
$version = (Get-Content package.json | ConvertFrom-Json).version

# INS-000: artifact names.
$setup = Join-Path $ArtifactDir "Dual.Pal_${version}_x64-setup.exe"
$msi = Join-Path $ArtifactDir "Dual.Pal_${version}_x64_en-US.msi"
if (-not (Test-Path $setup)) { throw "missing $setup" }
if (-not (Test-Path $msi)) { throw "missing $msi" }

# Launch `exe` with a temp data folder and check it is still running after 10 s.
function Test-Launch([string]$exe) {
    $tmp = Join-Path $env:TEMP ("dualpal-e2e-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    $env:DUALPAL_DATA_DIR = Join-Path $tmp "data"
    $env:DUALPAL_CACHE_DIR = Join-Path $tmp "cache"
    $env:DUALPAL_SECRETS = "memory"
    $p = Start-Process -FilePath $exe -PassThru
    Start-Sleep -Seconds 10
    if ($p.HasExited) { throw "$exe exited early with code $($p.ExitCode)" }
    Stop-Process -Id $p.Id -Force
}

# INS-002 / INS-WIN-009: the executable carries the version resource.
function Test-Version([string]$exe) {
    $vi = (Get-Item $exe).VersionInfo
    if ($vi.ProductVersion -ne $version) { throw "ProductVersion $($vi.ProductVersion) != $version" }
    if ($vi.FileVersion -notlike "$version*") { throw "FileVersion $($vi.FileVersion) != $version" }
}

# The "Dual Pal" entries in Programs and Features (per machine).
function Get-ArpEntries {
    $keys = @(
        "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
        "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*"
    )
    @(Get-ItemProperty $keys -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -eq "Dual Pal" })
}

# Run msiexec and fail on a non-zero exit code (3010 = reboot needed, fine).
function Invoke-Msi([string]$arguments, [string]$log) {
    $p = Start-Process -FilePath "msiexec.exe" -ArgumentList "$arguments /qn /l*v `"$log`"" -Wait -PassThru
    if ($p.ExitCode -ne 0 -and $p.ExitCode -ne 3010) {
        Get-Content $log -Tail 40 | Write-Host
        throw "msiexec $arguments failed with $($p.ExitCode)"
    }
}

# ---- NSIS -----------------------------------------------------------------------

# INS-WIN-001: NSIS silent per-user install.
Start-Process -FilePath $setup -ArgumentList "/S" -Wait
$exe = Join-Path $env:LOCALAPPDATA "Dual Pal\dualpal.exe"
if (-not (Test-Path $exe)) { throw "installed exe not found at $exe" }
Test-Version $exe

# INS-WIN-002: launch smoke.
Test-Launch $exe

# INS-WIN-003: NSIS silent uninstall.
Start-Process -FilePath (Join-Path $env:LOCALAPPDATA "Dual Pal\uninstall.exe") -ArgumentList "/S" -Wait
if (Test-Path $exe) { throw "uninstall left $exe" }

# ---- MSI ------------------------------------------------------------------------

$logs = Join-Path $env:TEMP ("dualpal-msi-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $logs | Out-Null
$msiExe = Join-Path $env:ProgramFiles "Dual Pal\dualpal.exe"

# INS-WIN-006 (first half): install the previous release, if given.
if ($PreviousMsi) {
    Invoke-Msi "/i `"$PreviousMsi`"" (Join-Path $logs "previous.txt")
    if ((Get-ArpEntries).Count -ne 1) { throw "the previous release did not register once" }
}

# INS-WIN-004: silent machine-wide install.
Invoke-Msi "/i `"$msi`"" (Join-Path $logs "install.txt")
if (-not (Test-Path $msiExe)) { throw "MSI did not install $msiExe" }
Test-Version $msiExe
$entries = Get-ArpEntries
# INS-WIN-006: an upgrade replaces the old entry instead of adding one.
if ($entries.Count -ne 1) { throw "expected one Programs and Features entry, found $($entries.Count)" }
if ($entries[0].DisplayVersion -notlike "$version*") { throw "entry version $($entries[0].DisplayVersion) != $version" }

# INS-WIN-005: MSI launch smoke.
Test-Launch $msiExe

# INS-WIN-007: silent uninstall removes the program and its entry.
Invoke-Msi "/x `"$msi`"" (Join-Path $logs "uninstall.txt")
if (Test-Path $msiExe) { throw "MSI uninstall left $msiExe" }
if ((Get-ArpEntries).Count -ne 0) { throw "MSI uninstall left a Programs and Features entry" }

Write-Host "windows installer checks passed"
