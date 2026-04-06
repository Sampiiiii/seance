use anyhow::anyhow;
use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, client};
use seance_terminal::{SharedSessionState, TerminalEmulator, TerminalGeometry};
use tokio::{io::AsyncWriteExt, sync::mpsc};

use crate::{auth::AcceptAnyHostKeyHandler, session::SessionCommand};

pub(crate) async fn run_ssh_session(
    _session: client::Handle<AcceptAnyHostKeyHandler>,
    channel: Channel<russh::client::Msg>,
    state: SharedSessionState,
    geometry: TerminalGeometry,
    mut command_rx: mpsc::UnboundedReceiver<SessionCommand>,
) {
    let mut emulator = match TerminalEmulator::new(geometry) {
        Ok(emulator) => emulator,
        Err(error) => {
            state.set_error(&error);
            return;
        }
    };
    emulator.publish(&state, None);

    let (mut read_half, write_half): (ChannelReadHalf, ChannelWriteHalf<russh::client::Msg>) =
        channel.split();
    let mut writer = write_half.make_writer();
    let mut exit_status = None;

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                match command {
                    SessionCommand::Input(bytes) => {
                        if let Err(error) = writer.write_all(&bytes).await {
                            state.set_error(&anyhow!("failed to write to SSH channel: {error}"));
                            break;
                        }
                        let _ = writer.flush().await;
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
                    }
                }
            }
            msg = read_half.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        emulator.write(&data);
                        emulator.publish(&state, exit_status.clone());
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        emulator.write(&data);
                        emulator.publish(&state, exit_status.clone());
                    }
                    Some(ChannelMsg::ExitStatus { exit_status: code }) => {
                        exit_status = Some(format!("remote exited with status {code}"));
                        emulator.publish(&state, exit_status.clone());
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        exit_status = Some(format!("remote exited via signal {signal_name:?}"));
                        emulator.publish(&state, exit_status.clone());
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                        emulator.publish(
                            &state,
                            Some(exit_status.unwrap_or_else(|| "remote session closed".into())),
                        );
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}
