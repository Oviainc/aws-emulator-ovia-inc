// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! A built emulator started as a customer starts it, in a home folder of its own.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{atomic::AtomicU32, atomic::Ordering::Relaxed, OnceLock};
use std::time::{Duration, Instant};

/// Ends an emulator a crashed test left behind.
const ORPHAN_IDLE_TIMEOUT_SECS: &str = "60";
/// How long a start may take to publish its files.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a start that must refuse to run may take to exit.
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the emulator may take to exit after SIGTERM, as `docker stop` allows.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const LOG_FILE: &str = "emulator.log";

/// A running emulator, killed and its home folder removed once dropped, however a test ends.
pub struct Emulator {
    child: Child,
    pub(crate) client: OnceLock<reqwest::blocking::Client>,
    pub home: PathBuf,
    /// The folder, named after the port, holding the files this start published.
    pub folder: PathBuf,
    pub port: u16,
}

impl Emulator {
    /// Starts the emulator with `args` on a free port, in a new home folder, with telemetry off.
    pub fn start(args: &[&str]) -> Self {
        Self::start_in(args, new_home())
    }

    /// [`Emulator::start`] in `home`, which an earlier, stopped start may have used.
    pub fn start_in(args: &[&str], home: PathBuf) -> Self {
        let mut started = Self::spawn(args, home, &[]);
        let folder = wait_until(STARTUP_TIMEOUT, || {
            if let Some(status) = started.child.try_wait().unwrap() {
                panic!("the emulator exited with {status}:\n{}", started.log());
            }
            folder_publishing_config(&started.home, 4)
        });
        started.folder = folder.unwrap_or_else(|| panic!("no files:\n{}", started.log()));
        let port = started.folder.file_name().and_then(|name| name.to_str());
        let port = port.and_then(|name| name.parse().ok());
        started.port = port.unwrap_or_else(|| panic!("{:?} names no port", started.folder));
        started
    }

    /// How the emulator run with `args` and extra `env` exits, if it does in time, and its log.
    pub fn exit_of(args: &[&str], env: &[(&str, &str)]) -> (Option<ExitStatus>, String) {
        let mut emulator = Self::spawn(args, new_home(), env);
        (emulator.exit_within(REFUSAL_TIMEOUT), emulator.log())
    }

    /// Asks the emulator to stop as a service manager does and waits for it to exit.
    pub fn stop(&mut self) -> ExitStatus {
        self.terminate();
        let exit = self.exit_within(STOP_TIMEOUT);
        exit.unwrap_or_else(|| panic!("the emulator did not exit after SIGTERM"))
    }

    /// SIGTERM, sent directly: a build image need not carry a `kill` program.
    #[cfg(unix)]
    fn terminate(&mut self) {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.child.id()).unwrap());
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).unwrap();
    }

    #[cfg(not(unix))]
    fn terminate(&mut self) {
        self.child.kill().unwrap();
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.folder.join(name)
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.home.join(LOG_FILE)).unwrap_or_default()
    }

    /// The key id and secret in the published `emulator_credentials`.
    pub fn published_key(&self) -> (String, String) {
        let text = std::fs::read_to_string(self.file("emulator_credentials")).unwrap();
        let value = |name| profile_value(&text, name).unwrap_or_else(|| panic!("no {name}"));
        (value("aws_access_key_id"), value("aws_secret_access_key"))
    }

    /// Only `PATH` comes from the test's environment, so nothing set for the test changes the run.
    fn spawn(args: &[&str], home: PathBuf, env: &[(&str, &str)]) -> Self {
        let log = std::fs::File::create(home.join(LOG_FILE)).unwrap();
        let mut command = Command::new(emulator_binary());
        command.env_clear().env("HOME", &home);
        command.envs(std::env::var_os("PATH").map(|path| ("PATH", path)));
        command.env("EMULATOR_TELEMETRY", "off");
        command.envs(env.iter().copied()).stdin(Stdio::null());
        command.args(["-p", "0", "--idle-timeout", ORPHAN_IDLE_TIMEOUT_SECS]);
        command.args(args).stderr(log.try_clone().unwrap());
        let child = command.stdout(log).spawn().unwrap();
        let (client, folder, port) = (OnceLock::new(), PathBuf::new(), 0);
        Self {
            child,
            client,
            home,
            folder,
            port,
        }
    }

    fn exit_within(&mut self, timeout: Duration) -> Option<ExitStatus> {
        wait_until(timeout, || self.child.try_wait().ok().flatten())
    }
}

impl Drop for Emulator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// The value of `name` in an AWS shared credentials or config file.
pub fn profile_value(text: &str, name: &str) -> Option<String> {
    let mut pairs = text.lines().filter_map(|line| line.split_once('='));
    let (_, value) = pairs.find(|(key, _)| key.trim() == name)?;
    Some(value.trim().to_string())
}

/// The binary Cargo built for the test's crate, which it names in `CARGO_BIN_EXE_<name>`.
fn emulator_binary() -> PathBuf {
    let built = std::env::vars().find(|(name, _)| name.starts_with("CARGO_BIN_EXE_"));
    PathBuf::from(
        built
            .expect("run under cargo test, or set CARGO_BIN_EXE_<name>")
            .1,
    )
}

/// A new folder to serve as an emulator's home: never one another user created first.
fn new_home() -> PathBuf {
    static STARTS: AtomicU32 = AtomicU32::new(0);
    let name = format!("emulator-security-{}", std::process::id());
    let home = std::env::temp_dir().join(format!("{name}-{}", STARTS.fetch_add(1, Relaxed)));
    let mut owner_only = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut owner_only, 0o700);
    owner_only.create(&home).map(|()| home).unwrap()
}

fn wait_until<T>(timeout: Duration, mut ready: impl FnMut() -> Option<T>) -> Option<T> {
    let give_up = Instant::now() + timeout;
    loop {
        match ready() {
            None if Instant::now() < give_up => std::thread::sleep(POLL_INTERVAL),
            outcome => return outcome,
        }
    }
}

/// The folder under `folder` holding a fully written `config`, published last, and not a link.
fn folder_publishing_config(folder: &Path, depth_left: usize) -> Option<PathBuf> {
    let config = folder.join("config");
    let is_file = std::fs::symlink_metadata(&config).is_ok_and(|file| file.is_file());
    if is_file && std::fs::read_to_string(config).is_ok_and(|text| text.ends_with('\n')) {
        return Some(folder.to_path_buf());
    }
    let entries = std::fs::read_dir(folder).ok().filter(|_| depth_left > 0)?;
    let is_folder = |entry: &std::fs::DirEntry| entry.file_type().is_ok_and(|kind| kind.is_dir());
    let mut folders = entries.flatten().filter(is_folder);
    folders.find_map(|entry| folder_publishing_config(&entry.path(), depth_left - 1))
}
