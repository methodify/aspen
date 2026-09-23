//! The node's listener with TLS (docs/TLS.md §3): one port serves both
//! `http://` and `https://` — the first byte of each connection says which
//! (a TLS ClientHello starts with 0x16) — or, with `--tls-listen`, a
//! second port that is TLS only. Either way the result is an
//! `axum::serve::Listener`, so routing, the graceful shutdown ladder and
//! the node token work exactly as they do over plain HTTP.
//!
//! The peek and the handshake run in a task per connection, so a client
//! that connects and says nothing never holds the accept loop.

use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

/// A plain or TLS-wrapped connection.
pub enum Io {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl AsyncRead for Io {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Io {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_flush(cx),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
    fn is_write_vectored(&self) -> bool {
        match self {
            Io::Plain(s) => s.is_write_vectored(),
            Io::Tls(s) => s.is_write_vectored(),
        }
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// http and https on one port, told apart by the first byte.
    Sniff,
    /// https only.
    TlsOnly,
    /// http only (the main port when `--tls-listen` carries https).
    Plain,
}

pub struct TlsListener {
    rx: mpsc::Receiver<(Io, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    pub fn new(listener: TcpListener, acceptor: TlsAcceptor, mode: Mode) -> Self {
        let local = listener
            .local_addr()
            .expect("bound listener has an address");
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            loop {
                let (stream, addr) = match listener.accept().await {
                    Ok(x) => x,
                    Err(e) => {
                        // Out of descriptors, say: wait a beat rather than
                        // spin (what axum's own TcpListener impl does).
                        tracing::warn!("accept failed: {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let tx = tx.clone();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let _ = stream.set_nodelay(true);
                    let tls = match mode {
                        Mode::TlsOnly => true,
                        Mode::Plain => false,
                        Mode::Sniff => {
                            let mut b = [0u8; 1];
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                stream.peek(&mut b),
                            )
                            .await
                            {
                                Ok(Ok(1)) => b[0] == 0x16,
                                // Nothing said in 10 s, or gone: hand it to
                                // hyper as plain; it closes idle ones.
                                _ => false,
                            }
                        }
                    };
                    let io = if tls {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(15),
                            acceptor.accept(stream),
                        )
                        .await
                        {
                            Ok(Ok(s)) => Io::Tls(Box::new(s)),
                            Ok(Err(e)) => {
                                tracing::debug!(%addr, "tls handshake failed: {e}");
                                return;
                            }
                            Err(_) => return,
                        }
                    } else {
                        Io::Plain(stream)
                    };
                    let _ = tx.send((io, addr)).await;
                });
            }
        });
        Self { rx, local }
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = Io;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(x) => x,
            // The acceptor task never ends while we live; if it somehow
            // did, park rather than spin.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}
