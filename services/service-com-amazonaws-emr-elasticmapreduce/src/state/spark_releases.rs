// SPDX-License-Identifier: CC0-1.0

//! The Amazon EMR SPARK release family, one concern of the release catalog.
//!
//! Beside the `emr-<major>.<minor>.<patch>` releases that bundle the whole Hadoop
//! ecosystem ([`RELEASE_LINES`]), Amazon EMR publishes a Spark-only release family whose
//! labels carry their own prefix ([`SPARK_RELEASE_PREFIX`]): a cluster launched on one of
//! them runs Spark, and it is the family the Spark Connect session surface needs — "this
//! operation is supported in Amazon EMR Spark 8.0.0 and later" (`StartSession` doc, Smithy
//! model), which is where the labels below come from.
//!
//! The family is one this service RESOLVES wherever a release has to be resolved: a launch
//! names it ([`catalog_release`]) and `DescribeReleaseLabel` describes it
//! ([`release_label_record`]). It is not part of the labels `ListReleaseLabels` enumerates
//! ([`catalog_release_labels`]), which are the `emr-<major>.<minor>.<patch>` series of the
//! Release Guide: what IS recorded about this family is that a cluster launches on it, and
//! nothing records it in that listing. Its names are re-exported by the state root, so
//! every caller keeps reaching them through `crate::state`.

use super::*;

/// The prefix every release of the Spark family carries, so its labels cannot be confused
/// with the `emr-<major>.<minor>.<patch>` ones: `emr-spark-<major>.<minor>.<patch>`.
pub const SPARK_RELEASE_PREFIX: &str = "emr-spark-";

/// The application an Amazon EMR Spark release bundles, as the catalog spells it.
pub const SPARK_RELEASE_APPLICATION: &str = "Spark";

/// Every release line of the Spark family this emulator offers, oldest first.
///
/// Recorded behavior for the labels: a cluster launched with `emr-spark-8.0.0` is served,
/// and that is the release `StartSession` names as the first one with the session surface
/// ("supported in Amazon EMR Spark 8.0.0 and later").
pub const SPARK_RELEASE_LINES: &[ReleaseLine] = &[ReleaseLine {
    major: 8,
    minor: 0,
    patches: &[0],
}];

/// The label of the Spark release `major.minor.patch`, as Amazon EMR spells it.
pub fn spark_release_label(major: u32, minor: u32, patch: u32) -> String {
    format!("{SPARK_RELEASE_PREFIX}{major}.{minor}.{patch}")
}

/// The Spark release `label` names, when the family offers it.
///
/// Spelled exactly as [`spark_release_label`] renders it — so a padded or differently cased
/// variant names no release — and naming a patch release published on one of the lines in
/// [`SPARK_RELEASE_LINES`].
pub fn spark_release(label: &str) -> Option<(u32, u32, u32)> {
    let mut parts = label.strip_prefix(SPARK_RELEASE_PREFIX)?.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || label != spark_release_label(major, minor, patch) {
        return None;
    }
    SPARK_RELEASE_LINES
        .iter()
        .find(|line| line.major == major && line.minor == minor && line.patches.contains(&patch))
        .map(|_| (major, minor, patch))
}

/// Every label of the Spark family this emulator offers for a launch, NEWEST release
/// first.
pub fn spark_release_labels() -> Vec<String> {
    let mut releases: Vec<(u32, u32, u32)> = SPARK_RELEASE_LINES
        .iter()
        .flat_map(|line| {
            line.patches
                .iter()
                .map(move |&patch| (line.major, line.minor, patch))
        })
        .collect();
    releases.sort_unstable_by(|left, right| right.cmp(left));
    releases
        .into_iter()
        .map(|(major, minor, patch)| spark_release_label(major, minor, patch))
        .collect()
}

/// The catalog's entry for a label of the Spark family, or `None` when `label` names no
/// release of it.
///
/// The label is reported as the family spells it, and the only application reported is the
/// one such a release bundles ([`SPARK_RELEASE_APPLICATION`]), at the version the catalog
/// holds for that release line — the same table every other release's versions come from.
pub fn spark_release_label_record(label: &str) -> Option<ReleaseLabelRecord> {
    let (major, minor, patch) = spark_release(label)?;
    Some(ReleaseLabelRecord {
        label: spark_release_label(major, minor, patch),
        applications: release_line_applications(major, minor)
            .into_iter()
            .filter(|application| {
                application
                    .name
                    .as_deref()
                    .is_some_and(|name| name == SPARK_RELEASE_APPLICATION)
            })
            .collect(),
        available_os_releases: os_releases_for(major)
            .iter()
            .map(|&label| OSRelease {
                label: Some(label.to_string()),
            })
            .collect(),
    })
}
