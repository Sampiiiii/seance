use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::mpsc::Sender,
    thread,
};

use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use seance_core::AppPaths;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcRequest {
    Ping,
    OpenWindow,
    OpenHost { host_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcResponse {
    Ok,
    Error { message: String },
}

#[derive(Debug, Clone)]
pub enum PlatformEvent {
    OpenWindow,
    OpenHost { host_id: String },
}

pub trait PlatformApp {
    fn on_launch(&mut self) -> anyhow::Result<()>;
    fn on_reopen_requested(&mut self) -> anyhow::Result<()>;
    fn on_last_window_closed(&mut self) -> anyhow::Result<seance_core::PlatformCloseAction>;
    fn open_window(&mut self) -> anyhow::Result<()>;
    fn show_app(&mut self) -> anyhow::Result<()>;
    fn hide_app(&mut self) -> anyhow::Result<()>;
}

pub trait PlatformRuntime {
    fn run(self, app: Box<dyn PlatformApp>) -> anyhow::Result<()>;
}

pub struct InstanceGuard {
    _lock_file: File,
}

pub enum InstanceStartup {
    Primary(InstanceGuard),
    Secondary(IpcResponse),
}

pub fn acquire_or_notify(paths: &AppPaths, request: IpcRequest) -> Result<InstanceStartup> {
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&paths.instance_lock_path)
        .with_context(|| {
            format!(
                "failed to open lock file at {}",
                paths.instance_lock_path.display()
            )
        })?;

    match lock_file.try_lock_exclusive() {
        Ok(()) => Ok(InstanceStartup::Primary(InstanceGuard {
            _lock_file: lock_file,
        })),
        Err(_) => Ok(InstanceStartup::Secondary(send_ipc_request(
            &paths.ipc_socket_path,
            &request,
        )?)),
    }
}

pub fn start_ipc_server(socket_path: &Path, events: Sender<PlatformEvent>) -> Result<()> {
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind IPC socket at {}", socket_path.display()))?;
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                continue;
            };
            let _ = handle_ipc_client(stream, &events);
        }
    });
    Ok(())
}

fn handle_ipc_client(mut stream: UnixStream, events: &Sender<PlatformEvent>) -> Result<()> {
    let request = decode_request(BufReader::new(stream.try_clone()?))?;
    let event = match request {
        IpcRequest::Ping => None,
        IpcRequest::OpenWindow => Some(PlatformEvent::OpenWindow),
        IpcRequest::OpenHost { host_id } => Some(PlatformEvent::OpenHost { host_id }),
    };
    if let Some(event) = event {
        events
            .send(event)
            .map_err(|_| anyhow!("failed to deliver IPC event to UI runtime"))?;
    }
    encode_response(&mut stream, &IpcResponse::Ok)
}

pub fn send_ipc_request(socket_path: &Path, request: &IpcRequest) -> Result<IpcResponse> {
    let mut stream = UnixStream::connect(socket_path).with_context(|| {
        format!(
            "failed to connect to IPC socket at {}",
            socket_path.display()
        )
    })?;
    encode_request(&mut stream, request)?;
    decode_response(BufReader::new(stream))
}

pub fn encode_request(writer: &mut impl Write, request: &IpcRequest) -> Result<()> {
    serde_json::to_writer(&mut *writer, request)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub fn encode_response(writer: &mut impl Write, response: &IpcResponse) -> Result<()> {
    serde_json::to_writer(&mut *writer, response)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub fn decode_request(reader: impl BufRead) -> Result<IpcRequest> {
    decode_line(reader)
}

pub fn decode_response(reader: impl BufRead) -> Result<IpcResponse> {
    decode_line(reader)
}

fn decode_line<T: for<'de> Deserialize<'de>>(mut reader: impl BufRead) -> Result<T> {
    let mut line = String::new();
    let len = reader.read_line(&mut line)?;
    if len == 0 {
        return Err(anyhow!("empty IPC payload"));
    }
    Ok(serde_json::from_str(line.trim_end())?)
}

#[cfg(test)]
mod tests {
    use super::{
        IpcRequest, IpcResponse, decode_request, decode_response, encode_request, encode_response,
    };

    #[test]
    fn open_window_round_trips() {
        let mut bytes = Vec::new();
        encode_request(&mut bytes, &IpcRequest::OpenWindow).unwrap();
        assert_eq!(
            decode_request(bytes.as_slice()).unwrap(),
            IpcRequest::OpenWindow
        );
    }

    #[test]
    fn malformed_payload_returns_error() {
        let err = decode_request("not-json\n".as_bytes()).unwrap_err();
        assert!(err.to_string().contains("expected"));
    }

    #[test]
    fn ok_response_round_trips() {
        let mut bytes = Vec::new();
        encode_response(&mut bytes, &IpcResponse::Ok).unwrap();
        assert_eq!(decode_response(bytes.as_slice()).unwrap(), IpcResponse::Ok);
    }
}
