use anyhow::{Context, Result, anyhow, bail};
use tokio::{io::AsyncReadExt, net::TcpStream};

const MAX_HTTP_HEADER_SIZE: usize = 64 * 1024;

pub async fn read_request_head(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 2048];

    loop {
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(buf);
        }

        if buf.len() >= MAX_HTTP_HEADER_SIZE {
            bail!("http header exceeds {MAX_HTTP_HEADER_SIZE} bytes");
        }

        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            bail!("client closed before sending a complete HTTP header");
        }

        buf.extend_from_slice(&chunk[..read]);
    }
}

pub fn parse_host(head: &[u8]) -> Result<String> {
    let header_end = find_header_end(head)?;
    let text = std::str::from_utf8(&head[..header_end]).context("http header is not valid utf-8")?;

    for line in text.lines() {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("host")
        {
            let host = value.trim();
            if host.is_empty() {
                bail!("host header is empty");
            }
            return Ok(host.to_string());
        }
    }

    Err(anyhow!("missing Host header"))
}

pub fn rewrite_request_for_upstream_proxy(
    head: &[u8],
    host: &str,
    port: u16,
    proxy_authorization: Option<&str>,
) -> Result<Vec<u8>> {
    let header_end = find_header_end(head)?;
    let head_text = std::str::from_utf8(&head[..header_end]).context("http header is not valid utf-8")?;
    let body_prefix = &head[header_end + 4..];
    let mut lines = head_text.split("\r\n");

    let request_line = lines.next().ok_or_else(|| anyhow!("missing http request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or_else(|| anyhow!("missing http method"))?;
    let path = parts.next().ok_or_else(|| anyhow!("missing http request target"))?;
    let version = parts.next().ok_or_else(|| anyhow!("missing http version"))?;

    let absolute_target = if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("http://{}{}", format_authority(host, port, 80), path)
    };

    let mut output = String::new();
    output.push_str(method);
    output.push(' ');
    output.push_str(&absolute_target);
    output.push(' ');
    output.push_str(version);
    output.push_str("\r\n");

    let mut has_proxy_authorization = false;

    for line in lines {
        if line.is_empty() {
            continue;
        }

        if let Some((name, _)) = line.split_once(':')
            && name.eq_ignore_ascii_case("proxy-authorization")
        {
            has_proxy_authorization = true;
        }

        output.push_str(line);
        output.push_str("\r\n");
    }

    if !has_proxy_authorization
        && let Some(value) = proxy_authorization
    {
        output.push_str("Proxy-Authorization: ");
        output.push_str(value);
        output.push_str("\r\n");
    }

    output.push_str("\r\n");

    let mut bytes = output.into_bytes();
    bytes.extend_from_slice(body_prefix);
    Ok(bytes)
}

fn find_header_end(head: &[u8]) -> Result<usize> {
    head.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("missing http header terminator"))
}

pub fn split_host_port(host: &str, default_port: u16) -> Result<(String, u16)> {
    let host = host.trim();
    if host.is_empty() {
        bail!("empty host");
    }

    if let Some(rest) = host.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| anyhow!("invalid ipv6 host"))?;
        let hostname = &rest[..end];
        let remainder = &rest[end + 1..];
        let port = if let Some(port_text) = remainder.strip_prefix(':') {
            port_text.parse::<u16>().context("invalid host port")?
        } else {
            default_port
        };
        return Ok((hostname.to_string(), port));
    }

    if let Some((hostname, port_text)) = host.rsplit_once(':')
        && !hostname.contains(':')
    {
        let port = port_text.parse::<u16>().context("invalid host port")?;
        return Ok((hostname.to_string(), port));
    }

    Ok((host.to_string(), default_port))
}

pub fn format_authority(host: &str, port: u16, default_port: u16) -> String {
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    if port == default_port {
        host
    } else {
        format!("{host}:{port}")
    }
}
