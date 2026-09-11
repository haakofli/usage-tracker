#!/usr/bin/env bash
#
# Installs the usage tracker dock for the current user on macOS.
#
# Builds a release binary, wraps it in /Applications/Usage Tracker.app, and
# optionally starts it at login. Per-user only: nothing is written outside your
# home directory except the bundle itself, and no sudo is needed.
#
#   ./install.sh                # install and run at login
#   ./install.sh --no-startup   # install without running at login
#   ./install.sh --uninstall    # remove it again

set -euo pipefail

APP_NAME="usage-tracker"
BUNDLE_ID="dev.haakofli.usage-tracker"
APP_DIR="/Applications/Usage Tracker.app"
AGENT="$HOME/Library/LaunchAgents/$BUNDLE_ID.plist"
SUPPORT="$HOME/Library/Application Support/$APP_NAME"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

no_startup=0
uninstall=0
for arg in "$@"; do
  case "$arg" in
    --no-startup) no_startup=1 ;;
    --uninstall)  uninstall=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

stop_dock() {
  # launchctl first, so it is not restarted the moment it is killed.
  launchctl unload "$AGENT" 2>/dev/null || true
  pkill -x "$APP_NAME" 2>/dev/null || true
}

if [[ $uninstall -eq 1 ]]; then
  stop_dock
  rm -f "$AGENT"
  rm -rf "$APP_DIR"
  echo "Uninstalled. Settings and cached readings remain in:"
  echo "  $SUPPORT"
  exit 0
fi

echo "Building release binary..."
cargo build --release --manifest-path "$ROOT/Cargo.toml"

built="$ROOT/target/release/$APP_NAME"
[[ -f "$built" ]] || { echo "build produced no binary at $built" >&2; exit 1; }

stop_dock
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "$built" "$APP_DIR/Contents/MacOS/$APP_NAME"
cp "$ROOT/assets/branding/macos/app.icns" "$APP_DIR/Contents/Resources/app.icns"

# LSUIElement keeps the dock out of the Dock and the app switcher: it is a menu
# bar accessory, and a bouncing icon for a quota readout would be noise.
cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Usage Tracker</string>
    <key>CFBundleDisplayName</key><string>Usage Tracker</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key><string>$APP_NAME</string>
    <key>CFBundleIconFile</key><string>app.icns</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>0.1.0</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "Installed to $APP_DIR"

if [[ $no_startup -eq 1 ]]; then
  rm -f "$AGENT"
  echo "Run at login: disabled"
else
  mkdir -p "$(dirname "$AGENT")"
  cat > "$AGENT" <<AGENTPLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$BUNDLE_ID</string>
    <key>ProgramArguments</key>
    <array><string>$APP_DIR/Contents/MacOS/$APP_NAME</string></array>
    <key>RunAtLoad</key><true/>
</dict>
</plist>
AGENTPLIST
  launchctl load "$AGENT"
  echo "Run at login: enabled"
fi

open "$APP_DIR"
echo
echo "Running. Look for the dock at the right edge of your screen,"
echo "and the gauge icon in the menu bar."
