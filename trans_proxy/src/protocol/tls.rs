use anyhow::{Context, Result, anyhow, bail};
use tokio::{io::AsyncReadExt, net::TcpStream};

const MAX_TLS_CLIENT_HELLO_SIZE: usize = 16 * 1024;

pub async fn read_client_hello(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut header = [0_u8; 5];
    stream
        .read_exact(&mut header)
        .await
        .context("failed to read TLS record header")?;

    if header[0] != 0x16 {
        bail!("not a TLS handshake record");
    }

    let record_len = u16::from_be_bytes([header[3], header[4]]) as usize;
    if record_len == 0 || record_len > MAX_TLS_CLIENT_HELLO_SIZE {
        bail!("invalid TLS record length {record_len}");
    }

    let mut payload = vec![0_u8; record_len];
    stream
        .read_exact(&mut payload)
        .await
        .with_context(|| format!("failed to read TLS payload ({record_len} bytes)"))?;

    let mut record = header.to_vec();
    record.extend_from_slice(&payload);
    Ok(record)
}

pub fn parse_sni(record: &[u8]) -> Result<String> {
    if record.len() < 5 {
        bail!("short TLS record");
    }

    let payload = &record[5..];
    let mut cursor = 0;

    let handshake_type = *payload
        .get(cursor)
        .ok_or_else(|| anyhow!("missing handshake type"))?;
    cursor += 1;
    if handshake_type != 0x01 {
        bail!("first TLS handshake is not ClientHello");
    }

    let hello_len = read_u24(payload, &mut cursor)? as usize;
    if payload.len().saturating_sub(cursor) < hello_len {
        bail!("truncated TLS ClientHello");
    }

    let body_end = cursor + hello_len;

    cursor += 2;
    cursor += 32;

    let session_id_len = read_u8(payload, &mut cursor)? as usize;
    cursor += session_id_len;

    let cipher_suites_len = read_u16(payload, &mut cursor)? as usize;
    cursor += cipher_suites_len;

    let compression_methods_len = read_u8(payload, &mut cursor)? as usize;
    cursor += compression_methods_len;

    if cursor == body_end {
        bail!("tls client hello has no extensions");
    }

    let extensions_len = read_u16(payload, &mut cursor)? as usize;
    if cursor + extensions_len > body_end {
        bail!("truncated tls extensions block");
    }
    let extensions_end = cursor + extensions_len;

    while cursor + 4 <= extensions_end {
        let ext_type = read_u16(payload, &mut cursor)?;
        let ext_len = read_u16(payload, &mut cursor)? as usize;
        if cursor + ext_len > extensions_end {
            bail!("truncated tls extension payload");
        }

        if ext_type == 0x0000 {
            return parse_sni_extension(&payload[cursor..cursor + ext_len]);
        }

        cursor += ext_len;
    }

    Err(anyhow!("client hello does not contain SNI"))
}

fn parse_sni_extension(ext: &[u8]) -> Result<String> {
    let mut cursor = 0;
    let list_len = read_u16(ext, &mut cursor)? as usize;
    if cursor + list_len > ext.len() {
        bail!("truncated server name list");
    }

    while cursor + 3 <= ext.len() {
        let name_type = read_u8(ext, &mut cursor)?;
        let name_len = read_u16(ext, &mut cursor)? as usize;
        if cursor + name_len > ext.len() {
            bail!("truncated server name");
        }

        if name_type == 0 {
            let host = std::str::from_utf8(&ext[cursor..cursor + name_len])
                .context("sni host is not valid utf-8")?;
            if host.is_empty() {
                bail!("sni host is empty");
            }
            return Ok(host.to_string());
        }

        cursor += name_len;
    }

    Err(anyhow!(
        "server name extension does not contain a host_name entry"
    ))
}

fn read_u8(data: &[u8], cursor: &mut usize) -> Result<u8> {
    let value = *data
        .get(*cursor)
        .ok_or_else(|| anyhow!("unexpected eof while reading u8"))?;
    *cursor += 1;
    Ok(value)
}

fn read_u16(data: &[u8], cursor: &mut usize) -> Result<u16> {
    if *cursor + 2 > data.len() {
        bail!("unexpected eof while reading u16");
    }
    let value = u16::from_be_bytes([data[*cursor], data[*cursor + 1]]);
    *cursor += 2;
    Ok(value)
}

fn read_u24(data: &[u8], cursor: &mut usize) -> Result<u32> {
    if *cursor + 3 > data.len() {
        bail!("unexpected eof while reading u24");
    }
    let value = ((data[*cursor] as u32) << 16)
        | ((data[*cursor + 1] as u32) << 8)
        | data[*cursor + 2] as u32;
    *cursor += 3;
    Ok(value)
}
