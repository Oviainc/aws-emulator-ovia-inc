// SPDX-License-Identifier: CC0-1.0

//! What a keyspace is replicated in under a strategy other than `MULTI_REGION`, driven end
//! to end over the service's own `awsJson1_0` wire protocol.
//!
//! The rule under test, exercised over a SET of specifications rather than one example:
//! "`regionList` - if the `replicationStrategy` is `MULTI_REGION`, the `regionList`
//! requires the current Region and at least one additional Amazon Web Services Region"
//! (`CreateKeyspaceRequest$replicationSpecification`), so a `regionList` names the Regions
//! a keyspace is replicated in only under that strategy; under any other strategy the
//! keyspace is replicated in exactly the Region of the call, whatever list the request
//! carried.
//!
//! The rows span the list the request supplies: absent, present without the current
//! Region, present with it, longer than two entries, repeating one Region, and the whole
//! Region catalog one entry at a time — plus a `MULTI_REGION` row, where the list IS what
//! the keyspace is replicated in.
//!
//! The replication a single-Region keyspace stores is not in any response of its own
//! (`replicationRegions` is reported for a `MULTI_REGION` keyspace only), so each row is
//! checked through the one operation that compares against it: `UpdateKeyspace`, which
//! refuses a widening that omits a Region the keyspace is already replicated in. A
//! widening that names only the current Region and one addition therefore succeeds exactly
//! when nothing but the current Region was stored.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{assert_failure, assert_ok, settle_wait, Emulator, REGION};

/// Every Amazon Web Services Region this emulator knows: the shared catalog
/// `emulator_telemetry::policy::KNOWN_REGIONS` that a `regionList` entry is held to.
const CATALOG: &[&str] = ::emulator_telemetry::policy::KNOWN_REGIONS;

/// The Region a widening adds. Never the request's own Region, and never an entry of any
/// row's supplied list, so a row passes only if its list was not stored.
const ADDED_REGION: &str = "eu-central-1";

/// Regions the rows below build their lists from: catalog entries, none of them the
/// request's Region or [`ADDED_REGION`].
const OTHER_REGIONS: [&str; 3] = ["eu-west-1", "ap-south-1", "sa-east-1"];

/// One row: the `replicationSpecification` a `CreateKeyspace` carries, and the keyspace
/// name it is created under.
struct Row {
    case: &'static str,
    keyspace: String,
    specification: Value,
}

/// `GetKeyspace` for one keyspace, as a 200 body.
async fn get_keyspace(emulator: &Emulator, case: &str, keyspace: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call("GetKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    )
}

/// The rows: a `SINGLE_REGION` specification carrying every shape of `regionList` the model
/// allows (`length min 2`), and one carrying none at all.
fn single_region_rows() -> Vec<Row> {
    let [first, second, third] = OTHER_REGIONS;
    let mut rows = vec![
        Row {
            case: "no_region_list",
            keyspace: "single_no_list".to_string(),
            specification: json!({"replicationStrategy": keyspaces::SINGLE_REGION_STRATEGY}),
        },
        Row {
            case: "list_without_the_current_region",
            keyspace: "single_list_elsewhere".to_string(),
            specification: json!({
                "replicationStrategy": keyspaces::SINGLE_REGION_STRATEGY,
                "regionList": [first, second],
            }),
        },
        Row {
            case: "list_with_the_current_region",
            keyspace: "single_list_with_current".to_string(),
            specification: json!({
                "replicationStrategy": keyspaces::SINGLE_REGION_STRATEGY,
                "regionList": [REGION, first],
            }),
        },
        Row {
            case: "list_of_three_regions",
            keyspace: "single_list_of_three".to_string(),
            specification: json!({
                "replicationStrategy": keyspaces::SINGLE_REGION_STRATEGY,
                "regionList": [first, second, third],
            }),
        },
        Row {
            case: "list_repeating_one_region",
            keyspace: "single_list_repeated".to_string(),
            specification: json!({
                "replicationStrategy": keyspaces::SINGLE_REGION_STRATEGY,
                "regionList": [second, second],
            }),
        },
        Row {
            case: "list_of_the_added_region",
            keyspace: "single_list_added_region".to_string(),
            specification: json!({
                "replicationStrategy": keyspaces::SINGLE_REGION_STRATEGY,
                "regionList": [ADDED_REGION, third],
            }),
        },
    ];
    // One row per catalog entry as well, so the rule is not pinned only on the Regions
    // named above: a two-entry list of that Region and the request's own.
    for (index, region) in CATALOG.iter().enumerate() {
        if *region == REGION {
            continue;
        }
        rows.push(Row {
            case: "catalog_entry",
            keyspace: format!("single_catalog_{index}"),
            specification: json!({
                "replicationStrategy": keyspaces::SINGLE_REGION_STRATEGY,
                "regionList": [REGION, region],
            }),
        });
    }
    rows
}

/// RULE: a keyspace created under a strategy other than `MULTI_REGION` is replicated in the
/// Region of the call alone, whatever `regionList` the request supplied — so it reports no
/// `replicationRegions`, and a widening to the current Region plus one addition is accepted.
#[tokio::test]
async fn a_single_region_keyspace_is_replicated_in_the_region_it_was_created_in() {
    let emulator = Emulator::start();
    let rows = single_region_rows();
    assert!(
        CATALOG.contains(&REGION) && CATALOG.contains(&ADDED_REGION) && rows.len() > 6,
        "case setup: the catalog must hold {REGION:?} and {ADDED_REGION:?} and give more \
         than the hand-written rows, got {} rows over {} Regions",
        rows.len(),
        CATALOG.len()
    );

    // Every row is created first, so the one wait below settles them all.
    for row in &rows {
        let created = assert_ok(
            row.case,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": row.keyspace,
                        "replicationSpecification": row.specification,
                    }),
                )
                .await,
        );
        assert!(
            created["resourceArn"]
                .as_str()
                .is_some_and(|arn| arn.contains(&format!("/keyspace/{}/", row.keyspace))),
            "case {:?}: expected the keyspace's own ARN, got {created}",
            row.case
        );

        let got = get_keyspace(&emulator, row.case, &row.keyspace).await;
        assert_eq!(
            (
                got["replicationStrategy"].clone(),
                got["replicationRegions"].clone(),
                got["replicationGroupStatuses"].clone()
            ),
            (
                json!(keyspaces::SINGLE_REGION_STRATEGY),
                Value::Null,
                Value::Null
            ),
            "case {:?}: expected a {} keyspace reporting no replication Regions, got {got}",
            row.case,
            keyspaces::SINGLE_REGION_STRATEGY
        );
    }

    ::tokio::time::sleep(settle_wait()).await;

    for row in &rows {
        // The widening names the current Region and ONE addition. It omits every Region the
        // row's own list named, so it is accepted only because none of them is stored.
        let widened = assert_ok(
            row.case,
            emulator
                .call(
                    "UpdateKeyspace",
                    json!({
                        "keyspaceName": row.keyspace,
                        "replicationSpecification": {
                            "replicationStrategy": keyspaces::MULTI_REGION_STRATEGY,
                            "regionList": [REGION, ADDED_REGION],
                        },
                    }),
                )
                .await,
        );
        assert!(
            widened["resourceArn"]
                .as_str()
                .is_some_and(|arn| arn.contains(&format!("/keyspace/{}/", row.keyspace))),
            "case {:?}: expected the widened keyspace's ARN, got {widened}",
            row.case
        );

        let got = get_keyspace(&emulator, row.case, &row.keyspace).await;
        assert_eq!(
            (
                got["replicationStrategy"].clone(),
                got["replicationRegions"].clone()
            ),
            (
                json!(keyspaces::MULTI_REGION_STRATEGY),
                json!([REGION, ADDED_REGION])
            ),
            "case {:?}: expected the keyspace replicated in exactly [{REGION}, \
             {ADDED_REGION}] after the widening, got {got}",
            row.case
        );
    }
}

/// The other side of the same rule: under `MULTI_REGION` the supplied `regionList` IS what
/// the keyspace is replicated in, and a widening that omits one of those Regions is refused.
///
/// The rows differ in the list's length (the modelled minimum of 2, and 3) and in where the
/// current Region sits in it.
#[tokio::test]
async fn a_multi_region_keyspace_stores_the_supplied_region_list() {
    let emulator = Emulator::start();
    let [first, second, third] = OTHER_REGIONS;
    let rows: [(&str, Vec<&str>); 4] = [
        ("current_region_first", vec![REGION, first]),
        ("current_region_last", vec![second, REGION]),
        ("three_regions", vec![REGION, first, third]),
        ("current_region_in_the_middle", vec![first, REGION, second]),
    ];

    for (case, list) in &rows {
        let keyspace = format!("multi_{case}");
        assert_ok(
            case,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": keyspace,
                        "replicationSpecification": {
                            "replicationStrategy": keyspaces::MULTI_REGION_STRATEGY,
                            "regionList": list,
                        },
                    }),
                )
                .await,
        );

        let got = get_keyspace(&emulator, case, &keyspace).await;
        assert_eq!(
            (
                got["replicationStrategy"].clone(),
                got["replicationRegions"].clone()
            ),
            (json!(keyspaces::MULTI_REGION_STRATEGY), json!(list)),
            "case {case:?}: expected the keyspace replicated in exactly the supplied list, \
             got {got}"
        );
    }

    ::tokio::time::sleep(settle_wait()).await;

    for (case, list) in &rows {
        // A widening that drops one of the stored Regions is the refusal the stored list
        // earns: this is what a single-Region keyspace must NOT suffer for a list it was
        // never replicated in.
        let dropped = list
            .iter()
            .find(|region| **region != REGION)
            .expect("every row names a Region besides the current one");
        let kept: Vec<&str> = list
            .iter()
            .copied()
            .filter(|region| region != dropped)
            .chain([ADDED_REGION])
            .collect();
        assert_failure(
            case,
            emulator
                .call(
                    "UpdateKeyspace",
                    json!({
                        "keyspaceName": format!("multi_{case}"),
                        "replicationSpecification": {
                            "replicationStrategy": keyspaces::MULTI_REGION_STRATEGY,
                            "regionList": kept,
                        },
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
    }
}
