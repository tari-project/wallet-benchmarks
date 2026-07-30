use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::Context;
use tokio::{process::Command, time};

use crate::config::SeedConfig;

const BENCHMARK_SEED_ENV_PREFIX: &str = "HARNESS_SEED_";

pub(crate) fn remove_benchmark_seed_envs(command: &mut Command, seeds: &SeedConfig) {
    remove_benchmark_seed_envs_from(command, seeds, std::env::vars_os().map(|(name, _)| name));
}

fn remove_benchmark_seed_envs_from(
    command: &mut Command,
    seeds: &SeedConfig,
    inherited_names: impl IntoIterator<Item = OsString>,
) {
    let configured_names = [
        &seeds.old_wallet_env,
        &seeds.new_wallet_env,
        &seeds.payment_processor_env,
        &seeds.recipient_sink_env,
    ]
    .into_iter()
    .map(OsString::from);
    let inherited_seed_names = inherited_names
        .into_iter()
        .filter(|name| is_benchmark_seed_env_name(name));
    let names = configured_names
        .chain(inherited_seed_names)
        .collect::<BTreeSet<_>>();
    for name in names {
        command.env_remove(name);
    }
}

fn is_benchmark_seed_env_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.get(..BENCHMARK_SEED_ENV_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(BENCHMARK_SEED_ENV_PREFIX))
    })
}

pub struct ManagedProcess {
    label: String,
    child: tokio::process::Child,
    process_group_id: Option<u32>,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

impl ManagedProcess {
    pub fn spawn(label: &str, mut command: Command, log_dir: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(log_dir)?;
        let stdout_path = log_dir.join(format!("{label}.stdout.log"));
        let stderr_path = log_dir.join(format!("{label}.stderr.log"));
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stdout_path)?;
        let stderr = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_path)?;
        #[cfg(unix)]
        command.process_group(0);
        let child = command
            .kill_on_drop(true)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("spawning {label}"))?;
        let process_group_id = child.id();
        Ok(Self {
            label: label.to_string(),
            child,
            process_group_id,
            stdout_path,
            stderr_path,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    pub fn try_wait(&mut self) -> anyhow::Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .with_context(|| format!("checking {} process status", self.label))
    }

    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.try_wait()?.is_some() {
            return self.ensure_descendants_stopped().await;
        }
        signal_group(self.process_group_id, graceful_signal());
        if time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            signal_group(self.process_group_id, force_signal());
            time::timeout(Duration::from_secs(5), self.child.wait())
                .await
                .with_context(|| format!("waiting for {} after forced shutdown", self.label))??;
        }
        self.ensure_descendants_stopped().await?;
        Ok(())
    }

    pub async fn wait(&mut self, timeout: Duration) -> anyhow::Result<ExitStatus> {
        let status = time::timeout(timeout, self.child.wait())
            .await
            .with_context(|| format!("waiting for {} to exit", self.label))??;
        self.ensure_descendants_stopped().await?;
        Ok(status)
    }

    async fn ensure_descendants_stopped(&self) -> anyhow::Result<()> {
        if !group_exists(self.process_group_id) {
            return Ok(());
        }
        signal_group(self.process_group_id, graceful_signal());
        let deadline = time::Instant::now() + Duration::from_secs(2);
        while group_exists(self.process_group_id) && time::Instant::now() < deadline {
            time::sleep(Duration::from_millis(25)).await;
        }
        if group_exists(self.process_group_id) {
            signal_group(self.process_group_id, force_signal());
        }
        let deadline = time::Instant::now() + Duration::from_secs(2);
        while group_exists(self.process_group_id) && time::Instant::now() < deadline {
            time::sleep(Duration::from_millis(25)).await;
        }
        if group_exists(self.process_group_id) {
            anyhow::bail!("{} process group still has live descendants", self.label);
        }
        Ok(())
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        let mut leader_exited = self.child.try_wait().ok().flatten().is_some();
        if leader_exited && !group_exists(self.process_group_id) {
            return;
        }
        signal_group(self.process_group_id, graceful_signal());
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            leader_exited |= self.child.try_wait().ok().flatten().is_some();
            if leader_exited && !group_exists(self.process_group_id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        signal_group(self.process_group_id, force_signal());
        let _ = self.child.start_kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            leader_exited |= self.child.try_wait().ok().flatten().is_some();
            if leader_exited && !group_exists(self.process_group_id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        signal_group(self.process_group_id, force_signal());
    }
}

#[cfg(unix)]
fn signal_group(pid: Option<u32>, signal: i32) {
    if let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) {
        // SAFETY: a negative PID targets the subprocess group created at spawn.
        unsafe {
            libc::kill(-pid, signal);
        }
    }
}

#[cfg(unix)]
fn group_exists(pid: Option<u32>) -> bool {
    pid.and_then(|pid| i32::try_from(pid).ok())
        .is_some_and(|pid| {
            // SAFETY: signal 0 checks whether any process remains in the group.
            unsafe { libc::kill(-pid, 0) == 0 }
        })
}

#[cfg(not(unix))]
fn group_exists(_: Option<u32>) -> bool {
    false
}

#[cfg(not(unix))]
fn signal_group(_: Option<u32>, _: i32) {}

#[cfg(unix)]
const fn graceful_signal() -> i32 {
    libc::SIGTERM
}

#[cfg(not(unix))]
const fn graceful_signal() -> i32 {
    0
}

#[cfg(unix)]
const fn force_signal() -> i32 {
    libc::SIGKILL
}

#[cfg(not(unix))]
const fn force_signal() -> i32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_seed_environment_is_removed_from_child_command() {
        let mut seeds = crate::config::Config::default().seeds;
        seeds.old_wallet_env = "CUSTOM_OLD_SEED".to_string();
        let mut command = Command::new("unused-test-program");
        command
            .env("CUSTOM_OLD_SEED", "old-secret")
            .env("HARNESS_SEED_UNRELATED", "unrelated-secret")
            .env("SAFE_CHILD_VALUE", "safe");

        remove_benchmark_seed_envs_from(
            &mut command,
            &seeds,
            [
                OsString::from("HARNESS_SEED_UNRELATED"),
                OsString::from("SAFE_CHILD_VALUE"),
            ],
        );

        let env = command
            .as_std()
            .get_envs()
            .map(|(name, value)| (name.to_os_string(), value.map(ToOwned::to_owned)))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(env.get(&OsString::from("CUSTOM_OLD_SEED")), Some(&None));
        assert_eq!(
            env.get(&OsString::from("HARNESS_SEED_UNRELATED")),
            Some(&None)
        );
        assert_eq!(
            env.get(&OsString::from("SAFE_CHILD_VALUE")),
            Some(&Some(OsString::from("safe")))
        );
    }

    #[test]
    fn benchmark_seed_prefix_matching_is_case_insensitive() {
        assert!(is_benchmark_seed_env_name(std::ffi::OsStr::new(
            "harness_seed_extra"
        )));
        assert!(!is_benchmark_seed_env_name(std::ffi::OsStr::new(
            "HARNESS_WALLET_PW"
        )));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_reaps_the_managed_process_group() {
        let logs = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]);
        let mut process = ManagedProcess::spawn("group-test", command, logs.path()).unwrap();
        assert!(process.id().is_some());
        process.shutdown().await.unwrap();
        assert!(process.try_wait().unwrap().is_some());
    }
}
