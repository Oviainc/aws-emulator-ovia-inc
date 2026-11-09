// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the CONTENT of each `ListInstanceTypes` entry, over the wire
//! this service speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.ListInstanceTypes`).
//!
//! The rule under test, in one sentence: a reported `InstanceTypeInfo` carries the
//! COMPLETE set of configuration combinations its instance type supports — every
//! platform image the model declares, crossed with the tenancies that instance type
//! runs under and both billing modes — so no combination the service accepts is
//! missing from the entry, and `InstanceConfigurationFilter` therefore reports every
//! instance type that supports the combination asked for.
//!
//! SOURCE of the rule: `InstanceTypeInfo$SupportedInstanceConfigurations` is documented
//! as "all valid combinations of tenancy, platform type, and billing mode supported for
//! the specific WorkSpace Instance type. Contains the complete set of configuration
//! options available for this instance type"
//! (`smithy_query(query="doc",shape="InstanceTypeInfo",member="SupportedInstanceConfigurations")`),
//! and the platform images are the whole `PlatformTypeEnum`
//! (`smithy_query(query="shape",shape="PlatformTypeEnum")`: Windows, Windows BYOL,
//! Linux/UNIX, Ubuntu Pro Linux, Red Hat Enterprise Linux, Red Hat BYOL Linux, SUSE
//! Linux). The member is a SET, so these cases assert WHICH combinations an entry
//! carries, never the sequence they are serialised in.
//!
//! The table spans what the rule generalises over rather than one recorded entry: a row
//! per instance-family CLASS (graphics — every graphics family the catalog holds —
//! general purpose, compute, memory, high-memory, storage, `-flex`, high-frequency and
//! burstable), at sizes from both ends of each family's ladder, and along the one
//! dimension that does vary per row, the tenancies the family runs under. The expected
//! combinations per row are built here from the model's own enum value lists, not read
//! out of the crate's catalog.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;
use wsinst::ops::types::{
    BILLING_MODE_VALUES, INSTANCE_CONFIGURATION_TENANCY_ENUM_VALUES, PLATFORM_TYPE_ENUM_VALUES,
};

use crate::workspace_instance_lifecycle::{ok, router};

/// One configuration combination as the wire reports it: billing mode, platform type,
/// tenancy.
type Combination = (String, String, String);

/// Both tenancies of `InstanceConfigurationTenancyEnum`, for a family AWS also offers
/// as a Dedicated Instance.
const BOTH_TENANCIES: &[&str] = &["SHARED", "DEDICATED"];

/// Shared tenancy only, for the burstable (`t`-prefixed) families EC2 does not offer on
/// dedicated hardware.
const SHARED_ONLY: &[&str] = &["SHARED"];

/// The instance types under test: the class each one stands for, its name, and the
/// tenancies it runs under.
///
/// The names are `<family>.<size>` entries of the service's catalog, chosen one per
/// family class and at both ends of the published EC2 size ladders — none of them is
/// the entry the issue cited.
const CONFIGURATION_ROWS: &[(&str, &str, &[&str])] = &[
    // Graphics: every graphics family the catalog holds, which is where a narrower
    // platform set would show.
    ("graphics (AMD)", "g4ad.xlarge", BOTH_TENANCIES),
    ("graphics (NVIDIA T4)", "g4dn.12xlarge", BOTH_TENANCIES),
    ("graphics (NVIDIA A10G)", "g5.48xlarge", BOTH_TENANCIES),
    ("graphics (NVIDIA L4)", "g6.xlarge", BOTH_TENANCIES),
    ("graphics (NVIDIA L40S)", "g6e.8xlarge", BOTH_TENANCIES),
    ("graphics, memory-heavy", "gr6.4xlarge", BOTH_TENANCIES),
    // Non-graphics, one row per other family class.
    ("compute, fifth generation", "c5.9xlarge", BOTH_TENANCIES),
    ("compute, seventh generation AMD", "c7a.medium", BOTH_TENANCIES),
    ("general purpose, high frequency", "m5zn.3xlarge", BOTH_TENANCIES),
    ("general purpose, -flex ladder", "m7i-flex.16xlarge", BOTH_TENANCIES),
    ("memory optimised, eighth generation", "r8i.96xlarge", BOTH_TENANCIES),
    ("high memory", "x1.32xlarge", BOTH_TENANCIES),
    ("storage optimised", "i7ie.18xlarge", BOTH_TENANCIES),
    ("dense storage", "d3en.6xlarge", BOTH_TENANCIES),
    // Burstable: shared tenancy only, so its entry carries half as many combinations.
    ("burstable, smallest size", "t2.nano", SHARED_ONLY),
    ("burstable, largest size", "t3.2xlarge", SHARED_ONLY),
    ("burstable (AMD)", "t3a.medium", SHARED_ONLY),
];

/// The combinations an entry must carry, built from the model's enum value lists: every
/// billing mode, crossed with every platform image the model declares, crossed with the
/// tenancies this instance type runs under.
fn expected_combinations(tenancies: &[&str]) -> BTreeSet<Combination> {
    let mut expected = BTreeSet::new();
    for billing_mode in BILLING_MODE_VALUES {
        for platform_type in PLATFORM_TYPE_ENUM_VALUES {
            for tenancy in tenancies {
                expected.insert((
                    (*billing_mode).to_string(),
                    (*platform_type).to_string(),
                    (*tenancy).to_string(),
                ));
            }
        }
    }
    expected
}

/// The whole catalog, as a map from instance type name to the combinations its entry
/// reports, in the order reported. Walks `NextToken` so no case can read a prefix.
async fn reported_catalog(router: &MultiServiceRouter) -> Vec<(String, Vec<Combination>)> {
    let mut rows: Vec<(String, Vec<Combination>)> = Vec::new();
    let mut request = json!({});
    loop {
        let page = ok(router, "ListInstanceTypes", request.clone()).await;
        let entries = page["InstanceTypes"]
            .as_array()
            .unwrap_or_else(|| panic!("ListInstanceTypes reported no InstanceTypes: {page}"))
            .clone();
        for entry in entries {
            let name = entry["InstanceType"]
                .as_str()
                .unwrap_or_else(|| panic!("an InstanceTypeInfo carries no InstanceType: {entry}"))
                .to_string();
            let combinations = entry["SupportedInstanceConfigurations"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {name:?}: the entry carries no SupportedInstanceConfigurations: {entry}")
                })
                .iter()
                .map(|configuration| {
                    let member = |field: &str| {
                        configuration[field]
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!(
                                    "case {name:?}: a SupportedInstanceConfiguration omits \
                                     {field}: {configuration}"
                                )
                            })
                            .to_string()
                    };
                    (
                        member("BillingMode"),
                        member("PlatformType"),
                        member("Tenancy"),
                    )
                })
                .collect();
            rows.push((name, combinations));
        }
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => request["NextToken"] = json!(token),
            None => return rows,
        }
    }
}

/// The combinations one name's entry reports.
fn entry_of<'a>(
    catalog: &'a [(String, Vec<Combination>)],
    wanted: &str,
) -> &'a Vec<Combination> {
    catalog
        .iter()
        .find(|(name, _)| name == wanted)
        .map(|(_, combinations)| combinations)
        .unwrap_or_else(|| panic!("case {wanted:?}: the catalog must report this instance type"))
}

/// Every table row's entry carries the complete set of combinations its instance type
/// supports — no platform image the model declares is missing from any of them.
#[tokio::test]
async fn an_entry_carries_every_configuration_combination_its_instance_type_supports() {
    let router = router();
    let catalog = reported_catalog(&router).await;

    for (case, name, tenancies) in CONFIGURATION_ROWS {
        let reported = entry_of(&catalog, name);
        let distinct: BTreeSet<Combination> = reported.iter().cloned().collect();
        let expected = expected_combinations(tenancies);

        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {case} ({name:?}): the complete set is reported once each, got \
             {} combinations of which {} are distinct",
            reported.len(),
            distinct.len()
        );
        assert_eq!(
            distinct, expected,
            "case {case} ({name:?}): expected the {} combinations of {} billing modes × {} \
             platform images × {} tenancies, got {} ({distinct:?})",
            expected.len(),
            BILLING_MODE_VALUES.len(),
            PLATFORM_TYPE_ENUM_VALUES.len(),
            tenancies.len(),
            distinct.len()
        );
        // The platform dimension is complete for every row, whatever the family class:
        // a graphics type is not told it has no Red Hat or SUSE image.
        for platform_type in PLATFORM_TYPE_ENUM_VALUES {
            assert!(
                distinct
                    .iter()
                    .any(|(_, reported_platform, _)| reported_platform == platform_type),
                "case {case} ({name:?}): expected a combination on the platform image \
                 {platform_type:?}, got {distinct:?}"
            );
        }
    }

    // The rule holds for the WHOLE catalog, not only the rows above: every entry
    // carries every modelled platform image, and the only dimension that narrows an
    // entry is tenancy.
    for (name, combinations) in &catalog {
        let platforms: BTreeSet<&str> = combinations
            .iter()
            .map(|(_, platform_type, _)| platform_type.as_str())
            .collect();
        let modelled: BTreeSet<&str> = PLATFORM_TYPE_ENUM_VALUES.iter().copied().collect();
        assert_eq!(
            platforms, modelled,
            "case {name:?}: every entry carries every modelled platform image, got {platforms:?}"
        );
        let tenancies: BTreeSet<&str> = combinations
            .iter()
            .map(|(_, _, tenancy)| tenancy.as_str())
            .collect();
        assert_eq!(
            combinations.len(),
            BILLING_MODE_VALUES.len() * PLATFORM_TYPE_ENUM_VALUES.len() * tenancies.len(),
            "case {name:?}: expected the complete cross product of the dimensions the entry \
             reports, got {} combinations over {tenancies:?}",
            combinations.len()
        );
    }

    // Reading the catalog is stable: the same call reports the same entries, so a
    // client that caches or diffs the answer is not reading a per-call value.
    let again = reported_catalog(&router).await;
    assert_eq!(
        again, catalog,
        "case \"two reads\": the catalog is service data, so two reads report the same entries"
    );
}

/// The other direction of the same rule: a filter reports every instance type whose
/// entry lists the requested combination — over every combination the model's three
/// enums allow, so no platform image hides a type that supports it.
#[tokio::test]
async fn a_filter_reports_every_instance_type_whose_entry_lists_the_combination() {
    let router = router();
    let catalog = reported_catalog(&router).await;

    for billing_mode in BILLING_MODE_VALUES {
        for platform_type in PLATFORM_TYPE_ENUM_VALUES {
            for tenancy in INSTANCE_CONFIGURATION_TENANCY_ENUM_VALUES {
                let case = format!("{billing_mode}/{platform_type}/{tenancy}");
                let wanted: Combination = (
                    (*billing_mode).to_string(),
                    (*platform_type).to_string(),
                    (*tenancy).to_string(),
                );
                // What the catalog itself says supports the combination, read off the
                // unfiltered answer.
                let expected: Vec<&str> = catalog
                    .iter()
                    .filter(|(_, combinations)| combinations.contains(&wanted))
                    .map(|(name, _)| name.as_str())
                    .collect();
                assert!(
                    !expected.is_empty(),
                    "case {case}: every modelled combination is supported by at least one \
                     catalogued instance type"
                );

                let page = ok(
                    &router,
                    "ListInstanceTypes",
                    json!({
                        "InstanceConfigurationFilter": {
                            "BillingMode": billing_mode,
                            "PlatformType": platform_type,
                            "Tenancy": tenancy,
                        }
                    }),
                )
                .await;
                let reported: Vec<&str> = page["InstanceTypes"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: reported no InstanceTypes: {page}"))
                    .iter()
                    .map(|entry| {
                        entry["InstanceType"]
                            .as_str()
                            .unwrap_or_else(|| panic!("case {case}: an entry carries no name"))
                    })
                    .collect();
                assert_eq!(
                    reported, expected,
                    "case {case}: expected every instance type whose entry lists the \
                     combination, got {} of {} types",
                    reported.len(),
                    expected.len()
                );

                // Each table row is reported exactly when its own entry lists the
                // combination — the graphics rows included, on every platform image.
                for (row_case, name, tenancies) in CONFIGURATION_ROWS {
                    let supports = expected_combinations(tenancies).contains(&wanted);
                    assert_eq!(
                        reported.contains(name),
                        supports,
                        "case {case} / {row_case} ({name:?}): expected reported={supports}, \
                         got reported={}",
                        reported.contains(name)
                    );
                }
            }
        }
    }
}
