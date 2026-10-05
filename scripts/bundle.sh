#!/bin/sh
# Build target/aomac.app (docs/play.md). The icon is the client's own (AnarchyOnline.exe icon resource), extracted at bundle time.
set -e
cd "$(dirname "$0")/.."
CLIENT="${AOMAC_CLIENT:-$HOME/Games/ProjectRubiKa/client}"
APP=target/aomac.app
cargo build --release -p aomac
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/aomac "$APP/Contents/MacOS/aomac-bin"
cat > "$APP/Contents/MacOS/aomac" <<'L'
#!/bin/sh
exec "$(dirname "$0")/aomac-bin" play "$@"
L
chmod +x "$APP/Contents/MacOS/aomac"
ICON=
if [ -f "$CLIENT/AnarchyOnline.exe" ]; then
  T=$(mktemp -d)
  python3 scripts/exe_icon.py "$CLIENT/AnarchyOnline.exe" "$T/ao.ico" >/dev/null
  sips -s format png "$T/ao.ico" --out "$T/ao.png" >/dev/null
  mkdir "$T/ao.iconset"
  # the exe only carries 32x32; larger sizes are upscaled
  for s in 16 32 64 128 256 512 1024; do sips -z $s $s "$T/ao.png" --out "$T/s$s.png" >/dev/null; done
  cp "$T/s16.png" "$T/ao.iconset/icon_16x16.png";    cp "$T/s32.png" "$T/ao.iconset/icon_16x16@2x.png"
  cp "$T/s32.png" "$T/ao.iconset/icon_32x32.png";    cp "$T/s64.png" "$T/ao.iconset/icon_32x32@2x.png"
  cp "$T/s128.png" "$T/ao.iconset/icon_128x128.png"; cp "$T/s256.png" "$T/ao.iconset/icon_128x128@2x.png"
  cp "$T/s256.png" "$T/ao.iconset/icon_256x256.png"; cp "$T/s512.png" "$T/ao.iconset/icon_256x256@2x.png"
  cp "$T/s512.png" "$T/ao.iconset/icon_512x512.png"; cp "$T/s1024.png" "$T/ao.iconset/icon_512x512@2x.png"
  iconutil -c icns "$T/ao.iconset" -o "$APP/Contents/Resources/aomac.icns" && ICON=aomac
  rm -rf "$T"
fi
cat > "$APP/Contents/Info.plist" <<P
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>aomac</string>
<key>CFBundleDisplayName</key><string>Anarchy Online (aomac)</string>
<key>CFBundleIdentifier</key><string>org.aomac.aomac</string>
<key>CFBundleExecutable</key><string>aomac</string>
<key>CFBundleIconFile</key><string>$ICON</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>LSApplicationCategoryType</key><string>public.app-category.games</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict></plist>
P
echo "built $APP"
