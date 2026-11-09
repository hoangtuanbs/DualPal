# Install, launch and uninstall the Windows bundles (Phase 7, L8).
# Usage: powershell -File scripts/installer-tests/windows.ps1 -ArtifactDir <dir>
param(
    [Parameter(Mandatory = $true)][string]$ArtifactDir
)
$ErrorActionPreference = "Stop"
$version = (Get-Content package.json | ConvertFrom-Json).version

# INS-000: artifact names.
$setup = Join-Path $ArtifactDir "Dual.Pal_${version}_x64-setup.exe"
$msi = Join-Path $ArtifactDir "Dual.Pal_${version}_x64_en-US.msi"
if (-not (Test-Path $setup)) { throw "missing $setup" }
if (-not (Test-Path $msi)) { throw "missing $msi" }

# INS-WIN-001: NSIS silent per-user install.
Start-Process -FilePath $setup -ArgumentList "/S" -Wait

# INS-WIN-009: the executable has the version resource.
$exe = Join-Path $env:LOCALAPPDATA "Dual Pal\dualpal.exe"
if (-not (Test-Path $exe)) { throw "installed exe not found at $exe" }
$vi = (Get-Item $exe).VersionInfo
if ($vi.ProductVersion -ne $version) { throw "version $($vi.ProductVersion) != $version" }

# INS-WIN-002: launch smoke with a temp data dir.
$tmp = Join-Path $env:TEMP ("dualpal-e2e-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
$env:DUALPAL_DATA_DIR = Join-Path $tmp "data"
$env:DUALPAL_CACHE_DIR = Join-Path $tmp "cache"
$env:DUALPAL_SECRETS = "memory"
$p = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds 10
if ($p.HasExited) { throw "app exited early with code $($p.ExitCode)" }
Stop-Process -Id $p.Id -Force

# INS-WIN-003: NSIS silent uninstall.
Start-Process -FilePath (Join-Path $env:LOCALAPPDATA "Dual Pal\uninstall.exe") -ArgumentList "/S" -Wait
if (Test-Path $exe) { throw "uninstall left $exe" }

Write-Host "windows installer checks passed"
