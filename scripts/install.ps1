<#
.SYNOPSIS
  Install REactor (the `reactor` CLI and `reactor-gui`) from source on Windows: check for the
  build dependencies, install what is missing, build, install, and seed ~/.reactor.

.EXAMPLE
  irm https://raw.githubusercontent.com/w1sent/reactor/main/scripts/install.ps1 | iex

.EXAMPLE
  # With options, from a download:
  .\install.ps1 -Yes -NoGui

.PARAMETER Yes        Do not ask before installing Visual Studio Build Tools, Git, CMake, NASM or Rust.
.PARAMETER DryRun     Say what would be installed and run; change nothing.
.PARAMETER Ref        Build this branch, tag or commit.
.PARAMETER Src        Where to keep the checkout (default: %LOCALAPPDATA%\reactor\src, or the
                      checkout this script is in).
.PARAMETER NoGui      Install the CLI only.
.PARAMETER NoSetup    Do not run `reactor setup` afterwards.
.PARAMETER SkipDeps   Do not look for or install system packages.
#>
[CmdletBinding()]
param(
  [switch]$Yes,
  [switch]$DryRun,
  [string]$Ref = '',
  [string]$Src = '',
  [switch]$NoGui,
  [switch]$NoSetup,
  [switch]$SkipDeps
)

$ErrorActionPreference = 'Stop'
$RepoUrl = if ($env:REACTOR_REPO) { $env:REACTOR_REPO } else { 'https://github.com/w1sent/reactor' }
if (-not $Src -and $env:REACTOR_SRC) { $Src = $env:REACTOR_SRC }
if ($env:REACTOR_YES -eq '1') { $Yes = $true }

function Step($m) { Write-Host "==> $m" -ForegroundColor Green }
function Info($m) { Write-Host "    $m" }
function Warn($m) { Write-Host "warning: $m" -ForegroundColor Yellow }
# `throw`, not `exit`: piped into `iex`, `exit` would close the person's terminal.
function Die($m)  { throw "error: $m" }
function Have($c) { [bool](Get-Command $c -ErrorAction SilentlyContinue) }

# Run a native command, or only say what would run. Stops the script when it fails.
function Run {
  param([string]$File, [string[]]$Arguments = @())
  if ($DryRun) { Info "+ $File $($Arguments -join ' ')"; return }
  & $File @Arguments
  if ($LASTEXITCODE -ne 0) { Die "$File failed (exit code $LASTEXITCODE)" }
}

function Confirm-Action($question) {
  if ($Yes -or $DryRun) { return $true }
  if (-not [Environment]::UserInteractive) { Warn "no terminal to ask on; re-run with -Yes to allow: $question"; return $false }
  $answer = Read-Host "$question [y/N]"
  return $answer -match '^(y|yes)$'
}

# Installing a program changes PATH for the machine and the user, not for this process.
function Update-SessionPath {
  $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
  $user = [Environment]::GetEnvironmentVariable('Path', 'User')
  $env:Path = "$machine;$user;$env:USERPROFILE\.cargo\bin"
}

function Install-WithWinget($id, $label, [string[]]$extra = @()) {
  if (-not (Have winget)) {
    Die "$label is missing and winget is not available (it ships with Windows 10 1809+ and 11, as 'App Installer' in the Microsoft Store). Install $label by hand and re-run."
  }
  if (-not (Confirm-Action "Install $label with winget?")) { Die "$label is required" }
  Run 'winget' (@('install', '--id', $id, '-e', '--accept-package-agreements', '--accept-source-agreements') + $extra)
  Update-SessionPath
}

# -- Visual Studio Build Tools: the linker, the C++ compiler, and the Windows SDK -----------------

function Find-BuildTools {
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (-not (Test-Path $vswhere)) { return $null }
  # The C++ tools component, and the Windows SDK (it has fxc.exe, which gpui compiles its
  # shaders with).
  $path = & $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath 2>$null
  if (-not $path) { return $null }
  $sdk = Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Filter fxc.exe -Recurse -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if (-not $sdk) { return $null }
  return $path
}

function Ensure-BuildTools {
  if (Find-BuildTools) { Info "Visual Studio Build Tools: found"; return }
  Info "Visual Studio Build Tools (C++ and the Windows SDK): missing"
  $override = '--quiet --wait --norestart --nocache ' +
    '--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended ' +
    '--add Microsoft.VisualStudio.Component.Windows11SDK.22621'
  Install-WithWinget 'Microsoft.VisualStudio.2022.BuildTools' 'Visual Studio 2022 Build Tools (C++ workload, about 6 GB)' @('--override', $override)
  if (-not $DryRun -and -not (Find-BuildTools)) {
    Warn "the Build Tools installer finished but the C++ tools or the Windows SDK were not found; a restart may be needed"
  }
}

# -- Rust -----------------------------------------------------------------------------------------

function Ensure-Rust {
  Update-SessionPath
  if (Have rustup) {
    Info "rustup found; updating the stable toolchain"
    Run 'rustup' @('toolchain', 'install', 'stable', '--profile', 'minimal')
    Run 'rustup' @('default', 'stable')
    return
  }
  if (-not (Confirm-Action 'Install Rust with rustup (https://rustup.rs)?')) { Die 'Rust is required' }
  if ($DryRun) { Info '+ download rustup-init.exe and run it with -y --profile minimal'; return }
  $exe = Join-Path $env:TEMP 'rustup-init.exe'
  Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile $exe -UseBasicParsing
  & $exe -y --profile minimal
  if ($LASTEXITCODE -ne 0) { Die "rustup-init failed (exit code $LASTEXITCODE)" }
  Update-SessionPath
}

# -- Source ---------------------------------------------------------------------------------------

function Find-Checkout {
  if (-not $PSScriptRoot) { return $null }
  $root = Split-Path -Parent $PSScriptRoot
  if ((Test-Path (Join-Path $root 'Cargo.toml')) -and (Test-Path (Join-Path $root 'crates\reactor-cli'))) { return $root }
  return $null
}

function Ensure-Source {
  if (-not $script:Src) {
    $here = Find-Checkout
    if ($here) {
      $script:Src = $here
      Info "building the checkout this script is in: $here"
      if ($Ref) { Warn '-Ref is ignored for an existing checkout; check the ref out yourself' }
      return
    }
    $script:Src = Join-Path $env:LOCALAPPDATA 'reactor\src'
  }
  if (-not $DryRun -and -not (Have git)) { Die 'git is needed to fetch the source' }
  if (Test-Path (Join-Path $script:Src '.git')) {
    Info "updating $script:Src"
    $target = if ($Ref) { $Ref } else { 'HEAD' }
    Run 'git' @('-C', $script:Src, 'fetch', '--depth', '1', 'origin', $target)
    Run 'git' @('-C', $script:Src, 'checkout', '--quiet', 'FETCH_HEAD')
  } else {
    Info "cloning $RepoUrl into $script:Src"
    if (-not $DryRun) { New-Item -ItemType Directory -Force -Path (Split-Path -Parent $script:Src) | Out-Null }
    if ($Ref) {
      Run 'git' @('init', '--quiet', $script:Src)
      Run 'git' @('-C', $script:Src, 'remote', 'add', 'origin', $RepoUrl)
      Run 'git' @('-C', $script:Src, 'fetch', '--depth', '1', 'origin', $Ref)
      Run 'git' @('-C', $script:Src, 'checkout', '--quiet', 'FETCH_HEAD')
    } else {
      Run 'git' @('clone', '--depth', '1', $RepoUrl, $script:Src)
    }
  }
}

# -- main -----------------------------------------------------------------------------------------

Step 'REactor installer (windows)'
if ($DryRun) { Info 'dry run: nothing will be changed' }
if ($env:OS -ne 'Windows_NT') { Die 'this script is for Windows; use scripts/install.sh elsewhere' }

Step 'System packages'
if ($SkipDeps) {
  Info 'skipping system packages (-SkipDeps)'
} else {
  Update-SessionPath
  if (-not (Have git)) { Install-WithWinget 'Git.Git' 'Git' }
  if (-not (Have cmake)) { Install-WithWinget 'Kitware.CMake' 'CMake' }
  # aws-lc-sys (the TLS library under rustls) assembles with NASM on Windows.
  if (-not (Have nasm)) {
    Install-WithWinget 'NASM.NASM' 'NASM'
    foreach ($candidate in @("$env:ProgramFiles\NASM", "$env:LOCALAPPDATA\bin\NASM")) {
      if (Test-Path (Join-Path $candidate 'nasm.exe')) { $env:Path += ";$candidate"; break }
    }
  }
  Ensure-BuildTools
}

Step 'Rust'
Ensure-Rust

Step 'Source'
Ensure-Source

# One target directory for both binaries: they share all their dependencies, and a later run
# builds only what changed.
$env:CARGO_TARGET_DIR = Join-Path $script:Src 'target'

Step 'Building and installing the reactor CLI (this takes a while)'
Run 'cargo' @('install', '--locked', '--path', (Join-Path $script:Src 'crates\reactor-cli'))
if (-not $NoGui) {
  Step 'Building and installing reactor-gui (this takes a while)'
  Run 'cargo' @('install', '--locked', '--path', (Join-Path $script:Src 'crates\reactor-gui'))
}

if (-not $NoSetup) {
  Step 'Seeding ~/.reactor (reactor setup)'
  Run (Join-Path $env:USERPROFILE '.cargo\bin\reactor.exe') @('setup')
}

Step 'Done'
Info 'reactor doctor       what tools are missing on this machine'
if (-not $NoGui) { Info 'reactor-gui <dir>    open a window on a working directory' }
Info 'Open a new terminal so the PATH change from the installers is picked up.'
