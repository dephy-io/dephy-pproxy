use libp2p::identity::Keypair;
use libp2p::relay;
use libp2p::swarm::NetworkBehaviour;

#[derive(NetworkBehaviour)]
pub(crate) struct PProxyNetworkBehaviour {
    pub(crate) stream: libp2p_stream::Behaviour,
    pub(crate) relay: relay::Behaviour,
    pub(crate) relay_client: relay::client::Behaviour,
}

impl PProxyNetworkBehaviour {
    pub fn new(key: &Keypair, relay_client: relay::client::Behaviour) -> Self {
        let stream = libp2p_stream::Behaviour::new();
        let relay = relay::Behaviour::new(key.public().to_peer_id(), Default::default());
        Self {
            stream,
            relay,
            relay_client,
        }
    }
}
