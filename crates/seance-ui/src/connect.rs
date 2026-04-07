// Owns per-window SSH connect attempt tracking so host-row busy state and late completion
// handling stay testable without GPUI wiring.

use std::{collections::HashMap, time::Instant};

use seance_ssh::SshConnectAbortHandle;

pub(crate) type ConnectAttemptId = u64;

pub(crate) struct PendingConnect {
    pub(crate) id: ConnectAttemptId,
    pub(crate) host_scope_key: String,
    pub(crate) vault_id: String,
    pub(crate) host_id: String,
    pub(crate) host_label: String,
    pub(crate) session_id: u64,
    pub(crate) started_at: Instant,
    pub(crate) abort_handle: SshConnectAbortHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingConnectSummary {
    pub(crate) id: ConnectAttemptId,
    pub(crate) host_scope_key: String,
    pub(crate) host_label: String,
}

pub(crate) struct RemovedConnectAttempt {
    pub(crate) pending: PendingConnect,
    pub(crate) was_foreground: bool,
}

pub(crate) struct ConnectAttemptTracker {
    next_attempt_id: ConnectAttemptId,
    by_attempt_id: HashMap<ConnectAttemptId, PendingConnect>,
    by_host_scope_key: HashMap<String, ConnectAttemptId>,
    last_foreground_attempt_id: Option<ConnectAttemptId>,
}

impl Default for ConnectAttemptTracker {
    fn default() -> Self {
        Self {
            next_attempt_id: 1,
            by_attempt_id: HashMap::new(),
            by_host_scope_key: HashMap::new(),
            last_foreground_attempt_id: None,
        }
    }
}

impl ConnectAttemptTracker {
    pub(crate) fn pending(&self, attempt_id: ConnectAttemptId) -> Option<&PendingConnect> {
        self.by_attempt_id.get(&attempt_id)
    }

    pub(crate) fn attempt_id_for_host(&self, host_scope_key: &str) -> Option<ConnectAttemptId> {
        self.by_host_scope_key.get(host_scope_key).copied()
    }

    pub(crate) fn start(&mut self, pending: PendingConnect) -> ConnectAttemptId {
        let attempt_id = pending.id;
        self.by_host_scope_key
            .insert(pending.host_scope_key.clone(), attempt_id);
        self.by_attempt_id.insert(attempt_id, pending);
        self.last_foreground_attempt_id = Some(attempt_id);
        self.next_attempt_id = self.next_attempt_id.max(attempt_id.saturating_add(1));
        attempt_id
    }

    pub(crate) fn next_attempt_id(&mut self) -> ConnectAttemptId {
        let attempt_id = self.next_attempt_id;
        self.next_attempt_id = self.next_attempt_id.saturating_add(1);
        attempt_id
    }

    pub(crate) fn remove(&mut self, attempt_id: ConnectAttemptId) -> Option<RemovedConnectAttempt> {
        let pending = self.by_attempt_id.remove(&attempt_id)?;
        self.by_host_scope_key.remove(&pending.host_scope_key);
        let was_foreground = self.last_foreground_attempt_id == Some(attempt_id);
        if was_foreground {
            self.last_foreground_attempt_id = None;
        }
        Some(RemovedConnectAttempt {
            pending,
            was_foreground,
        })
    }

    pub(crate) fn pending_summaries(&self) -> Vec<PendingConnectSummary> {
        let mut pending = self
            .by_attempt_id
            .values()
            .map(|attempt| PendingConnectSummary {
                id: attempt.id,
                host_scope_key: attempt.host_scope_key.clone(),
                host_label: attempt.host_label.clone(),
            })
            .collect::<Vec<_>>();
        pending.sort_by_key(|attempt| attempt.id);
        pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_connect(id: ConnectAttemptId, host_scope_key: &str, label: &str) -> PendingConnect {
        PendingConnect {
            id,
            host_scope_key: host_scope_key.into(),
            vault_id: "vault".into(),
            host_id: format!("host-{id}"),
            host_label: label.into(),
            session_id: id * 10,
            started_at: Instant::now(),
            abort_handle: SshConnectAbortHandle::default(),
        }
    }

    #[test]
    fn tracker_marks_hosts_independently() {
        let mut tracker = ConnectAttemptTracker::default();
        tracker.start(pending_connect(1, "vault::a", "alpha"));
        tracker.start(pending_connect(2, "vault::b", "beta"));

        assert_eq!(tracker.attempt_id_for_host("vault::a"), Some(1));
        assert_eq!(tracker.attempt_id_for_host("vault::b"), Some(2));
    }

    #[test]
    fn removing_one_attempt_preserves_others() {
        let mut tracker = ConnectAttemptTracker::default();
        tracker.start(pending_connect(1, "vault::a", "alpha"));
        tracker.start(pending_connect(2, "vault::b", "beta"));

        let removed = tracker.remove(1).unwrap();

        assert!(!removed.was_foreground);
        assert_eq!(tracker.attempt_id_for_host("vault::a"), None);
        assert_eq!(tracker.attempt_id_for_host("vault::b"), Some(2));
    }

    #[test]
    fn removing_foreground_attempt_clears_focus_candidate() {
        let mut tracker = ConnectAttemptTracker::default();
        tracker.start(pending_connect(1, "vault::a", "alpha"));
        tracker.start(pending_connect(2, "vault::b", "beta"));

        let removed = tracker.remove(2).unwrap();

        assert!(removed.was_foreground);
        assert_eq!(tracker.attempt_id_for_host("vault::a"), Some(1));
    }

    #[test]
    fn pending_summaries_reflect_active_attempts() {
        let mut tracker = ConnectAttemptTracker::default();
        tracker.start(pending_connect(2, "vault::b", "beta"));
        tracker.start(pending_connect(1, "vault::a", "alpha"));

        let summaries = tracker.pending_summaries();

        assert_eq!(
            summaries,
            vec![
                PendingConnectSummary {
                    id: 1,
                    host_scope_key: "vault::a".into(),
                    host_label: "alpha".into(),
                },
                PendingConnectSummary {
                    id: 2,
                    host_scope_key: "vault::b".into(),
                    host_label: "beta".into(),
                },
            ]
        );
    }

    #[test]
    fn removing_unknown_attempt_is_ignored() {
        let mut tracker = ConnectAttemptTracker::default();
        tracker.start(pending_connect(1, "vault::a", "alpha"));

        assert!(tracker.remove(99).is_none());
        assert_eq!(tracker.attempt_id_for_host("vault::a"), Some(1));
    }
}
