// SPDX-License-Identifier: CC0-1.0

//! The CATALOG `ListPlatformBranches` reports, as opposed to the filtering and
//! paging rules [`crate::platform_branches`] covers.
//!
//! The rule under test: the operation reports the AWS-managed platform-branch
//! inventory the service publishes — for every platform, EVERY branch it has
//! published, the current ones and the deprecated and retired generations
//! behind them — each carrying
//!
//! * `LifecycleState` in the capitalized form the wire carries (`Supported`,
//!   `Deprecated`, `Retired`, `Beta`), while a `LifecycleState` FILTER value is
//!   one of the documented lower-case spellings;
//! * `BranchOrder`, "an ordinal number that designates the order in which
//!   platform branches have been added to a platform" where "a larger
//!   `BranchOrder` value designates a newer platform branch within the
//!   platform" — an ordinal AWS issues per platform in steps of 100, NOT a
//!   0,1,2… position in the service's own table;
//! * `SupportedTierList` drawn from the two documented tiers;
//!
//! and the catalog agrees with every other view the service has of it: a
//! version is published for exactly the branches that have not retired, and a
//! branch's life-cycle state and tier list are the same whichever operation
//! reports them.
//!
//! Every assertion spans a SET of inputs: the platform rows are generated from
//! the catalog the server itself reports (so the suite holds as the catalog
//! grows), the life-cycle rows cover all four documented states, and the spot
//! rows below are catalog entries read off Elastic Beanstalk's published
//! platform inventory.
//!
//! The HTTP harness and the branch reader are [`crate::lifecycle`]'s and
//! [`crate::platform_branches`]', so this suite adds no second copy of either.

use std::collections::BTreeMap;

use crate::lifecycle::{elements, Emulator};
use crate::platform_branches::{
    branches, filter_params, full_catalog, list, reported, Branch, LIFECYCLE_STATES,
    REPORTED_LIFECYCLE_STATES, TIERS,
};

/// The step AWS issues `BranchOrder` in within a platform: the recorded
/// ordinals of every platform run in hundreds (…, 11200, 11100, 11000, …), two
/// branches published together sharing one ordinal.
const BRANCH_ORDER_STEP: i32 = 100;

/// One row of the catalog table below: a branch of the published inventory,
/// with the life-cycle state and the ordinal AWS publishes it under.
struct CatalogRow {
    platform_name: &'static str,
    branch_name: &'static str,
    /// The REPORTED (capitalized) life-cycle state.
    lifecycle_state: &'static str,
    branch_order: i32,
    /// Whether the branch runs worker environments as well as web server ones.
    both_tiers: bool,
}

/// Branches of Elastic Beanstalk's published platform inventory, one per shape
/// of catalog entry, NONE of them a branch a divergence report cited.
///
/// SOURCE: the "Elastic Beanstalk supported platforms" and "Retired platform
/// branches" documents — the inventory the real `ListPlatformBranches`
/// enumerates. The rows span the forms an entry takes: a current Linux branch
/// of a runtime platform, the ECS branch of the Docker platform, a current
/// Windows branch (web server tier only), a retired Amazon Linux 2 generation,
/// a retired Amazon Linux generation, and a legacy preconfigured-Docker
/// platform that has retired whole.
const CATALOG_ROWS: &[CatalogRow] = &[
    CatalogRow {
        platform_name: "Corretto",
        branch_name: "Corretto 21 running on 64bit Amazon Linux 2023",
        lifecycle_state: "Supported",
        branch_order: 12300,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Docker",
        branch_name: "ECS running on 64bit Amazon Linux 2023",
        lifecycle_state: "Supported",
        branch_order: 11500,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: ".NET on Windows Server",
        branch_name: "IIS 10.0 running on 64bit Windows Server Core 2022",
        lifecycle_state: "Supported",
        branch_order: 10100,
        both_tiers: false,
    },
    CatalogRow {
        platform_name: "Node.js",
        branch_name: "Node.js 14 running on 64bit Amazon Linux 2",
        lifecycle_state: "Retired",
        branch_order: 11200,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Tomcat",
        branch_name: "Tomcat 7 with Java 7 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 10000,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "GlassFish",
        branch_name: "Preconfigured Docker - GlassFish 4.1 running on 64bit Debian jessie",
        lifecycle_state: "Retired",
        branch_order: 9800,
        both_tiers: false,
    },
];

/// The branches of `platform`, as the server reports them for a
/// `PlatformName = <platform>` filter.
async fn of_platform(emulator: &Emulator, platform: &str) -> Vec<Branch> {
    let params = filter_params(0, "PlatformName", "=", &[platform.to_string()]);
    let answer = list(emulator, &params).await;
    branches(&answer, platform)
}

/// The text of the first `<tag>` in `xml`, or `""` when there is none.
fn field(xml: &str, tag: &str) -> String {
    elements(xml, tag)
        .first()
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// The members of the `<tag>` list in `xml`.
fn string_list(xml: &str, tag: &str) -> Vec<String> {
    elements(xml, tag)
        .iter()
        .flat_map(|list| elements(list, "member"))
        .map(|value| value.trim().to_string())
        .collect()
}

#[tokio::test]
async fn every_published_branch_carries_an_aws_shaped_ordinal_and_state() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    for branch in &catalog {
        let case = branch.key();
        assert!(
            REPORTED_LIFECYCLE_STATES.contains(&branch.lifecycle_state.as_str()),
            "case {case:?}: expected LifecycleState in {REPORTED_LIFECYCLE_STATES:?} (the \
             capitalized form the wire carries), got {:?}",
            branch.lifecycle_state
        );
        // The ordinal is AWS's per-platform ordinal, issued in steps of 100 —
        // not a 0,1,2… counter over the service's own table, which is what a
        // placeholder catalog produces.
        assert_eq!(
            branch.branch_order % BRANCH_ORDER_STEP,
            0,
            "case {case:?}: expected a BranchOrder issued in steps of {BRANCH_ORDER_STEP}, got {}",
            branch.branch_order
        );
        assert!(
            branch.branch_order >= BRANCH_ORDER_STEP,
            "case {case:?}: expected an ordinal in the published bands (at least \
             {BRANCH_ORDER_STEP}), got {}",
            branch.branch_order
        );
        assert!(
            branch
                .supported_tiers
                .iter()
                .all(|tier| TIERS.contains(&tier.as_str())),
            "case {case:?}: expected a SupportedTierList drawn from {TIERS:?}, got {:?}",
            branch.supported_tiers
        );
        // A branch that names a runtime and an operating system runs
        // environments, so it names the tiers they run in; only a branch that
        // is not a runtime at all (the custom-platform builder) names none.
        if branch.branch_name.contains(" running on ") {
            assert!(
                !branch.supported_tiers.is_empty(),
                "case {case:?}: expected the tiers this runtime branch supports, got {:?}",
                branch.supported_tiers
            );
        }
    }

    // A placeholder catalog is small and uniform; the published one spans
    // ordinal bands (an operating-system generation per band) within a single
    // platform, which is what makes BranchOrder informative at all.
    let banded = catalog
        .iter()
        .filter(|branch| {
            catalog.iter().any(|other| {
                other.platform_name == branch.platform_name
                    && other.branch_order / 1000 != branch.branch_order / 1000
            })
        })
        .count();
    assert!(
        banded > 0,
        "case \"ordinal bands\": expected at least one platform publishing branches in more than \
         one ordinal band, got {:?}",
        catalog
            .iter()
            .map(|branch| (branch.key(), branch.branch_order))
            .collect::<Vec<(String, i32)>>()
    );
}

#[tokio::test]
async fn a_platform_filter_reports_that_platforms_whole_inventory_newest_first() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // The rows are every platform the service publishes, so this spans the
    // shapes a platform takes: many branches or few, a single tier or both,
    // fully supported or retired whole.
    let mut platforms: Vec<String> = catalog
        .iter()
        .map(|branch| branch.platform_name.clone())
        .collect();
    platforms.sort();
    platforms.dedup();
    assert!(
        platforms.len() >= 2,
        "case \"catalog\": expected several published platforms, got {platforms:?}"
    );

    for platform in &platforms {
        let case = platform.as_str();
        let expected: Vec<Branch> = catalog
            .iter()
            .filter(|branch| branch.platform_name == *platform)
            .cloned()
            .collect();
        let reported_branches = of_platform(&emulator, platform).await;
        assert_eq!(
            reported_branches, expected,
            "case {case:?}: a PlatformName filter must report exactly that platform's branches, \
             got {reported_branches:?}"
        );
        assert!(
            !reported_branches.is_empty(),
            "case {case:?}: expected the platform's branches, got none"
        );
        // "A larger BranchOrder value designates a newer platform branch within
        // the platform": newest first.
        for pair in reported_branches.windows(2) {
            assert!(
                pair[0].branch_order >= pair[1].branch_order,
                "case {case:?}: expected a non-increasing BranchOrder, got {} ({:?}) then {} \
                 ({:?})",
                pair[0].branch_order,
                pair[0].branch_name,
                pair[1].branch_order,
                pair[1].branch_name
            );
        }
        // Every branch of a platform is reported once, and under that platform.
        let mut names: Vec<&str> = reported_branches
            .iter()
            .map(|branch| branch.branch_name.as_str())
            .collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            before,
            "case {case:?}: expected each branch once, got {names:?}"
        );
    }

    // No platform is a one- or two-entry stub: the published inventory of a
    // runtime platform carries its current branches AND the generations behind
    // them, so at least one platform reports several and the catalog as a whole
    // reports far more than one per platform.
    let largest = platforms
        .iter()
        .map(|platform| {
            catalog
                .iter()
                .filter(|branch| branch.platform_name == *platform)
                .count()
        })
        .max()
        .unwrap_or_default();
    assert!(
        largest >= 8,
        "case \"inventory depth\": expected a platform publishing its whole branch history, got \
         at most {largest} branches for one platform"
    );
}

#[tokio::test]
async fn the_published_catalog_holds_the_branches_aws_publishes() {
    let emulator = Emulator::new();

    for row in CATALOG_ROWS {
        let case = row.branch_name;
        let reported_branches = of_platform(&emulator, row.platform_name).await;
        let branch = reported_branches
            .iter()
            .find(|branch| branch.branch_name == row.branch_name)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the platform {:?} to publish this branch, got {:?}",
                    row.platform_name,
                    reported_branches
                        .iter()
                        .map(|branch| branch.branch_name.clone())
                        .collect::<Vec<String>>()
                )
            });
        assert_eq!(
            branch.lifecycle_state, row.lifecycle_state,
            "case {case:?}: unexpected LifecycleState, got {:?}",
            branch.lifecycle_state
        );
        assert_eq!(
            branch.branch_order, row.branch_order,
            "case {case:?}: unexpected BranchOrder, got {}",
            branch.branch_order
        );
        let expected_tiers: Vec<String> = if row.both_tiers {
            TIERS.iter().map(|tier| (*tier).to_string()).collect()
        } else {
            vec![TIERS[0].to_string()]
        };
        assert_eq!(
            branch.supported_tiers, expected_tiers,
            "case {case:?}: unexpected SupportedTierList, got {:?}",
            branch.supported_tiers
        );
    }
}

#[tokio::test]
async fn each_documented_life_cycle_state_is_reported_capitalized() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // One row per documented filter value: the filter takes the lower-case
    // spelling, and every branch it selects reports the capitalized one.
    let mut selected = 0usize;
    for state in LIFECYCLE_STATES {
        let case = *state;
        let params = filter_params(0, "LifecycleState", "=", &[(*state).to_string()]);
        let answer = list(&emulator, &params).await;
        let reported_branches = branches(&answer, case);
        for branch in &reported_branches {
            assert_eq!(
                branch.lifecycle_state,
                reported(state),
                "case {case:?}: expected the branch {:?} to report the capitalized spelling, got \
                 {:?}",
                branch.branch_name,
                branch.lifecycle_state
            );
        }
        selected += reported_branches.len();
    }
    // The four documented states partition the catalog: every published branch
    // is selected by exactly one of them, so none reports a state outside the
    // documented set or a second spelling of one inside it.
    assert_eq!(
        selected,
        catalog.len(),
        "case \"partition\": expected the four documented states to select every branch exactly \
         once, got {selected} of {}",
        catalog.len()
    );
}

#[tokio::test]
async fn a_version_is_published_for_exactly_the_branches_an_environment_can_run_on() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    let versions = emulator.call("ListPlatformVersions", &[]).await;
    assert_eq!(
        versions.status, 200,
        "case \"published versions\": expected 200 from ListPlatformVersions, got {} and body {}",
        versions.status, versions.body
    );
    // `BranchName` -> what the platform-version surface says about that branch.
    let mut published: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for member in versions.values_at("PlatformSummaryList/member") {
        published.insert(
            field(member, "PlatformBranchName"),
            (
                field(member, "PlatformBranchLifecycleState"),
                string_list(member, "SupportedTierList"),
            ),
        );
    }

    for branch in &catalog {
        let case = branch.key();
        let retired = branch.lifecycle_state == reported("retired");
        match published.get(&branch.branch_name) {
            Some((state, tiers)) => {
                assert!(
                    !retired,
                    "case {case:?}: a retired branch has no version an account can deploy on, but \
                     a version is published for it"
                );
                // One state and one tier list per branch, whichever operation
                // reports it.
                assert_eq!(
                    state, &branch.lifecycle_state,
                    "case {case:?}: PlatformBranchLifecycleState must be the branch's own state, \
                     got {state:?}"
                );
                assert_eq!(
                    tiers, &branch.supported_tiers,
                    "case {case:?}: a version runs exactly the tiers its branch supports, got \
                     {tiers:?}"
                );
            }
            // A retired branch has no version an account can deploy on, and
            // neither has a branch that supports no environment tier — a branch
            // that is not a runtime an application is deployed on at all.
            None => assert!(
                retired || branch.supported_tiers.is_empty(),
                "case {case:?}: expected a published version for a branch in state {:?} \
                 supporting {:?}",
                branch.lifecycle_state,
                branch.supported_tiers
            ),
        }
    }
}
