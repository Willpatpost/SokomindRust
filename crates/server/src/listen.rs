use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use std::io::{self, IoSlice};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, Sleep};

/// Longer than the 30 s solve cap and nginx's 40 s proxy_read_timeout, so
/// only connections that move no bytes for a full minute are dropped.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Matches axum's own listener: fd exhaustion clears up on its own.
const ACCEPT_BACKOFF: Duration = Duration::from_secs(1);

/// TCP listener that stops accepting at `max` open connections, leaving
/// newcomers in the kernel backlog, and drops connections left idle.
pub struct Capped {
    listener: TcpListener,
    slots: Arc<Semaphore>,
    idle: Duration,
}
impl Capped {
    pub async fn bind(addr: &str, max: usize) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(addr).await?,
            slots: Arc::new(Semaphore::new(max)),
            idle: IDLE_TIMEOUT,
        })
    }
}
impl Listener for Capped {
    type Io = Conn;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Conn, SocketAddr) {
        let slot = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .expect("connection semaphore is never closed");
        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => {
                    let conn = Conn {
                        stream,
                        idle: Box::pin(tokio::time::sleep(self.idle)),
                        timeout: self.idle,
                        progress: Instant::now(),
                        _slot: slot,
                    };
                    return (conn, addr);
                }
                // The peer gave up before we got to it; nothing to wait out.
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::ConnectionReset
                    ) => {}
                Err(error) => {
                    eprintln!("accept failed: {error}");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            }
        }
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// Accepted connection; holds its slot until hyper drops it.
pub struct Conn {
    stream: TcpStream,
    idle: Pin<Box<Sleep>>,
    timeout: Duration,
    progress: Instant,
    _slot: OwnedSemaphorePermit,
}
impl Conn {
    /// A completed read or write restarts the idle clock; a pending one
    /// fails once the clock has run out, which makes hyper close the
    /// connection.
    fn track<T>(&mut self, cx: &mut Context<'_>, poll: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        if poll.is_ready() {
            self.progress = Instant::now();
            return poll;
        }
        loop {
            ready!(self.idle.as_mut().poll(cx));
            let deadline = self.progress + self.timeout;
            if deadline <= Instant::now() {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "connection idle",
                )));
            }
            self.idle.as_mut().reset(deadline);
        }
    }
}
impl AsyncRead for Conn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.stream).poll_read(cx, buf);
        this.track(cx, poll)
    }
}
impl AsyncWrite for Conn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.stream).poll_write(cx, buf);
        this.track(cx, poll)
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.stream).poll_write_vectored(cx, bufs);
        this.track(cx, poll)
    }
    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
    // Not tracked: hyper flushes on every wakeup, even with nothing
    // buffered, so counting flushes would keep an idle connection alive
    // on the idle timer's own wakeups.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

/// Socket peer of a `Capped` connection. axum only provides `SocketAddr`
/// connect info for its own listeners, and the orphan rule forbids adding it.
#[derive(Clone, Copy)]
pub struct Peer(pub SocketAddr);
impl Connected<IncomingStream<'_, Capped>> for Peer {
    fn connect_info(stream: IncomingStream<'_, Capped>) -> Self {
        Peer(*stream.remote_addr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn pair(max: usize) -> (Capped, SocketAddr) {
        let listener = Capped::bind("127.0.0.1:0", max).await.unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr)
    }

    #[tokio::test]
    async fn accepts_up_to_the_cap_until_a_slot_frees() {
        let (mut listener, addr) = pair(1).await;
        let _first = TcpStream::connect(addr).await.unwrap();
        let (conn, _) = listener.accept().await;
        let _second = TcpStream::connect(addr).await.unwrap();
        let waiting = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(waiting.is_err(), "accepted past the cap");
        drop(conn);
        tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("slot was freed");
    }

    #[tokio::test]
    async fn idle_connections_time_out_and_only_traffic_resets_the_clock() {
        let idle = Duration::from_millis(600);
        let (mut listener, addr) = pair(4).await;
        listener.idle = idle;
        let mut client = TcpStream::connect(addr).await.unwrap();
        let started = Instant::now();
        let (mut conn, _) = listener.accept().await;
        tokio::time::sleep(idle / 3).await;
        client.write_all(b"x").await.unwrap();
        let mut byte = [0; 1];
        conn.read_exact(&mut byte).await.unwrap();
        let traffic = started.elapsed();
        tokio::time::sleep(idle * 2 / 3).await;
        // hyper flushes on every wakeup; that is not traffic.
        conn.flush().await.unwrap();
        let error = conn.read(&mut byte).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // The read restarted the clock past the first deadline; the flush
        // did not restart it again.
        let elapsed = started.elapsed();
        assert!(elapsed >= idle / 3 + idle, "{elapsed:?}");
        assert!(elapsed < traffic + idle + idle / 2, "{elapsed:?}");
    }
}
