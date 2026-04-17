#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/macos-app-common.sh"

version="${1:-}"
manifest_path="${2:-}"
if [[ -z "${version}" || -z "${manifest_path}" ]]; then
  echo "usage: $0 <version> <manifest-path>" >&2
  exit 1
fi

release_dir="dist/release"
packager_dir="dist/packager"
app_name="Seance.app"
bundle_id="com.seance.app"
entitlements_template="packaging/macos/Seance.entitlements.plist.in"
entitlements_path="${packager_dir}/Seance.entitlements.plist"
feed_url="${SEANCE_SPARKLE_FEED_URL:-https://sampiiiii.github.io/seance/sparkle/stable/appcast.xml}"
dylib_path=""

mkdir -p "${release_dir}" "${packager_dir}"

app_zip_name="$(cargo run -q -p seance-build -- artifact-name --kind macos-app-zip)"
dmg_name="$(cargo run -q -p seance-build -- artifact-name --kind macos-dmg)"
sparkle_item_name="$(cargo run -q -p seance-build -- artifact-name --kind sparkle-item)"

cargo packager --release -p seance-app

app_bundle="$(find "${packager_dir}" -maxdepth 2 -type d -name "${app_name}" | head -n 1)"
dmg_path="$(find "${packager_dir}" -maxdepth 2 -type f -name '*.dmg' | head -n 1)"

if [[ -z "${app_bundle}" || -z "${dmg_path}" ]]; then
  echo "cargo-packager did not produce the expected .app and .dmg outputs" >&2
  exit 1
fi

dylib_path="$(resolve_ghostty_dylib_path release)"

cp packaging/macos/Info.plist "${app_bundle}/Contents/Info.plist"
mkdir -p "${app_bundle}/Contents/Resources"
cp -R packaging/macos/Resources/. "${app_bundle}/Contents/Resources/"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier ${bundle_id}" "${app_bundle}/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "${app_bundle}/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${version}" "${app_bundle}/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :SUFeedURL ${feed_url}" "${app_bundle}/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :SUPublicEDKey ${SPARKLE_PUBLIC_KEY}" "${app_bundle}/Contents/Info.plist"

if [[ -n "${SPARKLE_FRAMEWORK_PATH:-}" ]]; then
  mkdir -p "${app_bundle}/Contents/Frameworks"
  rsync -a "${SPARKLE_FRAMEWORK_PATH}" "${app_bundle}/Contents/Frameworks/"
fi
bundle_runtime_dylibs "${app_bundle}" "${dylib_path}"
patch_runtime_search_paths "${app_bundle}/Contents/MacOS/seance-app"

if [[ -n "${APPLE_CERT_P12_BASE64:-}" ]]; then
  security create-keychain -p temp build.keychain
  security default-keychain -s build.keychain
  security unlock-keychain -p temp build.keychain
  echo "${APPLE_CERT_P12_BASE64}" | base64 --decode > certificate.p12
  security import certificate.p12 -k build.keychain -P "${APPLE_CERT_PASSWORD}" -T /usr/bin/codesign
  security set-key-partition-list -S apple-tool:,apple: -s -k temp build.keychain
fi

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  if [[ -z "${APPLE_TEAM_ID:-}" ]]; then
    echo "APPLE_TEAM_ID is required when APPLE_SIGNING_IDENTITY is set" >&2
    exit 1
  fi
  if [[ -z "${APPLE_PROVISIONING_PROFILE:-}" ]]; then
    echo "APPLE_PROVISIONING_PROFILE is required when APPLE_SIGNING_IDENTITY is set" >&2
    exit 1
  fi

  render_entitlements \
    "${entitlements_template}" \
    "${entitlements_path}" \
    "${APPLE_TEAM_ID}" \
    "${bundle_id}" \
    "${APPLE_TEAM_ID}.${bundle_id}"
  embed_provisioning_profile \
    "${app_bundle}" \
    "${APPLE_PROVISIONING_PROFILE}" \
    "${APPLE_TEAM_ID}" \
    "${bundle_id}"
  sign_nested_macos_code "${APPLE_SIGNING_IDENTITY}" "${app_bundle}"
  sign_macos_app "${APPLE_SIGNING_IDENTITY}" "${entitlements_path}" "${app_bundle}"
  verify_macos_app "${app_bundle}"
  codesign --force --sign "${APPLE_SIGNING_IDENTITY}" "${dmg_path}"
fi

ditto -c -k --keepParent "${app_bundle}" "${release_dir}/${app_zip_name}"
cp "${dmg_path}" "${release_dir}/${dmg_name}"

if [[ -n "${APPLE_API_PRIVATE_KEY_BASE64:-}" ]]; then
  mkdir -p "${HOME}/private_keys"
  echo "${APPLE_API_PRIVATE_KEY_BASE64}" | base64 --decode > "${HOME}/private_keys/AuthKey_${APPLE_API_KEY_ID}.p8"
  xcrun notarytool submit "${release_dir}/${dmg_name}" \
    --key "${HOME}/private_keys/AuthKey_${APPLE_API_KEY_ID}.p8" \
    --key-id "${APPLE_API_KEY_ID}" \
    --issuer "${APPLE_API_ISSUER_ID}" \
    --wait
  xcrun stapler staple "${release_dir}/${dmg_name}"
fi

cargo run -q -p seance-build -- write-sparkle-item \
  --version "${version}" \
  --artifact "${release_dir}/${app_zip_name}" \
  --output "${release_dir}/${sparkle_item_name}"

cargo run -q -p seance-build -- write-platform-manifest \
  --manifest "${manifest_path}" \
  --platform macos \
  --arch aarch64 \
  --artifact "${release_dir}/${dmg_name}" \
  --artifact "${release_dir}/${app_zip_name}" \
  --artifact "${release_dir}/${sparkle_item_name}"
