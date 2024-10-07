use std::fmt;
use std::str::FromStr;

pub mod consts {
    pub const TUNNEL_VERSION: u8 = 0x01;

    pub const TUNNEL_CMD_TCP_CONNECT: u8 = 0x01;
    pub const TUNNEL_CMD_UDP_CONNECT: u8 = 0x02; // Not supported

    pub const TUNNEL_REPLY_SUCCEEDED: u8 = 0x00;
    pub const TUNNEL_REPLY_GENERAL_FAILURE: u8 = 0x01;
    pub const TUNNEL_REPLY_CONNECTION_NOT_ALLOWED: u8 = 0x02;
    pub const TUNNEL_REPLY_NETWORK_UNREACHABLE: u8 = 0x03;
    pub const TUNNEL_REPLY_HOST_UNREACHABLE: u8 = 0x04;
    pub const TUNNEL_REPLY_CONNECTION_REFUSED: u8 = 0x05;
    pub const TUNNEL_REPLY_TTL_EXPIRED: u8 = 0x06;
    pub const TUNNEL_REPLY_COMMAND_NOT_SUPPORTED: u8 = 0x07;
    pub const TUNNEL_REPLY_ADDRESS_TYPE_NOT_SUPPORTED: u8 = 0x08;
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct TunnelId(u64);

impl TunnelId {
    pub fn from<T: Into<u64>>(value: T) -> Self {
        TunnelId(value.into())
    }

    pub fn from_be_bytes(value: [u8; 8]) -> Self {
        TunnelId(u64::from_be_bytes(value))
    }

    pub fn to_be_bytes(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }
}

impl fmt::Display for TunnelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TunnelId {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(TunnelId)
    }
}

#[derive(Clone, Debug, Copy)]
pub enum TunnelCommand {
    TcpConnect,
    UdpConnect, // Not supported
}

impl TunnelCommand {
    pub fn as_u8(self) -> u8 {
        match self {
            TunnelCommand::TcpConnect => consts::TUNNEL_CMD_TCP_CONNECT,
            TunnelCommand::UdpConnect => consts::TUNNEL_CMD_UDP_CONNECT,
        }
    }

    pub fn from_u8(code: u8) -> Option<TunnelCommand> {
        match code {
            consts::TUNNEL_CMD_TCP_CONNECT => Some(TunnelCommand::TcpConnect),
            consts::TUNNEL_CMD_UDP_CONNECT => Some(TunnelCommand::UdpConnect),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub enum TunnelReply {
    Succeeded,
    GeneralFailure,
    ConnectionNotAllowed,
    NetworkUnreachable,
    HostUnreachable,
    ConnectionRefused,
    TtlExpired,
    CommandNotSupported,
    AddressTypeNotSupported,

    OtherReply(u8),
}

impl TunnelReply {
    pub fn as_u8(self) -> u8 {
        match self {
            TunnelReply::Succeeded => consts::TUNNEL_REPLY_SUCCEEDED,
            TunnelReply::GeneralFailure => consts::TUNNEL_REPLY_GENERAL_FAILURE,
            TunnelReply::ConnectionNotAllowed => consts::TUNNEL_REPLY_CONNECTION_NOT_ALLOWED,
            TunnelReply::NetworkUnreachable => consts::TUNNEL_REPLY_NETWORK_UNREACHABLE,
            TunnelReply::HostUnreachable => consts::TUNNEL_REPLY_HOST_UNREACHABLE,
            TunnelReply::ConnectionRefused => consts::TUNNEL_REPLY_CONNECTION_REFUSED,
            TunnelReply::TtlExpired => consts::TUNNEL_REPLY_TTL_EXPIRED,
            TunnelReply::CommandNotSupported => consts::TUNNEL_REPLY_COMMAND_NOT_SUPPORTED,
            TunnelReply::AddressTypeNotSupported => consts::TUNNEL_REPLY_ADDRESS_TYPE_NOT_SUPPORTED,
            TunnelReply::OtherReply(c) => c,
        }
    }

    pub fn from_u8(code: u8) -> TunnelReply {
        match code {
            consts::TUNNEL_REPLY_SUCCEEDED => TunnelReply::Succeeded,
            consts::TUNNEL_REPLY_GENERAL_FAILURE => TunnelReply::GeneralFailure,
            consts::TUNNEL_REPLY_CONNECTION_NOT_ALLOWED => TunnelReply::ConnectionNotAllowed,
            consts::TUNNEL_REPLY_NETWORK_UNREACHABLE => TunnelReply::NetworkUnreachable,
            consts::TUNNEL_REPLY_HOST_UNREACHABLE => TunnelReply::HostUnreachable,
            consts::TUNNEL_REPLY_CONNECTION_REFUSED => TunnelReply::ConnectionRefused,
            consts::TUNNEL_REPLY_TTL_EXPIRED => TunnelReply::TtlExpired,
            consts::TUNNEL_REPLY_COMMAND_NOT_SUPPORTED => TunnelReply::CommandNotSupported,
            consts::TUNNEL_REPLY_ADDRESS_TYPE_NOT_SUPPORTED => TunnelReply::AddressTypeNotSupported,
            _ => TunnelReply::OtherReply(code),
        }
    }
}

impl fmt::Display for TunnelReply {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TunnelReply::Succeeded => write!(f, "Succeeded"),
            TunnelReply::AddressTypeNotSupported => write!(f, "Address type not supported"),
            TunnelReply::CommandNotSupported => write!(f, "Command not supported"),
            TunnelReply::ConnectionNotAllowed => write!(f, "Connection not allowed"),
            TunnelReply::ConnectionRefused => write!(f, "Connection refused"),
            TunnelReply::GeneralFailure => write!(f, "General failure"),
            TunnelReply::HostUnreachable => write!(f, "Host unreachable"),
            TunnelReply::NetworkUnreachable => write!(f, "Network unreachable"),
            TunnelReply::OtherReply(u) => write!(f, "Other reply ({u})"),
            TunnelReply::TtlExpired => write!(f, "TTL expired"),
        }
    }
}
