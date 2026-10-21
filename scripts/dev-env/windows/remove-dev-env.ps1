#Requires -Version 5.1
<#
.SYNOPSIS
  Removes what setup-dev-env.ps1 installed for DocCraftTH, and only that, when the project is closed.

.DESCRIPTION
  Reads %LOCALAPPDATA%\DocCraftTH\dev-env.json and removes each component the setup script
  installed itself (installedByScript = true). Components that were already on the machine are
  never touched: other projects may need the C++ Build Tools or Rust.

  On a machine where the setup script never ran there is no record. Then name the components
  yourself with -Components; each removal still asks for confirmation.

  Every step asks before it runs (ConfirmImpact High). Preview everything with -WhatIf.

  Components:
    BuildTools    Visual Studio 2022 Build Tools (C++), then the Windows SDK entries installed on or
                  after -Since (or the Build Tools record date), then the installer's package cache
                  once no Visual Studio instance remains.
    Rustup        rustup self uninstall (removes %USERPROFILE%\.cargo and %USERPROFILE%\.rustup).
    Git, GhCli    winget uninstall Git.Git / GitHub.cli.
    Qpdf          winget uninstall QPDF.QPDF.
    CraftFonts    the craft-fonts clone, and the CRAFT_FONTS_DIR user variable.
    OcrModels     <ProjectPath>\assets\models.

.PARAMETER Components
  Override the record: remove exactly these (still confirmed one by one).

.PARAMETER Since
  Windows SDK entries installed before this date are kept. Defaults to the date Build Tools was
  recorded; required when -Components BuildTools is used without a record.

.PARAMETER ProjectPath
  The checkout, for -CleanBuild and OcrModels.

.PARAMETER CleanBuild
  Delete the checkout's build output (<ProjectPath>\target and every worktree's target).

.PARAMETER RemoveInstaller
  Also uninstall "Microsoft Visual Studio Installer" when no Visual Studio instance remains.

.EXAMPLE
  .\remove-dev-env.ps1 -WhatIf
.EXAMPLE
  .\remove-dev-env.ps1 -Components BuildTools,Rustup -Since 2026-10-16 -ProjectPath K:\src\DocCraftTH\pdfcraft -CleanBuild
#>
[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
  [string[]]$Components,
  [datetime]$Since,
  [string]$ProjectPath,
  [switch]$CleanBuild,
  [switch]$RemoveInstaller
)

# Cmdlets stop on error; native tools report through exit codes (see setup-dev-env.ps1).
$ErrorActionPreference = 'Continue'
$PSDefaultParameterValues['*:ErrorAction'] = 'Stop'
Set-StrictMode -Version 2.0

# `powershell -File … -Components A,B` passes one string "A,B"; accept both forms.
$Known = @('BuildTools', 'Rustup', 'Git', 'GhCli', 'Qpdf', 'CraftFonts', 'OcrModels')
if ($Components) {
  $Components = @($Components | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
  $bad = @($Components | Where-Object { $Known -notcontains $_ })
  if ($bad) { throw "Unknown component(s): $($bad -join ', '). Use: $($Known -join ', ')" }
}

$StateFile = Join-Path $env:LOCALAPPDATA 'DocCraftTH\dev-env.json'
$InstallerDir = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer'
$VsWhere = Join-Path $InstallerDir 'vswhere.exe'
$PackageCache = Join-Path $env:ProgramData 'Microsoft\VisualStudio\Packages'
$UninstallKeys = @(
  'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
)

function Write-Step([string]$Text) { Write-Host "==> $Text" -ForegroundColor Cyan }

function Get-FolderGb([string]$Path) {
  if (-not (Test-Path $Path)) { return 0 }
  $sum = (Get-ChildItem $Path -Recurse -Force -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
  return [math]::Round(($sum / 1GB), 2)
}

function Get-VsInstances {
  if (-not (Test-Path $VsWhere)) { return @() }
  return @(& $VsWhere -all -products * -property installationPath 2>$null | Where-Object { $_ })
}

function Get-EntryDate($Entry) {
  if ($Entry.PSObject.Properties['InstallDate'] -and $Entry.InstallDate -match '^\d{8}$') {
    return [datetime]::ParseExact($Entry.InstallDate, 'yyyyMMdd', $null)
  }
  if ($Entry.UninstallString -match '^\s*"([^"]+)"' -or $Entry.UninstallString -match '^\s*(\S+)') {
    $exe = Get-Item $Matches[1] -ErrorAction SilentlyContinue
    if ($exe) { return $exe.CreationTime.Date }
  }
  return $null
}

function Invoke-Uninstaller([string]$Command, [string]$QuietArgs) {
  # UninstallString is "path" args or path args; split off the executable.
  if ($Command -match '^\s*"([^"]+)"\s*(.*)$') { $exe = $Matches[1]; $rest = $Matches[2] }
  elseif ($Command -match '^\s*(\S+)\s*(.*)$') { $exe = $Matches[1]; $rest = $Matches[2] }
  else { throw "Unrecognised uninstall command: $Command" }
  if ($exe -match 'msiexec') { $rest = ($rest -replace '/I', '/X') + ' /qn /norestart' }
  else { $rest = "$rest $QuietArgs" }
  $p = Start-Process -FilePath $exe -ArgumentList $rest.Trim() -Wait -PassThru -Verb RunAs
  if ($p.ExitCode -ne 0 -and $p.ExitCode -ne 3010) { Write-Warning "$exe exited with $($p.ExitCode)" }
}

# --- what to remove --------------------------------------------------------------------------
$record = $null
if (Test-Path $StateFile) {
  try { $record = Get-Content $StateFile -Raw | ConvertFrom-Json }
  catch { throw "Can't read $StateFile ($($_.Exception.Message)). Fix or delete it, or pass -Components." }
}

if ($Components) {
  $targets = $Components
} elseif ($record) {
  $targets = @($record.components.PSObject.Properties | Where-Object { $_.Value.installedByScript } | ForEach-Object { $_.Name } |
      Where-Object { $_ -ne 'CraftFontsEnv' })
  $kept = @($record.components.PSObject.Properties | Where-Object { -not $_.Value.installedByScript } | ForEach-Object { $_.Name })
  if ($kept) { Write-Host "Kept (were on this machine before the setup script): $($kept -join ', ')" }
} else {
  throw "No record at $StateFile (setup-dev-env.ps1 never ran here). Name what to remove with -Components, e.g. -Components BuildTools,Rustup -Since 2026-10-16."
}

if (-not $Since -and $record -and $record.components.PSObject.Properties['BuildTools']) {
  $Since = ([datetime]$record.components.BuildTools.at).Date
}

Write-Host "Will consider: $($targets -join ', ')$(if ($CleanBuild) { ', CleanBuild' })"
$freeBefore = (Get-PSDrive -Name $env:SystemDrive.TrimEnd(':')).Free

# --- project build output first (the largest, and needs cargo while it still exists) ------------
if ($CleanBuild) {
  if (-not $ProjectPath -or -not (Test-Path (Join-Path $ProjectPath 'Cargo.toml'))) { throw '-CleanBuild needs -ProjectPath pointing at the checkout.' }
  $dirs = @(Join-Path $ProjectPath 'target') + @(Get-ChildItem (Join-Path $ProjectPath '.claude\worktrees') -Directory -ErrorAction SilentlyContinue |
      ForEach-Object { Join-Path $_.FullName 'target' })
  foreach ($d in ($dirs | Where-Object { Test-Path $_ })) {
    if ($PSCmdlet.ShouldProcess("$d ($(Get-FolderGb $d) GB)", 'Delete build output')) { Remove-Item $d -Recurse -Force }
  }
}

# --- Build Tools, SDK, package cache ----------------------------------------------------------
if ($targets -contains 'BuildTools') {
  Write-Step 'Visual Studio 2022 Build Tools'
  $bt = @(Get-VsInstances | Where-Object { $_ -like '*\BuildTools' })
  foreach ($path in $bt) {
    if ($PSCmdlet.ShouldProcess($path, 'Uninstall Visual Studio Build Tools')) {
      $p = Start-Process -FilePath (Join-Path $InstallerDir 'setup.exe') -ArgumentList "uninstall --installPath `"$path`" --passive --norestart" -Wait -PassThru -Verb RunAs
      if ($p.ExitCode -ne 0 -and $p.ExitCode -ne 3010) { throw "Build Tools uninstall exited with $($p.ExitCode)" }
    }
  }
  if (-not $bt) { Write-Host '    no Build Tools instance found' }

  if (-not $Since) { throw 'Removing the Windows SDK needs -Since (the day the project setup started), so older SDKs are kept.' }
  Write-Step "Windows SDK bundles installed on or after $($Since.ToString('yyyy-MM-dd'))"
  # The SDK bundle entry ("Windows Software Development Kit - Windows 10.0.x") carries no
  # InstallDate, so its date is that of its cached setup program. Its MSI parts are hidden
  # (SystemComponent = 1) and go with the bundle.
  $sdks = @(Get-ItemProperty $UninstallKeys -ErrorAction SilentlyContinue | Where-Object {
      $_.PSObject.Properties['DisplayName'] -and $_.DisplayName -like 'Windows Software Development Kit*' -and $_.PSObject.Properties['UninstallString']
    } | Where-Object { $d = Get-EntryDate $_; $d -and $d -ge $Since })
  foreach ($s in $sdks) {
    if ($PSCmdlet.ShouldProcess("$($s.DisplayName) $($s.DisplayVersion)", 'Uninstall')) { Invoke-Uninstaller $s.UninstallString '/quiet /norestart' }
  }
  if (-not $sdks) { Write-Host '    none found (the Build Tools uninstall may already have removed it)' }

  if (-not (Get-VsInstances)) {
    if ((Test-Path $PackageCache) -and $PSCmdlet.ShouldProcess("$PackageCache ($(Get-FolderGb $PackageCache) GB)", 'Delete the installer package cache (no Visual Studio instance remains)')) {
      Remove-Item $PackageCache -Recurse -Force
    }
    if ($RemoveInstaller) {
      $inst = Get-ItemProperty $UninstallKeys -ErrorAction SilentlyContinue |
        Where-Object { $_.PSObject.Properties['DisplayName'] -and $_.DisplayName -eq 'Microsoft Visual Studio Installer' -and $_.PSObject.Properties['UninstallString'] } |
        Select-Object -First 1
      if ($inst -and $PSCmdlet.ShouldProcess('Microsoft Visual Studio Installer', 'Uninstall')) { Invoke-Uninstaller $inst.UninstallString '--quiet' }
    }
  } else {
    Write-Host '    other Visual Studio instances remain; the package cache and installer are kept'
  }
}

# --- Rust ------------------------------------------------------------------------------------
if ($targets -contains 'Rustup') {
  Write-Step 'Rust (rustup self uninstall)'
  $rustup = Get-Command rustup -ErrorAction SilentlyContinue
  if (-not $rustup) { $rustup = Get-Item (Join-Path $env:USERPROFILE '.cargo\bin\rustup.exe') -ErrorAction SilentlyContinue }
  if ($rustup) {
    $size = (Get-FolderGb (Join-Path $env:USERPROFILE '.cargo')) + (Get-FolderGb (Join-Path $env:USERPROFILE '.rustup'))
    if ($PSCmdlet.ShouldProcess("rustup, ~/.cargo, ~/.rustup ($size GB)", 'rustup self uninstall')) {
      & $(if ($rustup -is [System.Management.Automation.CommandInfo]) { $rustup.Source } else { $rustup.FullName }) self uninstall -y
      if ($LASTEXITCODE -ne 0) { throw 'rustup self uninstall failed' }
    }
  } else { Write-Host '    rustup not found' }
}

# --- winget packages -------------------------------------------------------------------------
$wingetIds = @{ Git = 'Git.Git'; GhCli = 'GitHub.cli'; Qpdf = 'QPDF.QPDF' }
foreach ($name in @('Git', 'GhCli', 'Qpdf')) {
  if ($targets -notcontains $name) { continue }
  $id = $wingetIds[$name]
  Write-Step $id
  if ($PSCmdlet.ShouldProcess($id, 'winget uninstall')) {
    winget uninstall --id $id --exact --silent --accept-source-agreements
    if ($LASTEXITCODE -ne 0) { Write-Warning "winget uninstall $id exited with $LASTEXITCODE (not installed?)" }
  }
}

# --- project extras --------------------------------------------------------------------------
if ($targets -contains 'CraftFonts') {
  $fonts = $null
  if ($record -and $record.components.PSObject.Properties['CraftFonts']) { $fonts = $record.components.CraftFonts.detail }
  if (-not $fonts) { $fonts = [Environment]::GetEnvironmentVariable('CRAFT_FONTS_DIR', 'User') }
  Write-Step "craft-fonts ($fonts)"
  if ($fonts -and (Test-Path (Join-Path $fonts '.git')) -and $PSCmdlet.ShouldProcess("$fonts ($(Get-FolderGb $fonts) GB)", 'Delete the craft-fonts clone')) {
    Remove-Item $fonts -Recurse -Force
  }
  if ([Environment]::GetEnvironmentVariable('CRAFT_FONTS_DIR', 'User') -and $PSCmdlet.ShouldProcess('CRAFT_FONTS_DIR', 'Remove user environment variable')) {
    [Environment]::SetEnvironmentVariable('CRAFT_FONTS_DIR', $null, 'User')
  }
}

if ($targets -contains 'OcrModels') {
  if (-not $ProjectPath) { throw 'OcrModels needs -ProjectPath.' }
  $models = Join-Path $ProjectPath 'assets\models'
  if ((Test-Path $models) -and $PSCmdlet.ShouldProcess("$models ($(Get-FolderGb $models) GB)", 'Delete the OCR models')) { Remove-Item $models -Recurse -Force }
}

# --- record ----------------------------------------------------------------------------------
if ($record -and -not $WhatIfPreference -and -not $Components) {
  if ($PSCmdlet.ShouldProcess($StateFile, 'Delete the setup record')) { Remove-Item $StateFile -Force }
}

$freed = [math]::Round((((Get-PSDrive -Name $env:SystemDrive.TrimEnd(':')).Free) - $freeBefore) / 1GB, 2)
Write-Step "Done. $($env:SystemDrive) free space changed by $freed GB. A restart may be needed to finish removing Build Tools."
