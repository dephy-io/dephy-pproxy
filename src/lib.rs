use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::sync::Mutex;

use futures::channel::oneshot;
use futures::AsyncWriteExt;
use futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
use libp2p::Multiaddr;
use libp2p::PeerId;
use libp2p::Stream;
use libp2p::Swarm;
use tokio::sync::mpsc;
use tunnel::protocol::TcpResponseHeader;

use crate::access::AccessClient;
use crate::command::proto::AddPeerRequest;
use crate::command::proto::AddPeerResponse;
use crate::command::proto::ConnectRelayRequest;
use crate::command::proto::ConnectRelayResponse;
use crate::command::proto::CreateTunnelServerRequest;
use crate::command::proto::CreateTunnelServerResponse;
use crate::command::proto::ExpirePeerAccessRequest;
use crate::command::proto::ExpirePeerAccessResponse;
use crate::error::TunnelProtocolError;
use crate::p2p::PProxyNetworkBehaviour;
use crate::p2p::PProxyNetworkBehaviourEvent;
use crate::p2p::PPROXY_PROTOCOL;
use crate::tunnel::protocol::TunnelRequestHeader;
use crate::tunnel::tcp_connect_with_timeout;
use crate::tunnel::Tunnel;
use crate::tunnel::TunnelServer;
use crate::types::TunnelCommand;
use crate::types::TunnelId;
use crate::types::TunnelReply;

mod access;
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
        multiaddr: Multiaddr,
        peer_id: PeerId,
    },
    ConnectRelay {
        multiaddr: Multiaddr,
    },
    SendConnectCommand {
        peer_id: PeerId,
        tunnel_id: TunnelId,
    },
    ExpirePeerAccess {
        peer_id: PeerId,
    },
}

#[derive(Debug)]
pub enum PProxyCommandResponse {
    AddPeer { peer_id: PeerId },
    ConnectRelay { relaied_multiaddr: Multiaddr },
    SendConnectCommand { remote_stream: Stream },
    ExpirePeerAccess {},
}

pub struct PProxy {
    command_rx: mpsc::Receiver<(PProxyCommand, CommandNotifier)>,
    swarm: Swarm<PProxyNetworkBehaviour>,
    stream_control: libp2p_stream::Control,
    proxy_addr: Option<SocketAddr>,
    access_client: Option<AccessClient>,
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
        access_server_endpoint: Option<reqwest::Url>,
    ) -> Result<(Self, PProxyHandle)> {
        let (command_tx, command_rx) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let swarm = crate::p2p::new_swarm(keypair, listen_addr)
            .map_err(|e| Error::Libp2pSwarmCreateError(e.to_string()))?;
        let stream_control = swarm.behaviour().stream.new_control();
        let access_client = access_server_endpoint.map(AccessClient::new);

        Ok((
            Self {
                command_rx,
                swarm,
                stream_control,
                proxy_addr,
                access_client,
            },
            PProxyHandle {
                command_tx,
                next_tunnel_id: Default::default(),
                tunnel_servers: Default::default(),
            },
        ))
    }

    pub async fn run(mut self) {
        let mut incoming_streams = self
            .stream_control
            .accept(PPROXY_PROTOCOL)
            .expect("Failed to accept incoming streams");

        loop {
            tokio::select! {
                // Events coming from the network have higher priority than user commands
                biased;

                event = self.swarm.select_next_some() => {
                    if let Err(error) = self.handle_p2p_server_event(event).await {
                        tracing::warn!("failed to handle event: {:?}", error);
                    }
                },

                stream = incoming_streams.next() => match stream {
                    None => return,
                    Some((peer_id, stream)) => if let Err(error) = self.handle_incoming_stream(peer_id, stream).await {
                        tracing::warn!("failed to handle incoming stream: {:?}", error);
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

    async fn handle_p2p_server_event(
        &mut self,
        event: SwarmEvent<PProxyNetworkBehaviourEvent>,
    ) -> Result<()> {
        tracing::debug!("received SwarmEvent: {:?}", event);

        #[allow(clippy::single_match)]
        match event {
            SwarmEvent::NewListenAddr { mut address, .. } => {
                address.push(Protocol::P2p(*self.swarm.local_peer_id()));
                println!("Local node is listening on {address}");
            }
            _ => {}
        }

        Ok(())
    }

    async fn is_tunnel_valid(&mut self, peer_id: &PeerId) -> bool {
        let Some(ref mut ac) = self.access_client else {
            return true;
        };
        ac.is_valid(peer_id).await
    }

    // TODO: Should spawn?
    async fn handle_incoming_stream(
        &mut self,
        peer_id: PeerId,
        mut remote_stream: Stream,
    ) -> Result<()> {
        tracing::info!("received channel stream from peer: {:?}", peer_id);

        let Some(proxy_addr) = self.proxy_addr else {
            remote_stream.close().await?;
            let e = Error::ProtocolNotSupport("No proxy_addr".to_string());
            return Err(e);
        };

        match TunnelRequestHeader::read_from(&mut remote_stream).await {
            Err(e) => {
                remote_stream.close().await?;
                return Err(e.into());
            }

            Ok(header) => {
                if !self.is_tunnel_valid(&peer_id).await {
                    remote_stream.close().await?;
                    let e = Error::AccessDenied(peer_id.to_string());
                    return Err(e);
                }

                let local_stream = tcp_connect_with_timeout(proxy_addr, LOCAL_TCP_TIMEOUT).await?;
                let mut tunnel = Tunnel::new(peer_id, header.id);

                TcpResponseHeader::new(TunnelReply::Succeeded, header.id)
                    .write_to(&mut remote_stream)
                    .await?;

                tunnel.listen(local_stream, remote_stream).await?;
            }
        }

        Ok(())
    }

    async fn handle_command(&mut self, command: PProxyCommand, tx: CommandNotifier) -> Result<()> {
        match command {
            PProxyCommand::AddPeer { multiaddr, peer_id } => {
                self.on_add_peer(multiaddr, peer_id, tx).await
            }
            PProxyCommand::ConnectRelay { multiaddr } => self.on_connect_relay(multiaddr, tx).await,
            PProxyCommand::SendConnectCommand { peer_id, tunnel_id } => {
                self.on_send_connect_command(peer_id, tunnel_id, tx).await
            }
            PProxyCommand::ExpirePeerAccess { peer_id } => {
                self.on_expire_peer_access(peer_id, tx).await
            }
        }
    }

    async fn on_add_peer(
        &mut self,
        multiaddr: Multiaddr,
        peer_id: PeerId,
        tx: CommandNotifier,
    ) -> Result<()> {
        self.swarm.dial(multiaddr)?;
        tx.send(Ok(PProxyCommandResponse::AddPeer { peer_id }))
            .map_err(|_| Error::EssentialTaskClosed)
    }

    async fn on_connect_relay(&mut self, multiaddr: Multiaddr, tx: CommandNotifier) -> Result<()> {
        let relaied_multiaddr = multiaddr
            .with(Protocol::P2pCircuit)
            .with(Protocol::P2p(*self.swarm.local_peer_id()));

        self.swarm.listen_on(relaied_multiaddr.clone())?;

        tx.send(Ok(PProxyCommandResponse::ConnectRelay {
            relaied_multiaddr,
        }))
        .map_err(|_| Error::EssentialTaskClosed)
    }

    async fn on_send_connect_command(
        &mut self,
        peer_id: PeerId,
        tunnel_id: TunnelId,
        tx: CommandNotifier,
    ) -> Result<()> {
        let mut remote_stream = self
            .stream_control
            .open_stream(peer_id, PPROXY_PROTOCOL)
            .await?;

        TunnelRequestHeader::new(TunnelCommand::TcpConnect, tunnel_id)
            .write_to(&mut remote_stream)
            .await?;

        let resp = TcpResponseHeader::read_from(&mut remote_stream).await?;

        match resp.reply {
            TunnelReply::Succeeded => {
                tx.send(Ok(PProxyCommandResponse::SendConnectCommand {
                    remote_stream,
                }))
                .map_err(|_| Error::EssentialTaskClosed)?;
            }
            e => {
                remote_stream.close().await?;
                tx.send(Err(Error::TunnelProtocolError(
                    TunnelProtocolError::TunnelReply(e),
                )))
                .map_err(|_| Error::EssentialTaskClosed)?;
            }
        }

        Ok(())
    }

    async fn on_expire_peer_access(&mut self, peer_id: PeerId, tx: CommandNotifier) -> Result<()> {
        if let Some(ref mut ac) = self.access_client {
            ac.expire(&peer_id);
        }

        tx.send(Ok(PProxyCommandResponse::ExpirePeerAccess {}))
            .map_err(|_| Error::EssentialTaskClosed)?;

        Ok(())
    }
}

impl PProxyHandle {
    pub async fn add_peer(&self, request: AddPeerRequest) -> Result<AddPeerResponse> {
        let (tx, rx) = oneshot::channel();

        let multiaddr: Multiaddr = request
            .multiaddr
            .parse()
            .map_err(|_| Error::MultiaddrParseError(request.multiaddr.clone()))?;

        let peer_id = request.peer_id.map_or_else(
            || extract_peer_id_from_multiaddr(&multiaddr),
            |peer_id| {
                peer_id
                    .parse()
                    .map_err(|_| Error::PeerIdParseError(peer_id))
            },
        )?;

        self.command_tx
            .send((PProxyCommand::AddPeer { multiaddr, peer_id }, tx))
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

    pub async fn connect_relay(
        &self,
        request: ConnectRelayRequest,
    ) -> Result<ConnectRelayResponse> {
        let (tx, rx) = oneshot::channel();

        let multiaddr: Multiaddr = request
            .multiaddr
            .parse()
            .map_err(|_| Error::MultiaddrParseError(request.multiaddr.clone()))?;

        self.command_tx
            .send((PProxyCommand::ConnectRelay { multiaddr }, tx))
            .await?;

        let response = rx.await??;

        match response {
            PProxyCommandResponse::ConnectRelay { relaied_multiaddr } => Ok(ConnectRelayResponse {
                relaied_multiaddr: relaied_multiaddr.to_string(),
            }),
            _ => Err(Error::UnexpectedResponseType),
        }
    }

    pub async fn expire_peer_access(
        &self,
        request: ExpirePeerAccessRequest,
    ) -> Result<ExpirePeerAccessResponse> {
        let (tx, rx) = oneshot::channel();

        let peer_id = request
            .peer_id
            .parse()
            .map_err(|_| Error::PeerIdParseError(request.peer_id))?;

        self.command_tx
            .send((PProxyCommand::ExpirePeerAccess { peer_id }, tx))
            .await?;

        rx.await??;

        Ok(ExpirePeerAccessResponse {})
    }
}

fn extract_peer_id_from_multiaddr(multiaddr: &Multiaddr) -> Result<PeerId> {
    let protocol = multiaddr.iter().last();

    let Some(Protocol::P2p(peer_id)) = protocol else {
        return Err(Error::FailedToExtractPeerIdFromMultiaddr(
            multiaddr.to_string(),
        ));
    };

    Ok(peer_id)
}
