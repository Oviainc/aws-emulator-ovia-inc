// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The emulator's folder in the user's home.
//! Telemetry state and request-authentication credentials both live under it, so this is the
//! one place that decides where it is: the folder set with [`set_emulator_home_dir`], else
//! [`DIR_NAME`].

use std::path::PathBuf;
use std::sync::OnceLock;

/// The folder in the user's home when the emulator sets no other.
pub const DIR_NAME: &str = ".emulator";

static CHOSEN_DIR: OnceLock<&'static str> = OnceLock::new();

/// Sets the folder, a path relative to the user's home such as `.config/emulator`. Refused when it is
/// empty, absolute or has an empty, `.` or `..` component (trailing spaces and dots ignored, as
/// Windows ignores them). Call it before anything reads the folder: once it was set to or read as
/// another value, it is refused.
pub fn set_emulator_home_dir(relative: &'static str) -> Result<(), String> {
    let outside_home = relative.is_empty()
        || relative.starts_with(['/', '\\'])
        || relative.contains(':')
        || relative
            .split(['/', '\\'])
            .any(|component| component.trim_end_matches([' ', '.']).is_empty());
    if outside_home {
        return Err(format!(
            "emulator home folder {relative:?} is not inside the home folder"
        ));
    }
    match *CHOSEN_DIR.get_or_init(|| relative) {
        chosen if chosen == relative => Ok(()),
        chosen => Err(format!("emulator home folder is already {chosen:?}")),
    }
}

pub fn emulator_home() -> Option<PathBuf> {
    let dir = *CHOSEN_DIR.get_or_init(|| DIR_NAME);
    home_dir().map(|home| home.join(dir))
}

#[cfg(windows)]
fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(windows))]
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_folder_wins_once_and_bad_folders_are_refused() {
        for refused in [
            "",
            "/etc",
            "\\x",
            "C:\\x",
            "c:x",
            "\\\\srv\\share",
            "..",
            ".",
            "./x",
            "a/../b",
            "a\\..",
            "a//b",
            ".. ",
            "a/... ",
        ] {
            assert!(set_emulator_home_dir(refused).is_err(), "{refused}");
        }
        assert_eq!(set_emulator_home_dir(".aws/emulator"), Ok(()));
        assert_eq!(set_emulator_home_dir(".aws/emulator"), Ok(()));
        assert!(set_emulator_home_dir(".other").is_err());
        if let Some(home) = home_dir() {
            assert_eq!(emulator_home(), Some(home.join(".aws/emulator")));
        }
    }
}
