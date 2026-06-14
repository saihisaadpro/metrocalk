//! Local WebSocket transport (native; `ws` feature) — the fallback path and the Phase-2 collab
//! foundation (the same wire serves a real collab server unchanged). A polling reader thread shares
//! the socket with the writer via a mutex: it reads non-blocking, releasing the lock between polls so
//! `send` can interleave. `WsServer` is a minimal loopback echo server for the bench/tests.

use crate::{ConnectionState, DeltaTransport, OnRecv};
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::TransportError;

type Sock = Arc<Mutex<WebSocket<MaybeTlsStream<TcpStream>>>>;

pub struct WebSocketTransport {
    sock: Sock,
    state: Arc<Mutex<ConnectionState>>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    cb: Arc<Mutex<Option<OnRecv>>>,
}

impl WebSocketTransport {
    /// Connect to `ws://…`. Blocks for the handshake, then switches the socket to non-blocking.
    ///
    /// # Errors
    /// Returns [`TransportError`] if the connection or handshake fails.
    pub fn connect(url: &str) -> Result<Self, TransportError> {
        let (ws, _resp) =
            tungstenite::connect(url).map_err(|e| TransportError::Protocol(e.to_string()))?;
        if let MaybeTlsStream::Plain(s) = ws.get_ref() {
            s.set_nonblocking(true).map_err(TransportError::Io)?;
        }
        let sock: Sock = Arc::new(Mutex::new(ws));
        let state = Arc::new(Mutex::new(ConnectionState::Connected));
        let stop = Arc::new(AtomicBool::new(false));
        let cb: Arc<Mutex<Option<OnRecv>>> = Arc::new(Mutex::new(None));

        let reader = {
            let sock = sock.clone();
            let state = state.clone();
            let stop = stop.clone();
            let cb = cb.clone();
            thread::spawn(move || loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let msg = {
                    let mut g = sock.lock().unwrap();
                    g.read()
                };
                match msg {
                    Ok(Message::Binary(data)) => {
                        if let Some(f) = cb.lock().unwrap().as_mut() {
                            f(&data);
                        }
                    }
                    Ok(_) => {} // text/ping/pong/close-frame bookkeeping handled by tungstenite
                    Err(tungstenite::Error::Io(e))
                        if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
                    {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => {
                        *state.lock().unwrap() = ConnectionState::Disconnected;
                        break;
                    }
                }
            })
        };

        Ok(Self {
            sock,
            state,
            stop,
            reader: Some(reader),
            cb,
        })
    }
}

impl Drop for WebSocketTransport {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

impl DeltaTransport for WebSocketTransport {
    type Error = TransportError;
    fn send(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        let mut g = self.sock.lock().unwrap();
        g.send(Message::binary(frame.to_vec()))
            .map_err(|e| TransportError::Protocol(e.to_string()))
    }
    fn set_on_recv(&mut self, cb: OnRecv) {
        *self.cb.lock().unwrap() = Some(cb);
    }
    fn connection_state(&self) -> ConnectionState {
        *self.state.lock().unwrap()
    }
}

/// Minimal loopback WebSocket **echo** server for the bench/tests — accepts one connection and
/// bounces every binary message straight back (round-trips the enveloped frame).
pub struct WsServer {
    pub addr: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl WsServer {
    /// Bind an echo server on an OS-assigned localhost port.
    ///
    /// # Errors
    /// Returns [`TransportError`] if the listener cannot bind.
    pub fn spawn_echo() -> Result<Self, TransportError> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(TransportError::Io)?;
        let addr = format!(
            "ws://{}",
            listener.local_addr().map_err(TransportError::Io)?
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let handle = thread::spawn(move || {
            listener.set_nonblocking(true).ok();
            loop {
                if stop_t.load(Ordering::Relaxed) {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let stop_c = stop_t.clone();
                        thread::spawn(move || echo_conn(stream, &stop_c));
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            addr,
            stop,
            handle: Some(handle),
        })
    }
}

fn echo_conn(stream: TcpStream, stop: &AtomicBool) {
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    // `accept(TcpStream)` yields `WebSocket<TcpStream>` — the underlying stream is plain TCP.
    ws.get_ref().set_nonblocking(true).ok();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match ws.read() {
            Ok(Message::Binary(d)) => {
                if ws.send(Message::binary(d)).is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => break,
        }
    }
}

impl Drop for WsServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
