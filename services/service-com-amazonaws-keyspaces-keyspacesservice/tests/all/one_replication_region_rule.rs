// SPDX-License-Identifier: CC0-1.0

//! Every report that names the Regions a keyspace is replicated in names the SAME
//! Regions, driven end to end over the service's own `awsJson1_0` wire protocol through
//! the harness in [`super::keyspace_table_lifecycle`].
//!
//! The rule: the Regions a report carries for a keyspace, or for any table of it, are the
//! stored `replicationSpecification.regionList` of that keyspace exactly when its
//! `replicationStrategy` is `MULTI_REGION`, and are NONE under any other strategy
//! ("if the `replicationStrategy` of the keyspace is `MULTI_REGION`, a list of replication
//! Regions is returned" —
//! smithy_query(query="doc",shape="KeyspaceSummary",member="replicationRegions")). It is
//! ONE rule over the stored keyspace, so the five reports below — `GetKeyspace`'s
//! `replicationRegions` and its `replicationGroupStatuses`, the `ListKeyspaces` summary,
//! `GetTable$replicaSpecifications` and
//! `GetTableAutoScalingSettings$replicaSpecifications` — cannot answer it differently,
//! whether the keyspace was created `MULTI_REGION` or widened into one by
//! `UpdateKeyspace`.
//!
//! How a report SERIALIZES "no Region" is the member's own convention and is asserted here
//! too: the two `replicaSpecifications` members are served for every table (`len 0..`), so
//! a single-Region table's list is empty, while the keyspace's own `replicationRegions`
//! (modelled `len 2..`) is absent under any strategy but `MULTI_REGION`.

use serde_json::{json, Value};

use super::keyspace_table_lifecycle::{
    assert_ok, minimal_schema, settle_wait, Emulator, REGION,
};

/// Regions other than the request's own ([`REGION`]) a keyspace can be replicated in.
/// Each is within `RegionList`'s modelled member length (2..25).
const OTHER_REGIONS: [&str; 4] = ["eu-west-1", "ap-south-1", "us-east-2", "sa-east-1"];

/// A `MULTI_REGION` replication specification over `regions`.
fn multi_region(regions: &[&str]) -> Value {
    json!({"replicationStrategy": "MULTI_REGION", "regionList": regions})
}

/// The `region` member of each entry of a report's list member, or `None` when the report
/// carries no such member at all.
fn regions_of(case: &str, report: &Value, member: &str) -> Option<Vec<String>> {
    match &report[member] {
        Value::Null => None,
        entries => Some(
            entries
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {member} must be a list, got {entries:?}")
                })
                .iter()
                .map(|entry| entry["region"].as_str().unwrap_or_default().to_string())
                .collect(),
        ),
    }
}

/// A report's list-of-strings member, or `None` when the report carries none.
fn strings_of(case: &str, report: &Value, member: &str) -> Option<Vec<String>> {
    match &report[member] {
        Value::Null => None,
        entries => Some(
            entries
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {member} must be a list, got {entries:?}")
                })
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect(),
        ),
    }
}

/// Every report that names the keyspace's replication Regions, each labelled by the
/// response member it was read from and by whether that member is served even when it
/// names no Region at all.
///
/// The five span both former statements of the rule: `GetKeyspace$replicationRegions` and
/// the `ListKeyspaces` summary report the keyspace itself, while
/// `GetKeyspace$replicationGroupStatuses`, `GetTable$replicaSpecifications` and
/// `GetTableAutoScalingSettings$replicaSpecifications` report one entry per Region a table
/// of the keyspace is replicated in.
async fn every_report(
    emulator: &Emulator,
    case: &str,
    keyspace: &str,
    table: &str,
) -> Vec<(&'static str, bool, Option<Vec<String>>)> {
    let identity = json!({"keyspaceName": keyspace, "tableName": table});
    let got_keyspace = assert_ok(
        case,
        emulator
            .call("GetKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    let listing = assert_ok(case, emulator.call("ListKeyspaces", json!({})).await);
    let summary = listing["keyspaces"]
        .as_array()
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| summary["keyspaceName"] == json!(keyspace))
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing holds keyspace {keyspace:?}, got {listing}")
        });
    let got_table = assert_ok(case, emulator.call("GetTable", identity.clone()).await);
    let got_scaling = assert_ok(
        case,
        emulator
            .call("GetTableAutoScalingSettings", identity)
            .await,
    );

    vec![
        (
            "GetKeyspace$replicationRegions",
            false,
            strings_of(case, &got_keyspace, "replicationRegions"),
        ),
        (
            "GetKeyspace$replicationGroupStatuses",
            false,
            regions_of(case, &got_keyspace, "replicationGroupStatuses"),
        ),
        (
            "ListKeyspaces$keyspaces$replicationRegions",
            false,
            strings_of(case, &summary, "replicationRegions"),
        ),
        (
            "GetTable$replicaSpecifications",
            true,
            regions_of(case, &got_table, "replicaSpecifications"),
        ),
        (
            "GetTableAutoScalingSettings$replicaSpecifications",
            true,
            regions_of(case, &got_scaling, "replicaSpecifications"),
        ),
    ]
}

/// Assert every report names exactly `expected`, so the rule is answered once — and that a
/// member served for every table carries the empty list where it names no Region, while one
/// the model only returns under `MULTI_REGION` is absent there.
fn assert_every_report(
    case: &str,
    reports: &[(&'static str, bool, Option<Vec<String>>)],
    expected: &[&str],
) {
    let expected: Vec<String> = expected.iter().map(|region| (*region).to_string()).collect();
    for (member, always_served, actual) in reports {
        assert_eq!(
            actual.clone().unwrap_or_default(),
            expected,
            "case {case:?}: {member} answers the one replication-Region rule, so it names \
             {expected:?}, got {actual:?}"
        );
        if expected.is_empty() {
            assert_eq!(
                actual.is_some(),
                *always_served,
                "case {case:?}: {member} naming no Region is served as an empty list only \
                 where the model always serves it, got {actual:?}"
            );
        }
    }
}

/// The one rule over a SET of keyspaces, read through every report that states it.
///
/// The rows differ along what the rule generalises over: the replication specification
/// (omitted, `SINGLE_REGION`, `SINGLE_REGION` carrying a region list anyway, and
/// `MULTI_REGION` over two, three and five Regions), and the position of the request's own
/// Region inside the list (first, middle, last, absent).
#[tokio::test]
async fn every_report_of_a_keyspace_names_the_same_replication_regions() {
    struct Case {
        /// Also the keyspace name and the table name.
        name: &'static str,
        replication: Option<Value>,
        /// The Regions every report must name — empty when none must name any.
        expected: Vec<&'static str>,
    }

    let cases: Vec<Case> = vec![
        Case {
            // No replicationSpecification at all: a single-Region keyspace.
            name: "strategy_omitted",
            replication: None,
            expected: Vec::new(),
        },
        Case {
            name: "explicit_single_region",
            replication: Some(json!({"replicationStrategy": "SINGLE_REGION"})),
            expected: Vec::new(),
        },
        Case {
            // A region list under a strategy that is not MULTI_REGION names no Region the
            // keyspace is replicated in, so no report names one.
            name: "single_region_with_a_region_list",
            replication: Some(json!({
                "replicationStrategy": "SINGLE_REGION",
                "regionList": [REGION, OTHER_REGIONS[0]],
            })),
            expected: Vec::new(),
        },
        Case {
            // The modelled minimum length of a regionList (2).
            name: "multi_region_pair",
            replication: Some(multi_region(&[REGION, OTHER_REGIONS[0]])),
            expected: vec![REGION, OTHER_REGIONS[0]],
        },
        Case {
            // The request's own Region in the MIDDLE of the list.
            name: "multi_region_triple",
            replication: Some(multi_region(&[OTHER_REGIONS[1], REGION, OTHER_REGIONS[2]])),
            expected: vec![OTHER_REGIONS[1], REGION, OTHER_REGIONS[2]],
        },
        Case {
            // A wider group, with the request's own Region last.
            name: "multi_region_five",
            replication: Some(multi_region(&[
                OTHER_REGIONS[0],
                OTHER_REGIONS[1],
                OTHER_REGIONS[2],
                OTHER_REGIONS[3],
                REGION,
            ])),
            expected: vec![
                OTHER_REGIONS[0],
                OTHER_REGIONS[1],
                OTHER_REGIONS[2],
                OTHER_REGIONS[3],
                REGION,
            ],
        },
    ];

    let emulator = Emulator::start();

    for case in &cases {
        let mut keyspace_request = json!({"keyspaceName": case.name});
        if let Some(replication) = &case.replication {
            keyspace_request["replicationSpecification"] = replication.clone();
        }
        assert_ok(
            case.name,
            emulator.call("CreateKeyspace", keyspace_request).await,
        );
        assert_ok(
            case.name,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": case.name,
                        "tableName": case.name,
                        "schemaDefinition": minimal_schema("pk"),
                        // Every table of a keyspace being replicated "must use client-side
                        // timestamps for conflict resolution", so the widening below is
                        // open to each of these keyspaces.
                        "clientSideTimestamps": {"status": "ENABLED"},
                    }),
                )
                .await,
        );
    }

    // Read once while the creations the clock owes are still pending, so the reports are
    // compared in a transient status too, and once after they have settled.
    for case in &cases {
        let reports = every_report(&emulator, case.name, case.name, case.name).await;
        assert_every_report(case.name, &reports, &case.expected);
    }
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        let reports = every_report(&emulator, case.name, case.name, case.name).await;
        assert_every_report(case.name, &reports, &case.expected);
    }

    // The rule is about the STORED keyspace, so when `UpdateKeyspace` widens a
    // single-Region keyspace every report moves to the new list together — and when it
    // widens one keyspace, no report of another keyspace moves at all.
    let widened = "explicit_single_region";
    let widened_regions = [REGION, OTHER_REGIONS[3], OTHER_REGIONS[1]];
    assert_ok(
        "widened",
        emulator
            .call(
                "UpdateKeyspace",
                json!({
                    "keyspaceName": widened,
                    "replicationSpecification": multi_region(&widened_regions),
                }),
            )
            .await,
    );
    let reports = every_report(&emulator, "widened", widened, widened).await;
    assert_every_report("widened", &reports, &widened_regions);

    for case in &cases {
        if case.name == widened {
            continue;
        }
        let reports = every_report(&emulator, case.name, case.name, case.name).await;
        assert_every_report(case.name, &reports, &case.expected);
    }
}
