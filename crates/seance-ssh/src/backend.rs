use std::{sync::Arc, time::SystemTime};

use anyhow::anyhow;
use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, client};
use seance_terminal::{
    SharedSessionState, TerminalEmulator, TerminalGeometry, TerminalScrollCommand,
    TerminalTranscriptSink, TranscriptEvent, TranscriptStream,
};
use tokio::{io::AsyncWriteExt, sync::mpsc};

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

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                match command {
                    SessionCommand::Input(bytes) => {
                        if let Err(error) = write_input_bytes(&mut writer, &transcript_sink, &bytes).await {
                            state.set_error(&anyhow!("failed to write to SSH channel: {error}"), geometry);
                            break;
                        }
                    }
                    SessionCommand::Text(event) => {
                        let bytes = emulator.encode_text_event(&event);
                        if !bytes.is_empty()
                            && let Err(error) = write_input_bytes(&mut writer, &transcript_sink, &bytes).await
                        {
                            state.set_error(&anyhow!("failed to write to SSH channel: {error}"), geometry);
                            break;
                        }
                    }
                    SessionCommand::Key(event) => {
                        let bytes = match emulator.encode_key_event(&event) {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                state.set_error(&error, geometry);
                                break;
                            }
                        };
                        if !bytes.is_empty()
                            && let Err(error) = write_input_bytes(&mut writer, &transcript_sink, &bytes).await
                        {
                            state.set_error(&anyhow!("failed to write to SSH channel: {error}"), geometry);
                            break;
                        }
                    }
                    SessionCommand::Mouse(event) => {
                        let bytes = match emulator.encode_mouse_event(&event) {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                state.set_error(&error, geometry);
                                break;
                            }
                        };
                        if !bytes.is_empty()
                            && let Err(error) = write_input_bytes(&mut writer, &transcript_sink, &bytes).await
                        {
                            state.set_error(&anyhow!("failed to write to SSH channel: {error}"), geometry);
                            break;
                        }
                    }
                    SessionCommand::Paste(paste) => {
                        let bytes = emulator.encode_paste(&paste);
                        if !bytes.is_empty()
                            && let Err(error) = write_input_bytes(&mut writer, &transcript_sink, &bytes).await
                        {
                            state.set_error(&anyhow!("failed to write to SSH channel: {error}"), geometry);
                            break;
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
                        emulator.refresh(&state, exit_status.clone(), true, transcript_sink.dropped_events());
                    }
                    SessionCommand::ScrollViewport(command) => {
                        emulator.scroll_viewport(command);
                        emulator.refresh(
                            &state,
                            exit_status.clone(),
                            false,
                            transcript_sink.dropped_events(),
                        );
                    }
                    SessionCommand::ScrollToBottom => {
                        emulator.scroll_viewport(TerminalScrollCommand::Bottom);
                        emulator.refresh(
                            &state,
                            exit_status.clone(),
                            false,
                            transcript_sink.dropped_events(),
                        );
                    }
                }
            }
            msg = read_half.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        transcript_sink.record(TranscriptEvent {
                            timestamp: SystemTime::now(),
                            stream: TranscriptStream::Output,
                            bytes: Arc::from(data.as_ref()),
                        });
                        emulator.write(&data);
                        emulator.refresh(&state, exit_status.clone(), false, transcript_sink.dropped_events());
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        transcript_sink.record(TranscriptEvent {
                            timestamp: SystemTime::now(),
                            stream: TranscriptStream::Output,
                            bytes: Arc::from(data.as_ref()),
                        });
                        emulator.write(&data);
                        emulator.refresh(&state, exit_status.clone(), false, transcript_sink.dropped_events());
                    }
                    Some(ChannelMsg::ExitStatus { exit_status: code }) => {
                        exit_status = Some(format!("remote exited with status {code}"));
                        emulator.refresh(&state, exit_status.clone(), true, transcript_sink.dropped_events());
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        exit_status = Some(format!("remote exited via signal {signal_name:?}"));
                        emulator.refresh(&state, exit_status.clone(), true, transcript_sink.dropped_events());
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                        emulator.refresh(
                            &state,
                            Some(exit_status.unwrap_or_else(|| "remote session closed".into())),
                            true,
                            transcript_sink.dropped_events(),
                        );
                        break;
                    }
                    _ => {}
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
