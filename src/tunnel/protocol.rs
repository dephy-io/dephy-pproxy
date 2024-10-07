use std::io;

use bytes::BufMut;
use bytes::BytesMut;
use futures::AsyncRead;
use futures::AsyncReadExt;
use futures::AsyncWrite;
use futures::AsyncWriteExt;

use crate::error::TunnelProtocolError;
use crate::types::consts;
use crate::types::TunnelCommand;
use crate::types::TunnelId;
use crate::types::TunnelReply;

/// Request header when connected to the tunnel server.
///
/// ```plain
/// +-----+-----+------+
/// | VER | CMD |  ID  |
/// +-----+-----+------+
/// |  1  |  1  |  8   |
/// +-----+-----+------+
/// ```
#[derive(Clone, Debug)]
pub struct TunnelRequestHeader {
    pub command: TunnelCommand,
    pub id: TunnelId,
}

impl TunnelRequestHeader {
    /// Creates a request header
    pub fn new(command: TunnelCommand, id: TunnelId) -> TunnelRequestHeader {
        TunnelRequestHeader { command, id }
    }

    /// Read from a reader
    pub async fn read_from<R>(r: &mut R) -> Result<TunnelRequestHeader, TunnelProtocolError>
    where R: AsyncRead + Unpin {
        let mut buf = [0u8; 10];
        r.read_exact(&mut buf).await?;

        let ver = buf[0];
        if ver != consts::TUNNEL_VERSION {
            return Err(TunnelProtocolError::UnsupportedTunnelVersion(ver));
        }

        let cmd = buf[1];
        let command = match TunnelCommand::from_u8(cmd) {
            Some(c) => c,
            None => return Err(TunnelProtocolError::UnsupportedCommand(cmd)),
        };

        let id = TunnelId::from_be_bytes(buf[2..].try_into().unwrap());

        Ok(TunnelRequestHeader { command, id })
    }

    /// Write data into a writer
    pub async fn write_to<W>(&self, w: &mut W) -> io::Result<()>
    where W: AsyncWrite + Unpin {
        let mut buf = BytesMut::with_capacity(10);
        self.write_to_buf(&mut buf);
        w.write_all(&buf).await
    }

    /// Writes to buffer
    pub fn write_to_buf<B: BufMut>(&self, buf: &mut B) {
        let TunnelRequestHeader {
            ref command,
            ref id,
        } = *self;

        buf.put_slice(&[consts::TUNNEL_VERSION, command.as_u8()]);
        buf.put_slice(&id.to_be_bytes())
    }
}

/// Response header for TunnelRequestHeader with TcpConnect command.
///
/// ```plain
/// +-----+-----+------+
/// | VER | REP |  ID  |
/// +-----+-----+------+
/// |  1  |  1  |  8   |
/// +-----+-----+------+
/// ```
#[derive(Clone, Debug)]
pub struct TcpResponseHeader {
    pub reply: TunnelReply,
    pub id: TunnelId,
}

impl TcpResponseHeader {
    /// Creates a response header
    pub fn new(reply: TunnelReply, id: TunnelId) -> TcpResponseHeader {
        TcpResponseHeader { reply, id }
    }

    /// Read from a reader
    pub async fn read_from<R>(r: &mut R) -> Result<TcpResponseHeader, TunnelProtocolError>
    where R: AsyncRead + Unpin {
        let mut buf = [0u8; 10];
        r.read_exact(&mut buf).await?;

        let ver = buf[0];
        let reply_code = buf[1];

        if ver != consts::TUNNEL_VERSION {
            return Err(TunnelProtocolError::UnsupportedTunnelVersion(ver));
        }

        let id = TunnelId::from_be_bytes(buf[2..].try_into().unwrap());

        Ok(TcpResponseHeader {
            reply: TunnelReply::from_u8(reply_code),
            id,
        })
    }

    /// Write to a writer
    pub async fn write_to<W>(&self, w: &mut W) -> io::Result<()>
    where W: AsyncWrite + Unpin {
        let mut buf = BytesMut::with_capacity(10);
        self.write_to_buf(&mut buf);
        w.write_all(&buf).await
    }

    /// Writes to buffer
    pub fn write_to_buf<B: BufMut>(&self, buf: &mut B) {
        let TcpResponseHeader { ref reply, ref id } = *self;
        buf.put_slice(&[consts::TUNNEL_VERSION, reply.as_u8()]);
        buf.put_slice(&id.to_be_bytes());
    }
}
