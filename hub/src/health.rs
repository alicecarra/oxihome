//! `oxihome-hub health <config.toml>`: asks the running hub's `/health`, for a Docker
//! healthcheck in an image without `curl`.

use std::time::Duration;

use anyhow::{Context, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(5);

/// Fails unless the hub answers 200.
pub async fn check(listen_host: &str, port: u16) -> anyhow::Result<()> {
    let address = (local_address(listen_host), port);
    let response = tokio::time::timeout(TIMEOUT, ask(address))
        .await
        .context("no answer from the hub")??;
    match status(&response) {
        Some(200) => Ok(()),
        Some(code) => bail!("hub answered {code}: {}", body(&response)),
        None => bail!("not an HTTP answer"),
    }
}

async fn ask(address: (&str, u16)) -> anyhow::Result<String> {
    let mut stream = TcpStream::connect(address)
        .await
        .with_context(|| format!("connecting to {}:{}", address.0, address.1))?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

/// The hub listening on every address answers on loopback too.
fn local_address(listen_host: &str) -> &str {
    match listen_host {
        "0.0.0.0" => "127.0.0.1",
        "::" | "[::]" => "::1",
        host => host,
    }
}

fn status(response: &str) -> Option<u16> {
    let line = response.lines().next()?;
    let mut parts = line.split(' ');
    parts.next()?.starts_with("HTTP/").then_some(())?;
    parts.next()?.parse().ok()
}

fn body(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map_or("", |(_, body)| body.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_status_and_body() {
        let answer =
            "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 19\r\n\r\nbroker disconnected";
        assert_eq!(status(answer), Some(503));
        assert_eq!(body(answer), "broker disconnected");
        assert_eq!(status("HTTP/1.1 200 OK\r\n\r\nok"), Some(200));
        assert_eq!(status("garbage"), None);
    }

    #[test]
    fn asks_on_loopback_when_listening_everywhere() {
        assert_eq!(local_address("0.0.0.0"), "127.0.0.1");
        assert_eq!(local_address("::"), "::1");
        assert_eq!(local_address("192.168.15.10"), "192.168.15.10");
    }
}
