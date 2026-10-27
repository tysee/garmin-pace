//! src/interface/listener.rs bounds sockets before HTTP parsing or authentication.
//! BoundedListener owns connection permits; each stream has a non-resetting lifetime deadline.

use axum::serve::Listener;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Sleep,
};

pub(super) struct BoundedListener {
    listener: TcpListener,
    permits: Arc<Semaphore>,
    lifetime: Duration,
}
impl BoundedListener {
    pub(super) fn new(listener: TcpListener, capacity: usize, lifetime: Duration) -> Self {
        Self {
            listener,
            permits: Arc::new(Semaphore::new(capacity)),
            lifetime,
        }
    }
}
impl Listener for BoundedListener {
    type Io = BoundedStream;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            // This semaphore is never closed; dropping accept also drops the pending permit.
            let permit = match self.permits.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => unreachable!("listener owns the open connection semaphore"),
            };
            match self.listener.accept().await {
                Ok((stream, address)) => {
                    return (
                        BoundedStream {
                            stream,
                            _permit: permit,
                            deadline: Box::pin(tokio::time::sleep(self.lifetime)),
                        },
                        address,
                    );
                }
                Err(_) => {
                    tracing::warn!("HTTP accept failed; retrying after a bounded delay");
                    drop(permit);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

pub(super) struct BoundedStream {
    stream: TcpStream,
    _permit: OwnedSemaphorePermit,
    deadline: Pin<Box<Sleep>>,
}
impl BoundedStream {
    fn check_deadline(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        use std::future::Future;
        if self.deadline.as_mut().poll(cx).is_ready() {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HTTP connection lifetime exceeded",
            ))
        } else {
            Ok(())
        }
    }
}
impl AsyncRead for BoundedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Err(error) = self.check_deadline(cx) {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for BoundedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Err(error) = self.check_deadline(cx) {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Err(error) = self.check_deadline(cx) {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
