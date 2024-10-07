use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::oneshot;
use libp2p::PeerId;
use libp2p::Stream;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::compat::Compat;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::sync::CancellationToken;

use crate::error::TunnelError;
use crate::types::TunnelId;
use crate::CommandNotifier;
use crate::PProxyCommand;
use crate::PProxyCommandResponse;

pub mod protocol;

pub struct TunnelServer {
    peer_id: PeerId,
    next_tunnel_id: Arc<AtomicUsize>,
    pproxy_command_tx: mpsc::Sender<(PProxyCommand, CommandNotifier)>,
    listener_cancel_token: Option<CancellationToken>,
    listener: Option<tokio::task::JoinHandle<()>>,
}

pub struct TunnelServerListener {
    peer_id: PeerId,
    next_tunnel_id: Arc<AtomicUsize>,
    pproxy_command_tx: mpsc::Sender<(PProxyCommand, CommandNotifier)>,
    tunnels: HashMap<TunnelId, Tunnel>,
    cancel_token: CancellationToken,
}

pub struct Tunnel {
    peer_id: PeerId,
    tunnel_id: TunnelId,
    listener_cancel_token: Option<CancellationToken>,
    listener: Option<tokio::task::JoinHandle<()>>,
}

pub struct TunnelListener {
    local_stream: TcpStream,
    remote_stream: Compat<Stream>,
    cancel_token: CancellationToken,
}

impl Drop for TunnelServer {
    fn drop(&mut self) {
        if let Some(cancel_token) = self.listener_cancel_token.take() {
            cancel_token.cancel();
        }

        if let Some(listener) = self.listener.take() {
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                listener.abort();
            });
        }

        tracing::info!("TunnelServer to {} dropped", self.peer_id);
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if let Some(cancel_token) = self.listener_cancel_token.take() {
            cancel_token.cancel();
        }

        if let Some(listener) = self.listener.take() {
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                listener.abort();
            });
        }

        tracing::info!("Tunnel {}-{} dropped", self.peer_id, self.tunnel_id);
    }
}

impl TunnelServer {
    pub fn new(
        peer_id: PeerId,
        next_tunnel_id: Arc<AtomicUsize>,
        pproxy_command_tx: mpsc::Sender<(PProxyCommand, CommandNotifier)>,
    ) -> Self {
        Self {
            peer_id,
            next_tunnel_id,
            pproxy_command_tx,
            listener: None,
            listener_cancel_token: None,
        }
    }

    pub async fn listen(&mut self, address: SocketAddr) -> Result<SocketAddr, TunnelError> {
        if self.listener.is_some() {
            return Err(TunnelError::AlreadyListened);
        }

        let tcp_listener = TcpListener::bind(address).await?;
        let local_addr = tcp_listener.local_addr()?;

        let mut listener = TunnelServerListener::new(
            self.peer_id,
            self.next_tunnel_id.clone(),
            self.pproxy_command_tx.clone(),
        );
        let listener_cancel_token = listener.cancel_token();
        let listener_handler =
            tokio::spawn(Box::pin(async move { listener.listen(tcp_listener).await }));

        self.listener = Some(listener_handler);
        self.listener_cancel_token = Some(listener_cancel_token);

        Ok(local_addr)
    }
}

impl TunnelServerListener {
    fn new(
        peer_id: PeerId,
        next_tunnel_id: Arc<AtomicUsize>,
        pproxy_command_tx: mpsc::Sender<(PProxyCommand, CommandNotifier)>,
    ) -> Self {
        Self {
            peer_id,
            next_tunnel_id,
            pproxy_command_tx,
            tunnels: HashMap::new(),
            cancel_token: CancellationToken::new(),
        }
    }

    fn next_tunnel_id(&mut self) -> TunnelId {
        TunnelId::from(self.next_tunnel_id.fetch_add(1usize, Ordering::Relaxed) as u64)
    }

    fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    async fn listen(&mut self, listener: TcpListener) {
        loop {
            if self.cancel_token.is_cancelled() {
                break;
            }

            let Ok((stream, address)) = listener.accept().await else {
                continue;
            };
            tracing::debug!("Received new connection from: {address}");

            let tunnel_id = self.next_tunnel_id();
            let mut tunnel = Tunnel::new(self.peer_id, tunnel_id);

            let (tx, rx) = oneshot::channel();
            if let Err(e) = self
                .pproxy_command_tx
                .send((
                    PProxyCommand::SendConnectCommand {
                        peer_id: self.peer_id,
                        tunnel_id,
                    },
                    tx,
                ))
                .await
            {
                tracing::error!("Send connect command channel tx failed: {e:?}");
                continue;
            }

            match rx.await {
                Err(e) => {
                    tracing::error!("Send connect command channel rx failed: {e:?}");
                    continue;
                }
                Ok(Err(e)) => {
                    tracing::error!("Send connect command channel failed: {e:?}");
                    continue;
                }
                Ok(Ok(resp)) => match resp {
                    PProxyCommandResponse::SendConnectCommand { remote_stream } => {
                        if let Err(e) = tunnel.listen(stream, remote_stream).await {
                            tracing::error!("Tunnel listen failed: {e:?}");
                            continue;
                        };
                    }
                    other_resp => {
                        tracing::error!("Send connect command channel got invalid pproxy command response {other_resp:?}");
                        continue;
                    }
                },
            }

            self.tunnels.insert(tunnel_id, tunnel);
        }
    }
}

impl Tunnel {
    pub fn new(peer_id: PeerId, tunnel_id: TunnelId) -> Self {
        Self {
            peer_id,
            tunnel_id,
            listener: None,
            listener_cancel_token: None,
        }
    }

    pub async fn listen(
        &mut self,
        local_stream: TcpStream,
        remote_stream: Stream,
    ) -> Result<(), TunnelError> {
        if self.listener.is_some() {
            return Err(TunnelError::AlreadyListened);
        }

        let mut listener = TunnelListener::new(local_stream, remote_stream).await;
        let listener_cancel_token = listener.cancel_token();
        let listener_handler = tokio::spawn(Box::pin(async move { listener.listen().await }));

        self.listener = Some(listener_handler);
        self.listener_cancel_token = Some(listener_cancel_token);

        Ok(())
    }
}

impl TunnelListener {
    async fn new(local_stream: TcpStream, remote_stream: Stream) -> Self {
        let remote_stream = remote_stream.compat();
        Self {
            local_stream,
            remote_stream,
            cancel_token: CancellationToken::new(),
        }
    }

    fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    async fn listen(&mut self) {
        tokio::io::copy_bidirectional(&mut self.local_stream, &mut self.remote_stream)
            .await
            .unwrap();
    }
}

pub async fn tcp_connect_with_timeout(
    addr: SocketAddr,
    request_timeout_s: u64,
) -> Result<TcpStream, TunnelError> {
    let fut = TcpStream::connect(addr);
    match timeout(Duration::from_secs(request_timeout_s), fut).await {
        Ok(result) => result.map_err(From::from),
        Err(_) => Err(TunnelError::ConnectionTimeout),
    }
}
