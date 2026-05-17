// SPDX-FileCopyrightText: 2026 Matthias Wende
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::Notify,
};

/// IO wrapper that signals activity on reads/writes via a [`Notify`].
///
/// When any bytes flow through the connection the shared `Notify` is signalled,
/// allowing an external idle-timeout sleep to be reset.
///
/// `Notify::notify_one()` is intentionally used as a lossy edge trigger here.
/// Multiple completed reads/writes may collapse into one pending notification,
/// but idle-timeout handling only needs to know that some real byte activity
/// happened since the last reset.
pub(crate) struct ActivityIo<T> {
    inner: T,
    activity: Arc<Notify>,
}

impl<T> ActivityIo<T> {
    /// Creates a new activity-signalling IO wrapper.
    ///
    /// # Arguments
    /// * `inner` - Wrapped IO object.
    /// * `activity` - Shared notification used to signal non-zero byte activity.
    ///
    /// # Returns
    /// A new [`ActivityIo`] wrapper around `inner`.
    pub(crate) fn new(inner: T, activity: Arc<Notify>) -> Self {
        Self { inner, activity }
    }
}

impl<T> AsyncRead for ActivityIo<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let filled_before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > filled_before {
                    self.activity.notify_one();
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl<T> AsyncWrite for ActivityIo<T>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(written)) => {
                if written > 0 {
                    self.activity.notify_one();
                }
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write_vectored(cx, bufs) {
            Poll::Ready(Ok(written)) => {
                if written > 0 {
                    self.activity.notify_one();
                }
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        task::{Context, Poll},
        time::Duration,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Default)]
    struct MockIo {
        read_data: Vec<u8>,
        read_offset: usize,
        write_data: Vec<u8>,
    }

    impl MockIo {
        fn with_read_data(read_data: &[u8]) -> Self {
            Self {
                read_data: read_data.to_vec(),
                read_offset: 0,
                write_data: Vec::new(),
            }
        }
    }

    impl AsyncRead for MockIo {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.read_offset >= self.read_data.len() {
                return Poll::Ready(Ok(()));
            }

            let remaining = &self.read_data[self.read_offset..];
            let to_copy = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            self.read_offset += to_copy;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for MockIo {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.write_data.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn activity_io_notifies_on_non_empty_read_and_write() {
        let activity = Arc::new(Notify::new());
        let mut io = ActivityIo::new(MockIo::with_read_data(b"hello"), Arc::clone(&activity));

        let read_notification = activity.notified();
        let mut buffer = [0_u8; 5];
        let read = io.read(&mut buffer).await.expect("read succeeds");
        assert_eq!(read, 5);
        tokio::time::timeout(Duration::from_millis(50), read_notification)
            .await
            .expect("read should notify");

        let write_notification = activity.notified();
        let written = io.write(b"world").await.expect("write succeeds");
        assert_eq!(written, 5);
        assert_eq!(io.inner.write_data, b"world");
        tokio::time::timeout(Duration::from_millis(50), write_notification)
            .await
            .expect("write should notify");
    }

    #[tokio::test]
    async fn activity_io_does_not_notify_on_zero_byte_read() {
        let activity = Arc::new(Notify::new());
        let mut io = ActivityIo::new(MockIo::default(), Arc::clone(&activity));

        let notification = activity.notified();
        let mut buffer = [0_u8; 8];
        let read = io.read(&mut buffer).await.expect("read succeeds");
        assert_eq!(read, 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), notification)
                .await
                .is_err(),
            "zero-byte read should not notify"
        );
    }

    #[tokio::test]
    async fn activity_io_write_vectored_notifies_on_non_zero_write() {
        let activity = Arc::new(Notify::new());
        let stream = tokio::io::duplex(64);
        let mut wrapped = ActivityIo::new(stream.0, Arc::clone(&activity));
        let mut reader = stream.1;

        let notification = activity.notified();
        let written = wrapped
            .write_vectored(&[io::IoSlice::new(b"ab"), io::IoSlice::new(b"cd")])
            .await
            .expect("vectored write");
        assert_eq!(written, 4);

        tokio::time::timeout(Duration::from_millis(50), notification)
            .await
            .expect("vectored write should notify");

        let mut buf = [0_u8; 4];
        reader.read_exact(&mut buf).await.expect("read data");
        assert_eq!(&buf, b"abcd");
    }
}
