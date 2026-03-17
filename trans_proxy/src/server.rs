use std::net::SocketAddr;

use anyhow::{Context, Result};
use tokio::{io::AsyncWriteExt, net::TcpListener, net::TcpStream};

use crate::{
    access::IpWhitelist,
    config::Config,
    protocol::{http, tls},
    upstream::{UpstreamProxy, bidirectional_copy},
};

#[derive(Clone, Copy, Debug)]
enum ProxyMode {
    Http,
    Https,
}

#[derive(Clone)]
struct AppState {
    ip_whitelist: IpWhitelist,
    upstream_proxy: Option<UpstreamProxy>,
}

pub async fn run(config: Config) -> Result<()> {
    let state = AppState {
        ip_whitelist: IpWhitelist::new(config.client_ip_whitelist.clone()),
        upstream_proxy: UpstreamProxy::from_config(&config.upstream_http_proxy)?,
    };

    let http_listener = TcpListener::bind(&config.http_bind)
        .await
        .with_context(|| format!("failed to bind HTTP listener on {}", config.http_bind))?;
    let https_listener = TcpListener::bind(&config.https_bind)
        .await
        .with_context(|| format!("failed to bind HTTPS listener on {}", config.https_bind))?;

    println!("transparent HTTP proxy listening on {}", config.http_bind);
    println!("transparent HTTPS proxy listening on {}", config.https_bind);
    if state.ip_whitelist.patterns().is_empty() {
        println!("client IP whitelist disabled");
    } else {
        println!("client IP whitelist enabled: {:?}", state.ip_whitelist.patterns());
    }
    if let Some(proxy) = &state.upstream_proxy {
        println!("upstream HTTP proxy enabled: {}", proxy.address());
    } else {
        println!("upstream HTTP proxy disabled");
    }

    let http_state = state.clone();
    let http_task = tokio::spawn(async move {
        if let Err(err) = accept_loop(http_listener, ProxyMode::Http, http_state).await {
            eprintln!("http accept loop exited: {err:#}");
        }
    });

    let https_state = state.clone();
    let https_task = tokio::spawn(async move {
        if let Err(err) = accept_loop(https_listener, ProxyMode::Https, https_state).await {
            eprintln!("https accept loop exited: {err:#}");
        }
    });

    let _ = tokio::join!(http_task, https_task);
    Ok(())
}

async fn accept_loop(listener: TcpListener, mode: ProxyMode, state: AppState) -> Result<()> {
    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, peer_addr, mode, state).await {
                eprintln!("[{mode:?}] {peer_addr} failed: {err:#}");
            }
        });
    }
}

async fn handle_connection(
    mut inbound: TcpStream,
    peer_addr: SocketAddr,
    mode: ProxyMode,
    state: AppState,
) -> Result<()> {
    if !state.ip_whitelist.is_allowed(peer_addr.ip()) {
        eprintln!("[{mode:?}] denied client {}", peer_addr.ip());
        return Ok(());
    }

    match mode {
        ProxyMode::Http => handle_http(&mut inbound, peer_addr, &state).await,
        ProxyMode::Https => handle_https(&mut inbound, peer_addr, &state).await,
    }
}

async fn handle_http(inbound: &mut TcpStream, peer_addr: SocketAddr, state: &AppState) -> Result<()> {
    let request_head = http::read_request_head(inbound).await?;
    let host_header = http::parse_host(&request_head)?;
    let (host, port) = http::split_host_port(&host_header, 80)?;
    let target = format!("{host}:{port}");

    println!("[Http] {peer_addr} -> {target}");

    let mut outbound = if let Some(proxy) = &state.upstream_proxy {
        let mut stream = proxy.connect_plain().await?;
        let rewritten = http::rewrite_request_for_upstream_proxy(
            &request_head,
            &host,
            port,
            proxy.authorization_header(),
        )?;
        stream
            .write_all(&rewritten)
            .await
            .with_context(|| format!("failed to write proxied HTTP request to upstream for {target}"))?;
        stream
    } else {
        let mut stream = TcpStream::connect(&target)
            .await
            .with_context(|| format!("failed to connect upstream {target}"))?;
        stream
            .write_all(&request_head)
            .await
            .with_context(|| format!("failed to write buffered bytes to {target}"))?;
        stream
    };

    bidirectional_copy(inbound, &mut outbound)
        .await
        .with_context(|| format!("failed while proxying {peer_addr} <-> {target}"))?;
    Ok(())
}

async fn handle_https(inbound: &mut TcpStream, peer_addr: SocketAddr, state: &AppState) -> Result<()> {
    let client_hello = tls::read_client_hello(inbound).await?;
    let host = tls::parse_sni(&client_hello)?;
    let target = format!("{host}:443");

    println!("[Https] {peer_addr} -> {target}");

    let mut outbound = if let Some(proxy) = &state.upstream_proxy {
        proxy.connect_tunnel(&host, 443).await?
    } else {
        TcpStream::connect(&target)
            .await
            .with_context(|| format!("failed to connect upstream {target}"))?
    };

    outbound
        .write_all(&client_hello)
        .await
        .with_context(|| format!("failed to write buffered tls client hello to {target}"))?;

    bidirectional_copy(inbound, &mut outbound)
        .await
        .with_context(|| format!("failed while proxying {peer_addr} <-> {target}"))?;
    Ok(())
}
