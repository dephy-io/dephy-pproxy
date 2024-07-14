use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::sync::Mutex;

use futures::channel::oneshot;
use litep2p::crypto::ed25519::SecretKey;
use litep2p::protocol::request_response::DialOptions;
use litep2p::protocol::request_response::RequestResponseEvent;
use litep2p::types::RequestId;
use litep2p::PeerId;
use multiaddr::Multiaddr;
use multiaddr::Protocol;
use prost::Message;
use tokio::sync::mpsc;

use crate::command::proto::AddPeerRequest;
use crate::command::proto::AddPeerResponse;
use crate::command::proto::CreateTunnelServerRequest;
use crate::command::proto::CreateTunnelServerResponse;
use crate::server::*;
use crate::tunnel::proto;
use crate::tunnel::tcp_connect_with_timeout;
use crate::tunnel::Tunnel;
use crate::tunnel::TunnelServer;
use crate::types::*;

pub mod command;
pub mod error;
mod server;
mod tunnel;
pub mod types;

/// pproxy version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default channel size.
const DEFAULT_CHANNEL_SIZE: usize = 4096;

/// Timeout for proxied TCP connections
pub const TCP_SERVER_TIMEOUT: u64 = 30;

/// Public result type error type used by the crate.
pub use crate::error::Error;
pub type Result<T> = std::result::Result<T, error::Error>;

type CommandNotification = Result<PProxyCommandResponse>;
type CommandNotifier = oneshot::Sender<CommandNotification>;

#[derive(Debug)]
pub enum PProxyCommand {
    AddPeer {
        address: Multiaddr,
        peer_id: PeerId,
    },
    SendConnectCommand {
        peer_id: PeerId,
        tunnel_id: TunnelId,
        tunnel_tx: mpsc::Sender<Vec<u8>>,
    },
    SendOutboundPackageCommand {
        peer_id: PeerId,
        tunnel_id: TunnelId,
        data: Vec<u8>,
    },
}

pub enum PProxyCommandResponse {
    AddPeer { peer_id: PeerId },
    SendConnectCommand {},
    SendOutboundPackageCommand {},
}

pub struct PProxy {
    command_tx: mpsc::Sender<(PProxyCommand, CommandNotifier)>,
    command_rx: mpsc::Receiver<(PProxyCommand, CommandNotifier)>,
    p2p_server: P2pServer,
    proxy_addr: Option<SocketAddr>,
    outbound_ready_notifiers: HashMap<RequestId, CommandNotifier>,
    inbound_tunnels: HashMap<(PeerId, TunnelId), Tunnel>,
    tunnel_txs: HashMap<(PeerId, TunnelId), mpsc::Sender<Vec<u8>>>,
}

pub struct PProxyHandle {
    command_tx: mpsc::Sender<(PProxyCommand, CommandNotifier)>,
    next_tunnel_id: Arc<AtomicUsize>,
    tunnel_servers: Mutex<HashMap<PeerId, TunnelServer>>,
}

pub enum FullLength {
    NotParsed,
    NotSet,
    Chunked,
    Parsed(usize),
}

impl FullLength {
    pub fn not_parsed(&self) -> bool {
        matches!(self, FullLength::NotParsed)
    }

    pub fn chunked(&self) -> bool {
        matches!(self, FullLength::Chunked)
    }
}

impl PProxy {
    pub fn new(
        secret_key: SecretKey,
        server_addr: SocketAddr,
        proxy_addr: Option<SocketAddr>,
    ) -> (Self, PProxyHandle) {
        let (command_tx, command_rx) = mpsc::channel(DEFAULT_CHANNEL_SIZE);

        (
            Self {
                command_tx: command_tx.clone(),
                command_rx,
                p2p_server: P2pServer::new(secret_key, server_addr),
                proxy_addr,
                outbound_ready_notifiers: HashMap::new(),
                inbound_tunnels: HashMap::new(),
                tunnel_txs: HashMap::new(),
            },
            PProxyHandle {
                command_tx,
                next_tunnel_id: Default::default(),
                tunnel_servers: Default::default(),
            },
        )
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                // Events coming from the network have higher priority than user commands
                biased;

                event = self.p2p_server.next_event() => match event {
                    None => return,
                    Some(event) => if let Err(error) = self.handle_p2p_server_event(event).await {
                        tracing::warn!("failed to handle event: {:?}", error);
                    }
                },

                command = self.command_rx.recv() => match command {
                    None => return,
                    Some((command, tx)) => if let Err(error) = self.handle_command(command, tx).await {
                        tracing::warn!("failed to handle command: {:?}", error);
                    }
                }
            }
        }
    }

    async fn handle_p2p_server_event(&mut self, event: P2pServerEvent) -> Result<()> {
        tracing::debug!("received P2pServerEvent: {:?}", event);
        #[allow(clippy::single_match)]
        match event {
            P2pServerEvent::Litep2p(ev) => {
                tracing::debug!("received Litep2p event: {:?}", ev);
            }
            P2pServerEvent::TunnelEvent(RequestResponseEvent::RequestReceived {
                peer,
                request_id,
                request,
                ..
            }) => {
                let msg = proto::Tunnel::decode(request.as_slice())?;
                tracing::debug!("received Tunnel request msg: {:?}", msg);

                match msg.command() {
                    proto::TunnelCommand::Connect => {
                        tracing::info!("received connect command from peer: {:?}", peer);
                        let Some(proxy_addr) = self.proxy_addr else {
                            return Err(Error::ProtocolNotSupport("No proxy_addr".to_string()));
                        };

                        let tunnel_id = msg
                            .tunnel_id
                            .parse()
                            .map_err(|_| Error::TunnelIdParseError(msg.tunnel_id))?;

                        let stream = tcp_connect_with_timeout(proxy_addr, 60).await?;
                        let mut tunnel = Tunnel::new(peer, tunnel_id, self.command_tx.clone());
                        let (tunnel_tx, tunnel_rx) = mpsc::channel(1024);
                        tunnel.listen(stream, tunnel_rx).await?;

                        self.inbound_tunnels.insert((peer, tunnel_id), tunnel);
                        self.tunnel_txs.insert((peer, tunnel_id), tunnel_tx);

                        let response = proto::Tunnel {
                            tunnel_id: tunnel_id.to_string(),
                            command: proto::TunnelCommand::ConnectResp.into(),
                            data: None,
                        };

                        self.p2p_server
                            .tunnel_handle
                            .send_response(request_id, response.encode_to_vec());
                    }

                    proto::TunnelCommand::Package => {
                        let tunnel_id = msg
                            .tunnel_id
                            .parse()
                            .map_err(|_| Error::TunnelIdParseError(msg.tunnel_id))?;

                        let Some(tx) = self.tunnel_txs.get(&(peer, tunnel_id)) else {
                            return Err(Error::ProtocolNotSupport(
                                "No tunnel for Package".to_string(),
                            ));
                        };

                        tx.send(msg.data.unwrap_or_default()).await?;

                        // Have to do this to close the response waiter in remote.
                        self.p2p_server
                            .tunnel_handle
                            .send_response(request_id, vec![]);
                    }

                    _ => {
                        return Err(Error::ProtocolNotSupport(
                            "Wrong tunnel request command".to_string(),
                        ));
                    }
                }
            }
            P2pServerEvent::TunnelEvent(RequestResponseEvent::ResponseReceived {
                peer,
                request_id,
                response,
                ..
            }) => {
                // This is response of TunnelCommand::Package
                if response.is_empty() {
                    return Ok(());
                }

                let msg = proto::Tunnel::decode(response.as_slice())?;
                tracing::debug!("received Tunnel response msg: {:?}", msg);

                match msg.command() {
                    proto::TunnelCommand::ConnectResp => {
                        let tx = self
                            .outbound_ready_notifiers
                            .remove(&request_id)
                            .ok_or_else(|| {
                                Error::TunnelNotWaiting(format!(
                                    "peer {}, tunnel {}",
                                    peer, msg.tunnel_id
                                ))
                            })?;

                        tx.send(Ok(PProxyCommandResponse::SendConnectCommand {}))
                            .map_err(|_| Error::EssentialTaskClosed)?;
                    }

                    _ => {
                        return Err(Error::ProtocolNotSupport(
                            "Wrong tunnel response command".to_string(),
                        ));
                    }
                }
            }

            _ => {}
        }

        Ok(())
    }

    async fn handle_command(&mut self, command: PProxyCommand, tx: CommandNotifier) -> Result<()> {
        match command {
            PProxyCommand::AddPeer { address, peer_id } => {
                self.on_add_peer(address, peer_id, tx).await
            }
            PProxyCommand::SendConnectCommand {
                peer_id,
                tunnel_id,
                tunnel_tx,
            } => {
                self.on_send_connect_command(peer_id, tunnel_id, tunnel_tx, tx)
                    .await
            }
            PProxyCommand::SendOutboundPackageCommand {
                peer_id,
                tunnel_id,
                data,
            } => {
                self.on_send_outbound_package_command(peer_id, tunnel_id, data, tx)
                    .await
            }
        }
    }

    async fn on_add_peer(
        &mut self,
        addr: Multiaddr,
        peer_id: PeerId,
        tx: CommandNotifier,
    ) -> Result<()> {
        self.p2p_server
            .litep2p
            .add_known_address(peer_id, vec![addr].into_iter());

        tx.send(Ok(PProxyCommandResponse::AddPeer { peer_id }))
            .map_err(|_| Error::EssentialTaskClosed)
    }

    async fn on_send_connect_command(
        &mut self,
        peer_id: PeerId,
        tunnel_id: TunnelId,
        tunnel_tx: mpsc::Sender<Vec<u8>>,
        tx: CommandNotifier,
    ) -> Result<()> {
        self.tunnel_txs.insert((peer_id, tunnel_id), tunnel_tx);

        let request = proto::Tunnel {
            tunnel_id: tunnel_id.to_string(),
            command: proto::TunnelCommand::Connect.into(),
            data: None,
        }
        .encode_to_vec();

        tracing::info!("send connect command to peer_id: {:?}", peer_id);
        let request_id = self
            .p2p_server
            .tunnel_handle
            .send_request(peer_id, request, DialOptions::Dial)
            .await?;

        self.outbound_ready_notifiers.insert(request_id, tx);

        Ok(())
    }

    async fn on_send_outbound_package_command(
        &mut self,
        peer_id: PeerId,
        tunnel_id: TunnelId,
        data: Vec<u8>,
        tx: CommandNotifier,
    ) -> Result<()> {
        let request = proto::Tunnel {
            tunnel_id: tunnel_id.to_string(),
            command: proto::TunnelCommand::Package.into(),
            data: Some(data),
        }
        .encode_to_vec();

        self.p2p_server
            .tunnel_handle
            .send_request(peer_id, request, DialOptions::Dial)
            .await?;

        tx.send(Ok(PProxyCommandResponse::SendOutboundPackageCommand {}))
            .map_err(|_| Error::EssentialTaskClosed)
    }
}

impl PProxyHandle {
    pub async fn add_peer(&self, request: AddPeerRequest) -> Result<AddPeerResponse> {
        let (tx, rx) = oneshot::channel();

        let address: Multiaddr = request
            .address
            .parse()
            .map_err(|_| Error::MultiaddrParseError(request.address.clone()))?;

        let peer_id = request.peer_id.map_or_else(
            || extract_peer_id_from_multiaddr(&address),
            |peer_id| {
                peer_id
                    .parse()
                    .map_err(|_| Error::PeerIdParseError(peer_id))
            },
        )?;

        self.command_tx
            .send((PProxyCommand::AddPeer { address, peer_id }, tx))
            .await?;

        let response = rx.await??;

        match response {
            PProxyCommandResponse::AddPeer { peer_id } => Ok(AddPeerResponse {
                peer_id: peer_id.to_string(),
            }),
            _ => Err(Error::UnexpectedResponseType),
        }
    }

    pub async fn create_tunnel_server(
        &self,
        request: CreateTunnelServerRequest,
    ) -> Result<CreateTunnelServerResponse> {
        let peer_id = request
            .peer_id
            .parse()
            .map_err(|_| Error::PeerIdParseError(request.peer_id))?;

        let address = request.address.unwrap_or("127.0.0.1:0".to_string());
        let address = address
            .parse()
            .map_err(|_| Error::SocketAddrParseError(address))?;

        let mut tunnel_server = TunnelServer::new(
            peer_id,
            self.next_tunnel_id.clone(),
            self.command_tx.clone(),
        );
        let address = tunnel_server.listen(address).await?;

        self.tunnel_servers
            .lock()
            .unwrap()
            .insert(peer_id, tunnel_server);

        Ok(CreateTunnelServerResponse {
            peer_id: peer_id.to_string(),
            address: address.to_string(),
        })
    }
}

fn extract_peer_id_from_multiaddr(multiaddr: &Multiaddr) -> Result<PeerId> {
    let protocol = multiaddr.iter().last();

    let Some(Protocol::P2p(multihash)) = protocol else {
        return Err(Error::FailedToExtractPeerIdFromMultiaddr(
            multiaddr.to_string(),
        ));
    };

    PeerId::from_multihash(multihash)
        .map_err(|_| Error::FailedToExtractPeerIdFromMultiaddr(multiaddr.to_string()))
}
