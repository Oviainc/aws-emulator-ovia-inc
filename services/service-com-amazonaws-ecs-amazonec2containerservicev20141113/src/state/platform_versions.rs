// SPDX-License-Identifier: CC0-1.0

//! Which Fargate PLATFORM VERSION a task of a given revision could be placed on.
//!
//! The platform versions are published per operating system, so a version is a version OF
//! one of the `OSFamily` values — which makes the pairing a cross-field rule, keyed on the
//! `runtimePlatform` the registration stored rather than on anything the request carries.
//! Read by every operation that pairs a `platformVersion` with a revision: `RunTask`,
//! `CreateService`, `UpdateService` and `CreateTaskSet`.

use super::*;

/// Whether a requested `platformVersion` names no platform the revision's tasks could be
/// placed on, because it is not published for the operating system they run.
///
/// The Fargate platform versions are published PER OPERATING SYSTEM, and the WINDOWS
/// families have exactly one of them ([`LATEST_WINDOWS_PLATFORM_VERSION`]): the Linux
/// versions are not published for Windows containers at all, so a Windows revision paired
/// with one names a platform that does not exist — which is what
/// `PlatformUnknownException` ("The specified platform version doesn't exist.") is
/// modelled for, exactly as a value that was never a version is.
/// [`PLATFORM_VERSION_LATEST`] names no version of either, so it resolves against
/// whichever family the revision declares and always exists.
///
/// The Linux families are NOT held to one version in the same way: Linux has had several
/// published platform versions, and which of them a task may still be placed on is what
/// [`is_unknown_platform_version`] decides, before this check.
///
/// Three string comparisons over values this function only reads: it parses nothing,
/// indexes nothing and allocates nothing, whatever the caller spelled.
pub fn names_no_platform_for_os_family(requested: Option<&str>, os_family: &str) -> bool {
    let Some(requested) = requested else {
        return false;
    };
    requested != PLATFORM_VERSION_LATEST
        && is_windows_os_family(os_family)
        && requested != LATEST_WINDOWS_PLATFORM_VERSION
}
