// SPDX-License-Identifier: CC0-1.0

//! `GetKeyspace`, driven end to end over the service's own `awsJson1_0` wire protocol
//! through the harness in [`crate::keyspace_table_lifecycle`].
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. `GetKeyspace` reports the keyspace STATE holds under the requested name: the name
//!    itself, the ARN `CreateKeyspace` minted, and the replication strategy it was created
//!    with — whatever that strategy is.
//! 2. `replicationRegions` and `replicationGroupStatuses` accompany a `MULTI_REGION`
//!    keyspace and only one ("If the `replicationStrategy` of the keyspace is
//!    `MULTI_REGION`, a list of replication Regions is returned"; both lists are modelled
//!    `length min 2`, which one Region cannot satisfy), with one status entry per
//!    replication Region.
//! 3. The keyspace status a status entry carries is a modelled `KeyspaceStatus`:
//!    `CREATING` while the creation the clock owes is still pending ("CreateKeyspace is an
//!    asynchronous operation. You can monitor the creation status of the new keyspace by
//!    using the GetKeyspace operation"), `ACTIVE` once the span has elapsed.
//! 4. `tablesReplicationProgress` is the proportion of the keyspace's tables that are
//!    `ACTIVE`, "expressed as a percentage ... compared to the total number of tables in
//!    the keyspace".
//! 5. A name no keyspace is stored under is the modelled `ResourceNotFoundException`,
//!    naming the ARN that was looked for; a name the model does not accept is the modelled
//!    `ValidationException`. Neither is a 501, and a read leaves the store as it was.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    KEYSPACE_STATUS_VALUES, RS_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, minimal_schema, settle_wait, Emulator, REGION,
};

/// A `keyspaceName` at the top of its modelled `length 1..48` range.
fn longest_name(prefix: &str) -> String {
    let mut name = prefix.to_string();
    name.truncate(48);
    while name.len() < 48 {
        name.push('x');
    }
    name
}

/// The replication Region of a `MULTI_REGION` keyspace that is not the request's own.
const OTHER_REGION: &str = "eu-west-1";

/// A third Region, so a `MULTI_REGION` case is not only ever a pair.
const THIRD_REGION: &str = "ap-south-1";

/// Call `GetKeyspace` for `name` and hand back its body, requiring 200.
async fn get_keyspace(emulator: &Emulator, case: &str, name: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call("GetKeyspace", json!({"keyspaceName": name}))
            .await,
    )
}

/// Create one table in `keyspace`, requiring 200.
async fn create_table(emulator: &Emulator, case: &str, keyspace: &str, table: &str) {
    assert_ok(
        case,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
}

/// RULES 1, 2 and 3: every keyspace reads back as it was created, with the replication
/// members that belong to its strategy and nothing else.
///
/// The rows differ along what the rule generalises over: the replication specification
/// (omitted, `SINGLE_REGION`, `MULTI_REGION` over two Regions, `MULTI_REGION` over three)
/// and the shape of the name (one character — the bottom of the modelled `length 1..48` —
/// a mixed-case name with digits and underscores, and a name at the top of that range).
#[tokio::test]
async fn get_keyspace_reports_the_keyspace_that_was_created() {
    struct Case {
        name: String,
        replication: Option<Value>,
        strategy: &'static str,
        /// The Regions `GetKeyspace` must report, or `None` when it must report none.
        regions: Option<Vec<String>>,
    }

    let cases: Vec<Case> = vec![
        Case {
            // No replicationSpecification at all: the keyspace is single-Region.
            name: "k".to_string(),
            replication: None,
            strategy: keyspaces::SINGLE_REGION_STRATEGY,
            regions: None,
        },
        Case {
            name: "Mixed_Case_9".to_string(),
            replication: Some(json!({"replicationStrategy": "SINGLE_REGION"})),
            strategy: keyspaces::SINGLE_REGION_STRATEGY,
            regions: None,
        },
        Case {
            name: "multi_two".to_string(),
            replication: Some(json!({
                "replicationStrategy": "MULTI_REGION",
                "regionList": [REGION, OTHER_REGION],
            })),
            strategy: keyspaces::MULTI_REGION_STRATEGY,
            regions: Some(vec![REGION.to_string(), OTHER_REGION.to_string()]),
        },
        Case {
            name: longest_name("multi_three_"),
            replication: Some(json!({
                "replicationStrategy": "MULTI_REGION",
                "regionList": [REGION, OTHER_REGION, THIRD_REGION],
            })),
            strategy: keyspaces::MULTI_REGION_STRATEGY,
            regions: Some(vec![
                REGION.to_string(),
                OTHER_REGION.to_string(),
                THIRD_REGION.to_string(),
            ]),
        },
    ];

    let emulator = Emulator::start();

    for case in &cases {
        let mut request = json!({"keyspaceName": case.name});
        if let Some(replication) = &case.replication {
            request["replicationSpecification"] = replication.clone();
        }
        let created = assert_ok(&case.name, emulator.call("CreateKeyspace", request).await);

        let got = get_keyspace(&emulator, &case.name, &case.name).await;
        assert_eq!(
            got["keyspaceName"],
            json!(case.name),
            "case {:?}: GetKeyspace reports the name it was asked for, got {got}",
            case.name
        );
        assert_eq!(
            got["resourceArn"], created["resourceArn"],
            "case {:?}: GetKeyspace reports the ARN CreateKeyspace minted, got {got}",
            case.name
        );
        assert_eq!(
            got["resourceArn"],
            json!(keyspace_arn(&case.name)),
            "case {:?}: the ARN is the published keyspace form, got {got}",
            case.name
        );

        let strategy = got["replicationStrategy"].as_str().unwrap_or_default();
        assert!(
            RS_VALUES.contains(&strategy),
            "case {:?}: replicationStrategy {strategy:?} is not a modelled value, got {got}",
            case.name
        );
        assert_eq!(
            strategy, case.strategy,
            "case {:?}: the keyspace reads back with the strategy it was created with, got {got}",
            case.name
        );

        match &case.regions {
            None => {
                assert!(
                    got["replicationRegions"].is_null(),
                    "case {:?}: a single-Region keyspace reports no replicationRegions, got {got}",
                    case.name
                );
                assert!(
                    got["replicationGroupStatuses"].is_null(),
                    "case {:?}: a single-Region keyspace reports no replicationGroupStatuses, \
                     got {got}",
                    case.name
                );
            }
            Some(regions) => {
                assert_eq!(
                    got["replicationRegions"],
                    json!(regions),
                    "case {:?}: a MULTI_REGION keyspace reports its Region list, got {got}",
                    case.name
                );
                let statuses = got["replicationGroupStatuses"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {:?}: a MULTI_REGION keyspace reports a status per Region, \
                             got {got}",
                            case.name
                        )
                    });
                assert_eq!(
                    statuses.len(),
                    regions.len(),
                    "case {:?}: one status entry per replication Region, got {got}",
                    case.name
                );
                for (status, region) in statuses.iter().zip(regions) {
                    assert_eq!(
                        status["region"],
                        json!(region),
                        "case {:?}: the status entries follow the Region list, got {got}",
                        case.name
                    );
                    let keyspace_status = status["keyspaceStatus"].as_str().unwrap_or_default();
                    assert!(
                        KEYSPACE_STATUS_VALUES.contains(&keyspace_status),
                        "case {:?}: keyspaceStatus {keyspace_status:?} is not a modelled \
                         KeyspaceStatus value, got {got}",
                        case.name
                    );
                    assert_eq!(
                        keyspace_status,
                        keyspaces::KEYSPACE_STATUS_CREATING,
                        "case {:?}: a keyspace whose creation was just accepted is CREATING, \
                         got {got}",
                        case.name
                    );
                    assert!(
                        status["tablesReplicationProgress"].is_null(),
                        "case {:?}: a keyspace holding no table has no replication progress, \
                         got {got}",
                        case.name
                    );
                }
            }
        }
    }

    // RULE 3: the transition the clock owed has become due, so every keyspace is ACTIVE —
    // and nothing else about it moved.
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        let got = get_keyspace(&emulator, &case.name, &case.name).await;
        assert_eq!(
            got["replicationStrategy"],
            json!(case.strategy),
            "case {:?}: settling a status must not disturb the strategy, got {got}",
            case.name
        );
        if let Some(regions) = &case.regions {
            for status in got["replicationGroupStatuses"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                assert_eq!(
                    status["keyspaceStatus"],
                    json!(keyspaces::KEYSPACE_STATUS_ACTIVE),
                    "case {:?}: the creation span has elapsed, so the keyspace is ACTIVE in \
                     every one of {regions:?}, got {got}",
                    case.name
                );
            }
        }

        // A read changes nothing a later caller sees: the very next read answers the same.
        let again = get_keyspace(&emulator, &case.name, &case.name).await;
        assert_eq!(
            got, again,
            "case {:?}: a settled keyspace reads back identically, got {again}",
            case.name
        );
    }
}

/// RULE 4: `tablesReplicationProgress` is the percentage of the keyspace's tables that are
/// `ACTIVE`, over the whole range from none of them to all of them.
#[tokio::test]
async fn get_keyspace_reports_the_share_of_tables_that_are_active() {
    struct Row {
        /// Also the keyspace name.
        case: &'static str,
        /// Tables created BEFORE the settle span elapses, so they are `ACTIVE` by the read.
        active_tables: usize,
        /// Tables created after it, so they are still `CREATING` at the read.
        creating_tables: usize,
        /// The percentage the read must report, or `None` for no progress at all.
        expected: Option<&'static str>,
    }

    let rows = [
        Row {
            case: "progress_no_tables",
            active_tables: 0,
            creating_tables: 0,
            expected: None,
        },
        Row {
            case: "progress_none_active",
            active_tables: 0,
            creating_tables: 3,
            expected: Some("0.00%"),
        },
        Row {
            case: "progress_one_of_three",
            active_tables: 1,
            creating_tables: 2,
            expected: Some("33.33%"),
        },
        Row {
            case: "progress_two_of_three",
            active_tables: 2,
            creating_tables: 1,
            expected: Some("66.67%"),
        },
        Row {
            case: "progress_three_of_four",
            active_tables: 3,
            creating_tables: 1,
            expected: Some("75.00%"),
        },
        Row {
            case: "progress_all_active",
            active_tables: 2,
            creating_tables: 0,
            expected: Some("100.00%"),
        },
    ];

    let emulator = Emulator::start();

    // Only a MULTI_REGION keyspace reports replication statuses at all, so every row is one.
    for row in &rows {
        assert_ok(
            row.case,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": row.case,
                        "replicationSpecification": {
                            "replicationStrategy": "MULTI_REGION",
                            "regionList": [REGION, OTHER_REGION],
                        },
                    }),
                )
                .await,
        );
        for index in 0..row.active_tables {
            create_table(&emulator, row.case, row.case, &format!("early_{index}")).await;
        }
    }

    // The tables created above are owed their ACTIVE transition; wait it out once.
    tokio::time::sleep(settle_wait()).await;

    for row in &rows {
        // Created after the wait, so these are still in the transient CREATING status.
        for index in 0..row.creating_tables {
            create_table(&emulator, row.case, row.case, &format!("late_{index}")).await;
        }

        let got = get_keyspace(&emulator, row.case, row.case).await;
        let statuses = got["replicationGroupStatuses"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a MULTI_REGION keyspace reports a status per Region, got {got}",
                    row.case
                )
            });
        for status in &statuses {
            let progress = status["tablesReplicationProgress"].clone();
            assert_eq!(
                progress,
                row.expected.map(Value::from).unwrap_or(Value::Null),
                "case {:?}: {} of {} tables are ACTIVE, got {got}",
                row.case,
                row.active_tables,
                row.active_tables + row.creating_tables
            );
        }
    }
}

/// RULE 5: a name no keyspace is stored under, and a name the model does not accept, are
/// each refused with the modelled error — never with a 501, and never by inventing the
/// keyspace.
///
/// The rows span the `keyspaceName` constraint (`length 1..48`, pattern
/// `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`): each published bound, one past it, and a character
/// the pattern excludes — plus well-formed names that simply name nothing.
#[tokio::test]
async fn getting_a_keyspace_that_is_not_stored_is_the_modelled_error() {
    let emulator = Emulator::start();

    // One keyspace really does exist, so an absent name is not merely an empty store.
    assert_ok(
        "setup",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": "orders_live"}))
            .await,
    );
    // And one is created and then deleted, so absence can also be something that was
    // there. A keyspace is deletable once its creation has settled, and the deletion
    // settles into absence a span later.
    assert_ok(
        "setup",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": "orders_old"}))
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "setup",
        emulator
            .call("DeleteKeyspace", json!({"keyspaceName": "orders_old"}))
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    // (case, requested name, expected status, expected error code)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        ("never_created", "no_such_keyspace".to_string(), 404, "ResourceNotFoundException"),
        ("deleted", "orders_old".to_string(), 404, "ResourceNotFoundException"),
        // A prefix of a stored name is a different name.
        ("prefix_of_a_stored_name", "orders".to_string(), 404, "ResourceNotFoundException"),
        // The bottom of the modelled length range, well formed and absent.
        ("shortest_absent", "z".to_string(), 404, "ResourceNotFoundException"),
        // The top of it, likewise.
        ("longest_absent", longest_name("absent_"), 404, "ResourceNotFoundException"),
        // One past the top.
        ("one_past_the_longest", format!("{}y", longest_name("absent_")), 400, "ValidationException"),
        // One below the bottom.
        ("empty", String::new(), 400, "ValidationException"),
        // Characters the pattern excludes, and a leading character it excludes.
        ("hyphen", "orders-live".to_string(), 400, "ValidationException"),
        ("leading_underscore", "_orders".to_string(), 400, "ValidationException"),
    ];

    for (case, name, expected_status, expected_code) in &rows {
        let answer = emulator
            .call("GetKeyspace", json!({"keyspaceName": name}))
            .await;
        let body = answer.1.clone();
        assert_failure(case, answer, *expected_status, expected_code);
        if *expected_status == 404 {
            // Which resource was missing: the modelled member a client reads to find out.
            assert_eq!(
                body["resourceArn"],
                json!(keyspace_arn(name)),
                "case {case:?}: the not-found names the keyspace that was looked for, got {body}"
            );
        }
    }

    // None of those reads invented anything: the one stored keyspace still reads back, and
    // the names that were refused are still absent.
    let got = get_keyspace(&emulator, "stored_keyspace_survives", "orders_live").await;
    assert_eq!(
        got["resourceArn"],
        json!(keyspace_arn("orders_live")),
        "case \"stored_keyspace_survives\": the stored keyspace is untouched, got {got}"
    );
    for (case, name, expected_status, _) in &rows {
        if *expected_status != 404 {
            continue;
        }
        assert_failure(
            case,
            emulator
                .call("GetKeyspace", json!({"keyspaceName": name}))
                .await,
            404,
            "ResourceNotFoundException",
        );
    }
}
