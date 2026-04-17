// Owns startup of the macOS-only animated Dock tile prototype.
use std::{
    ffi::{CStr, CString},
    sync::atomic::{AtomicBool, Ordering},
};

use tracing::warn;

const DEFAULT_RESOURCE_NAME: &str = "cat-ai-pufferfish-cat";
const DEFAULT_RESOURCE_EXTENSION: &str = "gif";

#[cfg(target_os = "macos")]
use std::os::raw::c_char;

#[cfg(target_os = "macos")]
static LIVE_ICON_INSTALLED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn seance_dock_icon_start(resource_name: *const c_char, extension: *const c_char) -> bool;
}

pub(crate) fn install_default_live_icon() {
    #[cfg(target_os = "macos")]
    install_with_bridge(
        &LIVE_ICON_INSTALLED,
        DEFAULT_RESOURCE_NAME,
        DEFAULT_RESOURCE_EXTENSION,
        ffi_start,
    );
}

fn install_with_bridge(
    installed: &AtomicBool,
    resource_name: &str,
    extension: &str,
    start: impl FnOnce(&CStr, &CStr) -> bool,
) {
    if installed.load(Ordering::Acquire) {
        return;
    }

    let Ok(resource_name) = CString::new(resource_name) else {
        warn!("macOS Dock icon resource name contained a NUL byte");
        return;
    };
    let Ok(extension) = CString::new(extension) else {
        warn!("macOS Dock icon resource extension contained a NUL byte");
        return;
    };

    if start(resource_name.as_c_str(), extension.as_c_str()) {
        installed.store(true, Ordering::Release);
    } else {
        warn!(
            resource = resource_name.as_c_str().to_string_lossy().as_ref(),
            extension = extension.as_c_str().to_string_lossy().as_ref(),
            "failed to start live macOS Dock icon; falling back to static app icon"
        );
    }
}

#[cfg(target_os = "macos")]
fn ffi_start(resource_name: &CStr, extension: &CStr) -> bool {
    unsafe { seance_dock_icon_start(resource_name.as_ptr(), extension.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::{DEFAULT_RESOURCE_EXTENSION, DEFAULT_RESOURCE_NAME, install_with_bridge};

    #[test]
    fn uses_expected_default_asset_name() {
        assert_eq!(DEFAULT_RESOURCE_NAME, "cat-ai-pufferfish-cat");
        assert_eq!(DEFAULT_RESOURCE_EXTENSION, "gif");
    }

    #[test]
    fn bridge_failure_is_ignored_without_marking_installed() {
        let installed = super::AtomicBool::new(false);
        let calls = AtomicUsize::new(0);

        install_with_bridge(
            &installed,
            DEFAULT_RESOURCE_NAME,
            DEFAULT_RESOURCE_EXTENSION,
            |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                false
            },
        );

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!installed.load(Ordering::SeqCst));
    }

    #[test]
    fn repeated_installs_are_idempotent_after_success() {
        let installed = super::AtomicBool::new(false);
        let captured = Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));

        install_with_bridge(
            &installed,
            DEFAULT_RESOURCE_NAME,
            DEFAULT_RESOURCE_EXTENSION,
            {
                let captured = Arc::clone(&captured);
                move |name, extension| {
                    captured.lock().unwrap().push((
                        name.to_string_lossy().into_owned(),
                        extension.to_string_lossy().into_owned(),
                    ));
                    true
                }
            },
        );

        install_with_bridge(
            &installed,
            DEFAULT_RESOURCE_NAME,
            DEFAULT_RESOURCE_EXTENSION,
            |_, _| {
                panic!("bridge should not be called once the live icon is installed");
            },
        );

        assert_eq!(
            captured.lock().unwrap().as_slice(),
            [(
                DEFAULT_RESOURCE_NAME.to_string(),
                DEFAULT_RESOURCE_EXTENSION.to_string()
            )]
        );
    }
}
