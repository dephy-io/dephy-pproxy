use std::net::IpAddr;
use std::net::SocketAddr;

use futures::StreamExt;
use litep2p::config::ConfigBuilder;
use litep2p::crypto::ed25519::SecretKey;
use litep2p::protocol::request_response::Config as RequestResponseConfig;
use litep2p::protocol::request_response::ConfigBuilder as RequestResponseConfigBuilder;
use litep2p::protocol::request_response::RequestResponseEvent;
use litep2p::protocol::request_response::RequestResponseHandle;
use litep2p::transport::tcp::config::Config as TcpConfig;
use litep2p::types::protocol::ProtocolName;
use litep2p::Litep2p;
use litep2p::Litep2pEvent;

#[derive(Debug)]
pub enum P2pServerEvent {
    Litep2p(Litep2pEvent),
    TunnelEvent(RequestResponseEvent),
}

pub struct P2pServer {
    pub(crate) litep2p: Litep2p,
    pub(crate) tunnel_handle: RequestResponseHandle,
}

impl P2pServer {
    pub fn new(secret_key: SecretKey, server_addr: SocketAddr) -> Self {
        let (tunnel_config, tunnel_handle) = Self::init_tunnel();

        let (ip_type, ip, port) = match server_addr.ip() {
            IpAddr::V4(ip) => ("ip4", ip.to_string(), server_addr.port()),
            IpAddr::V6(ip) => ("ip6", ip.to_string(), server_addr.port()),
        };

        let litep2p_config = ConfigBuilder::new()
            .with_keypair(secret_key.into())
            .with_tcp(TcpConfig {
                listen_addresses: vec![format!("/{ip_type}/{ip}/tcp/{port}").parse().unwrap()],
                ..Default::default()
            })
            .with_request_response_protocol(tunnel_config)
            .build();

        let litep2p = Litep2p::new(litep2p_config).expect("failed to create litep2p");
        let addresses = litep2p.listen_addresses().next().unwrap();
        println!("Litep2p listening on {:?}", addresses);

        Self {
            litep2p,
            tunnel_handle,
        }
    }

    fn init_tunnel() -> (RequestResponseConfig, RequestResponseHandle) {
        RequestResponseConfigBuilder::new(ProtocolName::from("/pproxy/tunnel/1"))
            .with_max_size(1024 * 1024)
            .with_timeout(std::time::Duration::from_secs(120))
            .build()
    }

    pub async fn next_event(&mut self) -> Option<P2pServerEvent> {
        tokio::select! {
            // TODO: next_event may not be cancel safe
            ev = self.litep2p.next_event() => {
                ev.map(P2pServerEvent::Litep2p)
            }
            ev = self.tunnel_handle.next() => {
                ev.map(P2pServerEvent::TunnelEvent)
            }
        }
    }
}
