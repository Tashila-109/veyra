#!/bin/bash
set -eu
cd "$(dirname "$0")/.."
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
cargo build -p veyra
BUNDLE=target/Veyra.app/Contents
mkdir -p "$BUNDLE/MacOS"
cp target/debug/veyra "$BUNDLE/MacOS/veyra"
cat > "$BUNDLE/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.veyra.app</string>
<key>CFBundleName</key><string>Veyra</string>
<key>CFBundleExecutable</key><string>veyra</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>0.0.1</string>
</dict></plist>
PLIST
codesign --force --sign - target/Veyra.app
printf '%s\n' "target/Veyra.app"
