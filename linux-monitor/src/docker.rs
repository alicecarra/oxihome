//! Containers through the Docker API on its Unix socket (Podman's is compatible). Only GET.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use jiff::Timestamp;
use serde::Deserialize;

/// Fixed so answers do not change with Docker updates; Docker 25 or newer.
const API: &str = "/v1.44";
const TIMEOUT: Duration = Duration::from_secs(5);
const BYTES_PER_MEGABYTE: f32 = 1e6;

pub const HEALTH_OPTIONS: [&str; 4] = ["healthy", "unhealthy", "starting", "none"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Healthy,
    Unhealthy,
    Starting,
    /// No healthcheck, or the container is not running.
    None,
}

impl Health {
    pub fn name(self) -> &'static str {
        match self {
            Health::Healthy => "healthy",
            Health::Unhealthy => "unhealthy",
            Health::Starting => "starting",
            Health::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub running: bool,
    pub health: Health,
    /// Of one whole CPU, like `docker stats`; `None` on the first reading after a start.
    /// Zero, like memory and uptime, when not running: an absent value would keep the old one.
    pub cpu_percent: Option<f32>,
    pub memory_mb: Option<f32>,
    /// `None` when the container does not exist.
    pub restarts: Option<i32>,
    pub uptime_s: Option<i64>,
}

impl Container {
    fn missing() -> Self {
        Self {
            running: false,
            health: Health::None,
            cpu_percent: Some(0.0),
            memory_mb: Some(0.0),
            restarts: None,
            uptime_s: Some(0),
        }
    }
}

/// CPU time counters, kept between readings: the one-shot stats come without the previous ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuCounters {
    container: u64,
    system: u64,
}

pub struct Docker {
    socket: PathBuf,
}

impl Docker {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    /// Returns the container and the counters to pass next time.
    pub fn read(
        &self,
        name: &str,
        previous: Option<CpuCounters>,
    ) -> anyhow::Result<(Container, Option<CpuCounters>)> {
        let Some(body) = self.get(&format!("{API}/containers/{name}/json"))? else {
            return Ok((Container::missing(), None));
        };
        let inspect: Inspect =
            serde_json::from_str(&body).with_context(|| format!("reading inspect of {name}"))?;
        let mut container = from_inspect(&inspect, Timestamp::now());
        if !container.running {
            return Ok((container, None));
        }
        let Some(body) = self.get(&format!(
            "{API}/containers/{name}/stats?stream=false&one-shot=true"
        ))?
        else {
            return Ok((Container::missing(), None));
        };
        let stats: Stats =
            serde_json::from_str(&body).with_context(|| format!("reading stats of {name}"))?;
        let counters = stats.counters();
        container.cpu_percent = counters
            .and_then(|now| cpu_percent(previous?, now, stats.cpu_stats.online_cpus.unwrap_or(1)));
        container.memory_mb = stats.memory_mb();
        Ok((container, counters))
    }

    /// `None` on 404.
    fn get(&self, path: &str) -> anyhow::Result<Option<String>> {
        let mut stream = UnixStream::connect(&self.socket)
            .with_context(|| format!("connecting to {}", self.socket.display()))?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        // HTTP/1.0: no chunked body, and the connection closes at the end of the answer.
        write!(stream, "GET {path} HTTP/1.0\r\nHost: docker\r\n\r\n")?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer)?;
        parse_response(&answer).with_context(|| format!("GET {path}"))
    }
}

fn parse_response(answer: &str) -> anyhow::Result<Option<String>> {
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .context("answer without a body")?;
    let status = head
        .split_whitespace()
        .nth(1)
        .context("answer without a status")?;
    match status {
        "200" => Ok(Some(body.to_string())),
        "404" => Ok(None),
        _ => bail!("status {status}: {}", body.trim()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Inspect {
    state: State,
    restart_count: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct State {
    running: bool,
    started_at: String,
    health: Option<HealthState>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HealthState {
    status: String,
}

#[derive(Deserialize)]
struct Stats {
    cpu_stats: CpuStats,
    memory_stats: MemoryStats,
}

#[derive(Deserialize)]
struct CpuStats {
    cpu_usage: CpuUsage,
    system_cpu_usage: Option<u64>,
    online_cpus: Option<u32>,
}

#[derive(Deserialize)]
struct CpuUsage {
    total_usage: u64,
}

#[derive(Deserialize)]
struct MemoryStats {
    usage: Option<u64>,
    stats: Option<MemoryDetail>,
}

#[derive(Deserialize)]
struct MemoryDetail {
    #[serde(default)]
    inactive_file: u64,
}

impl Stats {
    fn counters(&self) -> Option<CpuCounters> {
        Some(CpuCounters {
            container: self.cpu_stats.cpu_usage.total_usage,
            system: self.cpu_stats.system_cpu_usage?,
        })
    }

    /// Without the page cache that can be dropped, as `docker stats` shows it.
    fn memory_mb(&self) -> Option<f32> {
        let usage = self.memory_stats.usage?;
        let inactive = self
            .memory_stats
            .stats
            .as_ref()
            .map_or(0, |detail| detail.inactive_file);
        Some(usage.saturating_sub(inactive) as f32 / BYTES_PER_MEGABYTE)
    }
}

fn from_inspect(inspect: &Inspect, now: Timestamp) -> Container {
    let state = &inspect.state;
    let health = match state.health.as_ref().map(|health| health.status.as_str()) {
        Some("healthy") => Health::Healthy,
        Some("unhealthy") => Health::Unhealthy,
        Some("starting") => Health::Starting,
        _ => Health::None,
    };
    if !state.running {
        return Container {
            restarts: Some(inspect.restart_count),
            ..Container::missing()
        };
    }
    let uptime_s = state
        .started_at
        .parse::<Timestamp>()
        .ok()
        .map(|started| now.duration_since(started).as_secs().max(0));
    Container {
        running: true,
        health,
        cpu_percent: None,
        memory_mb: None,
        restarts: Some(inspect.restart_count),
        uptime_s,
    }
}

/// `None` when a counter went back, as after a restart.
fn cpu_percent(previous: CpuCounters, now: CpuCounters, cpus: u32) -> Option<f32> {
    let container = now.container.checked_sub(previous.container)?;
    let system = now.system.checked_sub(previous.system)?;
    if system == 0 {
        return None;
    }
    Some(container as f32 / system as f32 * cpus as f32 * 100.0)
}

/// What changed between two readings, as event data.
pub fn changes(before: &Container, after: &Container) -> Vec<&'static str> {
    let mut changes = Vec::new();
    match (before.running, after.running) {
        (true, false) => changes.push("stopped"),
        (false, true) => changes.push("started"),
        _ => {}
    }
    match (before.health, after.health) {
        (previous, Health::Unhealthy) if previous != Health::Unhealthy => changes.push("unhealthy"),
        (Health::Unhealthy, Health::Healthy) => changes.push("healthy"),
        _ => {}
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from the trilium container on docker-host.
    const INSPECT: &str = r#"{
        "Name": "/trilium", "RestartCount": 2,
        "State": {"Status": "running", "Running": true, "Paused": false, "ExitCode": 0,
                  "StartedAt": "2026-10-12T12:06:50.479556696Z",
                  "FinishedAt": "2026-10-12T12:06:49.320832207Z",
                  "Health": {"Status": "healthy", "FailingStreak": 0, "Log": []}}
    }"#;

    /// Trimmed from the oxihome-mosquitto container.
    const STATS: &str = r#"{
        "name": "/oxihome-mosquitto",
        "cpu_stats": {"cpu_usage": {"total_usage": 2934766000, "usage_in_kernelmode": 1902238000},
                      "system_cpu_usage": 132761100000000, "online_cpus": 16},
        "memory_stats": {"usage": 10190848, "stats": {"active_file": 3338240, "inactive_file": 3911680},
                         "limit": 24536489984}
    }"#;

    fn container(running: bool, health: Health) -> Container {
        Container {
            running,
            health,
            ..Container::missing()
        }
    }

    #[test]
    fn reads_the_inspect() {
        let inspect: Inspect = serde_json::from_str(INSPECT).unwrap();
        let now: Timestamp = "2026-10-12T13:06:50.479556696Z".parse().unwrap();
        assert_eq!(
            from_inspect(&inspect, now),
            Container {
                running: true,
                health: Health::Healthy,
                cpu_percent: None,
                memory_mb: None,
                restarts: Some(2),
                uptime_s: Some(3600),
            }
        );
    }

    #[test]
    fn no_healthcheck_and_stopped_mean_none() {
        let without: Inspect = serde_json::from_str(
            r#"{"RestartCount": 0, "State": {"Running": true, "StartedAt": "2026-10-12T12:06:50Z"}}"#,
        )
        .unwrap();
        assert_eq!(
            from_inspect(&without, Timestamp::now()).health,
            Health::None
        );

        let stopped = INSPECT.replace("\"Running\": true", "\"Running\": false");
        let inspect: Inspect = serde_json::from_str(&stopped).unwrap();
        let container = from_inspect(&inspect, Timestamp::now());
        assert_eq!(container.health, Health::None);
        assert_eq!(container.uptime_s, Some(0));
        assert_eq!(container.memory_mb, Some(0.0));
        assert_eq!(container.restarts, Some(2));
    }

    #[test]
    fn reads_memory_without_the_inactive_cache() {
        let stats: Stats = serde_json::from_str(STATS).unwrap();
        assert_eq!(stats.memory_mb(), Some(6.279168));
        assert_eq!(
            stats.counters(),
            Some(CpuCounters {
                container: 2_934_766_000,
                system: 132_761_100_000_000,
            })
        );
    }

    #[test]
    fn cpu_comes_from_two_readings() {
        let before = CpuCounters {
            container: 1_000,
            system: 100_000,
        };
        let after = CpuCounters {
            container: 3_000,
            system: 200_000,
        };
        assert_eq!(cpu_percent(before, after, 4), Some(8.0));
        assert_eq!(cpu_percent(after, before, 4), None, "restarted");
        assert_eq!(cpu_percent(before, before, 4), None);
    }

    #[test]
    fn changes_between_readings() {
        let healthy = container(true, Health::Healthy);
        let unhealthy = container(true, Health::Unhealthy);
        let stopped = container(false, Health::None);
        assert_eq!(changes(&healthy, &stopped), ["stopped"]);
        assert_eq!(changes(&stopped, &healthy), ["started"]);
        assert_eq!(changes(&healthy, &unhealthy), ["unhealthy"]);
        assert_eq!(changes(&unhealthy, &healthy), ["healthy"]);
        assert_eq!(changes(&stopped, &unhealthy), ["started", "unhealthy"]);
        assert_eq!(changes(&healthy, &healthy), [] as [&str; 0]);
        assert_eq!(
            changes(&container(true, Health::Starting), &healthy),
            [] as [&str; 0]
        );
    }

    #[test]
    fn reads_http_answers() {
        let ok = "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"a\":1}";
        assert_eq!(parse_response(ok).unwrap(), Some("{\"a\":1}".into()));
        let missing = "HTTP/1.0 404 Not Found\r\n\r\n{\"message\":\"No such container: nope\"}";
        assert_eq!(parse_response(missing).unwrap(), None);
        let error = "HTTP/1.0 500 Internal Server Error\r\n\r\n{\"message\":\"boom\"}";
        assert!(parse_response(error).is_err());
        assert!(parse_response("HTTP/1.0 200 OK\r\n").is_err(), "cut short");
    }
}
