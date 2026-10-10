#!/usr/bin/env bash
# Install REactor (the `reactor` CLI and the `reactor-gui` window) from source: check for the
# build dependencies, install what is missing, build, install, and seed ~/.reactor.
#
#   curl -fsSL https://raw.githubusercontent.com/w1sent/reactor/main/scripts/install.sh | bash
#   curl -fsSL .../install.sh | bash -s -- --yes --no-gui
#
# macOS and Linux (Debian/Ubuntu and derivatives, Fedora/RHEL/Rocky/Alma, Arch and derivatives,
# openSUSE, Alpine, Void). Windows is `scripts/install.ps1`.
#
# Written for bash 3.2 (macOS's own), so: no arrays, no associative arrays, no `${x,,}`.

set -eu

REPO_URL="${REACTOR_REPO:-https://github.com/w1sent/reactor}"
DEFAULT_SRC="${XDG_DATA_HOME:-$HOME/.local/share}/reactor/src"
# Rust edition 2024 needs 1.85; the dependency tree is newer than that, so rustup's stable is
# what is installed when there is rustup. This is the floor for a toolchain we did not install.
MIN_RUST_MAJOR=1
MIN_RUST_MINOR=85

# PATH as it came in, to tell the person at the end whether Cargo's bin is on it.
ORIG_PATH="$PATH"

YES=0
DRY=0
SKIP_DEPS=0
WITH_GUI=1
RUN_SETUP=1
REF=""
SRC="${REACTOR_SRC:-}"

# -- output -------------------------------------------------------------------------------------

if [ -t 1 ]; then
  C_B=$(printf '\033[1m'); C_G=$(printf '\033[32m'); C_Y=$(printf '\033[33m')
  C_R=$(printf '\033[31m'); C_0=$(printf '\033[0m')
else
  C_B=""; C_G=""; C_Y=""; C_R=""; C_0=""
fi
step() { printf '%s==>%s %s%s%s\n' "$C_G" "$C_0" "$C_B" "$*" "$C_0"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '%swarning:%s %s\n' "$C_Y" "$C_0" "$*" >&2; }
die()  { printf '%serror:%s %s\n' "$C_R" "$C_0" "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# Run a command, or only say what would run.
run() {
  if [ "$DRY" = 1 ]; then
    printf '    + %s\n' "$*"
  else
    "$@"
  fi
}

usage() {
  cat <<'EOF'
Install REactor from source.

  install.sh [options]

Options:
  -y, --yes        do not ask before installing system packages or Rust
      --dry-run    show what would be installed and run, change nothing
      --ref REF    build this branch, tag or commit (default: the default branch)
      --src DIR    keep the checkout here (default: ~/.local/share/reactor/src, or the
                   checkout this script is in)
      --no-gui     install the `reactor` CLI only (no windowing libraries needed)
      --no-setup   do not run `reactor setup` afterwards
      --skip-deps  do not look for or install system packages
  -h, --help       this text

Environment: REACTOR_REPO (clone URL), REACTOR_SRC (same as --src), REACTOR_YES=1 (--yes).
EOF
}

# -- arguments ----------------------------------------------------------------------------------

parse_args() {
  [ "${REACTOR_YES:-}" = 1 ] && YES=1
  while [ $# -gt 0 ]; do
    case "$1" in
      -y|--yes) YES=1 ;;
      --dry-run) DRY=1 ;;
      --ref) [ $# -ge 2 ] || die "--ref needs a value"; REF="$2"; shift ;;
      --src) [ $# -ge 2 ] || die "--src needs a value"; SRC="$2"; shift ;;
      --no-gui) WITH_GUI=0 ;;
      --no-setup) RUN_SETUP=0 ;;
      --skip-deps) SKIP_DEPS=1 ;;
      -h|--help) usage; exit 0 ;;
      *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
  done
}

# Ask a yes/no question. Piped into bash, stdin is the script, so the answer comes from the
# terminal; with no terminal (CI, a provisioning script) the answer is "yes" only under --yes.
confirm() {
  [ "$YES" = 1 ] && return 0
  [ "$DRY" = 1 ] && return 0
  if [ -r /dev/tty ] && : </dev/tty 2>/dev/null; then
    printf '%s [y/N] ' "$1" >/dev/tty
    read -r answer </dev/tty || return 1
    case "$answer" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
  fi
  warn "no terminal to ask on; re-run with --yes to allow: $1"
  return 1
}

# -- privileges ---------------------------------------------------------------------------------

as_root() {
  if [ "$(id -u)" = 0 ]; then
    run "$@"
  elif have sudo; then
    run sudo "$@"
  elif have doas; then
    run doas "$@"
  else
    die "need root to run: $* (no sudo or doas; re-run as root)"
  fi
}

# -- platform -----------------------------------------------------------------------------------

OS=""
PM=""
DISTRO=""

detect_platform() {
  case "$(uname -s)" in
    Darwin) OS=macos ;;
    Linux) OS=linux ;;
    MINGW*|MSYS*|CYGWIN*)
      die "this is a Windows shell: run scripts/install.ps1 from PowerShell instead" ;;
    *) die "unsupported system: $(uname -s)" ;;
  esac
  [ "$OS" = linux ] || return 0

  ID=""; ID_LIKE=""
  if [ -r /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
  fi
  DISTRO="${PRETTY_NAME:-${ID:-linux}}"
  # Derivatives name their parent in ID_LIKE ("ubuntu debian", "rhel centos fedora", "arch").
  for token in $ID $ID_LIKE; do
    case "$token" in
      debian|ubuntu|raspbian|linuxmint|pop|kali|neon|elementary|zorin) PM=apt; break ;;
      fedora|rhel|centos|rocky|almalinux|ol|amzn|nobara) PM=dnf; break ;;
      arch|manjaro|endeavouros|cachyos|garuda|artix) PM=pacman; break ;;
      opensuse*|suse|sles|sled) PM=zypper; break ;;
      alpine) PM=apk; break ;;
      void) PM=xbps; break ;;
    esac
  done
  if [ -z "$PM" ]; then
    # An unrecognised distro: go by what is installed.
    for candidate in apt-get dnf yum pacman zypper apk xbps-install; do
      if have "$candidate"; then
        case "$candidate" in
          apt-get) PM=apt ;; xbps-install) PM=xbps ;; yum) PM=dnf ;; *) PM=$candidate ;;
        esac
        break
      fi
    done
  fi
}

# -- what is missing ----------------------------------------------------------------------------

# Names of what is not there, space separated. Empty means the machine can build REactor.
missing_linux() {
  out=""
  for tool in cc make cmake pkg-config git curl; do
    have "$tool" || out="$out $tool"
  done
  # `cc` may be spelled gcc or clang only.
  if ! have cc && { have gcc || have clang; }; then
    out=$(printf '%s' "$out" | sed 's/ cc//')
  fi
  if [ "$WITH_GUI" = 1 ] && have pkg-config; then
    for lib in fontconfig freetype2 xkbcommon xkbcommon-x11 wayland-client xcb; do
      pkg-config --exists "$lib" 2>/dev/null || out="$out lib:$lib"
    done
  elif [ "$WITH_GUI" = 1 ]; then
    out="$out libraries"
  fi
  printf '%s' "$out"
}

# -- system packages ----------------------------------------------------------------------------

# One package out of "a|b|c" alternatives, whichever the repositories have (names move between
# releases: libfontconfig1-dev became libfontconfig-dev).
pick_apt() {
  for candidate in $(printf '%s' "$1" | tr '|' ' '); do
    if apt-cache show "$candidate" >/dev/null 2>&1; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  return 1
}

# Install packages. All at once first (one transaction, one prompt); when that fails, one at a
# time, so a name that does not exist on this release costs only itself.
pm_install_each() {
  failed=""
  for pkg in "$@"; do
    case "$PM" in
      apt) as_root apt-get install -y --no-install-recommends "$pkg" || failed="$failed $pkg" ;;
      dnf) as_root "$DNF" install -y "$pkg" || failed="$failed $pkg" ;;
      pacman) as_root pacman -S --needed --noconfirm "$pkg" || failed="$failed $pkg" ;;
      zypper) as_root zypper --non-interactive install --no-recommends "$pkg" || failed="$failed $pkg" ;;
      apk) as_root apk add "$pkg" || failed="$failed $pkg" ;;
      xbps) as_root xbps-install -Sy "$pkg" || failed="$failed $pkg" ;;
    esac
  done
  printf '%s' "$failed"
}

pm_install() {
  # $1: "required" or "recommended"; the rest: packages.
  kind="$1"; shift
  [ $# -gt 0 ] || return 0
  case "$PM" in
    apt) as_root apt-get install -y --no-install-recommends "$@" && return 0 ;;
    dnf) as_root "$DNF" install -y "$@" && return 0 ;;
    pacman) as_root pacman -S --needed --noconfirm "$@" && return 0 ;;
    zypper) as_root zypper --non-interactive install --no-recommends "$@" && return 0 ;;
    apk) as_root apk add "$@" && return 0 ;;
    xbps) as_root xbps-install -Sy "$@" && return 0 ;;
  esac
  warn "installing the $kind packages in one go failed; trying them one at a time"
  failed=$(pm_install_each "$@")
  if [ -n "$failed" ]; then
    if [ "$kind" = required ]; then
      die "could not install:$failed"
    fi
    warn "could not install (not needed to build):$failed"
  fi
}

install_linux_packages() {
  case "$PM" in
    apt)
      as_root apt-get update
      font=$(pick_apt "libfontconfig-dev|libfontconfig1-dev" || echo libfontconfig1-dev)
      free=$(pick_apt "libfreetype-dev|libfreetype6-dev" || echo libfreetype6-dev)
      pm_install required build-essential cmake pkg-config git curl ca-certificates
      if [ "$WITH_GUI" = 1 ]; then
        pm_install required "$font" "$free" libxkbcommon-dev libxkbcommon-x11-dev \
          libwayland-dev libxcb1-dev libxcb-xkb-dev libx11-xcb-dev
        # What draws, when there is no GPU: Mesa's software Vulkan (lavapipe) and GL (llvmpipe).
        pm_install recommended libvulkan1 mesa-vulkan-drivers libgl1-mesa-dri libegl1
      fi
      ;;
    dnf)
      if have dnf; then DNF=dnf; else DNF=yum; fi
      # RHEL, Rocky and Alma keep some -devel packages in CRB (PowerTools on 8) and EPEL.
      case " $ID $ID_LIKE " in
        *" rhel "*|*" rocky "*|*" almalinux "*|*" centos "*)
          as_root "$DNF" install -y epel-release || warn "EPEL is not available"
          if have dnf; then
            as_root dnf install -y dnf-plugins-core || true
            as_root dnf config-manager --set-enabled crb 2>/dev/null \
              || as_root dnf config-manager --set-enabled powertools 2>/dev/null \
              || warn "could not enable CRB/PowerTools; some -devel packages may be missing"
          fi
          ;;
      esac
      pm_install required gcc gcc-c++ make cmake pkgconf-pkg-config git curl
      if [ "$WITH_GUI" = 1 ]; then
        pm_install required fontconfig-devel freetype-devel libxkbcommon-devel \
          libxkbcommon-x11-devel wayland-devel libxcb-devel libX11-devel
        pm_install recommended vulkan-loader mesa-vulkan-drivers mesa-dri-drivers mesa-libEGL
      fi
      ;;
    pacman)
      pm_install required base-devel cmake pkgconf git curl
      if [ "$WITH_GUI" = 1 ]; then
        pm_install required fontconfig freetype2 libxkbcommon libxkbcommon-x11 wayland libxcb
        pm_install recommended vulkan-icd-loader vulkan-swrast mesa
      fi
      ;;
    zypper)
      pm_install required gcc gcc-c++ make cmake pkg-config git curl
      if [ "$WITH_GUI" = 1 ]; then
        pm_install required fontconfig-devel freetype2-devel libxkbcommon-devel \
          libxkbcommon-x11-devel wayland-devel libxcb-devel libX11-devel
        pm_install recommended libvulkan1 Mesa-vulkan-device-select Mesa-dri libvulkan_lvp
      fi
      ;;
    apk)
      pm_install required build-base cmake pkgconf git curl
      if [ "$WITH_GUI" = 1 ]; then
        pm_install required fontconfig-dev freetype-dev libxkbcommon-dev wayland-dev \
          libxcb-dev libx11-dev
        pm_install recommended vulkan-loader mesa-vulkan-swrast mesa-dri-gallium
      fi
      ;;
    xbps)
      pm_install required base-devel cmake pkg-config git curl
      if [ "$WITH_GUI" = 1 ]; then
        pm_install required fontconfig-devel freetype-devel libxkbcommon-devel \
          wayland-devel libxcb-devel libX11-devel
        pm_install recommended vulkan-loader mesa-vulkan-lavapipe mesa-dri
      fi
      ;;
    *)
      die "no supported package manager found on $DISTRO. Install a C toolchain, cmake,
       pkg-config, git, curl and the development packages for fontconfig, freetype,
       xkbcommon (+x11), wayland and xcb, then re-run with --skip-deps"
      ;;
  esac
}

ensure_linux_dependencies() {
  [ "$SKIP_DEPS" = 1 ] && { info "skipping system packages (--skip-deps)"; return 0; }
  missing=$(missing_linux)
  if [ -z "$missing" ]; then
    info "system packages: nothing missing"
    return 0
  fi
  info "missing:$missing"
  [ -n "$PM" ] || install_linux_packages # dies with the instructions
  confirm "Install the build dependencies with $PM on $DISTRO?" \
    || die "cannot build without them. Install them and re-run with --skip-deps"
  install_linux_packages
  if [ "$DRY" = 0 ]; then
    still=$(missing_linux)
    [ -z "$still" ] || warn "still missing after installing:$still"
  fi
}

# -- macOS ----------------------------------------------------------------------------------------

ensure_macos_dependencies() {
  [ "$SKIP_DEPS" = 1 ] && { info "skipping system packages (--skip-deps)"; return 0; }

  # The compiler and git: the Command Line Tools.
  if ! xcode-select -p >/dev/null 2>&1; then
    confirm "Install the Xcode Command Line Tools?" || die "they are needed to build"
    run xcode-select --install || true
    info "finish the installer window that opened, then run this script again"
    [ "$DRY" = 1 ] || exit 1
  fi

  # gpui compiles its Metal shaders at build time with `xcrun metal`, which the Command Line
  # Tools alone do not have: it needs Xcode.app (and, from Xcode 26, the Metal toolchain).
  if ! xcrun -sdk macosx --find metal >/dev/null 2>&1; then
    if have xcodebuild && xcodebuild -version >/dev/null 2>&1; then
      info "Xcode is installed but its Metal toolchain is not"
      confirm "Download the Metal toolchain (xcodebuild -downloadComponent MetalToolchain)?" \
        && run xcodebuild -downloadComponent MetalToolchain
    fi
    if [ "$DRY" = 0 ] && ! xcrun -sdk macosx --find metal >/dev/null 2>&1; then
      if [ "$WITH_GUI" = 1 ]; then
        die "the GUI needs Xcode's Metal compiler. Install Xcode from the App Store (or
       developer.apple.com), run it once, run:  sudo xcode-select -s /Applications/Xcode.app
       and re-run this script. (--no-gui builds the CLI without it.)"
      fi
    fi
  fi

  if ! have cmake; then
    if have brew; then
      confirm "Install cmake with Homebrew?" && run brew install cmake
    else
      warn "cmake is missing and Homebrew is not installed; the build may still work, if not:"
      warn "  install Homebrew (https://brew.sh) and run: brew install cmake"
    fi
  fi
}

# -- Rust -----------------------------------------------------------------------------------------

rust_is_new_enough() {
  have rustc || return 1
  version=$(rustc --version | sed -n 's/^rustc \([0-9]*\)\.\([0-9]*\).*/\1 \2/p')
  [ -n "$version" ] || return 1
  set -- $version
  [ "$1" -gt "$MIN_RUST_MAJOR" ] || { [ "$1" -eq "$MIN_RUST_MAJOR" ] && [ "$2" -ge "$MIN_RUST_MINOR" ]; }
}

ensure_rust() {
  # A rustup we installed earlier is not on this shell's PATH yet.
  [ -d "$HOME/.cargo/bin" ] && PATH="$HOME/.cargo/bin:$PATH"
  export PATH

  if have rustup; then
    info "rustup found; updating the stable toolchain"
    run rustup toolchain install stable --profile minimal
    run rustup default stable
    return 0
  fi
  if have cargo && rust_is_new_enough; then
    info "using $(rustc --version)"
    return 0
  fi
  have curl || die "curl is needed to install Rust"
  if have cargo; then
    warn "$(rustc --version 2>/dev/null || echo 'rustc') is older than ${MIN_RUST_MAJOR}.${MIN_RUST_MINOR}; installing rustup next to it"
  fi
  confirm "Install Rust with rustup (https://rustup.rs) into ~/.cargo?" \
    || die "a Rust toolchain >= ${MIN_RUST_MAJOR}.${MIN_RUST_MINOR} is required"
  if [ "$DRY" = 1 ]; then
    info "+ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
  else
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    PATH="$HOME/.cargo/bin:$PATH"
    export PATH
  fi
}

# -- source ---------------------------------------------------------------------------------------

# The directory of this script, when it is a file in a checkout (not when piped into bash).
checkout_here() {
  script="${BASH_SOURCE:-}"
  [ -n "$script" ] && [ -f "$script" ] || return 1
  dir=$(cd "$(dirname "$script")/.." 2>/dev/null && pwd) || return 1
  [ -f "$dir/Cargo.toml" ] && [ -d "$dir/crates/reactor-cli" ] || return 1
  printf '%s' "$dir"
}

ensure_source() {
  if [ -z "$SRC" ]; then
    if here=$(checkout_here); then
      SRC="$here"
      info "building the checkout this script is in: $SRC"
      [ -z "$REF" ] || warn "--ref is ignored for an existing checkout; check the ref out yourself"
      return 0
    fi
    SRC="$DEFAULT_SRC"
  fi
  have git || [ "$DRY" = 1 ] || die "git is needed to fetch the source"
  if [ -d "$SRC/.git" ]; then
    info "updating $SRC"
    run git -C "$SRC" fetch --depth 1 origin "${REF:-HEAD}"
    run git -C "$SRC" checkout --quiet FETCH_HEAD
  else
    info "cloning $REPO_URL into $SRC"
    run mkdir -p "$(dirname "$SRC")"
    if [ -n "$REF" ]; then
      run git init --quiet "$SRC"
      run git -C "$SRC" remote add origin "$REPO_URL"
      run git -C "$SRC" fetch --depth 1 origin "$REF"
      run git -C "$SRC" checkout --quiet FETCH_HEAD
    else
      run git clone --depth 1 "$REPO_URL" "$SRC"
    fi
  fi
}

# -- build ------------------------------------------------------------------------------------------

build_and_install() {
  [ "$DRY" = 1 ] || cd "$SRC"
  # One target directory for both binaries: they share all their dependencies, and a later
  # run of this script builds only what changed.
  CARGO_TARGET_DIR="$SRC/target"
  export CARGO_TARGET_DIR

  step "Building and installing the reactor CLI (this takes a while)"
  run cargo install --locked --path "$SRC/crates/reactor-cli"
  if [ "$WITH_GUI" = 1 ]; then
    step "Building and installing reactor-gui (this takes a while)"
    run cargo install --locked --path "$SRC/crates/reactor-gui"
  fi
}

finish() {
  cargo_bin="$HOME/.cargo/bin"
  if [ "$RUN_SETUP" = 1 ]; then
    step "Seeding ~/.reactor (reactor setup)"
    if [ "$DRY" = 1 ]; then
      info "+ $cargo_bin/reactor setup"
    else
      "$cargo_bin/reactor" setup || warn "reactor setup failed; run it yourself once you are in a shell with reactor on PATH"
    fi
  fi

  step "Done"
  case ":$ORIG_PATH:" in
    *":$cargo_bin:"*) ;;
    *)
      info "Cargo's bin directory is not on your PATH yet. A new login shell picks it up from"
      info "rustup's profile edit; this one needs:"
      info "    export PATH=\"\$HOME/.cargo/bin:\$PATH\""
      ;;
  esac
  info "reactor doctor       what tools are missing on this machine"
  [ "$WITH_GUI" = 0 ] || info "reactor-gui <dir>    open a window on a working directory"

  if [ "$OS" = linux ] && have systemd-detect-virt && systemd-detect-virt -q 2>/dev/null; then
    info ""
    info "This is a virtual machine ($(systemd-detect-virt)). Without graphics acceleration the window"
    info "is drawn by the CPU; REactor notices that and draws lightly (Settings > Behaviour >"
    info "Rendering). Mesa's software Vulkan/GL drivers were installed with the packages above."
  fi
}

main() {
  parse_args "$@"
  detect_platform
  step "REactor installer ($OS${DISTRO:+, $DISTRO}${PM:+, $PM})"
  [ "$DRY" = 0 ] || info "dry run: nothing will be changed"

  step "System packages"
  if [ "$OS" = macos ]; then
    ensure_macos_dependencies
  else
    ensure_linux_dependencies
  fi

  step "Rust"
  ensure_rust

  step "Source"
  ensure_source

  build_and_install
  finish
}

main "$@"
