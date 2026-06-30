#!/usr/bin/env bash
#
# make-app.sh — assemble a macOS `SideTone.app` bundle for the Mac Beta 0 track.
#
# This is GROUNDWORK: it builds the bundle layout, copies the binary + bundled
# helpers + license notices, generates an .icns when tools are present, and
# ad-hoc signs the result if `codesign` is available. It does NOT notarize and
# does NOT require the Apple Developer Program (Beta 0 is unsigned/ad-hoc only).
#
# Must run on macOS (uses sips/iconutil/codesign/ditto). It is intentionally
# tolerant: missing helpers or icon tools produce warnings, not hard failures,
# so the layout can be exercised before the Mac helper binaries are in place.
#
# Usage:
#   scripts/make-app.sh --target aarch64-apple-darwin [--version 0.1.0] [--zip]
#   scripts/make-app.sh --target x86_64-apple-darwin  --version 0.1.0  --zip
#
# Expected (NOT committed) Mac helper binaries, bare-named so resolve_tool finds
# them next to the app binary:
#   assets/deps/macos/arm64/yt-dlp      assets/deps/macos/arm64/ffmpeg
#   assets/deps/macos/x86_64/yt-dlp     assets/deps/macos/x86_64/ffmpeg
#
set -euo pipefail

# --- repo root (script lives in <root>/scripts) ------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${ROOT_DIR}"

# --- defaults / args ---------------------------------------------------------
TARGET=""
VERSION="0.1.0"
MAKE_ZIP=0
# BIN_NAME is the lowercase executable name. Kept as an explicit variable
# rather than a `${APP_NAME,,}` expansion, which is bash 4+ only — macOS ships
# bash 3.2, where that syntax fails.
BUNDLE_ID="com.adeelxo.sidetone"
APP_NAME="SideTone"
BIN_NAME="sidetone"
MIN_MACOS="11.0"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --target)  TARGET="${2:?--target requires a value}"; shift 2 ;;
    --version) VERSION="${2:?--version requires a value}"; shift 2 ;;
    --zip)     MAKE_ZIP=1; shift ;;
    -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "make-app: unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ -z "${TARGET}" ]; then
  echo "make-app: --target is required (e.g. aarch64-apple-darwin)" >&2
  exit 2
fi

case "${TARGET}" in
  aarch64-apple-darwin) ARCH="arm64" ;;
  x86_64-apple-darwin)  ARCH="x86_64" ;;
  *) echo "make-app: unsupported --target '${TARGET}'" >&2; exit 2 ;;
esac

if [ "$(uname -s)" != "Darwin" ]; then
  echo "make-app: WARNING — not running on macOS; codesign/iconutil/sips will be skipped." >&2
fi

BIN_SRC="target/${TARGET}/release/${BIN_NAME}"  # target/.../release/sidetone
if [ ! -f "${BIN_SRC}" ]; then
  echo "make-app: binary not found at ${BIN_SRC}" >&2
  echo "          build it first: cargo build --release --target ${TARGET}" >&2
  exit 1
fi

# --- bundle skeleton ---------------------------------------------------------
DIST_DIR="dist/macos/${ARCH}"
APP_DIR="${DIST_DIR}/${APP_NAME}.app"
CONTENTS="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS}/MacOS"
RES_DIR="${CONTENTS}/Resources"

rm -rf "${APP_DIR}"
mkdir -p "${MACOS_DIR}" "${RES_DIR}"

# Main binary.
cp "${BIN_SRC}" "${MACOS_DIR}/${BIN_NAME}"
chmod 755 "${MACOS_DIR}/${BIN_NAME}"

# Bundled helpers, placed next to the binary in Contents/MacOS so resolve_tool's
# first candidate (the exe dir itself) hits immediately — no resolver change.
HELPER_SRC="assets/deps/macos/${ARCH}"
for helper in yt-dlp ffmpeg; do
  if [ -f "${HELPER_SRC}/${helper}" ]; then
    cp "${HELPER_SRC}/${helper}" "${MACOS_DIR}/${helper}"
    chmod 755 "${MACOS_DIR}/${helper}"
    echo "make-app: bundled helper ${helper} (${ARCH})"
  else
    echo "make-app: WARNING — missing helper ${HELPER_SRC}/${helper}; streaming will not work in this bundle." >&2
  fi
done

# --- icon (best effort; needs sips + iconutil) -------------------------------
# Generated BEFORE Info.plist so the plist only declares an icon that actually
# exists. NOTE: the source PNG is currently NOT tracked in git, so a clean
# checkout (e.g. CI) will skip this step — that's expected for Beta 0; the
# bundle just ships without a custom icon until the asset is committed.
ICON_SRC="assets/sidetone-icon-1024.png"
ICON_GENERATED=0
if [ -f "${ICON_SRC}" ] && command -v sips >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
  ICONSET="$(mktemp -d)/${BIN_NAME}.iconset"
  mkdir -p "${ICONSET}"
  for sz in 16 32 64 128 256 512; do
    sips -z "${sz}" "${sz}"     "${ICON_SRC}" --out "${ICONSET}/icon_${sz}x${sz}.png"      >/dev/null
    sips -z "$((sz*2))" "$((sz*2))" "${ICON_SRC}" --out "${ICONSET}/icon_${sz}x${sz}@2x.png" >/dev/null
  done
  iconutil -c icns "${ICONSET}" -o "${RES_DIR}/${BIN_NAME}.icns"
  ICON_GENERATED=1
  echo "make-app: generated ${BIN_NAME}.icns"
else
  echo "make-app: NOTE — no custom icon (need tracked ${ICON_SRC} + sips + iconutil); bundle will use the default." >&2
fi

# Only declare CFBundleIconFile when the .icns was actually produced, so the
# bundle never points at a missing icon.
ICON_PLIST_LINE=""
if [ "${ICON_GENERATED}" -eq 1 ]; then
  ICON_PLIST_LINE="  <key>CFBundleIconFile</key>        <string>${BIN_NAME}.icns</string>"
fi

# --- Info.plist --------------------------------------------------------------
cat > "${CONTENTS}/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>            <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>     <string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key>      <string>${BUNDLE_ID}</string>
  <key>CFBundleVersion</key>         <string>${VERSION}</string>
  <key>CFBundleShortVersionString</key> <string>${VERSION}</string>
  <key>CFBundlePackageType</key>     <string>APPL</string>
  <key>CFBundleExecutable</key>      <string>${BIN_NAME}</string>
${ICON_PLIST_LINE}
  <key>LSMinimumSystemVersion</key>  <string>${MIN_MACOS}</string>
  <key>NSHighResolutionCapable</key> <true/>
  <key>LSApplicationCategoryType</key> <string>public.app-category.music</string>
</dict>
</plist>
PLIST

# --- license / third-party notices -------------------------------------------
for doc in LICENSE COPYING-GPL-3.0.txt THIRD-PARTY-NOTICES.md; do
  [ -f "${doc}" ] && cp "${doc}" "${RES_DIR}/${doc}"
done

# --- ad-hoc signing (no Developer ID, no notarization) -----------------------
if command -v codesign >/dev/null 2>&1; then
  # Sign helpers first (inner-out), then the bundle. Ad-hoc identity "-".
  for helper in yt-dlp ffmpeg; do
    [ -f "${MACOS_DIR}/${helper}" ] && codesign --force --sign - "${MACOS_DIR}/${helper}" || true
  done
  codesign --force --deep --sign - "${APP_DIR}"
  echo "make-app: ad-hoc signed ${APP_DIR} (NOT notarized — testers must clear quarantine)."
else
  echo "make-app: WARNING — codesign not found; bundle is unsigned." >&2
fi

echo "make-app: built ${APP_DIR}"

# --- optional zip for GitHub Release upload ----------------------------------
if [ "${MAKE_ZIP}" -eq 1 ]; then
  ZIP_PATH="${DIST_DIR}/${APP_NAME}-mac-beta-${VERSION}-${ARCH}.zip"
  rm -f "${ZIP_PATH}"
  if command -v ditto >/dev/null 2>&1; then
    ditto -c -k --keepParent "${APP_DIR}" "${ZIP_PATH}"
  else
    ( cd "${DIST_DIR}" && zip -q -r "$(basename "${ZIP_PATH}")" "${APP_NAME}.app" )
  fi
  echo "make-app: packaged ${ZIP_PATH}"
fi
