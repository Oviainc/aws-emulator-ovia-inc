// SPDX-License-Identifier: CC0-1.0

//! THE SERVERS A DIRECTORY RUNS ON: the operating system this service runs a directory's
//! servers on, as `DirectoryDescription$OsVersion` reports it.
//!
//! `DirectoryDescription$OsVersion` is "the operating system (OS) version of the directory",
//! one of the two values the modelled `OSVersion` enum declares
//! (`smithy_query(query="shape",shape="OSVersion")` answers `["SERVER_2012",
//! "SERVER_2019"]`). It is a fact of the PLATFORM a directory type runs on rather than of
//! any one directory, so it is decided here from the directory's type and nothing else — not
//! from the request, and not from the size or edition the directory runs at.
//!
//! Only a directory whose servers this service runs a Windows Server on has one: AWS Managed
//! Microsoft AD runs the directory's domain controllers for the caller. A Simple AD directory
//! is Samba-based, and an AD Connector runs no directory at all — it forwards to a
//! self-managed one, whose OS is the customer's — so neither names an `OSVersion`.

use super::TYPE_MICROSOFT_AD;

/// The `OSVersion` an AWS Managed Microsoft AD directory runs: the `SERVER_2019` value of
/// the modelled enum.
///
/// Windows Server 2019 is the operating system AWS Managed Microsoft AD runs its domain
/// controllers on, which is why the enum's other value — `SERVER_2012`, the OS of the
/// directories created before it — belongs to no directory this emulator creates.
pub const MICROSOFT_AD_OS_VERSION: &str = "SERVER_2019";

/// The `OSVersion` a directory of `directory_type` reports, or `None` for a type whose
/// operating system Directory Service does not name.
///
/// THE one place a directory's OS version is decided, so every operation that reports a
/// directory reports the same one. A Simple AD directory HAS an operating system — Samba on
/// Linux — which the modelled enum has no value for, so the member is absent for it rather
/// than carrying a Windows Server version it does not run, and an AD Connector runs no
/// directory whose OS this service could name.
#[must_use]
pub fn os_version_of(directory_type: &str) -> Option<&'static str> {
    (directory_type == TYPE_MICROSOFT_AD).then_some(MICROSOFT_AD_OS_VERSION)
}
