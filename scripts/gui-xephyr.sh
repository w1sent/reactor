#!/usr/bin/env bash
# Run reactor-gui inside a nested X server (Xephyr) so it can be driven and screenshotted from a
# shell: for debugging the window, on Linux, without touching the real desktop.
#
#   scripts/gui-xephyr.sh up [reactor-gui args...]   start Xephyr and the GUI inside it
#   scripts/gui-xephyr.sh shot FILE.png              screenshot the whole nested display
#   scripts/gui-xephyr.sh click X Y                  left-click at X,Y in the GUI window's pixels
#   scripts/gui-xephyr.sh type TEXT                  type TEXT into the focused field
#   scripts/gui-xephyr.sh key KEY...                 press keys (xdotool names: Return, ctrl+a, ...)
#   scripts/gui-xephyr.sh win                        print the window id and its geometry
#   scripts/gui-xephyr.sh down                       stop the GUI and Xephyr started by `up`
#
# Why nested: under GNOME/Wayland a synthetic click from xdotool to an XWayland window goes
# through the RemoteDesktop portal (a permission prompt) and can land on any window. In Xephyr the
# input stays inside its own display, and the window is still visible on the desktop.
#
# Needs: Xephyr (xserver-xephyr), xdotool, import (imagemagick). The GUI is run from
# REACTOR_GUI_BIN (default target/debug/reactor-gui, relative to where this is run from).
#
# Environment: REACTOR_XDISPLAY (default :7), REACTOR_XSIZE (default 1400x900).

set -eu

D="${REACTOR_XDISPLAY:-:7}"
SIZE="${REACTOR_XSIZE:-1400x900}"
BIN="${REACTOR_GUI_BIN:-target/debug/reactor-gui}"
STATE="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/reactor-xephyr${D}"

die() { echo "gui-xephyr: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing $1 ($2)"; }

window() {
  DISPLAY="$D" xdotool search --onlyvisible --class reactor-gui 2>/dev/null | head -n 1
}

stop_pid() {
  [ -f "$STATE/$1.pid" ] || return 0
  kill "$(cat "$STATE/$1.pid")" 2>/dev/null || true
  rm -f "$STATE/$1.pid"
}

cmd="${1:-}"
[ -n "$cmd" ] || die "usage: $0 up|shot|click|type|key|win|down ..."
shift

case "$cmd" in
  up)
    need Xephyr xserver-xephyr; need xdotool xdotool; need import imagemagick
    [ -x "$BIN" ] || die "$BIN is not built (cargo build -p reactor-gui)"
    mkdir -p "$STATE"
    if [ ! -S "/tmp/.X11-unix/X${D#:}" ]; then
      setsid nohup Xephyr "$D" -screen "$SIZE" -resizeable -ac -title "REactor test display" \
        >"$STATE/xephyr.log" 2>&1 &
      echo $! >"$STATE/xephyr.pid"
      for _ in 1 2 3 4 5 6 7 8 9 10; do [ -S "/tmp/.X11-unix/X${D#:}" ] && break; sleep 0.3; done
      [ -S "/tmp/.X11-unix/X${D#:}" ] || die "Xephyr did not start (see $STATE/xephyr.log)"
    fi
    # GPUI picks its X11 backend when WAYLAND_DISPLAY is unset.
    env -u WAYLAND_DISPLAY DISPLAY="$D" setsid nohup "$BIN" "$@" >"$STATE/gui.log" 2>&1 &
    echo $! >"$STATE/gui.pid"
    for _ in $(seq 1 40); do [ -n "$(window)" ] && break; sleep 0.5; done
    [ -n "$(window)" ] || die "no reactor-gui window on $D (see $STATE/gui.log)"
    echo "up: display $D, window $(window)"
    ;;
  win)
    w="$(window)"; [ -n "$w" ] || die "no reactor-gui window on $D"
    DISPLAY="$D" xdotool getwindowgeometry "$w"
    ;;
  shot)
    [ $# -ge 1 ] || die "shot needs an output file"
    DISPLAY="$D" import -window root "$1"
    ;;
  click)
    [ $# -eq 2 ] || die "click needs X Y"
    w="$(window)"; [ -n "$w" ] || die "no reactor-gui window on $D"
    DISPLAY="$D" xdotool mousemove --window "$w" "$1" "$2" click 1
    ;;
  type)
    [ $# -ge 1 ] || die "type needs text"
    DISPLAY="$D" xdotool type --delay 40 "$*"
    ;;
  key)
    [ $# -ge 1 ] || die "key needs at least one key"
    DISPLAY="$D" xdotool key "$@"
    ;;
  down)
    stop_pid gui
    stop_pid xephyr
    rmdir "$STATE" 2>/dev/null || true
    echo "down"
    ;;
  *)
    die "unknown command: $cmd"
    ;;
esac
