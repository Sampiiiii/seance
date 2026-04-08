// Owns transcript-capture interfaces used by terminal workers without coupling capture to rendering.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptStream {
    Input,
    Output,
}

#[derive(Clone, Debug)]
pub struct TranscriptEvent {
    pub timestamp: SystemTime,
    pub stream: TranscriptStream,
    pub bytes: Arc<[u8]>,
}

pub trait TerminalTranscriptSink: Send + Sync {
    fn record(&self, event: TranscriptEvent);
    fn dropped_events(&self) -> u64 {
        0
    }
}

#[derive(Debug, Default)]
pub struct NoopTranscriptSink;

impl TerminalTranscriptSink for NoopTranscriptSink {
    fn record(&self, _event: TranscriptEvent) {}
}

#[derive(Debug, Default)]
pub struct DroppedEventCounter(AtomicU64);

impl DroppedEventCounter {
    pub fn increment(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    pub fn load(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}
