use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::Path;

use nix::errno::Errno;
use nix::sys::socket::{
    AddressFamily, MsgFlags, SockFlag, SockType, UnixAddr, connect, recv, send, setsockopt, socket,
    sockopt,
};
use thiserror::Error;

const SOCKET_BUF_SIZE: usize = 7 * 1024 * 1024;
const QEMU_FRAME_HEADER_LEN: usize = 4;

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("no frame available")]
    WouldBlock,
    #[error("stream frame length {0} exceeds buffer size")]
    FrameTooLarge(usize),
    #[error("socket read failed: {0}")]
    Socket(nix::Error),
}

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("socket would block before any bytes were written")]
    WouldBlock,
    #[error("socket accepted only part of the frame")]
    PartialWrite,
    #[error("gvproxy socket closed while writing")]
    ProcessNotRunning,
    #[error("socket write failed: {0}")]
    Socket(nix::Error),
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("failed to create socket: {0}")]
    CreateSocket(nix::Error),
    #[error("invalid unix socket address: {0}")]
    InvalidAddress(nix::Error),
    #[error("failed to connect to gvproxy socket: {0}")]
    Connect(nix::Error),
    #[error("failed to set socket send buffer: {0}")]
    SetSendBuffer(nix::Error),
    #[error("failed to set socket receive buffer: {0}")]
    SetReceiveBuffer(nix::Error),
}

pub struct GvproxyTransport {
    fd: OwnedFd,
    expecting_frame_length: usize,
    pending_write_offset: usize,
}

impl GvproxyTransport {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ConnectError> {
        let fd = socket(
            AddressFamily::Unix,
            SockType::Stream,
            SockFlag::empty(),
            None,
        )
        .map_err(ConnectError::CreateSocket)?;

        let peer_addr = UnixAddr::new(path.as_ref()).map_err(ConnectError::InvalidAddress)?;
        connect(fd.as_raw_fd(), &peer_addr).map_err(ConnectError::Connect)?;

        setsockopt(&fd, sockopt::SndBuf, &(16 * 1024 * 1024))
            .map_err(ConnectError::SetSendBuffer)?;
        setsockopt(&fd, sockopt::RcvBuf, &SOCKET_BUF_SIZE)
            .map_err(ConnectError::SetReceiveBuffer)?;

        Ok(Self {
            fd,
            expecting_frame_length: 0,
            pending_write_offset: 0,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_fd(fd: OwnedFd) -> Self {
        Self {
            fd,
            expecting_frame_length: 0,
            pending_write_offset: 0,
        }
    }

    pub fn read_frame(&mut self, buf: &mut [u8]) -> Result<usize, ReadError> {
        if self.expecting_frame_length == 0 {
            let mut header = [0u8; QEMU_FRAME_HEADER_LEN];
            self.read_exact(&mut header, false)?;
            self.expecting_frame_length = u32::from_be_bytes(header) as usize;
        }

        if self.expecting_frame_length > buf.len() {
            let len = self.expecting_frame_length;
            self.expecting_frame_length = 0;
            return Err(ReadError::FrameTooLarge(len));
        }

        self.read_exact(&mut buf[..self.expecting_frame_length], true)?;
        let frame_len = self.expecting_frame_length;
        self.expecting_frame_length = 0;
        Ok(frame_len)
    }

    pub fn write_frame(&mut self, hdr_len: usize, buf: &mut [u8]) -> Result<(), WriteError> {
        if self.pending_write_offset != 0 {
            panic!("cannot start a new gvproxy write while a partial write is pending");
        }

        assert!(
            hdr_len >= QEMU_FRAME_HEADER_LEN,
            "not enough header space to prepend the gvproxy qemu stream length"
        );
        assert!(buf.len() > hdr_len);

        let frame_len = u32::try_from(buf.len() - hdr_len)
            .map_err(|_| WriteError::Socket(nix::Error::from(Errno::EINVAL)))?;
        buf[hdr_len - QEMU_FRAME_HEADER_LEN..hdr_len].copy_from_slice(&frame_len.to_be_bytes());
        self.write_loop(&buf[hdr_len - QEMU_FRAME_HEADER_LEN..])
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn has_unfinished_write(&self) -> bool {
        self.pending_write_offset != 0
    }

    pub fn try_finish_write(&mut self, hdr_len: usize, buf: &[u8]) -> Result<(), WriteError> {
        if self.pending_write_offset == 0 {
            return Ok(());
        }

        let already_written = self.pending_write_offset;
        self.write_loop(&buf[hdr_len - QEMU_FRAME_HEADER_LEN + already_written..])
    }

    fn read_exact(&self, buf: &mut [u8], block_until_has_data: bool) -> Result<(), ReadError> {
        let mut bytes_read = 0;

        if !block_until_has_data {
            match recv(
                self.fd.as_raw_fd(),
                buf,
                MsgFlags::MSG_DONTWAIT | MsgFlags::MSG_NOSIGNAL,
            ) {
                Ok(size) => bytes_read += size,
                #[allow(unreachable_patterns)]
                Err(nix::Error::EAGAIN | nix::Error::EWOULDBLOCK) => {
                    return Err(ReadError::WouldBlock);
                }
                Err(err) => return Err(ReadError::Socket(err)),
            }
        }

        while bytes_read < buf.len() {
            match recv(
                self.fd.as_raw_fd(),
                &mut buf[bytes_read..],
                MsgFlags::MSG_WAITALL | MsgFlags::MSG_NOSIGNAL,
            ) {
                #[allow(unreachable_patterns)]
                Err(nix::Error::EAGAIN | nix::Error::EWOULDBLOCK | nix::Error::EINTR) => continue,
                Err(err) => return Err(ReadError::Socket(err)),
                Ok(size) => {
                    if size == 0 {
                        return Err(ReadError::Socket(nix::Error::from(Errno::ECONNRESET)));
                    }
                    bytes_read += size;
                }
            }
        }

        Ok(())
    }

    fn write_loop(&mut self, buf: &[u8]) -> Result<(), WriteError> {
        let mut written = 0usize;

        while written < buf.len() {
            match send(
                self.fd.as_raw_fd(),
                &buf[written..],
                MsgFlags::MSG_DONTWAIT | MsgFlags::MSG_NOSIGNAL,
            ) {
                Ok(size) => {
                    if size == 0 {
                        return Err(WriteError::ProcessNotRunning);
                    }
                    written += size;
                }
                #[allow(unreachable_patterns)]
                Err(nix::Error::EAGAIN | nix::Error::EWOULDBLOCK) => {
                    if written == 0 {
                        return Err(WriteError::WouldBlock);
                    }

                    self.pending_write_offset += written;
                    return Err(WriteError::PartialWrite);
                }
                #[allow(unreachable_patterns)]
                Err(nix::Error::EINTR) => continue,
                Err(nix::Error::EPIPE) => return Err(WriteError::ProcessNotRunning),
                Err(err) => return Err(WriteError::Socket(err)),
            }
        }

        self.pending_write_offset = 0;
        Ok(())
    }
}
