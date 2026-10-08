//! A connection's socket with the game stream's opcode masking applied
//! transparently: reads return unmasked bytes, writes are masked on their way
//! out (`rs910_protocol::wire_cipher`). Everything above a [`WireStream`]
//! sees the plain framing, so the recorder, the replay transport and the
//! frame decoders never see a cipher.
//!
//! The wrapped socket is a tokio stream during login and the startup drain
//! and a non-blocking `std` stream afterwards; the same type serves both
//! ([`WireStream::into_std`] moves it across). Without a cipher it is a plain
//! pass-through (tests and recorded replays).
//!
//! Writes always accept their whole buffer: the masked bytes are queued and
//! sent as far as the socket takes them, the rest on later writes or flushes.
//! That keeps the masking in frame order however the socket splits a write.
//! The queue is shared with a [`Sender`], the keep-alive thread's handle, so
//! its frames take their turn in the same masking sequence.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};

use rs910_protocol::wire_cipher::{InboundCipher, OutboundCipher, WireCipher};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The outgoing masking state and the masked bytes the socket has not taken.
#[derive(Debug)]
struct Outgoing {
    cipher: OutboundCipher,
    unsent: Vec<u8>,
}

type SharedOutgoing = Arc<Mutex<Outgoing>>;

fn lock(outgoing: &SharedOutgoing) -> MutexGuard<'_, Outgoing> {
    outgoing
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn invalid_data(error: anyhow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{error:#}"))
}

/// Write as much of `unsent` as `socket` takes; a socket that would block
/// leaves the rest queued.
fn drain_unsent<W: Write>(socket: &mut W, unsent: &mut Vec<u8>) -> io::Result<()> {
    while !unsent.is_empty() {
        match socket.write(unsent) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(taken) => {
                unsent.drain(..taken);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// A socket plus the connection's cipher, if it has one.
#[derive(Debug)]
pub struct WireStream<S> {
    inner: S,
    inbound: Option<InboundCipher>,
    outgoing: Option<SharedOutgoing>,
}

impl<S> WireStream<S> {
    /// `inner` with `cipher` applied; no cipher is a pass-through.
    pub fn new(inner: S, cipher: Option<WireCipher>) -> Self {
        let (inbound, outgoing) = match cipher {
            Some(WireCipher { inbound, outbound }) => (
                Some(inbound),
                Some(Arc::new(Mutex::new(Outgoing {
                    cipher: outbound,
                    unsent: Vec::new(),
                }))),
            ),
            None => (None, None),
        };
        Self {
            inner,
            inbound,
            outgoing,
        }
    }

    /// `inner` with no cipher.
    pub fn plain(inner: S) -> Self {
        Self::new(inner, None)
    }

    /// Whether the stream applies a cipher.
    pub fn is_masked(&self) -> bool {
        self.outgoing.is_some()
    }

    pub fn get_ref(&self) -> &S {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Queue the masked form of `plain`.
    fn seal(&mut self, plain: &[u8]) -> io::Result<()> {
        if let Some(outgoing) = &self.outgoing {
            let mut guard = lock(outgoing);
            let Outgoing { cipher, unsent } = &mut *guard;
            cipher.seal(plain, unsent).map_err(invalid_data)?;
        }
        Ok(())
    }

    fn unmask(&mut self, bytes: &mut [u8]) -> io::Result<()> {
        match &mut self.inbound {
            Some(cipher) => cipher.open(bytes).map_err(invalid_data),
            None => Ok(()),
        }
    }
}

impl<S: Read> Read for WireStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let taken = self.inner.read(buf)?;
        self.unmask(&mut buf[..taken])?;
        Ok(taken)
    }
}

impl<S: Write> Write for WireStream<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(outgoing) = self.outgoing.clone() else {
            return self.inner.write(buf);
        };
        self.seal(buf)?;
        drain_unsent(&mut self.inner, &mut lock(&outgoing).unsent)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(outgoing) = self.outgoing.clone() {
            drain_unsent(&mut self.inner, &mut lock(&outgoing).unsent)?;
        }
        self.inner.flush()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for WireStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let fresh = &mut buf.filled_mut()[before..];
                Poll::Ready(this.unmask(fresh))
            }
            other => other,
        }
    }
}

impl<S: AsyncWrite + Unpin> WireStream<S> {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Some(outgoing) = self.outgoing.clone() else {
            return Poll::Ready(Ok(()));
        };
        loop {
            let mut guard = lock(&outgoing);
            if guard.unsent.is_empty() {
                return Poll::Ready(Ok(()));
            }
            match Pin::new(&mut self.inner).poll_write(cx, &guard.unsent) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                }
                Poll::Ready(Ok(taken)) => {
                    guard.unsent.drain(..taken);
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for WireStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.outgoing.is_none() {
            return Pin::new(&mut this.inner).poll_write(cx, buf);
        }
        if let Err(error) = this.seal(buf) {
            return Poll::Ready(Err(error));
        }
        // The bytes are queued; what the socket cannot take now goes on the
        // next poll or flush.
        match this.poll_drain(cx) {
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            _ => Poll::Ready(Ok(buf.len())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match this.poll_drain(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.inner).poll_flush(cx),
            other => other,
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match this.poll_drain(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.inner).poll_shutdown(cx),
            other => other,
        }
    }
}

impl WireStream<tokio::net::TcpStream> {
    /// Move the connection out of the login worker's reactor into a blocking
    /// `std` socket (the caller sets it non-blocking), keeping the cipher.
    pub fn into_std(self) -> io::Result<WireStream<TcpStream>> {
        Ok(WireStream {
            inner: self.inner.into_std()?,
            inbound: self.inbound,
            outgoing: self.outgoing,
        })
    }
}

impl<S> std::ops::Deref for WireStream<S> {
    type Target = S;

    /// The socket's own `&self` methods (addresses, timeouts, `peek`,
    /// `shutdown`, `set_nonblocking`). Reads and writes go through the stream
    /// itself; the socket is not to be written to directly.
    fn deref(&self) -> &S {
        &self.inner
    }
}

impl WireStream<TcpStream> {
    /// A handle another thread writes frames through, in the masking order of
    /// this stream (see the module docs). The thread must stop before this
    /// stream writes again.
    pub fn sender(&self) -> io::Result<Sender> {
        Ok(Sender {
            socket: self.inner.try_clone()?,
            outgoing: self.outgoing.clone(),
        })
    }
}

/// Another thread's handle on a connection's write side.
#[derive(Debug)]
pub struct Sender {
    socket: TcpStream,
    outgoing: Option<SharedOutgoing>,
}

impl Sender {
    /// Send the plain client frames `plain`. What a full socket cannot take
    /// stays queued behind the stream's own bytes.
    pub fn send(&mut self, plain: &[u8]) -> io::Result<()> {
        let Some(outgoing) = &self.outgoing else {
            return self.socket.write_all(plain);
        };
        let mut guard = lock(outgoing);
        let Outgoing { cipher, unsent } = &mut *guard;
        cipher.seal(plain, unsent).map_err(invalid_data)?;
        drain_unsent(&mut self.socket, unsent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs910_protocol::proto;

    const SEEDS: [i32; 4] = [7, -8, 9, 10];

    fn loopback() -> (TcpStream, TcpStream) {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    /// Frames written through the stream arrive masked in order (also from
    /// the keep-alive's sender), and the server's masked frames read back
    /// plain, split anywhere.
    #[test]
    fn masks_both_directions_in_frame_order() {
        let (client, mut server) = loopback();
        let mut wire = WireStream::new(client, Some(WireCipher::new(SEEDS)));
        let mut sender = wire.sender().unwrap();
        let frame = [proto::client::MAP_BUILD_COMPLETE, 0, 0, 0, 1];
        wire.write_all(&frame).unwrap();
        sender.send(&[proto::client::NO_TIMEOUT]).unwrap();
        wire.write_all(&frame).unwrap();
        let mut got = [0u8; 11];
        server.read_exact(&mut got).unwrap();
        let mut expected = Vec::new();
        let mut cipher = OutboundCipher::new(SEEDS);
        cipher.seal(&frame, &mut expected).unwrap();
        cipher
            .seal(&[proto::client::NO_TIMEOUT], &mut expected)
            .unwrap();
        cipher.seal(&frame, &mut expected).unwrap();
        assert_eq!(got.to_vec(), expected);
        assert_ne!(got[0], frame[0], "the opcode is masked");

        // The server's frames, masked and sent in two segments: opcode 83 (one
        // byte, no payload) and opcode 129 (two bytes, no payload).
        assert_eq!(proto::server::size(proto::server::SERVER_TICK_END), Some(0));
        let plain_frames = [proto::server::NO_TIMEOUT, 128, 129];
        let mut generator = rs910_protocol::isaac_cipher::IsaacCipher::inbound_from_seeds(SEEDS);
        let masked_frames = plain_frames.map(|byte| byte.wrapping_add(generator.next_byte()));
        wire.set_nonblocking(true).unwrap();
        let mut seen = Vec::new();
        let started = std::time::Instant::now();
        for segment in [&masked_frames[..1], &masked_frames[1..]] {
            server.write_all(segment).unwrap();
            let wanted = seen.len() + segment.len();
            while seen.len() < wanted && started.elapsed() < std::time::Duration::from_secs(5) {
                let mut buf = [0u8; 8];
                match wire.read(&mut buf) {
                    Ok(n) => seen.extend_from_slice(&buf[..n]),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => panic!("{error}"),
                }
            }
        }
        assert_eq!(seen, plain_frames);
    }

    #[test]
    fn without_a_cipher_it_is_a_pass_through() {
        let (client, mut server) = loopback();
        let mut wire = WireStream::plain(client);
        wire.write_all(&[proto::client::NO_TIMEOUT]).unwrap();
        let mut byte = [0u8];
        server.read_exact(&mut byte).unwrap();
        assert_eq!(byte, [proto::client::NO_TIMEOUT]);
        assert!(!wire.is_masked());
    }
}
