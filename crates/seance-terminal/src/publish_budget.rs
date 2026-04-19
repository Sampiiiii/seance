//! Time-budget helpers that cap how often the terminal emulator publishes a
//! new viewport snapshot. Both the SSH and local PTY backends drain a burst of
//! incoming data into the emulator and then call `refresh` at most once per
//! [`DEFAULT_PUBLISH_BUDGET`]. This prevents per-chunk refreshes from
//! flooding the UI watcher.

use std::time::Duration;

/// Maximum time we accumulate incoming bytes before publishing a viewport
/// snapshot. Tuned just under a 250 Hz cap so a 120 Hz display always has a
/// fresh frame available while heavy-output bursts don't force one refresh
/// per read from the remote channel.
pub const DEFAULT_PUBLISH_BUDGET: Duration = Duration::from_millis(4);
