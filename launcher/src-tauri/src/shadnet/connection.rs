//! One TCP connection to a shadNet server: the ServerInfo handshake, then a reader thread that
//! matches replies to requests by packet id and hands notifications to a callback.

use super::packet::{self, Header, HEADER_SIZE};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum NetError {
    Unreachable(String),
    Protocol(String),
    Timeout,
    Closed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub error: u8,
    pub payload: Vec<u8>,
}

pub enum Event {
    Notification(u16, Vec<u8>),
    Closed,
}

pub struct Connection {
    stream: TcpStream,
    writer: Mutex<TcpStream>,
    pending: Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    next_id: AtomicU64,
    closed: Arc<(Mutex<bool>, Condvar)>,
    closing: AtomicBool,
}

pub fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, NetError> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| NetError::Unreachable(format!("cannot resolve {host}: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(NetError::Unreachable(format!("cannot resolve {host}")));
    }
    Ok(addrs)
}

pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, NetError> {
    let mut last = String::new();
    for addr in resolve(host, port)? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last = e.to_string(),
        }
    }
    Err(NetError::Unreachable(last))
}

/// Whether something accepts TCP connections there.
pub fn port_open(host: &str, port: u16, timeout: Duration) -> bool {
    connect(host, port, timeout).map(|s| s.shutdown(Shutdown::Both)).is_ok()
}

fn read_packet(r: &mut impl Read) -> std::io::Result<(Header, Vec<u8>)> {
    let mut h = [0u8; HEADER_SIZE];
    r.read_exact(&mut h)?;
    let header = Header::parse(&h);
    let len = header
        .payload_len()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad packet size"))?;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((header, payload))
}

impl Connection {
    pub fn open(
        host: &str,
        port: u16,
        timeout: Duration,
        on_event: impl Fn(Event) + Send + 'static,
    ) -> Result<Arc<Connection>, NetError> {
        let mut stream = connect(host, port, timeout)?;
        let _ = stream.set_nodelay(true);
        let keepalive = socket2::TcpKeepalive::new().with_time(Duration::from_secs(20)).with_interval(Duration::from_secs(5));
        let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(&keepalive);
        stream.set_read_timeout(Some(timeout)).map_err(|e| NetError::Protocol(e.to_string()))?;
        let (header, payload) = read_packet(&mut stream).map_err(|e| match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => NetError::Timeout,
            _ => NetError::Protocol(format!("no ServerInfo: {e}")),
        })?;
        match packet::server_info_version(&header, &payload) {
            Some(packet::PROTOCOL_VERSION) => {}
            Some(v) => return Err(NetError::Protocol(format!("server protocol version {v}, expected 1"))),
            None => return Err(NetError::Protocol("the server did not start with ServerInfo".into())),
        }
        stream.set_read_timeout(None).map_err(|e| NetError::Protocol(e.to_string()))?;
        stream.set_write_timeout(Some(timeout)).map_err(|e| NetError::Protocol(e.to_string()))?;
        let clone = |s: &TcpStream| s.try_clone().map_err(|e| NetError::Protocol(e.to_string()));
        let conn = Arc::new(Connection {
            writer: Mutex::new(clone(&stream)?),
            stream: clone(&stream)?,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            closed: Arc::new((Mutex::new(false), Condvar::new())),
            closing: AtomicBool::new(false),
        });
        let pending = conn.pending.clone();
        let closed = conn.closed.clone();
        std::thread::spawn(move || {
            let mut reader = stream;
            while let Ok((header, payload)) = read_packet(&mut reader) {
                match header.kind {
                    packet::REPLY => {
                        let waiter = pending.lock().unwrap().remove(&header.id);
                        if let (Some(tx), Some((&error, rest))) = (waiter, payload.split_first()) {
                            let _ = tx.send(Reply { error, payload: rest.to_vec() });
                        }
                    }
                    packet::NOTIFICATION => on_event(Event::Notification(header.command, payload)),
                    _ => {}
                }
            }
            pending.lock().unwrap().clear();
            *closed.0.lock().unwrap() = true;
            closed.1.notify_all();
            on_event(Event::Closed);
        });
        Ok(conn)
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.0.lock().unwrap()
    }

    /// Whether `close` was called (the end of the connection was asked for, not suffered).
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    pub fn request(&self, command: u16, payload: &[u8], timeout: Duration) -> Result<Reply, NetError> {
        if self.is_closed() {
            return Err(NetError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let bytes = packet::request(command, id, payload);
        if self.writer.lock().unwrap().write_all(&bytes).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(NetError::Closed);
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => Ok(r),
            Err(RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(NetError::Timeout)
            }
            Err(RecvTimeoutError::Disconnected) => Err(NetError::Closed),
        }
    }

    fn wait_closed(&self, timeout: Duration) -> bool {
        let guard = self.closed.0.lock().unwrap();
        let (guard, _) = self.closed.1.wait_timeout_while(guard, timeout, |c| !*c).unwrap();
        *guard
    }

    /// Clean disconnect: `Terminate (1)`, then close the socket. The protocol says the server closes
    /// it after the reply, but the current server fails to (it logs "No such method
    /// QTcpSocket::disconnectFromHost()"), so the launcher closes its side after a short wait; the
    /// server drops the session as soon as it sees the socket close.
    pub fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        if !self.is_closed() {
            let _ = self.request(packet::TERMINATE, &[], Duration::from_secs(3));
            self.wait_closed(Duration::from_millis(300));
        }
        let _ = self.stream.shutdown(Shutdown::Both);
        self.wait_closed(Duration::from_secs(1));
    }

    /// Drops the connection without telling the server (it is unreachable anyway).
    pub fn abort(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn server_info() -> Vec<u8> {
        let mut p = vec![packet::SERVER_INFO, 0, 0, 19, 0, 0, 0];
        p.extend([0u8; 8]);
        p.extend(1u32.to_le_bytes());
        p
    }

    fn reply(command: u16, id: u64, error: u8, body: &[u8]) -> Vec<u8> {
        let mut p = packet::request(command, id, &[[error].as_slice(), body].concat());
        p[0] = packet::REPLY;
        p
    }

    #[test]
    fn replies_notifications_and_terminate_over_a_fake_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(&server_info()).unwrap();
            let (h, body) = read_packet(&mut s).unwrap();
            assert_eq!((h.kind, h.command), (packet::REQUEST, packet::ADD_FRIEND));
            assert_eq!(packet::read_blob(&body).unwrap(), crate::shadnet::proto::friend_command("Bob"));
            let mut n = packet::request(proto_lost(), 0, &packet::blob(&crate::shadnet::proto::friend_command("Bob")));
            n[0] = packet::NOTIFICATION;
            s.write_all(&n).unwrap();
            s.write_all(&reply(packet::ADD_FRIEND, h.id, 26, &[])).unwrap();
            let (h, _) = read_packet(&mut s).unwrap();
            assert_eq!(h.command, packet::TERMINATE);
            s.write_all(&reply(packet::TERMINATE, h.id, 0, &[])).unwrap();
        });
        let (tx, rx) = mpsc::channel();
        let conn = Connection::open("127.0.0.1", port, Duration::from_secs(2), move |e| {
            let _ = tx.send(match e {
                Event::Notification(k, p) => Some((k, p)),
                Event::Closed => None,
            });
        })
        .unwrap();
        let r = conn.request(packet::ADD_FRIEND, &packet::blob(&crate::shadnet::proto::friend_command("Bob")), Duration::from_secs(2));
        assert_eq!(r, Ok(Reply { error: 26, payload: vec![] }));
        let (kind, payload) = rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        assert_eq!(kind, proto_lost());
        assert_eq!(packet::read_blob(&payload).unwrap(), crate::shadnet::proto::friend_command("Bob"));
        conn.close();
        assert!(conn.is_closed());
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_none());
        server.join().unwrap();
        assert_eq!(conn.request(packet::ADD_FRIEND, &[], Duration::from_millis(100)), Err(NetError::Closed));
    }

    fn proto_lost() -> u16 {
        crate::shadnet::proto::FRIEND_LOST
    }

    #[test]
    fn a_server_that_does_not_greet_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(&reply(0, 0, 0, &[1, 2, 3, 4])).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let r = Connection::open("127.0.0.1", port, Duration::from_secs(2), |_| {});
        assert!(matches!(r, Err(NetError::Protocol(_))));
        server.join().unwrap();
        assert!(!port_open("127.0.0.1", port, Duration::from_millis(300)));
    }
}
