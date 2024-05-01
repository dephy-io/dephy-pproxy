# dephy-pproxy

dephy-pproxy is a proxy over litep2p.

## Usage
Run the server pproxy:
```shell
cargo run -- serve --server-addr 127.0.0.1:6666 --commander-server-addr 127.0.0.1:7777 --proxy-addr 127.0.0.1:8000
```

Provide a simple http server for testing:
```shell
python3 -m http.server 8000
```

Run the client pproxy:
```shell
cargo run -- serve
```

Create a tunnel server on client:
```shell
cargo run -- create_tunnel_server --tunnel-server-addr 127.0.0.1:8080 --peer-multiaddr <The litep2p multiaddr>
```

Access server via the gateway address:
```shell
curl 127.0.0.1:8080
```
