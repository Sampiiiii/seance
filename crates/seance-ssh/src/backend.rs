use std::{sync::Arc, time::SystemTime};

use anyhow::anyhow;
use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, client};
use seance_terminal::{
    SharedSessionState, TerminalEmulator, TerminalGeometry, TerminalScrollCommand,
    TerminalTranscriptSink, TranscriptEvent, TranscriptStream,
    publish_budget::DEFAULT_PUBLISH_BUDGET,
};
use tokio::{io::AsyncWriteExt, sync::mpsc, time::Instant as TokioInstant};
use tracing::trace;

use crate::{auth::SshClientHandler, session::SessionCommand};

pub(crate) async fn run_ssh_session(
    _session: client::Handle<SshClientHandler>,
    channel: Channel<russh::client::Msg>,
    state: SharedSessionState,
    geometry: TerminalGeometry,
    mut command_rx: mpsc::UnboundedReceiver<SessionCommand>,
    transcript_sink: Arc<dyn TerminalTranscriptSink>,
) {
    let mut emulator = match TerminalEmulator::new(geometry, "Connecting SSH session...") {
        Ok(emulator) => emulator,
        Err(error) => {
            state.set_error(&error, geometry);
            return;
        }
    };
    emulator.refresh(&state, None, true, transcript_sink.dropped_events());

    let (mut read_half, write_half): (ChannelReadHalf, ChannelWriteHalf<russh::client::Msg>) =
        channel.split();
    let mut writer = write_half.make_writer();
    let mut exit_status = None;
    let mut pending_command: Option<SessionCommand> = None;

    loop {
        if let Some(command) = pending_command.take() {
            if !apply_session_command(
                command,
                &state,
                geometry,
                &mut emulator,
                &write_half,
                &mut writer,
                &transcript_sink,
                &mut command_rx,
                &mut pending_command,
                &exit_status,
            )
            .await
            {
                break;
            }
            continue;
        }

        tokio::select! {
            Some(command) = command_rx.recv() => {
                if !apply_session_command(
                    command,
                    &state,
                    geometry,
                    &mut emulator,
                    &write_half,
                    &mut writer,
                    &transcript_sink,
                    &mut command_rx,
                    &mut pending_command,
                    &exit_status,
                )
                .await
                {
                    break;
                }
            }
            msg = read_half.wait() => {
                let mut wrote_output = false;
                let mut batch_count: usize = 0;
                let mut deferred_control: Option<ChannelMsg> = None;
                let mut channel_closed = false;

                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        record_and_write_output(&transcript_sink, &mut emulator, &data);
                        wrote_output = true;
                        batch_count = 1;
                        let (extra, deferred, closed) = drain_data_burst(
                            &mut read_half,
                            &transcript_sink,
                            &mut emulator,
                        )
                        .await;
                        batch_count += extra;
                        deferred_control = deferred;
                        channel_closed = closed;
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        record_and_write_output(&transcript_sink, &mut emulator, &data);
                        wrote_output = true;
                        batch_count = 1;
                        let (extra, deferred, closed) = drain_data_burst(
                            &mut read_half,
                            &transcript_sink,
                            &mut emulator,
                        )
                        .await;
                        batch_count += extra;
                        deferred_control = deferred;
                        channel_closed = closed;
                    }
                    Some(other) => {
                        deferred_control = Some(other);
                    }
                    None => {
                        channel_closed = true;
                    }
                }

                if wrote_output {
                    emulator.refresh(
                        &state,
                        exit_status.clone(),
                        false,
                        transcript_sink.dropped_events(),
                    );
                    trace!(batch_count, "published coalesced ssh data refresh");
                }

                match deferred_control {
                    Some(ChannelMsg::ExitStatus { exit_status: code }) => {
                        exit_status = Some(format!("remote exited with status {code}"));
                        emulator.refresh(
                            &state,
                            exit_status.clone(),
                            true,
                            transcript_sink.dropped_events(),
                        );
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        exit_status = Some(format!("remote exited via signal {signal_name:?}"));
                        emulator.refresh(
                            &state,
                            exit_status.clone(),
                            true,
                            transcript_sink.dropped_events(),
                        );
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => {
                        channel_closed = true;
                    }
                    _ => {}
                }

                if channel_closed {
                    emulator.refresh(
                        &state,
                        Some(exit_status.unwrap_or_else(|| "remote session closed".into())),
                        true,
                        transcript_sink.dropped_events(),
                    );
                    break;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_session_command(
    command: SessionCommand,
    state: &SharedSessionState,
    geometry: TerminalGeometry,
    emulator: &mut TerminalEmulator,
    write_half: &ChannelWriteHalf<russh::client::Msg>,
    writer: &mut (impl AsyncWriteExt + Unpin),
    transcript_sink: &Arc<dyn TerminalTranscriptSink>,
    command_rx: &mut mpsc::UnboundedReceiver<SessionCommand>,
    pending_command: &mut Option<SessionCommand>,
    exit_status: &Option<String>,
) -> bool {
    match command {
        SessionCommand::Input(bytes) => {
            emulator.track_input_bytes(&bytes);
            if let Err(error) = write_input_bytes(writer, transcript_sink, &bytes).await {
                state.set_error(
                    &anyhow!("failed to write to SSH channel: {error}"),
                    geometry,
                );
                return false;
            }
        }
        SessionCommand::Text(event) => {
            let bytes = emulator.encode_text_event(&event);
            if !bytes.is_empty() {
                emulator.track_input_bytes(&bytes);
                if let Err(error) = write_input_bytes(writer, transcript_sink, &bytes).await {
                    state.set_error(
                        &anyhow!("failed to write to SSH channel: {error}"),
                        geometry,
                    );
                    return false;
                }
            }
        }
        SessionCommand::Key(event) => {
            let bytes = match emulator.encode_key_event(&event) {
                Ok(bytes) => bytes,
                Err(error) => {
                    state.set_error(&error, geometry);
                    return false;
                }
            };
            if !bytes.is_empty() {
                emulator.track_input_bytes(&bytes);
                if let Err(error) = write_input_bytes(writer, transcript_sink, &bytes).await {
                    state.set_error(
                        &anyhow!("failed to write to SSH channel: {error}"),
                        geometry,
                    );
                    return false;
                }
            }
        }
        SessionCommand::Mouse(event) => {
            let bytes = match emulator.encode_mouse_event(&event) {
                Ok(bytes) => bytes,
                Err(error) => {
                    state.set_error(&error, geometry);
                    return false;
                }
            };
            if !bytes.is_empty()
                && let Err(error) = write_input_bytes(writer, transcript_sink, &bytes).await
            {
                state.set_error(
                    &anyhow!("failed to write to SSH channel: {error}"),
                    geometry,
                );
                return false;
            }
        }
        SessionCommand::Paste(paste) => {
            let bytes = emulator.encode_paste(&paste);
            if !bytes.is_empty() {
                emulator.track_input_bytes(&bytes);
                if let Err(error) = write_input_bytes(writer, transcript_sink, &bytes).await {
                    state.set_error(
                        &anyhow!("failed to write to SSH channel: {error}"),
                        geometry,
                    );
                    return false;
                }
            }
        }
        SessionCommand::Resize(geometry) => {
            let _ = write_half
                .window_change(
                    u32::from(geometry.size.cols),
                    u32::from(geometry.size.rows),
                    u32::from(geometry.pixel_size.width_px),
                    u32::from(geometry.pixel_size.height_px),
                )
                .await;
            let _ = emulator.resize(geometry);
            emulator.refresh(
                state,
                exit_status.clone(),
                true,
                transcript_sink.dropped_events(),
            );
        }
        command @ (SessionCommand::ScrollViewport(_) | SessionCommand::ScrollToBottom) => {
            let (compacted_batch, deferred_command, disconnected, drained_count) =
                drain_scroll_command_batch(command, command_rx);
            *pending_command = deferred_command;
            let compacted_count = compacted_batch.len();

            for batch_command in compacted_batch {
                match batch_command {
                    SessionCommand::ScrollViewport(scroll_command) => {
                        emulator.scroll_viewport(scroll_command);
                    }
                    SessionCommand::ScrollToBottom => {
                        emulator.scroll_viewport(TerminalScrollCommand::Bottom);
                    }
                    _ => {}
                }
            }

            emulator.refresh(
                state,
                exit_status.clone(),
                false,
                transcript_sink.dropped_events(),
            );
            trace!(
                drained_count,
                compacted_count, disconnected, "published coalesced ssh scroll refresh"
            );
            if disconnected {
                return false;
            }
        }
        SessionCommand::CopyActiveScreen { reply_tx } => {
            let _ = reply_tx.send(emulator.copy_active_screen_plain_text_readable());
        }
        SessionCommand::CopySelectionText {
            selection,
            reply_tx,
        } => {
            let _ = reply_tx.send(emulator.copy_selection_text(selection));
        }
        SessionCommand::PreviousTurn { reply_tx } => {
            let _ = reply_tx.send(Ok(emulator.previous_turn()));
        }
    }

    true
}

fn is_scroll_session_command(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::ScrollViewport(_) | SessionCommand::ScrollToBottom
    )
}

fn compact_scroll_command_burst(commands: Vec<SessionCommand>) -> Vec<SessionCommand> {
    let mut compacted = Vec::with_capacity(commands.len());
    let mut pending_delta: Option<isize> = None;

    for command in commands {
        match command {
            SessionCommand::ScrollViewport(TerminalScrollCommand::DeltaRows(delta_rows)) => {
                pending_delta = Some(pending_delta.unwrap_or_default().saturating_add(delta_rows));
            }
            other => {
                if let Some(delta_rows) = pending_delta.take() {
                    compacted.push(SessionCommand::ScrollViewport(
                        TerminalScrollCommand::DeltaRows(delta_rows),
                    ));
                }
                compacted.push(other);
            }
        }
    }

    if let Some(delta_rows) = pending_delta.take() {
        compacted.push(SessionCommand::ScrollViewport(
            TerminalScrollCommand::DeltaRows(delta_rows),
        ));
    }

    compacted
}

fn drain_scroll_command_batch(
    first: SessionCommand,
    command_rx: &mut mpsc::UnboundedReceiver<SessionCommand>,
) -> (Vec<SessionCommand>, Option<SessionCommand>, bool, usize) {
    let mut drained = vec![first];
    let mut deferred_command = None;
    let mut disconnected = false;

    loop {
        match command_rx.try_recv() {
            Ok(command) if is_scroll_session_command(&command) => {
                drained.push(command);
            }
            Ok(command) => {
                deferred_command = Some(command);
                break;
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                disconnected = true;
                break;
            }
        }
    }

    let drained_count = drained.len();
    (
        compact_scroll_command_burst(drained),
        deferred_command,
        disconnected,
        drained_count,
    )
}

fn record_and_write_output(
    transcript_sink: &Arc<dyn TerminalTranscriptSink>,
    emulator: &mut TerminalEmulator,
    data: &[u8],
) {
    transcript_sink.record(TranscriptEvent {
        timestamp: SystemTime::now(),
        stream: TranscriptStream::Output,
        bytes: Arc::from(data),
    });
    emulator.write(data);
}

/// Greedily pulls additional `Data`/`ExtendedData` frames from `read_half` into
/// the emulator, bounded by [`DEFAULT_PUBLISH_BUDGET`]. Returns the number of
/// additional data chunks consumed, an optional deferred non-data control
/// message (that still needs dispatch by the caller), and whether the channel
/// closed while we were draining.
async fn drain_data_burst(
    read_half: &mut ChannelReadHalf,
    transcript_sink: &Arc<dyn TerminalTranscriptSink>,
    emulator: &mut TerminalEmulator,
) -> (usize, Option<ChannelMsg>, bool) {
    let deadline = TokioInstant::now() + DEFAULT_PUBLISH_BUDGET;
    let mut extra: usize = 0;

    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                return (extra, None, false);
            }
            msg = read_half.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        record_and_write_output(transcript_sink, emulator, &data);
                        extra += 1;
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        record_and_write_output(transcript_sink, emulator, &data);
                        extra += 1;
                    }
                    Some(other) => return (extra, Some(other), false),
                    None => return (extra, None, true),
                }
            }
        }
    }
}

async fn write_input_bytes(
    writer: &mut (impl AsyncWriteExt + Unpin),
    transcript_sink: &Arc<dyn TerminalTranscriptSink>,
    bytes: &[u8],
) -> std::io::Result<()> {
    transcript_sink.record(TranscriptEvent {
        timestamp: SystemTime::now(),
        stream: TranscriptStream::Input,
        bytes: Arc::from(bytes),
    });
    writer.write_all(bytes).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_delta_rows_compact_with_saturating_add() {
        let compacted = compact_scroll_command_burst(vec![
            SessionCommand::ScrollViewport(TerminalScrollCommand::DeltaRows(isize::MAX)),
            SessionCommand::ScrollViewport(TerminalScrollCommand::DeltaRows(2)),
            SessionCommand::ScrollViewport(TerminalScrollCommand::Top),
            SessionCommand::ScrollViewport(TerminalScrollCommand::DeltaRows(-3)),
            SessionCommand::ScrollViewport(TerminalScrollCommand::DeltaRows(-4)),
        ]);

        let mut iter = compacted.into_iter();
        assert!(matches!(
            iter.next(),
            Some(SessionCommand::ScrollViewport(
                TerminalScrollCommand::DeltaRows(delta)
            )) if delta == isize::MAX
        ));
        assert!(matches!(
            iter.next(),
            Some(SessionCommand::ScrollViewport(TerminalScrollCommand::Top))
        ));
        assert!(matches!(
            iter.next(),
            Some(SessionCommand::ScrollViewport(
                TerminalScrollCommand::DeltaRows(delta)
            )) if delta == -7
        ));
        assert!(iter.next().is_none());
    }

    #[test]
    fn mixed_order_is_preserved_when_compacting_delta_runs() {
        let compacted = compact_scroll_command_burst(vec![
            SessionCommand::ScrollViewport(TerminalScrollCommand::DeltaRows(3)),
            SessionCommand::Input(vec![0x0d]),
            SessionCommand::ScrollViewport(TerminalScrollCommand::DeltaRows(1)),
            SessionCommand::ScrollViewport(TerminalScrollCommand::DeltaRows(2)),
            SessionCommand::ScrollToBottom,
            SessionCommand::ScrollViewport(TerminalScrollCommand::DeltaRows(-1)),
        ]);

        let mut iter = compacted.into_iter();
        assert!(matches!(
            iter.next(),
            Some(SessionCommand::ScrollViewport(
                TerminalScrollCommand::DeltaRows(delta)
            )) if delta == 3
        ));
        assert!(matches!(
            iter.next(),
            Some(SessionCommand::Input(bytes)) if bytes == vec![0x0d]
        ));
        assert!(matches!(
            iter.next(),
            Some(SessionCommand::ScrollViewport(
                TerminalScrollCommand::DeltaRows(delta)
            )) if delta == 3
        ));
        assert!(matches!(iter.next(), Some(SessionCommand::ScrollToBottom)));
        assert!(matches!(
            iter.next(),
            Some(SessionCommand::ScrollViewport(
                TerminalScrollCommand::DeltaRows(delta)
            )) if delta == -1
        ));
        assert!(iter.next().is_none());
    }

    #[test]
    fn draining_scroll_batch_stops_at_first_non_scroll_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(SessionCommand::ScrollViewport(
            TerminalScrollCommand::DeltaRows(2),
        ))
        .expect("send first scroll");
        tx.send(SessionCommand::ScrollViewport(
            TerminalScrollCommand::DeltaRows(4),
        ))
        .expect("send second scroll");
        tx.send(SessionCommand::Input(vec![0x7f]))
            .expect("send non-scroll");

        let first = rx.try_recv().expect("receive first");
        let (compacted, deferred, disconnected, drained_count) =
            drain_scroll_command_batch(first, &mut rx);

        assert_eq!(drained_count, 2);
        assert!(!disconnected);
        assert!(matches!(
            compacted.as_slice(),
            [SessionCommand::ScrollViewport(
                TerminalScrollCommand::DeltaRows(6)
            )]
        ));
        assert!(matches!(
            deferred,
            Some(SessionCommand::Input(bytes)) if bytes == vec![0x7f]
        ));
    }
}
