#Requires -Version 5.1
<#
.SYNOPSIS
  Installs what DocCraftTH (PdfCraft fork) needs to build on Windows, and records what it installed
  so remove-dev-env.ps1 can take back exactly that and nothing else.

.DESCRIPTION
  Installs, through winget, only what is missing:
    - Visual Studio 2022 Build Tools with the C++ workload (MSVC compiler/linker + Windows SDK);
      Rust's default Windows toolchain (x86_64-pc-windows-msvc) links with it.
    - rustup, the stable Rust toolchain (>= 1.95, the workspace's rust-version), rustfmt, clippy
      and the wasm32-unknown-unknown target (`cargo xtask wasm`).
    - Git and the GitHub CLI.
    - Optional: qpdf (an oracle some tests use), the craft-fonts checkout (Japanese/Thai fonts,
      CRAFT_FONTS_DIR) and the OCR models (`cargo xtask models`).
  Anything already installed is left alone and marked as pre-existing, so the removal script
  never touches it.

  The record is written to %LOCALAPPDATA%\DocCraftTH\dev-env.json.

.PARAMETER ProjectPath
  The DocCraftTH checkout. Needed for -WithFonts, -WithModels and -Verify.

.PARAMETER WithOracles
  Also install qpdf.

.PARAMETER WithFonts
  Clone https://github.com/storytold/craft-fonts next to the checkout (read-only use) and set
  CRAFT_FONTS_DIR for the current user.

.PARAMETER WithModels
  Run `cargo xtask models` in the checkout (downloads the OCR models into assets/models).

.PARAMETER Verify
  Build one small crate at the end to prove the toolchain links.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\scripts\dev-env\windows\setup-dev-env.ps1 -ProjectPath K:\src\DocCraftTH -Verify
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
  [string]$ProjectPath,
  [switch]$WithOracles,
  [switch]$WithFonts,
  [switch]$WithModels,
  [switch]$Verify
)

# Cmdlets stop on error; native tools (rustup, winget, cargo) report through their exit codes,
# which every call checks. A global 'Stop' would turn rustup's informational stderr lines into
# terminating errors under Windows PowerShell 5.1.
$ErrorActionPreference = 'Continue'
$PSDefaultParameterValues['*:ErrorAction'] = 'Stop'
Set-StrictMode -Version 2.0

$MinRust = [version]'1.95.0'
$StateDir = Join-Path $env:LOCALAPPDATA 'DocCraftTH'
$StateFile = Join-Path $StateDir 'dev-env.json'
$VsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'

function Write-Step([string]$Text) { Write-Host "==> $Text" -ForegroundColor Cyan }

function Read-State {
  if (Test-Path $StateFile) {
    try { return Get-Content $StateFile -Raw | ConvertFrom-Json }
    catch { Write-Warning "Ignoring unreadable $StateFile ($($_.Exception.Message))" }
  }
  return [pscustomobject]@{ createdAt = (Get-Date).ToString('s'); components = [pscustomobject]@{} }
}

function Set-Component($State, [string]$Name, [bool]$InstalledByScript, [string]$Detail = '') {
  # A component the script installed once stays "installed by script" on later runs.
  $old = $State.components.PSObject.Properties[$Name]
  if ($old -and $old.Value.installedByScript) { $InstalledByScript = $true }
  $entry = [pscustomobject]@{ installedByScript = $InstalledByScript; detail = $Detail; at = (Get-Date).ToString('s') }
  $State.components | Add-Member -NotePropertyName $Name -NotePropertyValue $entry -Force
}

function Save-State($State) {
  if (-not (Test-Path $StateDir)) { New-Item -ItemType Directory -Path $StateDir | Out-Null }
  $State | ConvertTo-Json -Depth 5 | Set-Content -Path $StateFile -Encoding UTF8
}

function Install-WingetPackage([string]$Id, [string]$Override = '') {
  $wingetArgs = @('install', '--id', $Id, '--exact', '--source', 'winget', '--accept-package-agreements', '--accept-source-agreements', '--silent')
  if ($Override) { $wingetArgs += @('--override', $Override) }
  & winget @wingetArgs
  # 0 = installed; -1978335189 (0x8A15002B) = already installed / no newer version.
  if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) { throw "winget install $Id failed (exit $LASTEXITCODE)" }
}

function Update-SessionPath {
  $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
  $user = [Environment]::GetEnvironmentVariable('Path', 'User')
  $env:Path = "$machine;$user"
  if ((Test-Path $CargoBin) -and ($env:Path -notlike "*$CargoBin*")) { $env:Path = "$CargoBin;$env:Path" }
}

function Get-MsvcInstall {
  if (-not (Test-Path $VsWhere)) { return $null }
  $path = & $VsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
  if ($path) { return ($path | Select-Object -First 1) }
  return $null
}

# --- preflight -------------------------------------------------------------------------------
if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
  throw 'winget is not available. Install "App Installer" from the Microsoft Store, then run this script again.'
}
$freeGb = [math]::Round((Get-PSDrive -Name $env:SystemDrive.TrimEnd(':')).Free / 1GB, 1)
Write-Step "Free space on $($env:SystemDrive): $freeGb GB (Build Tools + SDK + Rust need about 10 GB; one debug build of the project needs about 15 GB more)"
if ($freeGb -lt 12) { Write-Warning 'Less than 12 GB free on the system drive: the install may fail part-way.' }
if ($ProjectPath -and -not (Test-Path (Join-Path $ProjectPath 'Cargo.toml'))) { throw "$ProjectPath is not a Cargo workspace (no Cargo.toml)." }

$state = Read-State

# --- 1. Visual Studio Build Tools + C++ workload + Windows SDK ---------------------------------
Write-Step 'Visual Studio 2022 Build Tools (C++ workload, Windows SDK)'
$msvc = Get-MsvcInstall
if ($msvc) {
  Write-Host "    already installed: $msvc"
  Set-Component $state 'BuildTools' $false $msvc
} elseif ($PSCmdlet.ShouldProcess('Microsoft.VisualStudio.2022.BuildTools', 'winget install')) {
  $override = '--wait --passive --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
  Install-WingetPackage 'Microsoft.VisualStudio.2022.BuildTools' $override
  $msvc = Get-MsvcInstall
  if (-not $msvc) { throw 'Build Tools installed, but the MSVC x64 tools were not found. Open Visual Studio Installer and add "Desktop development with C++".' }
  Set-Component $state 'BuildTools' $true $msvc
  Save-State $state
}

# --- 2. Rust ---------------------------------------------------------------------------------
Write-Step 'Rust (rustup, stable toolchain, rustfmt, clippy, wasm32 target)'
Update-SessionPath
if (Get-Command rustup -ErrorAction SilentlyContinue) {
  Write-Host '    rustup already installed'
  Set-Component $state 'Rustup' $false (rustup --version 2>$null | Out-String).Trim()
} elseif ($PSCmdlet.ShouldProcess('Rustlang.Rustup', 'winget install')) {
  Install-WingetPackage 'Rustlang.Rustup'
  Update-SessionPath
  if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { throw 'rustup was installed but is not on PATH yet. Open a new terminal and run this script again.' }
  Set-Component $state 'Rustup' $true 'winget Rustlang.Rustup'
  Save-State $state
}
if ($PSCmdlet.ShouldProcess('stable toolchain', 'rustup install/update')) {
  rustup toolchain install stable --profile minimal --component rustfmt --component clippy
  if ($LASTEXITCODE -ne 0) { throw 'rustup toolchain install stable failed' }
  rustup target add wasm32-unknown-unknown --toolchain stable
  if ($LASTEXITCODE -ne 0) { throw 'rustup target add wasm32-unknown-unknown failed' }
  $rustc = (rustc +stable --version) -replace '^rustc (\d+\.\d+\.\d+).*', '$1'
  if ([version]$rustc -lt $MinRust) {
    rustup update stable
    $rustc = (rustc +stable --version) -replace '^rustc (\d+\.\d+\.\d+).*', '$1'
  }
  if ([version]$rustc -lt $MinRust) { throw "rustc $rustc is older than the workspace's rust-version $MinRust." }
  Write-Host "    rustc $rustc"
}

# --- 3. Git and GitHub CLI -------------------------------------------------------------------
foreach ($pkg in @(@{ Name = 'Git'; Id = 'Git.Git'; Cmd = 'git' }, @{ Name = 'GhCli'; Id = 'GitHub.cli'; Cmd = 'gh' })) {
  Write-Step $pkg.Id
  if (Get-Command $pkg.Cmd -ErrorAction SilentlyContinue) {
    Write-Host "    already installed"
    Set-Component $state $pkg.Name $false ''
  } elseif ($PSCmdlet.ShouldProcess($pkg.Id, 'winget install')) {
    Install-WingetPackage $pkg.Id
    Set-Component $state $pkg.Name $true "winget $($pkg.Id)"
    Save-State $state
  }
}

# --- 4. Optional extras ----------------------------------------------------------------------
if ($WithOracles) {
  Write-Step 'qpdf (test oracle)'
  if (Get-Command qpdf -ErrorAction SilentlyContinue) {
    Set-Component $state 'Qpdf' $false ''
  } elseif ($PSCmdlet.ShouldProcess('QPDF.QPDF', 'winget install')) {
    Install-WingetPackage 'QPDF.QPDF'
    Set-Component $state 'Qpdf' $true 'winget QPDF.QPDF'
  }
}

if ($WithFonts) {
  if (-not $ProjectPath) { throw '-WithFonts needs -ProjectPath.' }
  Update-SessionPath
  $fonts = Join-Path (Split-Path $ProjectPath -Parent) 'craft-fonts'
  Write-Step "craft-fonts -> $fonts"
  if (Test-Path (Join-Path $fonts '.git')) {
    Set-Component $state 'CraftFonts' $false $fonts
  } elseif ($PSCmdlet.ShouldProcess($fonts, 'git clone craft-fonts')) {
    git clone --depth 1 https://github.com/storytold/craft-fonts $fonts
    if ($LASTEXITCODE -ne 0) { throw 'git clone craft-fonts failed' }
    Set-Component $state 'CraftFonts' $true $fonts
  }
  [Environment]::SetEnvironmentVariable('CRAFT_FONTS_DIR', $fonts, 'User')
  $env:CRAFT_FONTS_DIR = $fonts
  Set-Component $state 'CraftFontsEnv' $true 'User env CRAFT_FONTS_DIR'
}

if ($WithModels) {
  if (-not $ProjectPath) { throw '-WithModels needs -ProjectPath.' }
  Write-Step 'OCR models (cargo xtask models)'
  if ($PSCmdlet.ShouldProcess($ProjectPath, 'cargo xtask models')) {
    Push-Location $ProjectPath
    try { cargo xtask models; if ($LASTEXITCODE -ne 0) { throw 'cargo xtask models failed' } }
    finally { Pop-Location }
    Set-Component $state 'OcrModels' $true (Join-Path $ProjectPath 'assets\models')
  }
}

Save-State $state

# --- 5. Verify -------------------------------------------------------------------------------
if ($Verify) {
  if (-not $ProjectPath) { throw '-Verify needs -ProjectPath.' }
  Write-Step 'Verify: cargo check -p pdfcraft-geom'
  Push-Location $ProjectPath
  try { cargo check -p pdfcraft-geom -j 2; if ($LASTEXITCODE -ne 0) { throw 'cargo check failed: the toolchain is not usable yet' } }
  finally { Pop-Location }
}

Write-Step "Done. Record: $StateFile"
Write-Host 'Open a new terminal so PATH changes apply everywhere.'
