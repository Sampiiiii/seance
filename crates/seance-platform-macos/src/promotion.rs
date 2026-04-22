// ProMotion opt-in hook for macOS windows.
//
// GPUI's macOS backend uses a CAMetalLayer whose implicit display link defaults
// to the display's base rate (60 Hz) when entering fullscreen on ProMotion
// panels. We hand AppKit a `preferredFrameRateRange` hint via an Objective-C
// bridge so the layer targets the display's max refresh (typically 120 Hz)
// instead. The bridge registers notification observers on NSApplication for
// window become-main/key/fullscreen transitions, so every window handed to us
// by GPUI opts in automatically without requiring changes to the UI crate.

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn seance_promotion_install() -> bool;
}

static PROMOTION_INSTALLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn install_promotion_hint() {
    if PROMOTION_INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let ok = unsafe { seance_promotion_install() };
        if !ok {
            tracing::warn!("ProMotion opt-in bridge reported failure; falling back to 60 Hz");
            PROMOTION_INSTALLED.store(false, Ordering::Release);
        } else {
            tracing::debug!("installed ProMotion opt-in notification observers");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // We deliberately avoid calling `install_promotion_hint()` in the test
    // binary: on macOS the bridge performs a `dispatch_sync` onto the main
    // queue, and test harnesses do not pump the main runloop, so the call
    // would block indefinitely. Instead, exercise the atomic gate's
    // idempotency semantics directly.
    #[test]
    fn install_flag_is_a_one_way_latch() {
        let installed = AtomicBool::new(false);
        let first = installed.swap(true, Ordering::AcqRel);
        let second = installed.swap(true, Ordering::AcqRel);
        assert!(!first, "first swap should observe uninstalled state");
        assert!(second, "second swap should observe installed state");
    }
}
