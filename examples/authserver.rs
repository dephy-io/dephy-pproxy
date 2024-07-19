use clap::Arg;
use clap::ArgAction;
use clap::ArgMatches;
use clap::Command;
use dephy_pproxy::auth::proto;
use tonic::transport::Server;
use tonic::Request;
use tonic::Response;
use tonic::Status;

struct PProxyAuth {
    peer_ids: Vec<String>,
}

#[tonic::async_trait]
impl proto::auth_service_server::AuthService for PProxyAuth {
    async fn get_tokens(
        &self,
        request: Request<proto::GetTokensRequest>,
    ) -> Result<Response<proto::GetTokensResponse>, Status> {
        let request = request.into_inner();

        let tokens = self
            .peer_ids
            .iter()
            .filter(|peer_id| {
                request.peer_id.is_none() || request.peer_id == Some(peer_id.to_owned().to_owned())
            })
            .cloned()
            .map(|peer_id| proto::Token {
                resource_id: request.resource_id.clone(),
                peer_id,
                ttl: 3600,
            })
            .collect();

        Ok(Response::new(proto::GetTokensResponse { tokens }))
    }
}

fn parse_args() -> ArgMatches {
    Command::new("pproxy-auth-server")
        .about("An example pproxy auth server")
        .version(dephy_pproxy::VERSION)
        .arg(
            Arg::new("SERVER_ADDR")
                .long("server-addr")
                .num_args(1)
                .default_value("127.0.0.1:3000")
                .action(ArgAction::Set)
                .help("Server address"),
        )
        .arg(
            Arg::new("PEER_IDS")
                .num_args(0..)
                .action(ArgAction::Set)
                .help("Will generate tokens for those peers"),
        )
        .arg_required_else_help(true)
        .get_matches()
}

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let args = parse_args();

    let server_addr = args
        .get_one::<String>("SERVER_ADDR")
        .unwrap()
        .parse()
        .expect("Invalid server address");
    let peer_ids = args.get_many("PEER_IDS").unwrap().cloned().collect();
    println!("server_addr: {}", server_addr);
    println!("peer_ids: {:?}", peer_ids);

    let auth = PProxyAuth { peer_ids };

    let auth_server = proto::auth_service_server::AuthServiceServer::new(auth);

    Server::builder()
        .add_service(tonic_web::enable(auth_server))
        .serve(server_addr)
        .await
        .expect("Auth server failed");
}
