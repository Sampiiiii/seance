// Proxies bytes between local TCP streams and SSH channels for port forwards.

use std::sync::Arc;

use russh::{Channel, ChannelMsg};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use crate::{model::SshError, tunnel::TunnelRegistry};

pub(crate) async fn proxy_tcp_stream_and_channel(
    tunnel_id: &str,
    registry: Arc<TunnelRegistry>,
    mut stream: TcpStream,
    mut channel: Channel<russh::client::Msg>,
) -> Result<(), SshError> {
    let mut stream_closed = false;
    let mut buf = vec![0_u8; 64 * 1024];

    loop {
        tokio::select! {
            result = stream.read(&mut buf), if !stream_closed => {
                match result {
                    Ok(0) => {
                        stream_closed = true;
                        let _ = channel.eof().await;
                    }
                    Ok(read) => {
                        registry.record_bytes_in(tunnel_id, read as u64);
                        channel
                            .data(&buf[..read])
                            .await
                            .map_err(|error| SshError::PortForwardChannel(error.to_string()))?;
                    }
                    Err(error) => {
                        return Err(SshError::PortForwardTargetConnect(error.to_string()));
                    }
                }
            }
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                        registry.record_bytes_out(tunnel_id, data.len() as u64);
                        stream
                            .write_all(&data)
                            .await
                            .map_err(|error| SshError::PortForwardTargetConnect(error.to_string()))?;
                    }
                    Some(ChannelMsg::Eof) => {
                        let _ = stream.shutdown().await;
                        stream_closed = true;
                    }
                    Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
        }
    }

    let _ = channel.eof().await;
    let _ = channel.close().await;
    Ok(())
}
