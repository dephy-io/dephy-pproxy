use std::net::SocketAddr;

use clap::Arg;
use clap::ArgAction;
use clap::ArgMatches;
use clap::Command;
use dephy_pproxy::command::proto::command_service_client::CommandServiceClient;
use dephy_pproxy::command::proto::AddPeerRequest;
use dephy_pproxy::command::proto::CreateTunnelServerRequest;
use dephy_pproxy::command::PProxyCommander;
use dephy_pproxy::PProxy;
use litep2p::crypto::ed25519::SecretKey;
use multiaddr::Multiaddr;
use tonic::transport::Server;

fn parse_args() -> Command {
    let mut app = Command::new("pproxy")
        .about("A proxy tool based on libp2p network")
        .version("0.1.0");

    let serve = Command::new("serve")
        .about("Start a pproxy server")
        .arg(
            Arg::new("KEY")
                .long("key")
                .num_args(1)
                .action(ArgAction::Set)
                .help("Ed25519 key. If not set, a random key will be generated"),
        )
        .arg(
            Arg::new("SERVER_ADDR")
                .long("server-addr")
                .num_args(1)
                .default_value("127.0.0.1:10010")
                .action(ArgAction::Set)
                .help("Server address"),
        )
        .arg(
            Arg::new("COMMANDER_SERVER_ADDR")
                .long("commander-server-addr")
                .num_args(1)
                .default_value("127.0.0.1:10086")
                .action(ArgAction::Set)
                .help("Commander server address"),
        )
        .arg(
            Arg::new("PROXY_ADDR")
                .long("proxy-addr")
                .num_args(1)
                .action(ArgAction::Set)
                .help("Will reverse proxy this address if set"),
        );

    let create_tunnel_server = Command::new("create_tunnel_server")
        .about("Set up a tunnel server that allows users proxy data to remote peer")
        .arg(
            Arg::new("COMMANDER_SERVER_ADDR")
                .long("commander-server-addr")
                .num_args(1)
                .default_value("127.0.0.1:10086")
                .action(ArgAction::Set)
                .help("Commander server address"),
        )
        .arg(
            Arg::new("TUNNEL_SERVER_ADDR")
                .long("tunnel-server-addr")
                .num_args(1)
                .action(ArgAction::Set)
                .help("Tunnel server address, if not set a random port will be used"),
        )
        .arg(
            Arg::new("PEER_MULTIADDR")
                .long("peer-multiaddr")
                .num_args(1)
                .action(ArgAction::Set)
                .required(true)
                .help("The multiaddr of the remote peer"),
        );

    app = app
        .arg_required_else_help(true)
        .subcommand(serve)
        .subcommand(create_tunnel_server);

    app
}

async fn serve(args: &ArgMatches) {
    let key = args
        .get_one::<String>("KEY")
        .map(|key| {
            SecretKey::try_from_bytes(hex::decode(key).expect("Invalid key")).expect("Invalid key")
        })
        .unwrap_or_else(|| {
            let key = SecretKey::generate();
            println!("Generated key: {}", hex::encode(&key));
            key
        });
    let server_addr = args
        .get_one::<String>("SERVER_ADDR")
        .unwrap()
        .parse()
        .expect("Invalid server address");
    let commander_server_addr = args
        .get_one::<String>("COMMANDER_SERVER_ADDR")
        .unwrap()
        .parse()
        .expect("Invalid command server address");
    let proxy_addr = args
        .get_one::<String>("PROXY_ADDR")
        .map(|addr| addr.parse().expect("Invalid proxy address"));

    println!("server_addr: {}", server_addr);
    println!("commander_server_addr: {}", commander_server_addr);

    let (pproxy, pproxy_handle) = PProxy::new(key, server_addr, proxy_addr);

    let commander = PProxyCommander::new(pproxy_handle);
    let commander_server =
        Server::builder().add_service(tonic_web::enable(commander.into_tonic_service()));

    futures::join!(
        async move {
            commander_server
                .serve(commander_server_addr)
                .await
                .expect("Commander server failed")
        },
        async move { pproxy.run().await }
    );
}

async fn create_tunnel_server(args: &ArgMatches) {
    let commander_server_addr = args
        .get_one::<String>("COMMANDER_SERVER_ADDR")
        .unwrap()
        .parse::<SocketAddr>()
        .expect("Invalid command server address");
    let tunnel_server_addr = args.get_one::<String>("TUNNEL_SERVER_ADDR").map(|addr| {
        addr.parse::<SocketAddr>()
            .expect("Invalid tunnel server address")
            .to_string()
    });
    let peer_multiaddr = args
        .get_one::<String>("PEER_MULTIADDR")
        .unwrap()
        .parse::<Multiaddr>()
        .expect("Missing peer multiaddr");

    let mut client = CommandServiceClient::connect(format!("http://{}", commander_server_addr))
        .await
        .expect("Connect to commander server failed");

    let pp_response = client
        .add_peer(AddPeerRequest {
            address: peer_multiaddr.to_string(),
            peer_id: None,
        })
        .await
        .expect("Add peer failed")
        .into_inner();

    let peer_id = pp_response.peer_id;

    let pp_response = client
        .create_tunnel_server(CreateTunnelServerRequest {
            peer_id,
            address: tunnel_server_addr,
        })
        .await
        .expect("Create tunnel failed")
        .into_inner();

    println!("tunnel_server_addr: {}", pp_response.address);
}

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let cmd = parse_args();

    match cmd.get_matches().subcommand() {
        Some(("serve", args)) => {
            serve(args).await;
        }
        Some(("create_tunnel_server", args)) => {
            create_tunnel_server(args).await;
        }
        _ => unreachable!(),
    }
}