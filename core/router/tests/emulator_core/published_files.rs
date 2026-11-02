// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The files a start publishes are owner-only, describe the running emulator, hold no private
//! key, replace whatever was at their path, and are removed by a clean stop.

use super::emulator_process::{profile_value, Emulator};
use std::fs::read_to_string;

const PUBLISHED_FILES: [&str; 3] = ["ca.pem", "emulator_credentials", "config"];
const LEFT_ALONE: &str = "a file the emulator must not write through a link\n";
const FOLDER_THEN_FILE_MODES: [u32; 4] = [0o700, 0o600, 0o600, 0o600];

#[cfg(unix)]
#[test]
#[cfg_attr(
    not(emulator_full_security_suite),
    ignore = "fails on long-running prod and preprod trees until their emulator server publishes owner-only folders"
)]
fn the_published_files_and_their_folder_are_readable_only_by_their_owner() {
    use std::os::unix::fs::PermissionsExt;
    let emulator = Emulator::start(&[]);
    let files = PUBLISHED_FILES.map(|name| emulator.file(name));
    let modes = [&emulator.folder].into_iter().chain(&files);
    let modes = modes.map(|path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777);
    assert_eq!(modes.collect::<Vec<_>>(), FOLDER_THEN_FILE_MODES);
}

#[test]
fn the_published_credentials_and_config_describe_the_running_emulator() {
    let emulator = Emulator::start(&[]);
    let (key, secret) = emulator.published_key();
    assert!(secret.len() >= 32, "a {}-character secret", secret.len());
    let credentials = read_to_string(emulator.file("emulator_credentials")).unwrap();
    let expected =
        format!("[default]\naws_access_key_id = {key}\naws_secret_access_key = <secret>\n");
    assert_eq!(credentials.replace(&secret, "<secret>"), expected);
    let config = read_to_string(emulator.file("config")).unwrap();
    let value = |name: &str| profile_value(&config, name);
    let region = value("region").unwrap_or_default();
    assert!(!region.is_empty(), "{config}");
    let endpoint = format!("https://127.0.0.1:{}", emulator.port);
    assert_eq!(value("endpoint_url"), Some(endpoint), "{config}");
    let ca_bundle = emulator.file("ca.pem").display().to_string();
    assert_eq!(value("ca_bundle"), Some(ca_bundle), "{config}");
}

#[test]
fn the_published_ca_pem_holds_only_the_certificate() {
    let emulator = Emulator::start(&[]);
    let ca_pem = read_to_string(emulator.file("ca.pem")).unwrap();
    let blocks: Vec<_> = ca_pem
        .lines()
        .filter(|line| line.starts_with("-----BEGIN "))
        .collect();
    assert_eq!(blocks, ["-----BEGIN CERTIFICATE-----"], "ca.pem");
}

#[cfg(unix)]
#[test]
fn a_clean_stop_removes_every_published_file_and_a_restart_replaces_links() {
    let mut first = Emulator::start(&[]);
    assert!(first.stop().success(), "{}", first.log());
    let left = PUBLISHED_FILES.map(|name| first.file(name).exists());
    assert_eq!(left, [false; 3], "{PUBLISHED_FILES:?} left on disk");
    let target = |name: &str| first.home.join(format!("left-alone-{name}"));
    for name in PUBLISHED_FILES {
        std::fs::write(target(name), LEFT_ALONE).unwrap();
        std::os::unix::fs::symlink(target(name), first.file(name)).unwrap();
    }
    let second = Emulator::start_in(&["-p", &first.port.to_string()], first.home.clone());
    for name in PUBLISHED_FILES {
        assert!(!second.file(name).is_symlink(), "{name} is still a link");
        let untouched = read_to_string(target(name)).unwrap();
        assert_eq!(untouched, LEFT_ALONE, "{name} written through a link");
    }
}
