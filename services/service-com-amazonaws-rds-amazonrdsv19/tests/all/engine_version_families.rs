// SPDX-License-Identifier: CC0-1.0

//! How the engine-version catalogue `DescribeDBEngineVersions` reports is POPULATED, and
//! how its `Marker` walks it.
//!
//! `tests/all/db_engine_versions.rs` pins what the operation DOES with the catalogue —
//! the filters, `DefaultOnly`, `IncludeAll`, the per-entry members. This suite pins which
//! rows the catalogue has to carry at all, as three rules over SETS of engines rather
//! than over one recorded listing:
//!
//! * a major version past its community end of life is published through this service's
//!   own dated Amazon RDS Extended Support builds, `<version>-rds.YYYYMMDD`;
//! * two engines that are EDITIONS of one database product publish the same
//!   major-version families and the same builds of them;
//! * a build that is not the latest of its family offers a MINOR upgrade inside that
//!   family, so an in-family upgrade chain exists rather than a single build standing in
//!   for the family;
//!
//! and the pagination rule the catalogue is read through: whatever the page size, walking
//! the `Marker` visits the report's own order exactly once, and a `Marker` re-presented
//! returns the same page.

use std::collections::{BTreeMap, BTreeSet};

use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::{
    all_versions_of, page_of_every_version, walk_every_version, Version,
};
use crate::db_instances::{emulator, subtrees, text};

/// The engines and major-version families this service publishes an Amazon RDS Extended
/// Support build line for.
///
/// Source: "Using Amazon RDS Extended Support" (Amazon RDS User Guide), which names RDS
/// for MySQL 5.7 and RDS for PostgreSQL 11 as the families RDS keeps releasing builds of
/// after the community stopped, each named `<version>-rds.YYYYMMDD`.
const EXTENDED_SUPPORT_FAMILIES: [(&str, &str); 2] = [("mysql", "5.7"), ("postgres", "11")];

/// Families that are reported `deprecated` too but carry NO dated build line: an Aurora,
/// Amazon DocumentDB or MariaDB family is versioned by the service itself, so a build of
/// it is a plain version rather than a dated extended-support one. These are the rows
/// that keep the rule above from being read as "every deprecated family is dated".
const SERVICE_MANAGED_FAMILIES: [(&str, &str); 4] = [
    ("aurora-mysql", "5.7"),
    ("aurora-postgresql", "11"),
    ("docdb", "3.6"),
    ("mariadb", "10.5"),
];

/// The database products this service offers in several EDITIONS, each edition its own
/// engine name.
///
/// The edition is the ENGINE and the major-version family is the product RELEASE, so
/// every edition of one product is offered the same releases and the same builds of them.
/// Microsoft SQL Server Developer Edition is the documented exception and is checked
/// separately: RDS offers it from the 2019 (15.00) release on, so its families are a
/// strict subset of the other editions'.
const EDITIONS_OF_A_PRODUCT: [(&str, &[&str]); 4] = [
    ("Db2", &["db2-ae", "db2-ce", "db2-se"]),
    ("Oracle Database (single-tenant)", &["oracle-ee", "oracle-se2"]),
    (
        "Oracle Database (multi-tenant)",
        &["oracle-ee-cdb", "oracle-se2-cdb"],
    ),
    (
        "Microsoft SQL Server",
        &[
            "sqlserver-ee",
            "sqlserver-se",
            "sqlserver-ex",
            "sqlserver-web",
        ],
    ),
];

/// The (engine, major-version family) pairs that must carry MORE than one build, so the
/// in-family minor upgrade chain a client reads is observable on each of them. They span
/// the three version forms the catalogue publishes: the dotted upstream form, the Oracle
/// release-update form and the Microsoft SQL Server build form.
const FAMILIES_WITH_A_MINOR_CHAIN: [(&str, &str); 10] = [
    ("mysql", "5.7"),
    ("postgres", "11"),
    ("oracle-ee", "19"),
    ("oracle-ee-cdb", "19"),
    ("oracle-se2", "19"),
    ("oracle-se2-cdb", "19"),
    ("sqlserver-ee", "14.00"),
    ("sqlserver-se", "14.00"),
    ("sqlserver-ex", "14.00"),
    ("sqlserver-web", "14.00"),
];

/// The builds of each major-version family one engine reports, in reported order.
fn families(reported: &[Version]) -> BTreeMap<String, Vec<Version>> {
    let mut grouped: BTreeMap<String, Vec<Version>> = BTreeMap::new();
    for build in reported {
        grouped
            .entry(build.major_engine_version.clone())
            .or_default()
            .push(build.clone());
    }
    grouped
}

/// One page of the whole catalogue: the `(engine, version)` keys it reported and the
/// `Marker` it carried. The request and the per-page guards are the shared ones.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    page_size: usize,
    marker: Option<&str>,
) -> (Vec<(String, String)>, Option<String>) {
    let (page, next) = page_of_every_version(router, case, page_size, marker).await;
    let keys = page
        .into_iter()
        .map(|build| (build.engine, build.engine_version))
        .collect();
    (keys, next)
}

/// The whole catalogue, walked one page of `page_size` at a time, as its
/// `(engine, version)` keys.
async fn walk(router: &MultiServiceRouter, case: &str, page_size: usize) -> Vec<(String, String)> {
    walk_every_version(router, case, page_size)
        .await
        .into_iter()
        .map(|build| (build.engine, build.engine_version))
        .collect()
}

/// The `(base version, YYYYMMDD)` halves of a dated Amazon RDS Extended Support build,
/// or `None` for a version that is not one.
///
/// `pub(crate)` so the catalogue suite's version-form table reads a dated build the same
/// way this one does.
pub(crate) fn dated_build(version: &str) -> Option<(&str, &str)> {
    version.split_once("-rds.")
}

/// Whether `date` has the `YYYYMMDD` FORM of an extended-support build stamp: eight
/// digits whose year, month and day fields are in range. A form check, not a calendar
/// one — it is the shape of the published name that the rule fixes, and the catalogue
/// holds no date arithmetic to get wrong.
///
/// `pub(crate)` for the same reason as [`dated_build`].
pub(crate) fn is_build_date(date: &str) -> bool {
    if date.len() != 8 || !date.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let number = |range: std::ops::Range<usize>| date[range].parse::<u32>().unwrap_or(0);
    let (year, month, day) = (number(0..4), number(4..6), number(6..8));
    (2000..=2100).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)
}

#[tokio::test]
async fn a_family_past_its_community_end_of_life_is_published_through_dated_builds() {
    let router = emulator();

    for (engine, family) in EXTENDED_SUPPORT_FAMILIES {
        let reported = all_versions_of(&router, engine).await;
        let grouped = families(&reported);
        let builds = grouped.get(family).unwrap_or_else(|| {
            panic!(
                "case {engine:?} {family:?}: the family must be reported, got {:?}",
                grouped.keys().collect::<Vec<_>>()
            )
        });

        let dated: Vec<&Version> = builds
            .iter()
            .filter(|build| dated_build(&build.engine_version).is_some())
            .collect();
        assert!(
            dated.len() > 1,
            "case {engine:?} {family:?}: an extended-support family must publish its \
             dated builds, got {:?}",
            builds
                .iter()
                .map(|build| build.engine_version.clone())
                .collect::<Vec<_>>()
        );

        let positions: BTreeMap<&str, usize> = builds
            .iter()
            .enumerate()
            .map(|(index, build)| (build.engine_version.as_str(), index))
            .collect();
        for build in dated {
            let case = format!("{engine} {}", build.engine_version);
            let (base, date) = dated_build(&build.engine_version)
                .expect("the filter above kept only dated builds");
            assert!(
                is_build_date(date),
                "case {case:?}: a dated build is named <version>-rds.YYYYMMDD, got the \
                 stamp {date:?}"
            );
            let base_position = positions.get(base).copied().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the base version {base:?} it extends must be \
                     reported too, got {:?}",
                    positions.keys().collect::<Vec<_>>()
                )
            });
            assert!(
                positions[build.engine_version.as_str()] > base_position,
                "case {case:?}: a dated build extends {base:?} and must be reported \
                 after it, got position {} against {base_position}",
                positions[build.engine_version.as_str()]
            );
            let base_build = &builds[base_position];
            assert_eq!(
                build.parameter_group_family, base_build.parameter_group_family,
                "case {case:?}: a dated build stays in the parameter group family of \
                 {base:?}, got {:?}",
                build.parameter_group_family
            );
            assert_eq!(
                build.major_engine_version, base_build.major_engine_version,
                "case {case:?}: a dated build stays in the major version of {base:?}, \
                 got {:?}",
                build.major_engine_version
            );
        }
    }

    // The same rule read the other way: a family the service versions itself carries no
    // dated build at all, so the form marks extended support rather than decorating
    // every deprecated entry.
    for (engine, family) in SERVICE_MANAGED_FAMILIES {
        let reported = all_versions_of(&router, engine).await;
        let grouped = families(&reported);
        let builds = grouped.get(family).unwrap_or_else(|| {
            panic!(
                "case {engine:?} {family:?}: the family must be reported, got {:?}",
                grouped.keys().collect::<Vec<_>>()
            )
        });
        let dated: Vec<String> = builds
            .iter()
            .filter(|build| dated_build(&build.engine_version).is_some())
            .map(|build| build.engine_version.clone())
            .collect();
        assert!(
            dated.is_empty(),
            "case {engine:?} {family:?}: a service-versioned family publishes no dated \
             extended-support build, got {dated:?}"
        );
    }
}

#[tokio::test]
async fn editions_of_one_product_publish_the_same_releases_and_builds() {
    let router = emulator();

    for (product, editions) in EDITIONS_OF_A_PRODUCT {
        let (first, rest) = editions
            .split_first()
            .expect("every product row names at least one edition");
        let reference = all_versions_of(&router, first).await;
        let reference_families: BTreeSet<String> = reference
            .iter()
            .map(|build| build.major_engine_version.clone())
            .collect();
        let reference_builds: BTreeSet<String> = reference
            .iter()
            .map(|build| build.engine_version.clone())
            .collect();
        assert!(
            !reference_families.is_empty(),
            "case {product:?}: {first:?} must report the product's releases, got none"
        );

        for edition in rest {
            let case = format!("{product}: {edition} against {first}");
            let reported = all_versions_of(&router, edition).await;
            let reported_families: BTreeSet<String> = reported
                .iter()
                .map(|build| build.major_engine_version.clone())
                .collect();
            let reported_builds: BTreeSet<String> = reported
                .iter()
                .map(|build| build.engine_version.clone())
                .collect();
            assert_eq!(
                reported_families, reference_families,
                "case {case:?}: every edition of one product is offered the same \
                 releases, got {reported_families:?}"
            );
            assert_eq!(
                reported_builds, reference_builds,
                "case {case:?}: every edition of one product is offered the same builds \
                 of a release, got {reported_builds:?}"
            );
            // The edition is the engine, so the parameter group family of a build names
            // that edition rather than the one it was compared against.
            for build in &reported {
                assert!(
                    build.parameter_group_family.starts_with(edition),
                    "case {case:?}: the DB parameter group family of {:?} must be the \
                     edition's own, got {:?}",
                    build.engine_version,
                    build.parameter_group_family
                );
            }
        }
    }

    // The documented exception: RDS offers Microsoft SQL Server Developer Edition from
    // the 2019 (15.00) release on, so it carries a STRICT subset of the other editions'
    // releases and no 14.00 family.
    let developer: BTreeSet<String> = all_versions_of(&router, "sqlserver-dev-ee")
        .await
        .iter()
        .map(|build| build.major_engine_version.clone())
        .collect();
    let enterprise: BTreeSet<String> = all_versions_of(&router, "sqlserver-ee")
        .await
        .iter()
        .map(|build| build.major_engine_version.clone())
        .collect();
    assert!(
        developer.is_subset(&enterprise) && developer.len() < enterprise.len(),
        "case \"sqlserver-dev-ee\": Developer Edition's releases must be a strict subset \
         of Enterprise Edition's, got {developer:?} against {enterprise:?}"
    );
    assert!(
        !developer.contains("14.00"),
        "case \"sqlserver-dev-ee\": Developer Edition is offered from the 2019 release \
         on, so it has no 14.00 family, got {developer:?}"
    );
}

#[tokio::test]
async fn a_build_that_is_not_the_latest_of_its_family_offers_a_minor_upgrade_inside_it() {
    let router = emulator();

    // Each row must carry a chain to read: more than one build in the family.
    for (engine, family) in FAMILIES_WITH_A_MINOR_CHAIN {
        let grouped = families(&all_versions_of(&router, engine).await);
        let builds = grouped.get(family).unwrap_or_else(|| {
            panic!(
                "case {engine:?} {family:?}: the family must be reported, got {:?}",
                grouped.keys().collect::<Vec<_>>()
            )
        });
        assert!(
            builds.len() > 1,
            "case {engine:?} {family:?}: the family must publish its successive builds, \
             got {:?}",
            builds
                .iter()
                .map(|build| build.engine_version.clone())
                .collect::<Vec<_>>()
        );
    }

    // …and the invariant the chain owes, over every engine the whole report names: a
    // build its family has moved past has exactly one AUTOMATIC upgrade destination, the
    // family's own later default. (Which targets an entry carries, and how each is
    // classified, is `an_entrys_upgrade_targets_are_that_engines_later_versions` in
    // `db_engine_versions.rs`; this is about the automatic one inside the family.)
    for engine in engines_reported(&router).await {
        for (_, builds) in families(&all_versions_of(&router, &engine).await) {
            for (index, build) in builds.iter().enumerate() {
                let case = format!("{engine} {}", build.engine_version);
                let later: BTreeSet<&str> = builds[index + 1..]
                    .iter()
                    .map(|build| build.engine_version.as_str())
                    .collect();
                let automatic: Vec<String> = subtrees(&build.xml, "UpgradeTarget")
                    .into_iter()
                    .filter(|target| text(target, "AutoUpgrade") == "true")
                    .map(|target| text(&target, "EngineVersion"))
                    .collect();
                assert!(
                    automatic.iter().all(|version| later.contains(version.as_str())),
                    "case {case:?}: an automatic upgrade stays inside the family, got \
                     {automatic:?} against the family's later builds {later:?}"
                );
                assert_eq!(
                    automatic.len(),
                    usize::from(!later.is_empty()),
                    "case {case:?}: a build the family has moved past has exactly ONE \
                     automatic destination and the family's latest build has none, got \
                     {automatic:?} for later builds {later:?}"
                );
            }
        }
    }
}

/// Every engine the whole catalogue reports, in the order it first names them, read by
/// walking the report's own pages rather than from a list written here.
async fn engines_reported(router: &MultiServiceRouter) -> Vec<String> {
    let mut engines: Vec<String> = Vec::new();
    for (engine, _) in walk(router, "every engine", 100).await {
        if !engines.contains(&engine) {
            engines.push(engine);
        }
    }
    engines
}

#[tokio::test]
async fn the_marker_walks_one_total_order_whatever_the_page_size() {
    let router = emulator();

    // "Constraints: Minimum 20, maximum 100" — the published bounds and a size between
    // them that divides neither the catalogue nor an engine's run of entries.
    let reference = walk(&router, "pages of 100", 100).await;
    assert!(
        reference.len() > 20,
        "case \"pages of 100\": the catalogue must be longer than one smallest page, got \
         {} entries",
        reference.len()
    );
    for page_size in [20usize, 37, 100] {
        let case = format!("pages of {page_size}");
        let walked = walk(&router, &case, page_size).await;
        assert_eq!(
            walked, reference,
            "case {case:?}: paging must visit the report's own order exactly once, \
             whatever the page size"
        );
    }

    // The entries of one engine are contiguous in that order, so a page is read as whole
    // engine families rather than interleaved ones.
    let mut started: Vec<String> = Vec::new();
    for (engine, _) in &reference {
        if started.last() != Some(engine) {
            assert!(
                !started.contains(engine),
                "case \"pages of 100\": the entries of {engine:?} must be contiguous, got \
                 the engine again after {started:?}"
            );
            started.push(engine.clone());
        }
    }

    // A Marker is a cursor, not a one-shot token: re-presenting it answers the same page.
    let (first, marker) = page(&router, "the first page", 20, None).await;
    let marker = marker.expect("a page of 20 leaves entries, so it carries a Marker");
    assert_eq!(
        first,
        reference[..first.len()].to_vec(),
        "case \"the first page\": the first page is the head of the report"
    );
    let (resumed, next) = page(&router, "resuming", 20, Some(&marker)).await;
    let (again, next_again) = page(&router, "resuming twice", 20, Some(&marker)).await;
    assert_eq!(
        (&resumed, &next),
        (&again, &next_again),
        "case \"resuming twice\": the same Marker must answer the same page"
    );
    assert_eq!(
        resumed,
        reference[first.len()..first.len() + resumed.len()].to_vec(),
        "case \"resuming\": a Marker resumes at the entry after the page that issued it"
    );
}
