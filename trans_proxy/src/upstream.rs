use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::{
    io,
    pin::Pin,
    task::{Context as TaskContext, Poll},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::{
    io::{
        AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, copy_bidirectional_with_sizes,
    },
    net::TcpStream,
};

use crate::config::UpstreamHttpProxyConfig;
use crate::protocol::http::format_authority;

#[derive(Clone, Debug)]
pub struct UpstreamProxy {
    address: String,
    authorization_header: Option<String>,
}

impl UpstreamProxy {
    pub fn from_config(config: &UpstreamHttpProxyConfig) -> Result<Option<Self>> {
        if !config.enabled {
            return Ok(None);
        }

        let address = config
            .address
            .clone()
            .ok_or_else(|| anyhow!("upstream HTTP proxy is enabled but address is missing"))?;

        let authorization_header = match (&config.username, &config.password) {
            (Some(username), Some(password)) => {
                let credential = STANDARD.encode(format!("{username}:{password}"));
                Some(format!("Basic {credential}"))
            }
            (None, None) => None,
            _ => bail!("upstream HTTP proxy auth requires both username and password"),
        };

        Ok(Some(Self {
            address,
            authorization_header,
        }))
    }

    pub fn authorization_header(&self) -> Option<&str> {
        self.authorization_header.as_deref()
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub async fn connect_tunnel(&self, host: &str, port: u16) -> Result<TcpStream> {
        let target = connect_authority(host, port);
        let mut stream = TcpStream::connect(&self.address)
            .await
            .with_context(|| format!("failed to connect upstream proxy {}", self.address))?;

        let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
        if let Some(value) = &self.authorization_header {
            request.push_str("Proxy-Authorization: ");
            request.push_str(value);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");

        stream
            .write_all(request.as_bytes())
            .await
            .context("failed to send CONNECT request to upstream proxy")?;

        let response = read_http_response_head(&mut stream).await?;
        ensure_connect_success(&response)?;
        Ok(stream)
    }

    pub async fn connect_plain(&self) -> Result<TcpStream> {
        TcpStream::connect(&self.address)
            .await
            .with_context(|| format!("failed to connect upstream proxy {}", self.address))
    }
}

async fn read_http_response_head(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];

    loop {
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(buf);
        }

        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            bail!("upstream proxy closed before CONNECT response completed");
        }

        buf.extend_from_slice(&chunk[..read]);
        if buf.len() > 16 * 1024 {
            bail!("upstream proxy CONNECT response is too large");
        }
    }
}

fn ensure_connect_success(response: &[u8]) -> Result<()> {
    let text =
        std::str::from_utf8(response).context("upstream CONNECT response is not valid utf-8")?;
    let status_line = text
        .lines()
        .next()
        .ok_or_else(|| anyhow!("upstream CONNECT response is empty"))?;

    let mut parts = status_line.split_whitespace();
    let _http_version = parts
        .next()
        .ok_or_else(|| anyhow!("missing CONNECT response version"))?;
    let status_code = parts
        .next()
        .ok_or_else(|| anyhow!("missing CONNECT response status code"))?;
    if !status_code.starts_with('2') {
        bail!("upstream CONNECT failed with status line: {status_line}");
    }

    Ok(())
}

pub async fn bidirectional_copy(
    inbound: &mut TcpStream,
    outbound: &mut TcpStream,
    uploaded: Arc<AtomicU64>,
    downloaded: Arc<AtomicU64>,
) -> Result<()> {
    const COPY_BUF_SIZE: usize = 16 * 1024;

    let mut inbound = CountedStream::new(inbound, downloaded);
    let mut outbound = CountedStream::new(outbound, uploaded);

    let _ =
        copy_bidirectional_with_sizes(&mut inbound, &mut outbound, COPY_BUF_SIZE, COPY_BUF_SIZE)
            .await?;
    Ok(())
}

fn connect_authority(host: &str, port: u16) -> String {
    let host = format_authority(host, port, port);
    format!("{host}:{port}")
}

struct CountedStream<'a> {
    inner: &'a mut TcpStream,
    write_counter: Arc<AtomicU64>,
}

impl<'a> CountedStream<'a> {
    fn new(inner: &'a mut TcpStream, write_counter: Arc<AtomicU64>) -> Self {
        Self {
            inner,
            write_counter,
        }
    }
}

impl AsyncRead for CountedStream<'_> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut *this.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for CountedStream<'_> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut *this.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(written)) => {
                this.write_counter
                    .fetch_add(written as u64, Ordering::Relaxed);
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut *this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut *this.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut *this.inner).poll_write_vectored(cx, bufs) {
            Poll::Ready(Ok(written)) => {
                this.write_counter
                    .fetch_add(written as u64, Ordering::Relaxed);
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}
