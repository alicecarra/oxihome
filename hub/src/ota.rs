//! Over-the-air updates: images per board in `<images>/<device id>/<version>.bin`, handed out by a
//! small HTTP server of their own through single-use links; the web interface stays on localhost.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use jiff::Timestamp;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A link not used by then stops working.
const LINK_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// From the command to the node's `Hello` with the new version.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REQUEST: usize = 4096;

#[derive(Deserialize)]
pub struct OtaConfig {
    /// Where the image server listens, reachable by the boards, e.g. "0.0.0.0:8081".
    pub listen: String,
    /// How the boards reach it, e.g. "http://192.168.15.10:8081".
    pub public_url: String,
    /// `<device id>/<version>.bin`, written by `firmware/build-images.sh`.
    pub images: PathBuf,
}

struct Link {
    image: PathBuf,
    expires: Instant,
}

pub struct Update {
    pub from_version: String,
    pub version: String,
    pub started_at: Timestamp,
    started: Instant,
}

pub struct Ota {
    public_url: String,
    images: PathBuf,
    links: Mutex<HashMap<String, Link>>,
    /// By device id.
    updates: Mutex<HashMap<String, Update>>,
}

impl Ota {
    pub fn new(config: &OtaConfig) -> Self {
        Self {
            public_url: config.public_url.trim_end_matches('/').to_string(),
            images: config.images.clone(),
            links: Mutex::default(),
            updates: Mutex::default(),
        }
    }

    /// The newest image for the node when it is newer than `running`.
    pub fn newer_image(&self, device_id: &str, running: &str) -> Option<(String, PathBuf)> {
        let running = version(running)?;
        newest_image(&self.images.join(device_id))
            .filter(|(newest, _)| version(newest).is_some_and(|newest| newest > running))
    }

    pub fn updating(&self, device_id: &str) -> Option<String> {
        let updates = self.updates.lock().unwrap();
        updates.get(device_id).map(|update| update.version.clone())
    }

    /// Records the update and returns the single-use link for the node.
    pub fn start(
        &self,
        device_id: &str,
        from_version: String,
        version: String,
        image: PathBuf,
    ) -> anyhow::Result<String> {
        let token = random_token()?;
        let now = Instant::now();
        self.links.lock().unwrap().insert(
            token.clone(),
            Link {
                image,
                expires: now + LINK_LIFETIME,
            },
        );
        self.updates.lock().unwrap().insert(
            device_id.to_string(),
            Update {
                from_version,
                version,
                started_at: Timestamp::now(),
                started: now,
            },
        );
        Ok(format!("{}/ota/{token}", self.public_url))
    }

    /// The command was refused or the node reported an error: no longer waiting for it.
    /// Ended: the node answered (`Hello`, `Error`) or the command was refused.
    pub fn finish(&self, device_id: &str) -> Option<Update> {
        self.updates.lock().unwrap().remove(device_id)
    }

    /// Updates without an answer in time, by device id; dropped from the list.
    pub fn timed_out(&self, now: Instant) -> Vec<(String, Update)> {
        self.links.lock().unwrap().retain(|_, link| link.expires > now);
        let mut updates = self.updates.lock().unwrap();
        let late: Vec<String> = updates
            .iter()
            .filter(|(_, update)| now.duration_since(update.started) >= UPDATE_TIMEOUT)
            .map(|(device_id, _)| device_id.clone())
            .collect();
        late.into_iter()
            .filter_map(|device_id| {
                let update = updates.remove(&device_id)?;
                Some((device_id, update))
            })
            .collect()
    }

    /// The image behind a link, once.
    fn take_link(&self, token: &str) -> Option<PathBuf> {
        let link = self.links.lock().unwrap().remove(token)?;
        (link.expires > Instant::now()).then_some(link.image)
    }
}

pub async fn serve(listener: TcpListener, ota: Arc<Ota>) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            continue;
        };
        let ota = ota.clone();
        tokio::spawn(async move {
            if let Err(error) = answer(stream, &ota).await {
                println!("OTA download from {peer}: {error:#}");
            }
        });
    }
}

async fn answer(mut stream: TcpStream, ota: &Ota) -> anyhow::Result<()> {
    let request = tokio::time::timeout(REQUEST_TIMEOUT, read_head(&mut stream))
        .await
        .context("request timed out")??;
    let image = request_token(&request).and_then(|token| ota.take_link(token));
    let Some(image) = image else {
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        anyhow::bail!("unknown or used link");
    };
    let mut file = tokio::fs::File::open(&image)
        .await
        .with_context(|| format!("opening {}", image.display()))?;
    let size = file.metadata().await?.len();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await?;
    tokio::io::copy(&mut file, &mut stream).await?;
    stream.shutdown().await?;
    println!("OTA: sent {} ({size} bytes)", image.display());
    Ok(())
}

async fn read_head(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut head = Vec::new();
    let mut buffer = [0; 512];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        anyhow::ensure!(head.len() < MAX_REQUEST, "request too long");
        let read = stream.read(&mut buffer).await?;
        anyhow::ensure!(read > 0, "connection closed");
        head.extend_from_slice(&buffer[..read]);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

/// The token of `GET /ota/<token> HTTP/1.1`.
fn request_token(request: &str) -> Option<&str> {
    let mut words = request.lines().next()?.split(' ');
    let (method, path) = (words.next()?, words.next()?);
    let token = path.strip_prefix("/ota/")?;
    (method == "GET" && !token.is_empty()).then_some(token)
}

fn random_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .context("reading /dev/urandom")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// `major.minor.patch`; anything else (e.g. "weather 0.1.0") is not an updatable version.
fn version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.').map(|part| part.parse().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

fn newest_image(directory: &Path) -> Option<(String, PathBuf)> {
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?.strip_suffix(".bin")?.to_string();
            let parsed = version(&name)?;
            Some((parsed, name, path))
        })
        .max_by_key(|(parsed, _, _)| *parsed)
        .map(|(_, name, path)| (name, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(version("0.12.0") > version("0.9.3"));
        assert_eq!(version("weather 0.1.0"), None);
        assert_eq!(version("1.2"), None);
        assert_eq!(version("1.2.3.4"), None);
    }

    #[test]
    fn token_comes_from_the_request_line() {
        assert_eq!(request_token("GET /ota/abc HTTP/1.1\r\nHost: x\r\n\r\n"), Some("abc"));
        assert_eq!(request_token("POST /ota/abc HTTP/1.1\r\n\r\n"), None);
        assert_eq!(request_token("GET /ota/ HTTP/1.1\r\n\r\n"), None);
        assert_eq!(request_token("GET / HTTP/1.1\r\n\r\n"), None);
    }

    #[test]
    fn a_link_works_once() {
        let ota = Ota::new(&OtaConfig {
            listen: "127.0.0.1:0".into(),
            public_url: "http://hub:8081/".into(),
            images: PathBuf::from("images"),
        });
        let url = ota
            .start(
                "esp32-a",
                "0.1.0".into(),
                "0.2.0".into(),
                PathBuf::from("images/esp32-a/0.2.0.bin"),
            )
            .unwrap();
        let token = url.strip_prefix("http://hub:8081/ota/").unwrap();
        assert_eq!(token.len(), 32);
        assert!(ota.take_link(token).is_some());
        assert!(ota.take_link(token).is_none());
        assert_eq!(ota.updating("esp32-a").as_deref(), Some("0.2.0"));
    }

    #[test]
    fn an_update_finishes_once() {
        let ota = Ota::new(&OtaConfig {
            listen: "127.0.0.1:0".into(),
            public_url: "http://hub:8081".into(),
            images: PathBuf::from("images"),
        });
        ota.start("esp32-a", "0.1.0".into(), "0.2.0".into(), PathBuf::new())
            .unwrap();
        let update = ota.finish("esp32-a").unwrap();
        assert_eq!((update.from_version.as_str(), update.version.as_str()), ("0.1.0", "0.2.0"));
        assert!(ota.finish("esp32-a").is_none());
        assert!(ota.updating("esp32-a").is_none());
    }
}
