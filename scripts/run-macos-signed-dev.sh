#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script only supports macOS." >&2
  exit 1
fi

source "$(dirname "$0")/macos-app-common.sh"

build_only=0
if [[ "${1:-}" == "--build-only" ]]; then
  build_only=1
fi

: "${APPLE_DEVELOPMENT_SIGNING_IDENTITY:?APPLE_DEVELOPMENT_SIGNING_IDENTITY is required}"
: "${APPLE_TEAM_ID:?APPLE_TEAM_ID is required}"
: "${APPLE_DEV_PROVISIONING_PROFILE:?APPLE_DEV_PROVISIONING_PROFILE is required}"

bundle_id="${SEANCE_DEV_BUNDLE_ID:-com.seance.app.dev}"
keychain_group="${APPLE_TEAM_ID}.${bundle_id}"
version="$(cargo run -q -p seance-build -- version)"
app_root="dist/dev-macos"
app_bundle="${app_root}/Seance.app"
binary_path="target/debug/seance-app"
entitlements_template="packaging/macos/Seance.entitlements.plist.in"
entitlements_path="${app_root}/Seance.entitlements.plist"
dylib_path=""

mkdir -p "${app_root}"

cargo build -p seance-app
dylib_path="$(resolve_ghostty_dylib_path debug)"

rm -rf "${app_bundle}"
mkdir -p "${app_bundle}/Contents/MacOS" "${app_bundle}/Contents/Resources"
cp packaging/macos/Info.plist "${app_bundle}/Contents/Info.plist"
cp "${binary_path}" "${app_bundle}/Contents/MacOS/seance-app"
chmod +x "${app_bundle}/Contents/MacOS/seance-app"

/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier ${bundle_id}" "${app_bundle}/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "${app_bundle}/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${version}" "${app_bundle}/Contents/Info.plist"

render_entitlements \
  "${entitlements_template}" \
  "${entitlements_path}" \
  "${APPLE_TEAM_ID}" \
  "${bundle_id}" \
  "${keychain_group}"
bundle_runtime_dylibs "${app_bundle}" "${dylib_path}"
embed_provisioning_profile \
  "${app_bundle}" \
  "${APPLE_DEV_PROVISIONING_PROFILE}" \
  "${APPLE_TEAM_ID}" \
  "${bundle_id}"
patch_runtime_search_paths "${app_bundle}/Contents/MacOS/seance-app"
sign_nested_macos_code "${APPLE_DEVELOPMENT_SIGNING_IDENTITY}" "${app_bundle}"

sign_macos_app "${APPLE_DEVELOPMENT_SIGNING_IDENTITY}" "${entitlements_path}" "${app_bundle}"
verify_macos_app "${app_bundle}"

if [[ "${build_only}" -eq 0 ]]; then
  open "${app_bundle}"
fi
