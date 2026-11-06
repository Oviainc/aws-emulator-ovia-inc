// SPDX-License-Identifier: CC0-1.0

//! The account's access to the EC2 SERIAL CONSOLE of its instances: the one flag
//! `EnableSerialConsoleAccess` and `DisableSerialConsoleAccess` move and
//! `GetSerialConsoleAccessStatus` reports.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`, for the reason
//! `classic_link.rs` gives: one subject in one file, and out of the end of `state.rs`,
//! where every concurrently landing EC2 fix inserts. Everything here is an inherent method
//! of [`ComAmazonawsEc2Amazonec2State`], so a handler reaches it on the store itself.
//!
//! The flag itself is a field of [`ComAmazonawsEc2Amazonec2State`]
//! (`serial_console_access_enabled`), because the state struct is one struct; only the
//! code that reads and writes it lives here. It is ONE value for the whole account — the
//! operation that reports it describes "access to the EC2 serial console of all instances
//! … for your account" and names no Region, unlike the EBS encryption-by-default setting,
//! whose own doc scopes it to "your account in the current Region".

use super::*;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether access to the EC2 serial console of all instances is enabled for this
    /// account.
    ///
    /// A pure read: the setting has no clock-owed move — AWS reports an enable's effect as
    /// soon as the enable answers — so nothing about it changes with time. An account that
    /// has never enabled it reads `false`: "By default, access to the EC2 serial console is
    /// disabled for your account" (`GetSerialConsoleAccessStatus`).
    pub fn serial_console_access_enabled(&self) -> bool {
        self.serial_console_access_enabled
    }

    /// Turn access to the EC2 serial console on or off for this account, and answer the
    /// setting as it now stands — "If true, access to the EC2 serial console of all
    /// instances is enabled for your account"
    /// (`EnableSerialConsoleAccessResult$SerialConsoleAccessEnabled`).
    ///
    /// The answer is read back off the field rather than echoed from the argument, so a
    /// writer reports exactly what a later `GetSerialConsoleAccessStatus` will. Idempotent:
    /// enabling an account it is already on for, or disabling one it is already off for,
    /// changes nothing and answers the same value.
    pub fn set_serial_console_access_enabled(&mut self, enabled: bool) -> bool {
        self.serial_console_access_enabled = enabled;
        self.serial_console_access_enabled
    }
}
