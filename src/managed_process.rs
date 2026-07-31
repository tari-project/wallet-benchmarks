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

const BENCHMARK_SECRET_ENV_PREFIX: &str = "HARNESS_";
const MINOTARI_CONFIG_ENV_PREFIX: &str = "MINOTARI_";
const TARI_CONFIG_ENV_PREFIX: &str = "TARI_";
const REVEAL_PII_ENV: &str = "REVEAL_PII";

pub(crate) fn remove_benchmark_secret_envs(command: &mut Command, seeds: &SeedConfig) {
    remove_benchmark_secret_envs_from(command, seeds, std::env::vars_os().map(|(name, _)| name));
}

pub(crate) fn remove_tari_config_envs(command: &mut Command) {
    remove_env_prefix_from(
        command,
        TARI_CONFIG_ENV_PREFIX,
        std::env::vars_os().map(|(name, _)| name),
    );
}

fn remove_env_prefix_from(
    command: &mut Command,
    prefix: &str,
    inherited_names: impl IntoIterator<Item = OsString>,
) {
    for name in inherited_names {
        if env_name_has_prefix(&name, prefix) {
            command.env_remove(name);
        }
    }
}

fn remove_benchmark_secret_envs_from(
    command: &mut Command,
    seeds: &SeedConfig,
    inherited_names: impl IntoIterator<Item = OsString>,
) {
    let configured_names = [
        &seeds.old_wallet_env,
        &seeds.new_wallet_env,
        &seeds.payment_processor_env,
        &seeds.recipient_sink_env,
        &seeds.wallet_password_env,
    ]
    .into_iter()
    .map(OsString::from);
    let inherited_secret_names = inherited_names
        .into_iter()
        .filter(|name| is_forbidden_child_env_name(name));
    let names = configured_names
        .chain(inherited_secret_names)
        .collect::<BTreeSet<_>>();
    for name in names {
        command.env_remove(name);
    }
}

pub(crate) fn sanitized_std_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let path = std::env::var_os("PATH");
    let mut command = std::process::Command::new(program);
    command.env_clear();
    if let Some(path) = path {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    for name in ["SYSTEMROOT", "WINDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

pub(crate) fn sanitized_tokio_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let path = std::env::var_os("PATH");
    let mut command = Command::new(program);
    command.env_clear();
    if let Some(path) = path {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    for name in ["SYSTEMROOT", "WINDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

fn is_forbidden_child_env_name(name: &std::ffi::OsStr) -> bool {
    env_name_has_prefix(name, BENCHMARK_SECRET_ENV_PREFIX)
        || env_name_has_prefix(name, MINOTARI_CONFIG_ENV_PREFIX)
        || name
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(REVEAL_PII_ENV))
}

fn env_name_has_prefix(name: &std::ffi::OsStr, prefix: &str) -> bool {
    name.to_str().is_some_and(|name| {
        name.get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
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
            .env("MINOTARI_WALLET_VIEW_PRIVATE_KEY", "private-key")
            .env("REVEAL_PII", "true")
            .env("SAFE_CHILD_VALUE", "safe");

        seeds.wallet_password_env = "CUSTOM_WALLET_PASSWORD".to_string();
        command.env("CUSTOM_WALLET_PASSWORD", "wallet-secret");

        remove_benchmark_secret_envs_from(
            &mut command,
            &seeds,
            [
                OsString::from("HARNESS_SEED_UNRELATED"),
                OsString::from("CUSTOM_WALLET_PASSWORD"),
                OsString::from("MINOTARI_WALLET_VIEW_PRIVATE_KEY"),
                OsString::from("REVEAL_PII"),
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
            env.get(&OsString::from("CUSTOM_WALLET_PASSWORD")),
            Some(&None)
        );
        assert_eq!(
            env.get(&OsString::from("MINOTARI_WALLET_VIEW_PRIVATE_KEY")),
            Some(&None)
        );
        assert_eq!(env.get(&OsString::from("REVEAL_PII")), Some(&None));
        assert_eq!(
            env.get(&OsString::from("SAFE_CHILD_VALUE")),
            Some(&Some(OsString::from("safe")))
        );
    }

    #[test]
    fn sanitized_std_command_does_not_forward_benchmark_secrets() {
        let command = sanitized_std_command("unused-test-program");
        let explicit_env = command
            .get_envs()
            .map(|(name, value)| (name.to_os_string(), value.map(ToOwned::to_owned)))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert!(!explicit_env.contains_key(&OsString::from("HARNESS_WALLET_PW")));
        assert!(!explicit_env.contains_key(&OsString::from("HARNESS_SEED_OLD")));
        assert!(explicit_env.keys().all(|name| {
            matches!(
                name.to_string_lossy().to_ascii_uppercase().as_str(),
                "PATH" | "SYSTEMROOT" | "WINDIR"
            )
        }));
    }

    #[test]
    fn benchmark_secret_prefix_matching_is_case_insensitive() {
        assert!(is_forbidden_child_env_name(std::ffi::OsStr::new(
            "harness_seed_extra"
        )));
        assert!(is_forbidden_child_env_name(std::ffi::OsStr::new(
            "HARNESS_WALLET_PW"
        )));
        assert!(!is_forbidden_child_env_name(std::ffi::OsStr::new(
            "UNRELATED_HARNESS_VALUE"
        )));
        assert!(is_forbidden_child_env_name(std::ffi::OsStr::new(
            "minotari_wallet_spend_key"
        )));
        assert!(is_forbidden_child_env_name(std::ffi::OsStr::new(
            "reveal_pii"
        )));
    }

    #[test]
    fn tari_config_environment_is_removed_case_insensitively() {
        let mut command = Command::new("unused-test-program");
        command
            .env("TARI_WALLET__WEBHOOK__URL", "https://invalid.example")
            .env("tari_wallet__account_name", "other")
            .env("SAFE_CHILD_VALUE", "safe");
        remove_env_prefix_from(
            &mut command,
            TARI_CONFIG_ENV_PREFIX,
            [
                OsString::from("TARI_WALLET__WEBHOOK__URL"),
                OsString::from("tari_wallet__account_name"),
                OsString::from("SAFE_CHILD_VALUE"),
            ],
        );

        let env = command
            .as_std()
            .get_envs()
            .map(|(name, value)| (name.to_os_string(), value.map(ToOwned::to_owned)))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            env.get(&OsString::from("TARI_WALLET__WEBHOOK__URL")),
            Some(&None)
        );
        assert_eq!(
            env.get(&OsString::from("tari_wallet__account_name")),
            Some(&None)
        );
        assert_eq!(
            env.get(&OsString::from("SAFE_CHILD_VALUE")),
            Some(&Some(OsString::from("safe")))
        );
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
