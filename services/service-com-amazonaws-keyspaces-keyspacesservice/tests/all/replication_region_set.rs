// SPDX-License-Identifier: CC0-1.0

//! A `regionList` denotes a SET of Amazon Web Services Regions, driven end to end over the
//! service's own `awsJson1_0` wire protocol.
//!
//! The rule under test, exercised over a SET of lists rather than one example: a
//! `regionList` holds "the Amazon Web Services Regions where the keyspace is replicated
//! in" (`ReplicationSpecification$regionList`), and `RegionList` carries no `uniqueItems`
//! (smithy_query shape="RegionList"), so a Region a request names twice is still ONE Region
//! the keyspace is replicated in. Therefore:
//!
//! * the documented "`regionList` requires the current Region and at least one additional
//!   Amazon Web Services Region" (`CreateKeyspaceRequest$replicationSpecification`) holds of
//!   the Regions the list DENOTES, not of its entry count — the modelled `length min 2` is
//!   on the list — so a list that only repeats one Region is refused 400
//!   `ValidationException`, whichever operation carries it;
//! * a list that is accepted is stored as the Regions it denotes, each once, in the order
//!   the request first named them, so every reader reports one entry per Region and a later
//!   widening that names a Region the keyspace does not hold yet is accepted.
//!
//! The rows differ in the list's length, in how many times an entry repeats, in which
//! Region repeats (the request's own and another), and in where the repeats sit.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, Emulator, REGION,
};

/// Regions the lists below are built from, none of them the request's own Region and none
/// of them [`ADDED_REGION`]: entries of the shared catalog
/// `emulator_telemetry::policy::KNOWN_REGIONS` a `regionList` entry is held to.
const OTHER_REGIONS: [&str; 3] = ["eu-west-1", "ap-south-1", "sa-east-1"];

/// The Region a widening adds, named by no row's own list.
const ADDED_REGION: &str = "eu-central-1";

/// `GetKeyspace` for one keyspace, as a 200 body.
async fn get_keyspace(emulator: &Emulator, case: &str, keyspace: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call("GetKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    )
}

/// A `MULTI_REGION` replication specification over `regions`.
fn multi_region(regions: &[&str]) -> Value {
    json!({
        "replicationStrategy": keyspaces::MULTI_REGION_STRATEGY,
        "regionList": regions,
    })
}

/// The `region` member of each entry of a response array, in order.
fn regions_of(entries: &Value) -> Vec<String> {
    entries
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["region"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the refusing side: a list whose entries denote a single Region.
struct SingleRegionList {
    case: &'static str,
    list: Vec<&'static str>,
}

/// Lists that satisfy the modelled `length min 2` and still denote ONE Region: the
/// request's own Region repeated twice, three and five times, and repeated up to the
/// emulator's own cap on how many Regions a keyspace is replicated in.
fn single_region_lists() -> Vec<SingleRegionList> {
    vec![
        SingleRegionList {
            case: "the current Region twice",
            list: vec![REGION, REGION],
        },
        SingleRegionList {
            case: "the current Region three times",
            list: vec![REGION; 3],
        },
        SingleRegionList {
            case: "the current Region five times",
            list: vec![REGION; 5],
        },
        SingleRegionList {
            case: "the current Region up to the replication-Region cap",
            list: vec![REGION; keyspaces::MAX_REPLICATION_REGIONS],
        },
    ]
}

/// RULE, refusing side: a `MULTI_REGION` `regionList` that denotes a single Region names no
/// additional Amazon Web Services Region, so `CreateKeyspace` refuses it with the modelled
/// `ValidationException` and stores nothing — exactly as `UpdateKeyspace` refuses the same
/// list.
#[tokio::test]
async fn a_region_list_denoting_one_region_is_refused_by_both_operations() {
    let emulator = Emulator::start();
    // One keyspace the update rows are aimed at, so each refusal below is about the list
    // rather than about a keyspace that does not exist.
    const EXISTING: &str = "set_rule_existing";
    assert_ok(
        EXISTING,
        emulator
            .call(
                "CreateKeyspace",
                json!({"keyspaceName": EXISTING}),
            )
            .await,
    );
    ::tokio::time::sleep(settle_wait()).await;

    for (index, row) in single_region_lists().iter().enumerate() {
        assert!(
            row.list.len() >= 2 && row.list.iter().all(|region| *region == row.list[0]),
            "case {:?}: this row is meant to satisfy the modelled minimum length while \
             naming one Region, got {:?}",
            row.case,
            row.list
        );
        let keyspace = format!("set_rule_refused_{index}");
        assert_failure(
            row.case,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": keyspace,
                        "replicationSpecification": multi_region(&row.list),
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
        // The refusal stored no keyspace at all.
        assert_failure(
            row.case,
            emulator
                .call("GetKeyspace", json!({"keyspaceName": keyspace}))
                .await,
            404,
            "ResourceNotFoundException",
        );
        // The same list through the sibling operation draws the same refusal, and the
        // keyspace it named keeps the replication it had.
        assert_failure(
            row.case,
            emulator
                .call(
                    "UpdateKeyspace",
                    json!({
                        "keyspaceName": EXISTING,
                        "replicationSpecification": multi_region(&row.list),
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
        let got = get_keyspace(&emulator, row.case, EXISTING).await;
        assert_eq!(
            (
                got["replicationStrategy"].clone(),
                got["replicationRegions"].clone()
            ),
            (json!(keyspaces::SINGLE_REGION_STRATEGY), Value::Null),
            "case {:?}: a refused update leaves the keyspace's replication as it was, got \
             {got}",
            row.case
        );
    }
}

/// One row of the accepting side: a list carrying repeats that still denotes two or more
/// Regions, and the Regions it denotes.
struct DenotedSet {
    case: &'static str,
    list: Vec<&'static str>,
    denoted: Vec<&'static str>,
}

/// The rows: repeats of the current Region and of another, leading, trailing and
/// interleaved, over lists of three to five entries — plus a list with no repeat at all,
/// which the same rule leaves untouched.
fn denoted_sets() -> Vec<DenotedSet> {
    let [first, second, third] = OTHER_REGIONS;
    vec![
        DenotedSet {
            case: "no repeat at all",
            list: vec![REGION, first],
            denoted: vec![REGION, first],
        },
        DenotedSet {
            case: "the current Region repeated before another",
            list: vec![REGION, REGION, first],
            denoted: vec![REGION, first],
        },
        DenotedSet {
            case: "another Region repeated around the current one",
            list: vec![first, REGION, first],
            denoted: vec![first, REGION],
        },
        DenotedSet {
            case: "both Regions repeated, three Regions denoted",
            list: vec![REGION, first, second, first, REGION],
            denoted: vec![REGION, first, second],
        },
        DenotedSet {
            case: "a repeat trailing a three-Region list",
            list: vec![second, REGION, third, third],
            denoted: vec![second, REGION, third],
        },
    ]
}

/// RULE, accepting side: a list that denotes two or more Regions is stored as that set,
/// each Region once and in the order the request first named it, so `GetKeyspace`,
/// `ListKeyspaces` and a table's per-Region settings each report one entry per Region — and
/// the keyspace can still be widened afterwards, because what a widening adds is a Region
/// the keyspace does not hold yet.
#[tokio::test]
async fn a_region_list_is_stored_as_the_set_of_regions_it_denotes() {
    let emulator = Emulator::start();
    let rows = denoted_sets();
    const TABLE: &str = "t";

    for (index, row) in rows.iter().enumerate() {
        let keyspace = format!("set_rule_stored_{index}");
        assert_ok(
            row.case,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": keyspace,
                        "replicationSpecification": multi_region(&row.list),
                    }),
                )
                .await,
        );

        let got = get_keyspace(&emulator, row.case, &keyspace).await;
        assert_eq!(
            (
                got["replicationStrategy"].clone(),
                got["replicationRegions"].clone()
            ),
            (
                json!(keyspaces::MULTI_REGION_STRATEGY),
                json!(row.denoted)
            ),
            "case {:?}: expected the keyspace replicated in exactly {:?}, got {got}",
            row.case,
            row.denoted
        );
        assert_eq!(
            regions_of(&got["replicationGroupStatuses"]),
            row.denoted
                .iter()
                .map(|region| (*region).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected one replication status per replication Region, got {got}",
            row.case
        );
    }

    // `ListKeyspaces` reports the same Regions it stored, for every row at once.
    let listed = assert_ok(
        "ListKeyspaces",
        emulator.call("ListKeyspaces", json!({})).await,
    );
    for (index, row) in rows.iter().enumerate() {
        let keyspace = format!("set_rule_stored_{index}");
        let summary = listed["keyspaces"]
            .as_array()
            .and_then(|summaries| {
                summaries
                    .iter()
                    .find(|summary| summary["keyspaceName"] == json!(keyspace))
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: ListKeyspaces reports the keyspace it stored, got {listed}",
                    row.case
                )
            });
        assert_eq!(
            summary["replicationRegions"],
            json!(row.denoted),
            "case {:?}: expected ListKeyspaces to report {:?}, got {summary}",
            row.case,
            row.denoted
        );
    }

    ::tokio::time::sleep(settle_wait()).await;

    // One table per keyspace, using client-side timestamps so the widening below is not
    // refused for want of them.
    for (index, row) in rows.iter().enumerate() {
        assert_ok(
            row.case,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": format!("set_rule_stored_{index}"),
                        "tableName": TABLE,
                        "schemaDefinition": minimal_schema("pk"),
                        "clientSideTimestamps": {"status": "ENABLED"},
                    }),
                )
                .await,
        );
    }
    ::tokio::time::sleep(settle_wait()).await;

    for (index, row) in rows.iter().enumerate() {
        let keyspace = format!("set_rule_stored_{index}");
        let expected: Vec<String> = row
            .denoted
            .iter()
            .map(|region| (*region).to_string())
            .collect();

        // A table of the keyspace carries one set of per-Region settings per Region the
        // keyspace is replicated in — the same shape `replicaSpecifications` refuses on
        // input when it names a Region twice.
        let table = assert_ok(
            row.case,
            emulator
                .call(
                    "GetTable",
                    json!({"keyspaceName": keyspace, "tableName": TABLE}),
                )
                .await,
        );
        assert_eq!(
            regions_of(&table["replicaSpecifications"]),
            expected,
            "case {:?}: expected one replica specification per replication Region, got {table}",
            row.case
        );
        let scaling = assert_ok(
            row.case,
            emulator
                .call(
                    "GetTableAutoScalingSettings",
                    json!({"keyspaceName": keyspace, "tableName": TABLE}),
                )
                .await,
        );
        assert_eq!(
            regions_of(&scaling["replicaSpecifications"]),
            expected,
            "case {:?}: expected one auto scaling replica entry per replication Region, got \
             {scaling}",
            row.case
        );

        // The keyspace can still be widened: the widening keeps every Region it holds and
        // names one more.
        let widened: Vec<&str> = row
            .denoted
            .iter()
            .copied()
            .chain([ADDED_REGION])
            .collect();
        assert_ok(
            row.case,
            emulator
                .call(
                    "UpdateKeyspace",
                    json!({
                        "keyspaceName": keyspace,
                        "replicationSpecification": multi_region(&widened),
                    }),
                )
                .await,
        );
        let got = get_keyspace(&emulator, row.case, &keyspace).await;
        assert_eq!(
            got["replicationRegions"],
            json!(widened),
            "case {:?}: expected the widened keyspace replicated in {widened:?}, got {got}",
            row.case
        );
    }
}
