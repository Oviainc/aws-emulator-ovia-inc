// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The emulator's command line: `-b`, `-p`, `-h`, `-v`, `-t on|off`, `-a on|off`, `--idle-timeout <seconds>`.

use emulator_telemetry::policy::{TELEMETRY_CLI_FLAG, TELEMETRY_CLI_SHORT_FLAG};
use std::net::IpAddr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

/// Release facts a release build compiles in with `option_env!`; `None` for a local build.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildInfo {
    pub version: Option<&'static str>,
    pub release_date: Option<&'static str>,
    pub commit: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// `-t on|off`; `None` leaves the decision to `EMULATOR_TELEMETRY` and the config file.
    pub telemetry: Option<bool>,
    /// `--idle-timeout`: stop after this long without an API call.
    pub idle_timeout: Option<Duration>,
    /// `-a on|off`: verify SigV4 request signatures (default on).
    pub authentication: bool,
    /// `-b`: overrides `EMULATOR_BIND_ADDRESS`.
    pub bind_address: Option<IpAddr>,
    /// `-p` (`0` = OS-assigned): overrides `EMULATOR_DATA_PORT`.
    pub port: Option<u16>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            telemetry: None,
            idle_timeout: None,
            authentication: true,
            bind_address: None,
            port: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(RunOptions),
    /// Print to stdout and exit 0 (`--help`, `--version`).
    Print(String),
}

const USAGE: &str = "\
Usage: <emulator> [OPTIONS]
  Starts an instance of the AWS Emulator.

Global options:
  -b, --bind <ip>            Address to bind. Default: 127.0.0.1 (EMULATOR_BIND_ADDRESS).
  -p, --port <port>          Port to listen on; 0 picks a free one. Default: 9297
                             (EMULATOR_DATA_PORT).
  -h, --help                 Display this help.
  -v, --version              Display the emulator version, release date and commit.
  -t, --telemetry on|off     Usage telemetry for this run. Default: on; off always wins over
                             EMULATOR_TELEMETRY and the config file, on cannot override them.
  -a, --auth on|off          Require SigV4 signatures with this run's keys, written to
                             <port>/emulator_credentials in its home folder. Default: on.
  --idle-timeout <seconds>   Emulator exits after <seconds> without an API call.

Admin routes:
  GET /__admin/health
      Return code of 200 indicates emulator working correctly.";

/// Parses the arguments after the program name. `Err` is the message to print before exiting 2.
pub fn parse_command_line(
    args: impl IntoIterator<Item = String>,
    build: BuildInfo,
) -> Result<Command, String> {
    let mut options = RunOptions::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline_value) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_string(), Some(value.to_string()))
            }
            _ => (arg, None),
        };
        let mut value = || {
            inline_value
                .clone()
                .or_else(|| args.next())
                .ok_or(format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Command::Print(USAGE.to_string())),
            "-v" | "--version" => return Ok(Command::Print(version_text(build))),
            TELEMETRY_CLI_SHORT_FLAG | TELEMETRY_CLI_FLAG => {
                options.telemetry = Some(parse_switch(TELEMETRY_CLI_FLAG, &value()?)?)
            }
            "-a" | "--auth" => options.authentication = parse_switch("--auth", &value()?)?,
            "-b" | "--bind" => {
                let raw = value()?;
                let parsed = raw.parse().map_err(|_| format!("--bind takes an IP address, not {raw:?}"));
                options.bind_address = Some(parsed?);
            }
            "-p" | "--port" => {
                let raw = value()?;
                let parsed = raw.parse().map_err(|_| format!("--port takes 0 to 65535, not {raw:?}"));
                options.port = Some(parsed?);
            }
            "--idle-timeout" => options.idle_timeout = Some(parse_seconds(&value()?)?),
            unknown => {
                return Err(format!(
                    "unknown argument {unknown:?}; run with --help to list the options"
                ));
            }
        }
    }
    Ok(Command::Run(options))
}

fn version_text(build: BuildInfo) -> String {
    let release_date = build.release_date.unwrap_or("unreleased");
    let commit = build.commit.unwrap_or("unknown");
    format!(
        "Version: {}\nRelease date: {release_date}\nCommit: {commit}",
        build.version.unwrap_or("unreleased")
    )
}

fn parse_switch(flag: &str, raw: &str) -> Result<bool, String> {
    match raw {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("{flag} takes on or off, not {raw:?}")),
    }
}

/// One year: longer values are refused rather than overflowing the deadline.
const MAX_IDLE_TIMEOUT_SECONDS: u64 = 365 * 24 * 3600;

fn parse_seconds(raw: &str) -> Result<Duration, String> {
    match raw.parse::<u64>() {
        Ok(seconds) if (1..=MAX_IDLE_TIMEOUT_SECONDS).contains(&seconds) => {
            Ok(Duration::from_secs(seconds))
        }
        _ => Err(format!(
            "--idle-timeout takes a whole number of seconds from 1 to {MAX_IDLE_TIMEOUT_SECONDS}, not {raw:?}"
        )),
    }
}

/// Time since the last API call, shared by the dispatch that records calls and the serve loop
/// that stops on `--idle-timeout`.
#[derive(Debug)]
pub(crate) struct IdleClock {
    started: Instant,
    last_call_ms: AtomicU64,
}

impl IdleClock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            last_call_ms: AtomicU64::new(0),
        }
    }

    pub(crate) fn record_call(&self) {
        let elapsed_ms = self.started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        self.last_call_ms.store(elapsed_ms, Ordering::Relaxed);
    }

    /// Resolves once `idle_timeout` has passed without a recorded call.
    pub(crate) async fn idle_for(&self, idle_timeout: Duration) {
        loop {
            let last_call =
                self.started + Duration::from_millis(self.last_call_ms.load(Ordering::Relaxed));
            if last_call.elapsed() >= idle_timeout {
                return;
            }
            tokio::time::sleep_until(last_call + idle_timeout).await;
        }
    }
}

static LISTEN_OVERRIDES: OnceLock<(Option<IpAddr>, Option<u16>)> = OnceLock::new();

/// `-b` / `-p` for the production serve path, overriding `EMULATOR_BIND_ADDRESS` and
/// `EMULATOR_DATA_PORT`; call before serving.
pub fn listen_on(bind_address: Option<IpAddr>, port: Option<u16>) {
    let _ = LISTEN_OVERRIDES.set((bind_address, port));
}

pub(crate) fn listen_overrides() -> (Option<IpAddr>, Option<u16>) {
    LISTEN_OVERRIDES.get().copied().unwrap_or_default()
}

static IDLE_TIMEOUT: OnceLock<Duration> = OnceLock::new();

fn idle_clock() -> &'static IdleClock {
    static CLOCK: OnceLock<IdleClock> = OnceLock::new();
    CLOCK.get_or_init(IdleClock::new)
}

/// Counts a request as an API call for `--idle-timeout`.
pub fn record_api_call() {
    idle_clock().record_call();
}

/// Makes the production serve path stop after `idle_timeout` without an API call.
pub fn stop_after_idle(idle_timeout: Duration) {
    let _ = IDLE_TIMEOUT.set(idle_timeout);
}

/// Pending forever unless [`stop_after_idle`] was called.
pub(crate) async fn idle_timeout_reached() {
    match IDLE_TIMEOUT.get() {
        Some(idle_timeout) => idle_clock().idle_for(*idle_timeout).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_command_line(args.iter().map(|arg| arg.to_string()), BuildInfo::default())
    }

    fn run(telemetry: Option<bool>, idle_secs: Option<u64>, authentication: bool) -> Result<Command, String> {
        Ok(Command::Run(RunOptions {
            telemetry,
            idle_timeout: idle_secs.map(Duration::from_secs),
            authentication,
            ..RunOptions::default()
        }))
    }

    #[test]
    fn arguments_parse_to_run_options() {
        let cases: [(&[&str], Result<Command, String>); 9] = [
            (&[], run(None, None, true)),
            (&["-t", "off"], run(Some(false), None, true)),
            (&["--telemetry=on"], run(Some(true), None, true)),
            (&["-t", "off", "-t", "on"], run(Some(true), None, true)),
            (&["--idle-timeout", "90"], run(None, Some(90), true)),
            (&["--idle-timeout=900", "-t", "on"], run(Some(true), Some(900), true)),
            (&["-a", "on"], run(None, None, true)),
            (&["--auth=off"], run(None, None, false)),
            (&["--auth", "on", "--idle-timeout", "1"], run(None, Some(1), true)),
        ];
        for (args, expected) in cases {
            assert_eq!(parse(args), expected, "{args:?}");
        }
        let Ok(Command::Run(options)) = parse(&["-b", "0.0.0.0", "--port=0"]) else { panic!() };
        assert_eq!((options.bind_address, options.port), (Some([0, 0, 0, 0].into()), Some(0)));
        let Ok(Command::Run(options)) = parse(&["--bind", "::1", "-p", "29999"]) else { panic!() };
        assert_eq!((options.bind_address, options.port), (Some("::1".parse().unwrap()), Some(29999)));
    }

    #[test]
    fn invalid_arguments_are_refused_with_a_message() {
        for args in [
            &["--bogus"][..],
            &["-t"],
            &["-t", "yes"],
            &["-a"],
            &["-a", "maybe"],
            &["-p", "70000"],
            &["-p", "-1"],
            &["-b", "nonsense"],
            &["-b"],
            &["--idle-timeout", "0"],
            &["--idle-timeout", "90s"],
            &["--idle-timeout", "-5"],
            &["--idle-timeout", "31536001"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
        assert_eq!(
            parse(&["-x"]).unwrap_err(),
            "unknown argument \"-x\"; run with --help to list the options"
        );
    }

    #[test]
    fn help_and_version_print_and_stop() {
        let Ok(Command::Print(help)) = parse(&["--idle-timeout", "5", "-h"]) else {
            panic!("help prints")
        };
        for row in ["-b, --bind <ip>", "Default: 9297", "-a, --auth on|off", "--idle-timeout <seconds>", "GET /__admin/health"] {
            assert!(help.contains(row), "{row}: {help}");
        }
        assert_eq!(
            parse(&["-v"]),
            Ok(Command::Print(
                "Version: unreleased\nRelease date: unreleased\nCommit: unknown".into()
            ))
        );
        let build = BuildInfo {
            version: Some("1.0.0"),
            release_date: Some("2026.10.10"),
            commit: Some("abc123"),
        };
        let printed = parse_command_line(["--version".to_string()], build);
        assert_eq!(
            printed,
            Ok(Command::Print(
                "Version: 1.0.0\nRelease date: 2026.10.10\nCommit: abc123".into()
            ))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_idle_timeout_counts_from_the_last_call() {
        let clock = IdleClock::new();
        tokio::time::advance(Duration::from_secs(4)).await;
        clock.record_call();
        let waited = Instant::now();
        clock.idle_for(Duration::from_secs(5)).await;
        assert_eq!(waited.elapsed(), Duration::from_secs(5));
    }
}
