use std::net::IpAddr;
use std::net::SocketAddr;

use libp2p::identity::Keypair;
use libp2p::noise;
use libp2p::swarm::Swarm;
use libp2p::tcp;
use libp2p::yamux;
use libp2p::StreamProtocol;

pub(crate) use crate::p2p::behaviour::PProxyNetworkBehaviour;
pub(crate) use crate::p2p::behaviour::PProxyNetworkBehaviourEvent;

mod behaviour;

pub const PPROXY_PROTOCOL: StreamProtocol = StreamProtocol::new("/pproxy/1.0.0");

pub(crate) fn new_swarm(
    keypair: Keypair,
    listen_addr: SocketAddr,
    external_ip: Option<IpAddr>,
) -> std::result::Result<Swarm<PProxyNetworkBehaviour>, Box<dyn std::error::Error>> {
    let (ip_type, ip, port) = match listen_addr.ip() {
        IpAddr::V4(ip) => ("ip4", ip.to_string(), listen_addr.port()),
        IpAddr::V6(ip) => ("ip6", ip.to_string(), listen_addr.port()),
    };

    let listen_multiaddr = format!("/{ip_type}/{ip}/tcp/{port}").parse()?;

    let external_multiaddr = match external_ip {
        None => None,
        Some(IpAddr::V4(ip)) => Some(format!("/ip4/{ip}/tcp/{port}").parse()?),
        Some(IpAddr::V6(ip)) => Some(format!("/ip6/{ip}/tcp/{port}").parse()?),
    };

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(PProxyNetworkBehaviour::new)?
        .with_swarm_config(|c| c.with_idle_connection_timeout(std::time::Duration::from_secs(60)))
        .build();

    swarm.listen_on(listen_multiaddr)?;

    if let Some(external_multiaddr) = external_multiaddr {
        swarm.add_external_address(external_multiaddr);
    }

    Ok(swarm)
}
