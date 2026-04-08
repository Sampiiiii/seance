// Owns SSH tunnel runtime state, status broadcasting, and local/remote forwarding loops.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc::Receiver, mpsc::Sender},
    time::{SystemTime, UNIX_EPOCH},
};

use russh::{Disconnect, client};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex as AsyncMutex, mpsc, oneshot},
};

use crate::{
    auth::{ForwardedTcpIpChannel, SshClientHandler, authenticate},
    model::{
        PortForwardRuntimeSnapshot, PortForwardStatus, SshError, SshPortForwardHandle,
        SshPortForwardMode, SshPortForwardRequest,
    },
    proxy::proxy_tcp_stream_and_channel,
};

pub(crate) struct TunnelRegistry {
    snapshots: Mutex<HashMap<String, PortForwardRuntimeSnapshot>>,
    handles: Mutex<HashMap<String, SshPortForwardHandle>>,
    subscribers: Mutex<Vec<Sender<Vec<PortForwardRuntimeSnapshot>>>>,
}

impl TunnelRegistry {
    pub(crate) fn new() -> Self {
        Self {
            snapshots: Mutex::new(HashMap::new()),
            handles: Mutex::new(HashMap::new()),
            subscribers: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn list(&self) -> Vec<PortForwardRuntimeSnapshot> {
        let mut snapshots = self
            .snapshots
            .lock()
            .expect("tunnel snapshots poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.label.to_lowercase().cmp(&right.label.to_lowercase()));
        snapshots
    }

    pub(crate) fn subscribe(&self) -> Receiver<Vec<PortForwardRuntimeSnapshot>> {
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut subscribers = self
                .subscribers
                .lock()
                .expect("tunnel subscribers poisoned");
            subscribers.push(tx.clone());
        }
        let _ = tx.send(self.list());
        rx
    }

    pub(crate) fn has_active_handle(&self, id: &str) -> bool {
        self.handles
            .lock()
            .expect("tunnel handles poisoned")
            .contains_key(id)
    }

    pub(crate) fn insert_handle(
        &self,
        id: String,
        handle: SshPortForwardHandle,
    ) -> Result<(), SshError> {
        let mut handles = self.handles.lock().expect("tunnel handles poisoned");
        if handles.contains_key(&id) {
            return Err(SshError::PortForwardAlreadyRunning(id));
        }
        handles.insert(id, handle);
        Ok(())
    }

    pub(crate) fn remove_handle(&self, id: &str) {
        self.handles
            .lock()
            .expect("tunnel handles poisoned")
            .remove(id);
    }

    pub(crate) fn stop(&self, id: &str) -> bool {
        if let Some(handle) = self
            .handles
            .lock()
            .expect("tunnel handles poisoned")
            .remove(id)
        {
            return handle.abort();
        }

        let removed = self
            .snapshots
            .lock()
            .expect("tunnel snapshots poisoned")
            .remove(id)
            .is_some();
        if removed {
            self.broadcast();
        }
        removed
    }

    pub(crate) fn upsert_starting(
        &self,
        request: &SshPortForwardRequest,
    ) -> PortForwardRuntimeSnapshot {
        let snapshot = PortForwardRuntimeSnapshot {
            id: request.id.clone(),
            vault_id: request.vault_id.clone(),
            forward_id: request.forward_id.clone(),
            host_id: request.host_id.clone(),
            label: request.label.clone(),
            host_label: request.host_label.clone(),
            mode: request.mode,
            status: PortForwardStatus::Starting,
            listen_address: request.listen_address.clone(),
            listen_port: request.listen_port,
            target_address: request.target_address.clone(),
            target_port: request.target_port,
            opened_at: None,
            active_connections: 0,
            bytes_in: 0,
            bytes_out: 0,
            last_error: None,
        };
        self.snapshots
            .lock()
            .expect("tunnel snapshots poisoned")
            .insert(request.id.clone(), snapshot.clone());
        self.broadcast();
        snapshot
    }

    pub(crate) fn mark_running(&self, id: &str, listen_port: u16) {
        self.update(id, |snapshot| {
            snapshot.status = PortForwardStatus::Running;
            snapshot.listen_port = listen_port;
            snapshot.opened_at = Some(now_ts());
            snapshot.last_error = None;
        });
    }

    pub(crate) fn mark_failed(&self, id: &str, error: String) {
        self.update(id, |snapshot| {
            snapshot.status = PortForwardStatus::Failed;
            snapshot.last_error = Some(error);
            snapshot.active_connections = 0;
        });
    }

    pub(crate) fn note_error(&self, id: &str, error: String) {
        self.update(id, |snapshot| {
            snapshot.last_error = Some(error);
        });
    }

    pub(crate) fn remove_snapshot(&self, id: &str) {
        self.snapshots
            .lock()
            .expect("tunnel snapshots poisoned")
            .remove(id);
        self.broadcast();
    }

    pub(crate) fn connection_opened(&self, id: &str) {
        self.update(id, |snapshot| {
            snapshot.active_connections = snapshot.active_connections.saturating_add(1);
        });
    }

    pub(crate) fn connection_closed(&self, id: &str) {
        self.update(id, |snapshot| {
            snapshot.active_connections = snapshot.active_connections.saturating_sub(1);
        });
    }

    pub(crate) fn record_bytes_in(&self, id: &str, bytes: u64) {
        self.update(id, |snapshot| {
            snapshot.bytes_in = snapshot.bytes_in.saturating_add(bytes);
        });
    }

    pub(crate) fn record_bytes_out(&self, id: &str, bytes: u64) {
        self.update(id, |snapshot| {
            snapshot.bytes_out = snapshot.bytes_out.saturating_add(bytes);
        });
    }

    fn update(&self, id: &str, update: impl FnOnce(&mut PortForwardRuntimeSnapshot)) {
        if let Some(snapshot) = self
            .snapshots
            .lock()
            .expect("tunnel snapshots poisoned")
            .get_mut(id)
        {
            update(snapshot);
        }
        self.broadcast();
    }

    fn broadcast(&self) {
        let snapshots = self.list();
        self.subscribers
            .lock()
            .expect("tunnel subscribers poisoned")
            .retain(|subscriber| subscriber.send(snapshots.clone()).is_ok());
    }
}

pub(crate) async fn run_port_forward(
    request: SshPortForwardRequest,
    registry: Arc<TunnelRegistry>,
    mut stop_rx: oneshot::Receiver<()>,
) -> Result<(), SshError> {
    match request.mode {
        SshPortForwardMode::Local => run_local_port_forward(request, registry, &mut stop_rx).await,
        SshPortForwardMode::Remote => {
            run_remote_port_forward(request, registry, &mut stop_rx).await
        }
    }
}

async fn run_local_port_forward(
    request: SshPortForwardRequest,
    registry: Arc<TunnelRegistry>,
    stop_rx: &mut oneshot::Receiver<()>,
) -> Result<(), SshError> {
    let handle = connect_handle(&request, SshClientHandler::default()).await?;
    let handle = Arc::new(AsyncMutex::new(handle));
    let listener = TcpListener::bind((request.listen_address.as_str(), request.listen_port))
        .await
        .map_err(|error| SshError::PortForwardBind(error.to_string()))?;
    registry.mark_running(&request.id, request.listen_port);

    loop {
        tokio::select! {
            _ = &mut *stop_rx => {
                disconnect_handle(&handle, None).await;
                return Ok(());
            }
            accept_result = listener.accept() => {
                let (stream, peer_addr) = accept_result
                    .map_err(|error| SshError::PortForwardBind(error.to_string()))?;
                let tunnel_id = request.id.clone();
                let registry = Arc::clone(&registry);
                let handle = Arc::clone(&handle);
                let target_address = request.target_address.clone();
                let target_port = request.target_port;
                tokio::spawn(async move {
                    registry.connection_opened(&tunnel_id);
                    let open_result = {
                        let handle = handle.lock().await;
                        handle
                            .channel_open_direct_tcpip(
                                target_address.clone(),
                                u32::from(target_port),
                                peer_addr.ip().to_string(),
                                peer_addr.port().into(),
                            )
                            .await
                    };
                    match open_result {
                        Ok(channel) => {
                            if let Err(error) = proxy_tcp_stream_and_channel(
                                &tunnel_id,
                                Arc::clone(&registry),
                                stream,
                                channel,
                            )
                            .await
                            {
                                registry.note_error(&tunnel_id, error.to_string());
                            }
                        }
                        Err(error) => {
                            registry.note_error(
                                &tunnel_id,
                                SshError::PortForwardChannel(error.to_string()).to_string(),
                            );
                        }
                    }
                    registry.connection_closed(&tunnel_id);
                });
            }
        }
    }
}

async fn run_remote_port_forward(
    request: SshPortForwardRequest,
    registry: Arc<TunnelRegistry>,
    stop_rx: &mut oneshot::Receiver<()>,
) -> Result<(), SshError> {
    let (forwarded_tx, mut forwarded_rx) = mpsc::unbounded_channel::<ForwardedTcpIpChannel>();
    let handle = connect_handle(
        &request,
        SshClientHandler::with_forwarded_tcpip(forwarded_tx),
    )
    .await?;
    let assigned_port = handle
        .tcpip_forward(
            request.listen_address.clone(),
            u32::from(request.listen_port),
        )
        .await
        .map_err(|error| SshError::PortForwardChannel(error.to_string()))?;
    registry.mark_running(&request.id, assigned_port as u16);
    let handle = Arc::new(AsyncMutex::new(handle));

    loop {
        tokio::select! {
            _ = &mut *stop_rx => {
                disconnect_handle(&handle, Some((request.listen_address.clone(), assigned_port))).await;
                return Ok(());
            }
            forwarded = forwarded_rx.recv() => {
                let Some(forwarded) = forwarded else {
                    return Err(SshError::Transport("remote port forward session ended".into()));
                };
                let tunnel_id = request.id.clone();
                let registry = Arc::clone(&registry);
                let target_address = request.target_address.clone();
                let target_port = request.target_port;
                tokio::spawn(async move {
                    registry.connection_opened(&tunnel_id);
                    match TcpStream::connect((target_address.as_str(), target_port)).await {
                        Ok(stream) => {
                            if let Err(error) = proxy_tcp_stream_and_channel(
                                &tunnel_id,
                                Arc::clone(&registry),
                                stream,
                                forwarded.channel,
                            )
                            .await
                            {
                                registry.note_error(&tunnel_id, error.to_string());
                            }
                        }
                        Err(error) => registry.note_error(
                            &tunnel_id,
                            SshError::PortForwardTargetConnect(error.to_string()).to_string(),
                        ),
                    }
                    registry.connection_closed(&tunnel_id);
                });
            }
        }
    }
}

async fn connect_handle(
    request: &SshPortForwardRequest,
    handler: SshClientHandler,
) -> Result<client::Handle<SshClientHandler>, SshError> {
    let config = Arc::new(client::Config::default());
    let addr = (
        request.connection.hostname.as_str(),
        request.connection.port,
    );
    let mut handle = client::connect(config, addr, handler)
        .await
        .map_err(|error| SshError::Transport(error.to_string()))?;
    authenticate(
        &mut handle,
        &request.connection.username,
        &request.auth_order,
    )
    .await?;
    Ok(handle)
}

async fn disconnect_handle(
    handle: &Arc<AsyncMutex<client::Handle<SshClientHandler>>>,
    cancel_forward: Option<(String, u32)>,
) {
    let handle = handle.lock().await;
    if let Some((address, port)) = cancel_forward {
        let _ = handle.cancel_tcpip_forward(address, port).await;
    }
    let _ = handle
        .disconnect(Disconnect::ByApplication, "port forward stopped", "en")
        .await;
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
