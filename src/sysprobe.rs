//! Environment disclosure and process resource sampling.
//!
//! The result profile has to say what the numbers were measured on, and the scan scenarios
//! have to report peak RSS and peak CPU. Both come from here.

use std::{
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::task::JoinHandle;

/// Sampling interval. sysinfo needs at least ~200 ms between refreshes for a usable CPU
/// reading, so this is comfortably above that.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// Hardware and OS the run happened on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub os: String,
    pub os_version: String,
    pub kernel_version: String,
    pub cpu_model: String,
    pub cpu_cores: usize,
    pub total_memory_bytes: u64,
    pub arch: String,
    /// Base node the wallets talked to, so local and remote runs are distinguishable.
    pub base_node_url: String,
    /// `local` when the base node is on loopback, otherwise `remote`.
    pub base_node_path: String,
}

impl EnvironmentInfo {
    pub fn collect(base_node_url: &str) -> Self {
        let sys = System::new_all();
        let cpu_model = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default();
        Self {
            os: System::name().unwrap_or_else(|| "unknown".to_string()),
            os_version: System::long_os_version().unwrap_or_else(|| "unknown".to_string()),
            kernel_version: System::kernel_version().unwrap_or_else(|| "unknown".to_string()),
            cpu_model,
            cpu_cores: sys.cpus().len(),
            total_memory_bytes: sys.total_memory(),
            arch: System::cpu_arch().unwrap_or_else(|| std::env::consts::ARCH.to_string()),
            base_node_url: base_node_url.to_string(),
            base_node_path: classify_base_node_path(base_node_url),
        }
    }
}

/// Whether the base node is reachable over loopback or over the network.
pub fn classify_base_node_path(url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    if matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") {
        "local".to_string()
    } else {
        "remote".to_string()
    }
}

/// Peak resource usage observed over a sampling window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub peak_rss_bytes: u64,
    pub peak_cpu_percent: f32,
    pub samples: u32,
}

/// Samples one process until it is stopped.
///
/// For the old wallet this is the wallet child process. For the new wallet the wallet *is*
/// the harness process, so the sampled figures include the harness itself; the result profile
/// says so.
pub struct ResourceSampler {
    peaks: Arc<Peaks>,
    handle: JoinHandle<()>,
}

#[derive(Default)]
struct Peaks {
    rss: AtomicU64,
    cpu_millipercent: AtomicU64,
    samples: AtomicU32,
}

impl Peaks {
    fn record(&self, rss: u64, cpu_percent: f32) {
        self.rss.fetch_max(rss, Ordering::Relaxed);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let cpu = (cpu_percent.max(0.0) * 1_000.0) as u64;
        self.cpu_millipercent.fetch_max(cpu, Ordering::Relaxed);
        self.samples.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> ResourceUsage {
        #[allow(clippy::cast_precision_loss)]
        let cpu = self.cpu_millipercent.load(Ordering::Relaxed) as f32 / 1_000.0;
        ResourceUsage {
            peak_rss_bytes: self.rss.load(Ordering::Relaxed),
            peak_cpu_percent: cpu,
            samples: self.samples.load(Ordering::Relaxed),
        }
    }
}

impl ResourceSampler {
    /// Starts sampling `pid`, or the current process when `pid` is `None`.
    pub fn start(pid: Option<u32>) -> Self {
        let peaks = Arc::new(Peaks::default());
        let target = pid
            .map(Pid::from_u32)
            .or_else(|| sysinfo::get_current_pid().ok());
        let task_peaks = Arc::clone(&peaks);
        let handle = tokio::spawn(async move {
            let Some(target) = target else { return };
            let mut sys = System::new();
            loop {
                sys.refresh_processes(ProcessesToUpdate::Some(&[target]), true);
                if let Some(process) = sys.process(target) {
                    task_peaks.record(process.memory(), process.cpu_usage());
                }
                tokio::time::sleep(SAMPLE_INTERVAL).await;
            }
        });
        Self { peaks, handle }
    }

    /// Stops sampling and returns the peaks observed.
    pub fn stop(self) -> ResourceUsage {
        self.handle.abort();
        self.peaks.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_node_path_is_classified() {
        assert_eq!(classify_base_node_path("http://127.0.0.1:18142"), "local");
        assert_eq!(classify_base_node_path("http://localhost:18142/"), "local");
        assert_eq!(
            classify_base_node_path("https://rpc.esmeralda.tari.com"),
            "remote"
        );
    }

    #[test]
    fn environment_is_populated() {
        let env = EnvironmentInfo::collect("https://rpc.esmeralda.tari.com");
        assert!(env.cpu_cores > 0);
        assert!(env.total_memory_bytes > 0);
        assert_eq!(env.base_node_path, "remote");
    }

    #[test]
    fn peaks_keep_the_maximum() {
        let peaks = Peaks::default();
        peaks.record(100, 10.0);
        peaks.record(50, 42.5);
        let usage = peaks.snapshot();
        assert_eq!(usage.peak_rss_bytes, 100);
        assert!((usage.peak_cpu_percent - 42.5).abs() < 0.01);
        assert_eq!(usage.samples, 2);
    }

    #[tokio::test]
    async fn sampler_observes_the_current_process() {
        let sampler = ResourceSampler::start(None);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let usage = sampler.stop();
        assert!(usage.samples >= 1);
        assert!(usage.peak_rss_bytes > 0);
    }
}
