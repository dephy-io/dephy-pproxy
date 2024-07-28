use libp2p::identity::Keypair;
use libp2p::request_response;
use libp2p::swarm::NetworkBehaviour;
use libp2p::StreamProtocol;

use crate::p2p::codec::Codec;

#[derive(NetworkBehaviour)]
pub(crate) struct PProxyNetworkBehaviour {
    pub(crate) request_response: request_response::Behaviour<Codec>,
}

impl PProxyNetworkBehaviour {
    pub fn new(_key: &Keypair) -> Self {
        let request_response = request_response::Behaviour::new(
            [(
                StreamProtocol::new("/pproxy/1"),
                request_response::ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        );
        Self { request_response }
    }
}
