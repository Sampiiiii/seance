#!/usr/bin/env bash
set -euo pipefail

render_entitlements() {
  local template_path="$1"
  local output_path="$2"
  local team_id="$3"
  local bundle_id="$4"
  local keychain_group="$5"

  sed \
    -e "s|__TEAM_ID__|${team_id}|g" \
    -e "s|__BUNDLE_ID__|${bundle_id}|g" \
    -e "s|__DEFAULT_KEYCHAIN_GROUP__|${keychain_group}|g" \
    "${template_path}" > "${output_path}"
}

decode_provisioning_profile() {
  local profile_path="$1"
  local output_path="$2"

  security cms -D -i "${profile_path}" > "${output_path}"
}

embed_provisioning_profile() {
  local app_bundle="$1"
  local profile_path="$2"
  local expected_team_id="$3"
  local expected_bundle_id="$4"
  local embedded_profile="${app_bundle}/Contents/embedded.provisionprofile"
  local decoded_profile
  decoded_profile="$(mktemp)"

  if [[ ! -f "${profile_path}" ]]; then
    echo "Provisioning profile not found: ${profile_path}" >&2
    exit 1
  fi

  decode_provisioning_profile "${profile_path}" "${decoded_profile}"

  if ! /usr/libexec/PlistBuddy -c "Print :TeamIdentifier:0" "${decoded_profile}" \
    | grep -Fxq "${expected_team_id}"; then
    echo "Provisioning profile team id does not match ${expected_team_id}: ${profile_path}" >&2
    rm -f "${decoded_profile}"
    exit 1
  fi

  local full_app_id="${expected_team_id}.${expected_bundle_id}"
  if ! /usr/libexec/PlistBuddy -c "Print :Entitlements:com.apple.application-identifier" "${decoded_profile}" \
    | grep -Fxq "${full_app_id}"; then
    echo "Provisioning profile app id does not match ${full_app_id}: ${profile_path}" >&2
    rm -f "${decoded_profile}"
    exit 1
  fi

  cp "${profile_path}" "${embedded_profile}"
  rm -f "${decoded_profile}"
}

resolve_ghostty_dylib_path() {
  if [[ -n "${SEANCE_GHOSTTY_DYLIB_PATH:-}" ]]; then
    if [[ ! -f "${SEANCE_GHOSTTY_DYLIB_PATH}" ]]; then
      echo "SEANCE_GHOSTTY_DYLIB_PATH does not exist: ${SEANCE_GHOSTTY_DYLIB_PATH}" >&2
      exit 1
    fi
    printf '%s\n' "${SEANCE_GHOSTTY_DYLIB_PATH}"
    return
  fi

  local profile="${1:-debug}"
  local dylib_path
  dylib_path="$(find "target/${profile}/build" -path '*/ghostty-install/lib/libghostty-vt.dylib' | head -n 1)"

  if [[ -z "${dylib_path}" || ! -f "${dylib_path}" ]]; then
    echo "Unable to locate libghostty-vt.dylib under target/${profile}/build. Set SEANCE_GHOSTTY_DYLIB_PATH explicitly." >&2
    exit 1
  fi

  printf '%s\n' "${dylib_path}"
}

bundle_runtime_dylibs() {
  local app_bundle="$1"
  local dylib_path="$2"
  local frameworks_dir="${app_bundle}/Contents/Frameworks"

  mkdir -p "${frameworks_dir}"
  cp "${dylib_path}" "${frameworks_dir}/libghostty-vt.dylib"
}

patch_runtime_search_paths() {
  local main_binary="$1"
  local desired_rpath="@executable_path/../Frameworks"

  if ! otool -l "${main_binary}" | grep -A2 LC_RPATH | grep -Fq "${desired_rpath}"; then
    install_name_tool -add_rpath "${desired_rpath}" "${main_binary}"
  fi
}

sign_nested_macos_code() {
  local signing_identity="$1"
  local app_bundle="$2"
  local frameworks_dir="${app_bundle}/Contents/Frameworks"

  if [[ ! -d "${frameworks_dir}" ]]; then
    return
  fi

  while IFS= read -r nested_item; do
    codesign --force --options runtime --sign "${signing_identity}" "${nested_item}"
  done < <(find "${frameworks_dir}" -mindepth 1 -maxdepth 1 \( -name '*.framework' -o -name '*.dylib' -o -perm -111 \) | sort)
}

sign_macos_app() {
  local signing_identity="$1"
  local entitlements_path="$2"
  local app_bundle="$3"

  codesign \
    --force \
    --options runtime \
    --entitlements "${entitlements_path}" \
    --sign "${signing_identity}" \
    "${app_bundle}"
}

verify_macos_app() {
  local app_bundle="$1"

  codesign --verify --deep --strict "${app_bundle}"
  codesign -d --entitlements - --xml "${app_bundle}"

  if [[ -f "${app_bundle}/Contents/embedded.provisionprofile" ]]; then
    security cms -D -i "${app_bundle}/Contents/embedded.provisionprofile" >/dev/null
  fi
}
