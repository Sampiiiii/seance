// Owns session transcript sink creation and append-only file persistence.

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use seance_config::AppConfig;
use seance_terminal::{
    DroppedEventCounter, NoopTranscriptSink, TerminalTranscriptSink, TranscriptEvent,
    TranscriptStream,
};

use crate::SessionKind;

const TRANSCRIPT_CHANNEL_CAPACITY: usize = 512;

#[derive(Debug)]
pub struct SessionLogManager {
    root_dir: PathBuf,
}

impl SessionLogManager {
    pub fn new(root_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root_dir).context("failed to create session log directory")?;
        Ok(Self { root_dir })
    }

    pub fn sink_for_session(
        &self,
        config: &AppConfig,
        kind: SessionKind,
        title: &str,
        host_label: Option<&str>,
    ) -> Arc<dyn TerminalTranscriptSink> {
        if !config.logging.session_transcript_enabled {
            return Arc::new(NoopTranscriptSink);
        }

        match FileTranscriptSink::new(
            &self.root_dir,
            kind,
            title,
            host_label,
            config.logging.session_transcript_max_bytes_per_session,
        ) {
            Ok(sink) => Arc::new(sink),
            Err(error) => {
                tracing::warn!(error = %error, "failed to initialize transcript sink");
                Arc::new(NoopTranscriptSink)
            }
        }
    }
}

#[derive(Debug)]
pub struct FileTranscriptSink {
    tx: SyncSender<TranscriptEvent>,
    dropped_events: Arc<DroppedEventCounter>,
}

impl FileTranscriptSink {
    pub fn new(
        root_dir: &Path,
        kind: SessionKind,
        title: &str,
        host_label: Option<&str>,
        max_bytes_per_session: u64,
    ) -> Result<Self> {
        let started_at = SystemTime::now();
        let timestamp = system_time_seconds(started_at);
        let session_dir =
            root_dir.join(format!("{}-{}", sanitize_path_component(title), timestamp));
        fs::create_dir_all(&session_dir)
            .context("failed to create session transcript directory")?;

        let metadata_path = session_dir.join("metadata.json");
        let events_path = session_dir.join("events.jsonl");
        let metadata = serde_json::json!({
            "kind": match kind {
                SessionKind::Local => "local",
                SessionKind::Remote => "remote",
            },
            "title": title,
            "host_label": host_label,
            "started_at": timestamp,
        });
        fs::write(
            metadata_path,
            serde_json::to_vec_pretty(&metadata)
                .context("failed to serialize transcript metadata")?,
        )
        .context("failed to persist transcript metadata")?;

        let (tx, rx) = mpsc::sync_channel::<TranscriptEvent>(TRANSCRIPT_CHANNEL_CAPACITY);
        let dropped_events = Arc::new(DroppedEventCounter::default());
        let dropped_events_for_thread = Arc::clone(&dropped_events);

        thread::Builder::new()
            .name("seance-session-transcript".to_string())
            .spawn(move || {
                if let Err(error) = write_transcript_events(
                    events_path,
                    rx,
                    max_bytes_per_session,
                    dropped_events_for_thread,
                ) {
                    tracing::warn!(error = %error, "session transcript writer stopped");
                }
            })
            .context("failed to spawn session transcript writer")?;

        Ok(Self { tx, dropped_events })
    }
}

impl TerminalTranscriptSink for FileTranscriptSink {
    fn record(&self, event: TranscriptEvent) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped_events.increment();
            }
        }
    }

    fn dropped_events(&self) -> u64 {
        self.dropped_events.load()
    }
}

fn write_transcript_events(
    events_path: PathBuf,
    rx: mpsc::Receiver<TranscriptEvent>,
    max_bytes_per_session: u64,
    dropped_events: Arc<DroppedEventCounter>,
) -> Result<()> {
    let mut file = File::create(&events_path).context("failed to create transcript events file")?;
    let mut bytes_written = 0_u64;
    let truncated = AtomicBool::new(false);

    while let Ok(event) = rx.recv() {
        if truncated.load(Ordering::Relaxed) {
            continue;
        }

        let record = serde_json::json!({
            "ts": system_time_seconds(event.timestamp),
            "stream": match event.stream {
                TranscriptStream::Input => "input",
                TranscriptStream::Output => "output",
            },
            "payload_base64": BASE64_STANDARD.encode(event.bytes.as_ref()),
        });
        let mut encoded =
            serde_json::to_vec(&record).context("failed to serialize transcript event")?;
        encoded.push(b'\n');

        let next_total = bytes_written.saturating_add(encoded.len() as u64);
        if next_total > max_bytes_per_session {
            truncated.store(true, Ordering::Relaxed);
            dropped_events.increment();
            let marker = serde_json::json!({
                "ts": system_time_seconds(SystemTime::now()),
                "stream": "meta",
                "payload_base64": "",
                "truncated": true,
            });
            let mut encoded = serde_json::to_vec(&marker)
                .context("failed to serialize transcript truncation marker")?;
            encoded.push(b'\n');
            file.write_all(&encoded)
                .context("failed to write transcript truncation marker")?;
            file.flush().ok();
            continue;
        }

        file.write_all(&encoded)
            .context("failed to append transcript event")?;
        bytes_written = next_total;
    }

    file.flush().ok();
    Ok(())
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

fn system_time_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn disabled_logging_returns_noop_sink() {
        let dir = tempdir().unwrap();
        let manager = SessionLogManager::new(dir.path().join("logs")).unwrap();
        let sink =
            manager.sink_for_session(&AppConfig::default(), SessionKind::Local, "local-1", None);

        sink.record(TranscriptEvent {
            timestamp: SystemTime::now(),
            stream: TranscriptStream::Output,
            bytes: Arc::from(b"hello".as_slice()),
        });

        assert_eq!(sink.dropped_events(), 0);
    }
}
