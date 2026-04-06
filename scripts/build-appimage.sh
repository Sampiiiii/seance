#!/usr/bin/env bash
set -euo pipefail

arch="${1:-}"
manifest_path="${2:-}"
if [[ -z "${arch}" || -z "${manifest_path}" ]]; then
  echo "usage: $0 <x86_64|aarch64> <manifest-path>" >&2
  exit 1
fi

repo_slug="${GITHUB_REPOSITORY:-Sampiiiii/seance}"
appdir="dist/appimage/${arch}/Seance.AppDir"
release_dir="dist/release"
target_triple="${TARGET_TRIPLE:-$(cargo run -q -p seance-build -- linux-target-triple --arch "${arch}")}"
appimage_name="$(cargo run -q -p seance-build -- artifact-name --kind linux-appimage --arch "${arch}")"
update_info="$(cargo run -q -p seance-build -- linux-update-information --arch "${arch}" --repo-slug "${repo_slug}")"

mkdir -p "${appdir}/usr/bin" "${appdir}/usr/share/applications" "${appdir}/usr/share/metainfo" "${appdir}/usr/share/icons/hicolor/scalable/apps" "${release_dir}"

cargo build --release -p seance-app --target "${target_triple}"
cp "target/${target_triple}/release/seance-app" "${appdir}/usr/bin/Seance"
cp packaging/linux/AppRun "${appdir}/AppRun"
cp packaging/linux/seance.desktop "${appdir}/usr/share/applications/seance.desktop"
cp packaging/linux/seance.appdata.xml "${appdir}/usr/share/metainfo/seance.appdata.xml"
cp packaging/linux/seance.svg "${appdir}/usr/share/icons/hicolor/scalable/apps/seance.svg"

if [[ -n "${APPIMAGEUPDATE_PATH:-}" ]]; then
  cp "${APPIMAGEUPDATE_PATH}" "${appdir}/usr/bin/AppImageUpdate"
  chmod +x "${appdir}/usr/bin/AppImageUpdate"
fi

chmod +x "${appdir}/AppRun" "${appdir}/usr/bin/Seance"

linuxdeploy="${LINUXDEPLOY_PATH:-linuxdeploy}"
appimagetool="${APPIMAGETOOL_PATH:-appimagetool}"

export ARCH="${arch}"
export UPDATE_INFORMATION="${update_info}"
export OUTPUT="${appimage_name}"

"${linuxdeploy}" --appdir "${appdir}" --plugin appimage
"${appimagetool}" -u "${UPDATE_INFORMATION}" "${appdir}" "${release_dir}/${OUTPUT}"

cargo run -q -p seance-build -- write-platform-manifest \
  --manifest "${manifest_path}" \
  --platform linux \
  --arch "${arch}" \
  --artifact "${release_dir}/${appimage_name}" \
  --artifact "${release_dir}/${appimage_name}.zsync"
