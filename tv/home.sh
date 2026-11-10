#!/bin/sh
# Optional native Home replacement. The webosbrew hook has no .sh suffix.
STATE=/var/lib/luma-home
# The cold-boot hook is a symlink to the copied $STATE/home.sh: find luma-helper beside its target.
SCRIPT=$0
if [ -L "$SCRIPT" ]; then
  LINK=$(readlink "$SCRIPT") || exit 1
  case "$LINK" in /*) SCRIPT=$LINK ;; *) SCRIPT=$(dirname -- "$SCRIPT")/$LINK ;; esac
fi
SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$SCRIPT")" && pwd) || exit 1
COMMAND=${1:-boot}
case "$COMMAND" in
  enable|disable|restore|status|apply|boot) ;;
  *) printf '%s\n' 'Usage: home.sh enable [APP_DIR] [HOME_ID] | disable | restore | status' >&2; exit 1 ;;
esac
[ "$(id -u)" = 0 ] || { printf '%s\n' 'Run as root.' >&2; exit 1; }
# boot only schedules the copied $STATE/home.sh apply, which checks for the helper itself.
[ "$COMMAND" = boot ] || [ -x "$SCRIPT_DIR/luma-helper" ] ||
  { printf '%s\n' 'The native Luma TV helper is missing.' >&2; exit 1; }
if [ "$COMMAND" = enable ] && grep -q 'release [569]\.' /etc/starfish-release 2>/dev/null; then
  printf '%s\n' \
    'Enabling Luma Home (webOS 5/6/24) will:' \
    '  1. Close any running normal Luma instance.' \
    "  2. Create $STATE with integration scripts and saved state." \
    "  3. Bind-mount Luma's executable over the selected LG Home executable." \
    "     LG's Home manifest and role stay; Luma registers with SAM itself." \
    '  4. Stop the previous Home process and launch Luma as Home.' \
    '  5. Install /var/lib/webosbrew/init.d/64-luma-home to reapply the mount at cold boot.' \
    '  6. Turn on Home Auto Launch, so Luma opens when the TV starts (restored on disable).' \
    '' \
    "LG Home's original executable remains underneath the mount." \
    'Quick Start+ retains its current setting.' \
    "Restore LG Home with: sh $STATE/home.sh disable" \
    ''
elif [ "$COMMAND" = enable ]; then
  printf '%s\n' \
    'Enabling Luma Home will:' \
    '  1. Close any running normal Luma instance.' \
    "  2. Create $STATE with integration scripts, saved state and a staged native Home app linked to Luma." \
    "  3. Bind-mount $STATE/app over /usr/palm/applications/com.webos.app.home." \
    '  4. Temporarily block and release Home so SAM rereads its manifest.' \
    '  5. Stop the previous Home process and launch Luma as Home.' \
    '  6. Install /var/lib/webosbrew/init.d/64-luma-home to reapply the mount at cold boot.' \
    '  7. Turn on Home Auto Launch, so Luma opens when the TV starts (restored on disable).' \
    '' \
    "LG Home's original files remain underneath the mount." \
    'Quick Start+ retains its current setting.' \
    "Restore LG Home with: sh $STATE/home.sh disable" \
    ''
fi
if [ "$COMMAND" = enable ]; then
  printf '%s\n' "Selected Home target: ${3:-saved target, otherwise com.webos.app.home}"

  while :; do
    printf '%s' 'Apply these changes? [y/n]: '
    if ! IFS= read -r answer; then
      printf '\n%s\n' 'Cancelled. No Home integration changes applied.'
      exit 1
    fi
    case "$answer" in
      y|Y) break ;;
      n|N) printf '%s\n' 'Cancelled. No Home integration changes applied.'; exit 0 ;;
      *) printf '%s\n' 'Please answer y or n.' ;;
    esac
  done
fi
[ ! -L "$STATE" ] || exit 1
if [ ! -d "$STATE" ]; then mkdir -m 755 "$STATE" || exit 1; fi
[ "$(stat -c %u "$STATE")" = 0 ] || exit 1
if [ "$COMMAND" = boot ]; then
  (
    sleep 5
    exec sh "$STATE/home.sh" apply
  ) </dev/null >>"$STATE/boot.log" 2>&1 &
  exit 0
fi
exec 9>"$STATE/lock"
flock -n 9 || { printf '%s\n' 'Luma Home integration is already being changed.' >&2; exit 1; }
# LG opens Home when the TV starts only while Home Auto Launch is on. Enabling turns it on so
# Luma, now Home, opens at power-on; disabling restores the value it had before.
AUTO_PREVIOUS="$STATE/home-auto-launch.previous"
auto_launch() {
  /usr/bin/luna-send -n 1 -w 4000 luna://com.webos.settingsservice/getSystemSettings \
    '{"category":"general","keys":["homeAutoLaunch"]}' 2>/dev/null |
    sed -n 's/.*"homeAutoLaunch"[^"]*"\([a-z]*\)".*/\1/p'
}
set_auto_launch() {
  /usr/bin/luna-send -n 1 -w 4000 luna://com.webos.settingsservice/setSystemSettings \
    "{\"category\":\"general\",\"settings\":{\"homeAutoLaunch\":\"$1\"}}" 2>/dev/null |
    grep -q '"returnValue": *true'
}
case "$COMMAND" in
  enable)
    "$SCRIPT_DIR/luma-helper" home enable "${2:-$SCRIPT_DIR}" "${3:-}" || exit $?
    current=$(auto_launch)
    if [ "$current" != on ]; then
      # Keep the first saved value when enable runs again.
      [ -f "$AUTO_PREVIOUS" ] || printf '%s\n' "${current:-unknown}" >"$AUTO_PREVIOUS"
      if set_auto_launch on; then
        printf '%s\n' 'Home Auto Launch: on. Luma opens when the TV starts.'
      else
        printf '%s\n' 'Could not turn on Home Auto Launch; turn it on in the TV settings.' >&2
      fi
    fi
    exit 0 ;;
  disable|restore)
    "$SCRIPT_DIR/luma-helper" home "$COMMAND" "${2:-$SCRIPT_DIR}" "${3:-}" || exit $?
    if [ -f "$AUTO_PREVIOUS" ]; then
      previous=$(cat "$AUTO_PREVIOUS")
      case "$previous" in
        on|off) set_auto_launch "$previous" && printf '%s\n' "Home Auto Launch: $previous again." ;;
      esac
      rm -f "$AUTO_PREVIOUS"
    fi
    exit 0 ;;
esac
exec "$SCRIPT_DIR/luma-helper" home "$COMMAND" "${2:-$SCRIPT_DIR}" "${3:-}"
