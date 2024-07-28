use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::sync::Mutex;

use futures::channel::oneshot;
use futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::multiaddr;
use libp2p::request_response;
use libp2p::swarm::SwarmEvent;
use libp2p::Multiaddr;
use libp2p::PeerId;
use libp2p::Swarm;
use tokio::sync::mpsc;

use crate::command::proto::AddPeerRequest;
use crate::command::proto::AddPeerResponse;
use crate::command::proto::CreateTunnelServerRequest;
use crate::command::proto::CreateTunnelServerResponse;
use crate::p2p::PProxyNetworkBehaviour;
use crate::p2p::PProxyNetworkBehaviourEvent;
use crate::tunnel::proto;
use crate::tunnel::tcp_connect_with_timeout;
use crate::tunnel::Tunnel;
use crate::tunnel::TunnelServer;
use crate::types::*;

pub mod auth;
pub mod command;
pub mod error;
mod p2p;
mod tunnel;
pub mod types;

/// pproxy version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default channel size.
const DEFAULT_CHANNEL_SIZE: usize = 4096;

/// Timeout for local TCP server.
pub const LOCAL_TCP_TIMEOUT: u64 = 5;

/// Timeout for remote TCP server.
pub const REMOTE_TCP_TIMEOUT: u64 = 30;

/// Public result type and error type used by the crate.
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
    swarm: Swarm<PProxyNetworkBehaviour>,
    proxy_addr: Option<SocketAddr>,
    outbound_ready_notifiers: HashMap<request_response::OutboundRequestId, CommandNotifier>,
    inbound_tunnels: HashMap<(PeerId, TunnelId), Tunnel>,
    tunnel_txs: HashMap<(PeerId, TunnelId), mpsc::Sender<Vec<u8>>>,
}

pub struct PProxyHandle {
    command_tx: mpsc::Sender<(PProxyCommand, CommandNotifier)>,
    next_tunnel_id: Arc<AtomicUsize>,
    tunnel_servers: Mutex<HashMap<PeerId, TunnelServer>>,
}

impl PProxy {
    pub fn new(
        keypair: Keypair,
        listen_addr: SocketAddr,
        proxy_addr: Option<SocketAddr>,
    ) -> Result<(Self, PProxyHandle)> {
        let (command_tx, command_rx) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let swarm = crate::p2p::new_swarm(keypair, listen_addr)
            .map_err(|e| Error::Libp2pSwarmCreateError(e.to_string()))?;

        Ok((
            Self {
                command_tx: command_tx.clone(),
                command_rx,
                swarm,
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
        ))
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                // Events coming from the network have higher priority than user commands
                biased;

                event = self.swarm.select_next_some() => {
                     if let Err(error) = self.handle_p2p_server_event(event).await {
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

    async fn dial_tunnel(
        &mut self,
        proxy_addr: SocketAddr,
        peer_id: PeerId,
        tunnel_id: TunnelId,
    ) -> Result<()> {
        let stream = tcp_connect_with_timeout(proxy_addr, LOCAL_TCP_TIMEOUT).await?;

        let mut tunnel = Tunnel::new(peer_id, tunnel_id, self.command_tx.clone());
        let (tunnel_tx, tunnel_rx) = mpsc::channel(1024);
        tunnel.listen(stream, tunnel_rx).await?;

        self.inbound_tunnels.insert((peer_id, tunnel_id), tunnel);
        self.tunnel_txs.insert((peer_id, tunnel_id), tunnel_tx);

        Ok(())
    }

    async fn handle_p2p_server_event(
        &mut self,
        event: SwarmEvent<PProxyNetworkBehaviourEvent>,
    ) -> Result<()> {
        tracing::debug!("received SwarmEvent: {:?}", event);

        #[allow(clippy::single_match)]
        match event {
            SwarmEvent::Behaviour(PProxyNetworkBehaviourEvent::RequestResponse(
                request_response::Event::Message { peer, message },
            )) => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    match request.command() {
                        proto::TunnelCommand::Connect => {
                            tracing::info!("received connect command from peer: {:?}", peer);
                            let Some(proxy_addr) = self.proxy_addr else {
                                return Err(Error::ProtocolNotSupport("No proxy_addr".to_string()));
                            };

                            let tunnel_id = request
                                .tunnel_id
                                .parse()
                                .map_err(|_| Error::TunnelIdParseError(request.tunnel_id))?;

                            let data = match self.dial_tunnel(proxy_addr, peer, tunnel_id).await {
                                Ok(_) => None,
                                Err(e) => {
                                    tracing::warn!("failed to dial tunnel: {:?}", e);
                                    Some(e.to_string().into_bytes())
                                }
                            };

                            let response = proto::Tunnel {
                                tunnel_id: tunnel_id.to_string(),
                                command: proto::TunnelCommand::ConnectResp.into(),
                                data,
                            };

                            self.swarm
                                .behaviour_mut()
                                .request_response
                                .send_response(channel, Some(response))
                                .map_err(|_| Error::EssentialTaskClosed)?;
                        }

                        proto::TunnelCommand::Package => {
                            let tunnel_id = request
                                .tunnel_id
                                .parse()
                                .map_err(|_| Error::TunnelIdParseError(request.tunnel_id))?;

                            let Some(tx) = self.tunnel_txs.get(&(peer, tunnel_id)) else {
                                return Err(Error::ProtocolNotSupport(
                                    "No tunnel for Package".to_string(),
                                ));
                            };

                            tx.send(request.data.unwrap_or_default()).await?;

                            // Have to do this to close the response waiter in remote.
                            self.swarm
                                .behaviour_mut()
                                .request_response
                                .send_response(channel, None)
                                .map_err(|_| Error::EssentialTaskClosed)?;
                        }

                        _ => {
                            return Err(Error::ProtocolNotSupport(
                                "Wrong tunnel request command".to_string(),
                            ));
                        }
                    }
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    // This is response of TunnelCommand::Package
                    let Some(response) = response else {
                        return Ok(());
                    };

                    match response.command() {
                        proto::TunnelCommand::ConnectResp => {
                            let tx = self
                                .outbound_ready_notifiers
                                .remove(&request_id)
                                .ok_or_else(|| {
                                    Error::TunnelNotWaiting(format!(
                                        "peer {}, tunnel {}",
                                        peer, response.tunnel_id
                                    ))
                                })?;

                            match response.data {
                                None => tx.send(Ok(PProxyCommandResponse::SendConnectCommand {})),
                                Some(data) => tx.send(Err(Error::TunnelDialFailed(
                                    String::from_utf8(data)
                                        .unwrap_or("Unknown (decode failed)".to_string()),
                                ))),
                            }
                            .map_err(|_| Error::EssentialTaskClosed)?;
                        }

                        _ => {
                            return Err(Error::ProtocolNotSupport(
                                "Wrong tunnel response command".to_string(),
                            ));
                        }
                    }
                }
            },

            SwarmEvent::NewListenAddr { mut address, .. } => {
                address.push(multiaddr::Protocol::P2p(*self.swarm.local_peer_id()));
                println!("Local node is listening on {address}");
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
        self.swarm.add_peer_address(peer_id, addr);

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
        };

        tracing::info!("send connect command to peer_id: {:?}", peer_id);
        let request_id = self
            .swarm
            .behaviour_mut()
            .request_response
            .send_request(&peer_id, request);

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
        };

        self.swarm
            .behaviour_mut()
            .request_response
            .send_request(&peer_id, request);

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

    let Some(multiaddr::Protocol::P2p(peer_id)) = protocol else {
        return Err(Error::FailedToExtractPeerIdFromMultiaddr(
            multiaddr.to_string(),
        ));
    };

    Ok(peer_id)
}
