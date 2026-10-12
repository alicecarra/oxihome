//! Reads the machine: `sysinfo` for CPU, memory, disks, network and sensors, `/sys` for what it
//! lacks (battery, link state).

use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysinfo::{Components, Disks, Networks, System};

use crate::docker::{Container, CpuCounters, Docker};

const BYTES_PER_GIGABYTE: f32 = 1e9;
/// Rates and CPU usage need two readings at least this far apart.
const MINIMUM_SPAN: Duration = Duration::from_secs(1);
/// Tried in order; the first one found is the CPU temperature.
const CPU_SENSORS: [&str; 5] = [
    "k10temp Tctl",
    "zenpower Tdie",
    "coretemp Package id 0",
    "cpu_thermal",
    "acpitz",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub cpu: Cpu,
    pub memory: Memory,
    /// In the order of the configuration; `None` when not mounted.
    pub disks: Vec<Option<Disk>>,
    /// In the order of the configuration; `None` when the interface does not exist.
    pub interfaces: Vec<Option<Interface>>,
    pub battery: Option<Battery>,
    pub uptime_s: u64,
    /// In the order of the configuration; `None` when Docker could not be read.
    pub containers: Vec<Option<Container>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cpu {
    /// `None` on the first reading.
    pub usage: Option<f32>,
    pub load: [f32; 3],
    pub frequency_mhz: f32,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Memory {
    pub used_percent: f32,
    pub used_gb: f32,
    pub available_gb: f32,
    pub total_gb: f32,
    pub swap_used_percent: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Disk {
    pub used_percent: f32,
    pub free_gb: f32,
    pub total_gb: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Interface {
    /// `None` on the first reading.
    pub download_kbps: Option<f32>,
    pub upload_kbps: Option<f32>,
    pub connected: bool,
    /// IPv4 when there is one; empty when there is none.
    pub address: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Battery {
    pub charge_percent: f32,
    pub charging: bool,
    pub plugged_in: bool,
}

/// What does not change while the program runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub hostname: String,
    pub kernel: String,
    pub distribution: String,
    pub has_cpu_temperature: bool,
    pub has_battery: bool,
    pub cpu_count: usize,
}

pub struct Sampler {
    disks: Vec<String>,
    interfaces: Vec<String>,
    system: System,
    disk_list: Disks,
    networks: Networks,
    components: Components,
    battery: Option<PathBuf>,
    /// Totals of the last reading, per configured interface.
    last_traffic: Vec<Option<(u64, u64)>>,
    last_read_at: Instant,
    containers: Vec<String>,
    docker: Docker,
    /// Per configured container, for its CPU usage.
    container_counters: Vec<Option<CpuCounters>>,
    /// The Docker error is written once, not on every report.
    docker_failing: bool,
}

impl Sampler {
    pub fn new(
        disks: Vec<String>,
        interfaces: Vec<String>,
        containers: Vec<String>,
        docker: Docker,
    ) -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        let mut sampler = Self {
            last_traffic: vec![None; interfaces.len()],
            disks,
            interfaces,
            system,
            disk_list: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
            battery: find_battery(Path::new(POWER_SUPPLY)),
            last_read_at: Instant::now(),
            container_counters: vec![None; containers.len()],
            containers,
            docker,
            docker_failing: false,
        };
        sampler.read_traffic();
        sampler
    }

    pub fn info(&self) -> Info {
        Info {
            hostname: System::host_name().unwrap_or_default(),
            kernel: System::kernel_version().unwrap_or_default(),
            distribution: System::long_os_version().unwrap_or_default(),
            has_cpu_temperature: self.cpu_temperature().is_some(),
            has_battery: self.battery.is_some(),
            cpu_count: self.system.cpus().len().max(1),
        }
    }

    /// Mount points and interface names on this machine, to help write the configuration.
    pub fn available(&self) -> (Vec<String>, Vec<String>) {
        let mounts = self
            .disk_list
            .list()
            .iter()
            .map(|disk| disk.mount_point().display().to_string())
            .collect();
        let mut interfaces: Vec<String> = self.networks.list().keys().cloned().collect();
        interfaces.sort();
        (mounts, interfaces)
    }

    pub fn sample(&mut self) -> Sample {
        // The first report comes right after connecting, too close to the reading in `new`.
        let span = Some(self.last_read_at.elapsed()).filter(|span| *span >= MINIMUM_SPAN);

        self.system.refresh_cpu_usage();
        self.system.refresh_cpu_frequency();
        self.system.refresh_memory();
        self.disk_list.refresh(true);
        self.networks.refresh(true);
        self.components.refresh(false);

        let previous_traffic = self.read_traffic();
        self.last_read_at = Instant::now();

        Sample {
            cpu: self.cpu(span.is_some()),
            memory: self.memory(),
            disks: self.disks.iter().map(|mount| self.disk(mount)).collect(),
            interfaces: self
                .interfaces
                .iter()
                .zip(previous_traffic.iter().zip(&self.last_traffic))
                .map(|(name, (before, now))| self.interface(name, span, *before, *now))
                .collect(),
            battery: self.battery.as_deref().and_then(read_battery),
            uptime_s: System::uptime(),
            containers: self.containers(),
        }
    }

    fn containers(&mut self) -> Vec<Option<Container>> {
        let mut containers = Vec::with_capacity(self.containers.len());
        let mut first_error = None;
        for (name, counters) in self.containers.iter().zip(&mut self.container_counters) {
            match self.docker.read(name, *counters) {
                Ok((container, now)) => {
                    *counters = now;
                    containers.push(Some(container));
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                    containers.push(None);
                }
            }
        }
        if let Some(error) = &first_error
            && !self.docker_failing
        {
            println!("reading the containers failed: {error:#}");
        }
        self.docker_failing = first_error.is_some();
        containers
    }

    /// Stores the current totals and returns the previous ones.
    fn read_traffic(&mut self) -> Vec<Option<(u64, u64)>> {
        let now = self
            .interfaces
            .iter()
            .map(|name| {
                let data = self.networks.list().get(name)?;
                Some((data.total_received(), data.total_transmitted()))
            })
            .collect();
        std::mem::replace(&mut self.last_traffic, now)
    }

    fn cpu(&self, has_span: bool) -> Cpu {
        let load = System::load_average();
        let cpus = self.system.cpus();
        let frequency_mhz = if cpus.is_empty() {
            0.0
        } else {
            cpus.iter().map(|cpu| cpu.frequency() as f32).sum::<f32>() / cpus.len() as f32
        };
        Cpu {
            usage: has_span.then(|| self.system.global_cpu_usage()),
            load: [load.one as f32, load.five as f32, load.fifteen as f32],
            frequency_mhz,
            temperature: self.cpu_temperature(),
        }
    }

    fn cpu_temperature(&self) -> Option<f32> {
        CPU_SENSORS.iter().find_map(|label| {
            self.components
                .list()
                .iter()
                .find(|component| component.label() == *label)
                .and_then(|component| component.temperature())
        })
    }

    fn memory(&self) -> Memory {
        let total = self.system.total_memory() as f32;
        let used = self.system.used_memory() as f32;
        let swap_total = self.system.total_swap() as f32;
        Memory {
            used_percent: percent(used, total),
            used_gb: used / BYTES_PER_GIGABYTE,
            available_gb: self.system.available_memory() as f32 / BYTES_PER_GIGABYTE,
            total_gb: total / BYTES_PER_GIGABYTE,
            swap_used_percent: percent(self.system.used_swap() as f32, swap_total),
        }
    }

    fn disk(&self, mount: &str) -> Option<Disk> {
        let disk = self
            .disk_list
            .list()
            .iter()
            .find(|disk| disk.mount_point() == Path::new(mount))?;
        let total = disk.total_space() as f32;
        let free = disk.available_space() as f32;
        Some(Disk {
            used_percent: percent(total - free, total),
            free_gb: free / BYTES_PER_GIGABYTE,
            total_gb: total / BYTES_PER_GIGABYTE,
        })
    }

    fn interface(
        &self,
        name: &str,
        span: Option<Duration>,
        before: Option<(u64, u64)>,
        now: Option<(u64, u64)>,
    ) -> Option<Interface> {
        let data = self.networks.list().get(name)?;
        let rate = |before: u64, now: u64| {
            let span = span?;
            Some(now.saturating_sub(before) as f32 / 1000.0 / span.as_secs_f32())
        };
        let (download_kbps, upload_kbps) = match (before, now) {
            (Some(before), Some(now)) => (rate(before.0, now.0), rate(before.1, now.1)),
            _ => (None, None),
        };
        let addresses = data.ip_networks();
        let address = addresses
            .iter()
            .find(|network| network.addr.is_ipv4())
            .or_else(|| {
                addresses
                    .iter()
                    .find(|network| is_global_ipv6(network.addr))
            })
            .map(|network| network.addr.to_string())
            .unwrap_or_default();
        Some(Interface {
            download_kbps,
            upload_kbps,
            connected: read_trimmed(&Path::new("/sys/class/net").join(name).join("operstate"))
                .is_some_and(|state| state == "up"),
            address,
        })
    }
}

const POWER_SUPPLY: &str = "/sys/class/power_supply";

/// The machine's own battery; mice and keyboards report theirs with the `Device` scope.
fn find_battery(root: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    entries.into_iter().find(|path| {
        read_trimmed(&path.join("type")).as_deref() == Some("Battery")
            && read_trimmed(&path.join("scope")).as_deref() != Some("Device")
    })
}

fn read_battery(path: &Path) -> Option<Battery> {
    let charge_percent = read_trimmed(&path.join("capacity"))?.parse().ok()?;
    let status = read_trimmed(&path.join("status")).unwrap_or_default();
    let plugged_in = path.parent().is_some_and(|root| {
        fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                let supply = entry.path();
                read_trimmed(&supply.join("type")).as_deref() == Some("Mains")
                    && read_trimmed(&supply.join("online")).as_deref() == Some("1")
            })
    });
    Some(Battery {
        charge_percent,
        charging: status == "Charging",
        plugged_in,
    })
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
}

fn is_global_ipv6(address: IpAddr) -> bool {
    matches!(address, IpAddr::V6(v6) if !v6.is_loopback() && (v6.segments()[0] & 0xffc0) != 0xfe80)
}

fn percent(part: f32, whole: f32) -> f32 {
    if whole > 0.0 {
        part / whole * 100.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let directory = root.join(name);
        fs::create_dir_all(&directory).unwrap();
        for (file, content) in files {
            fs::write(directory.join(file), format!("{content}\n")).unwrap();
        }
    }

    #[test]
    fn finds_the_machine_battery_and_ignores_the_mouse() {
        let root = std::env::temp_dir().join(format!("oxihome-power-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        supply(&root, "ADP1", &[("type", "Mains"), ("online", "1")]);
        supply(
            &root,
            "BAT0",
            &[
                ("type", "Battery"),
                ("capacity", "87"),
                ("status", "Charging"),
            ],
        );
        supply(
            &root,
            "hidpp_battery_0",
            &[("type", "Battery"), ("scope", "Device"), ("capacity", "40")],
        );

        let battery = find_battery(&root).unwrap();
        assert_eq!(battery, root.join("BAT0"));
        assert_eq!(
            read_battery(&battery),
            Some(Battery {
                charge_percent: 87.0,
                charging: true,
                plugged_in: true,
            })
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_battery_on_a_desktop() {
        let root = std::env::temp_dir().join(format!("oxihome-desktop-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        supply(
            &root,
            "hidpp_battery_0",
            &[("type", "Battery"), ("scope", "Device")],
        );
        assert_eq!(find_battery(&root), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn percent_of_nothing_is_zero() {
        assert_eq!(percent(1.0, 0.0), 0.0);
        assert_eq!(percent(1.0, 4.0), 25.0);
    }
}
