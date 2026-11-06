// SPDX-License-Identifier: CC0-1.0

//! Which functions Lambda SnapStart can be applied to, and the refusals for the two
//! configurations that ask for it where it does not hold.
//!
//! # The rule
//!
//! `SnapStart$ApplyOn` set to [`super::SNAP_START_APPLY_ON_PUBLISHED_VERSIONS`] asks Lambda
//! to "create a snapshot of the initialized execution environment when you publish a
//! function version" — which only a MANAGED RUNTIME that carries the snapshot/restore
//! machinery can do. AWS publishes the set as runtime FAMILIES from a minimum version
//! ([`SNAP_START_RUNTIMES`]), so a request applying SnapStart to any other runtime asks for
//! something that runtime cannot do and is refused with the modelled 400, before anything
//! is stored.
//!
//! The second rule is an EXCLUSION: provisioned concurrency keeps initialised environments
//! of a version warm, and SnapStart restores each environment from a snapshot instead, so
//! one version cannot be served both ways. The refusal belongs to the call that would add
//! the second of the two — `PutProvisionedConcurrencyConfig` on a version whose
//! configuration applies SnapStart.
//!
//! Both are properties of a CONFIGURATION rather than of one operation, so every operation
//! that writes one (`CreateFunction`, `UpdateFunctionConfiguration`) asks the same question
//! here instead of each deciding for itself.

/// One runtime FAMILY SnapStart is supported on, from the version it is supported from:
/// the identifier prefix the model's `Runtime` values are built from, and the lowest
/// `(major, minor)` of that family that carries SnapStart.
///
/// Source: AWS's SnapStart documentation, "Supported runtimes" — Java 11 and later, Python
/// 3.12 and later, and .NET 8 and later. A family absent from this table supports SnapStart
/// in no version of it, which is why the table is the whole statement of the rule.
const SNAP_START_RUNTIMES: &[(&str, (u32, u32))] =
    &[("java", (11, 0)), ("python", (3, 12)), ("dotnet", (8, 0))];

/// The `(major, minor)` version `identifier` names inside the runtime family `family`, or
/// `None` when it is not a versioned identifier of that family at all.
///
/// A managed runtime identifier is the family name and then the language version, with an
/// operating-system suffix after it on some of them (`java17.al2023`): the version is the
/// leading numeric part, so a suffix that is no number contributes nothing and
/// `dotnetcore3.1` is not a version of the `dotnet` family at all.
fn family_version(identifier: &str, family: &str) -> Option<(u32, u32)> {
    let mut parts = identifier.strip_prefix(family)?.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts
        .next()
        .and_then(|minor| minor.parse::<u32>().ok())
        .unwrap_or(0);
    Some((major, minor))
}

/// Whether SnapStart can be applied to a function running `runtime`.
///
/// A function with no `Runtime` at all is a container image (`PackageType=Image`), which
/// carries no managed runtime to snapshot, so it supports SnapStart no more than an
/// unsupported family does.
#[must_use]
pub fn runtime_supports_snap_start(runtime: Option<&str>) -> bool {
    let Some(runtime) = runtime else {
        return false;
    };
    SNAP_START_RUNTIMES
        .iter()
        .any(|(family, from)| family_version(runtime, family).is_some_and(|version| version >= *from))
}

/// The refusal a request applying SnapStart to a runtime that does not support it gets.
///
/// The runtime is the one variable part: it is what the caller has to change, and it comes
/// from the request or from the configuration the request leaves alone.
#[must_use]
pub fn snap_start_unsupported_message(runtime: Option<&str>) -> String {
    format!(
        "SnapStart is not supported for the runtime {}",
        runtime.unwrap_or_default()
    )
}

/// The refusal `PutProvisionedConcurrencyConfig` gives for a version whose configuration
/// applies SnapStart.
///
/// Fixed text: the request already names the version, and the two features are exclusive
/// for every version alike.
pub const PROVISIONED_CONCURRENCY_WITH_SNAP_START_MESSAGE: &str =
    "Provisioned concurrency cannot be configured on a function version with SnapStart \
     enabled";

/// Whether `apply_on` — a `SnapStart$ApplyOn` as a configuration holds it — asks for
/// SnapStart at all. The member's other value and its absence both mean no snapshot is
/// taken, so neither is a function SnapStart applies to.
#[must_use]
pub fn applies_snap_start(apply_on: Option<&str>) -> bool {
    apply_on == Some(super::SNAP_START_APPLY_ON_PUBLISHED_VERSIONS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: SnapStart is supported on the Java 11+, Python 3.12+ and .NET 8+ managed
    /// runtimes and on nothing else.
    ///
    /// The rows span what the rule generalises over: each supported family at its own
    /// minimum and above it, the release just below each minimum, an identifier carrying an
    /// operating-system suffix, a family the table does not name, an OS-only runtime and a
    /// function with no runtime at all.
    #[test]
    fn snap_start_is_supported_exactly_on_the_published_runtime_families() {
        let rows: &[(Option<&str>, bool)] = &[
            (Some("java11"), true),
            (Some("java11.al2023"), true),
            (Some("java17"), true),
            (Some("java21"), true),
            (Some("java25"), true),
            (Some("java8.al2"), false),
            (Some("python3.12"), true),
            (Some("python3.13"), true),
            (Some("python3.11"), false),
            (Some("python2.7"), false),
            (Some("dotnet8"), true),
            (Some("dotnet10"), true),
            (Some("dotnet6"), false),
            (Some("dotnetcore3.1"), false),
            (Some("nodejs22.x"), false),
            (Some("ruby3.4"), false),
            (Some("provided.al2023"), false),
            (Some("go1.x"), false),
            (None, false),
        ];
        for (runtime, supported) in rows {
            let answered = runtime_supports_snap_start(*runtime);
            assert_eq!(
                answered, *supported,
                "case {runtime:?}: expected supported {supported}, got {answered}"
            );
        }
    }

    /// Only the value that asks for a snapshot is a configuration SnapStart applies to.
    #[test]
    fn only_published_versions_applies_snap_start() {
        let rows: &[(Option<&str>, bool)] = &[
            (Some(super::super::SNAP_START_APPLY_ON_PUBLISHED_VERSIONS), true),
            (Some(super::super::SNAP_START_APPLY_ON_NONE), false),
            (None, false),
        ];
        for (apply_on, applies) in rows {
            let answered = applies_snap_start(*apply_on);
            assert_eq!(
                answered, *applies,
                "case {apply_on:?}: expected applies {applies}, got {answered}"
            );
        }
    }
}
