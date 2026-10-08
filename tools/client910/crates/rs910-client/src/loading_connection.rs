//! Keep the world connection alive while the startup owner builds resources.
//! The map rebuild sends NO_TIMEOUT while it runs; normal play sends it after
//! 50 idle logic updates. This adapter
//! uses a one-second cadence while synchronous native resource work owns the
//! calling thread. It must be dropped before another writer uses the socket.
use std::{net::TcpStream, sync::mpsc, thread, time::Duration};

use crate::wire_stream::WireStream;
pub struct LoadingConnection {
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<std::io::Result<()>>>,
}
impl LoadingConnection {
    /// The keep-alive writes through `stream`'s own masking, so its frames
    /// take their place in the connection's cipher sequence.
    pub fn start(stream: &WireStream<TcpStream>) -> std::io::Result<Self> {
        let mut sender = stream.sender()?;
        let (stop, receive) = mpsc::channel();
        let worker = thread::spawn(move || loop {
            match sender.send(&[crate::proto::client::NO_TIMEOUT]) {
                Ok(()) => {
                    crate::session_record::record(b"LOUT", &[crate::proto::client::NO_TIMEOUT])
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
            match receive.recv_timeout(Duration::from_secs(1)) {
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                _ => return Ok(()),
            }
        });
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
    pub fn finish(mut self) -> std::io::Result<()> {
        let _ = self.stop.send(());
        self.worker
            .take()
            .unwrap()
            .join()
            .map_err(|_| std::io::Error::other("loading connection worker panicked"))?
    }
}
impl Drop for LoadingConnection {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn loading_keepalive_stops_before_normal_writer_resumes() -> std::io::Result<()> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let mut client = WireStream::plain(TcpStream::connect(listener.local_addr()?)?);
        let (mut server, _) = listener.accept()?;
        server.set_read_timeout(Some(Duration::from_secs(3)))?;
        let loading = LoadingConnection::start(&client)?;
        let mut byte = [0];
        server.read_exact(&mut byte)?;
        assert_eq!(byte, [103]);
        loading.finish()?;
        client.write_all(&[33, 0, 1, 0, 2, 0])?;
        let mut packet = [0; 6];
        server.read_exact(&mut packet)?;
        assert_eq!(packet, [33, 0, 1, 0, 2, 0]);
        Ok(())
    }
}

/// The idle-update counter of the game update and the connection flush. It is
/// advanced by logic updates and reset by actual socket writes, not redraws.
#[derive(Default)]
pub struct IdleConnection(i32);
impl IdleConnection {
    pub fn tick(&mut self, outgoing: &mut Vec<u8>) {
        self.0 = self.0.wrapping_add(1);
        if self.0 > 50 {
            outgoing.push(crate::proto::client::NO_TIMEOUT);
        }
    }
    pub fn wrote(&mut self) {
        self.0 = 0;
    }
}
#[cfg(test)]
#[test]
fn idle_connection_times_out_at_the_update_boundary() {
    let mut idle = IdleConnection::default();
    let mut bytes = vec![];
    for _ in 0..50 {
        idle.tick(&mut bytes);
    }
    assert!(bytes.is_empty());
    idle.tick(&mut bytes);
    assert_eq!(bytes, [103]);
    idle.wrote();
    bytes.clear();
    for _ in 0..50 {
        idle.tick(&mut bytes);
    }
    assert!(bytes.is_empty());
}
