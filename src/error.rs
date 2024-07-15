use std::io::ErrorKind as IOErrorKind;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("Multiaddr parse error: {0}")]
    MultiaddrParseError(String),
    #[error("SocketAddr parse error: {0}")]
    SocketAddrParseError(String),
    #[error("Failed to extract peer id from multiaddr: {0}")]
    FailedToExtractPeerIdFromMultiaddr(String),
    #[error("PeerId parse error: {0}")]
    PeerIdParseError(String),
    #[error("TunnelId parse error: {0}")]
    TunnelIdParseError(String),
    #[error("Essential task closed")]
    EssentialTaskClosed,
    #[error("Litep2p error: {0}")]
    Litep2p(#[from] litep2p::Error),
    #[error("Litep2p request response error: {0:?}")]
    Litep2pRequestResponseError(litep2p::protocol::request_response::RequestResponseError),
    #[error("Protocol not support: {0}")]
    ProtocolNotSupport(String),
    #[error("Unexpected response type")]
    UnexpectedResponseType,
    #[error("Tunnel not waiting")]
    TunnelNotWaiting(String),
    #[error("Tunnel dial failed: {0}")]
    TunnelDialFailed(String),
    #[error("Tunnel error: {0:?}")]
    Tunnel(TunnelError),
    #[error("Protobuf decode error: {0}")]
    ProtobufDecode(#[from] prost::DecodeError),
}

/// A list specifying general categories of Tunnel error like [std::io::ErrorKind].
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
#[non_exhaustive]
pub enum TunnelError {
    /// Failed to send data to peer.
    DataSendFailed = 1,
    /// The connection timed out when dialing.
    ConnectionTimeout = 2,
    /// Got [std::io::ErrorKind::ConnectionRefused] error from local stream.
    ConnectionRefused = 3,
    /// Got [std::io::ErrorKind::ConnectionAborted] error from local stream.
    ConnectionAborted = 4,
    /// Got [std::io::ErrorKind::ConnectionReset] error from local stream.
    ConnectionReset = 5,
    /// Got [std::io::ErrorKind::NotConnected] error from local stream.
    NotConnected = 6,
    /// The connection is closed by peer.
    ConnectionClosed = 7,
    /// A socket address could not be bound because the address is already in
    /// use elsewhere.
    AddrInUse = 8,
    /// Tunnel already listened.
    TunnelInUse = 9,
    /// Unknown [std::io::ErrorKind] error.
    Unknown = u8::MAX,
}

impl<T> From<tokio::sync::mpsc::error::SendError<T>> for Error {
    fn from(_: tokio::sync::mpsc::error::SendError<T>) -> Self {
        Error::EssentialTaskClosed
    }
}

impl From<futures::channel::oneshot::Canceled> for Error {
    fn from(_: futures::channel::oneshot::Canceled) -> Self {
        Error::EssentialTaskClosed
    }
}

impl From<litep2p::protocol::request_response::RequestResponseError> for Error {
    fn from(err: litep2p::protocol::request_response::RequestResponseError) -> Self {
        Error::Litep2pRequestResponseError(err)
    }
}

impl From<TunnelError> for Error {
    fn from(error: TunnelError) -> Self {
        Error::Tunnel(error)
    }
}

impl From<IOErrorKind> for TunnelError {
    fn from(kind: IOErrorKind) -> TunnelError {
        match kind {
            IOErrorKind::ConnectionRefused => TunnelError::ConnectionRefused,
            IOErrorKind::ConnectionAborted => TunnelError::ConnectionAborted,
            IOErrorKind::ConnectionReset => TunnelError::ConnectionReset,
            IOErrorKind::NotConnected => TunnelError::NotConnected,
            IOErrorKind::AddrInUse => TunnelError::AddrInUse,
            _ => TunnelError::Unknown,
        }
    }
}

impl From<std::io::Error> for TunnelError {
    fn from(error: std::io::Error) -> TunnelError {
        error.kind().into()
    }
}