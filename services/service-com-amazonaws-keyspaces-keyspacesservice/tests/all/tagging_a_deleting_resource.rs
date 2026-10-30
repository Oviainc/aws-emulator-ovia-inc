// SPDX-License-Identifier: CC0-1.0

//! Tagging writes against an Amazon Keyspaces resource whose deletion is already in
//! progress, driven end to end over the service's own `awsJson1_0` wire protocol through
//! the harness in [`crate::keyspace_table_lifecycle`].
//!
//! The rule under test: a resource whose deletion has been accepted takes no further
//! change until that deletion completes, so EVERY write against it — the tagging writes
//! included — is refused with the modelled `ConflictException` ("Amazon Keyspaces couldn't
//! complete the requested action. This error may occur if you try to perform an action and
//! the same or a different action is already in progress") and stores nothing. Deleting a
//! keyspace deletes everything in it ("The `DeleteKeyspace` operation deletes a keyspace
//! and all of its tables"), so a table of a keyspace on its way out is on its way out too.
//!
//! `DELETING` is the ONLY status that blocks such a write: a resource whose creation or
//! update is still settling stays, so the write lands on a record a later reader still
//! finds.
//!
//! Each assertion below spans a SET of inputs rather than one example:
//!
//! 1. Every way a taggable resource reaches `DELETING` here — `DeleteKeyspace` addressed
//!    by the keyspace's ARN, `DeleteKeyspace` addressed by a table ARN of that keyspace,
//!    and `DeleteTable` addressed by the table's ARN while its keyspace stays `ACTIVE` —
//!    refuses `TagResource` and `UntagResource` alike, leaves the tag set exactly as it
//!    was, and settles into absence, where the same calls answer the not-found instead.
//! 2. Every other status these resources can be observed in — `CREATING`, `ACTIVE` and,
//!    for a table, `UPDATING` — still takes a tagging write, which the reader reports
//!    back. The statuses are the model's own `KeyspaceStatus` / `TableStatus` value sets.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    KEYSPACE_STATUS_VALUES, TABLE_STATUS_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, minimal_schema, settle_wait, table_arn, Emulator,
    REGION,
};

/// The wire answer this service gives for a resource that is already on its way out: the
/// modelled `ConflictException`, on the client-error status every refusal of this RPC
/// service travels with
/// (`service_..._keyspacesservice::scaffolded::CLIENT_ERROR_STATUS`).
const CONFLICT_STATUS: u16 = 400;
const CONFLICT_CODE: &str = "ConflictException";

/// The tag every row seeds while its resource is still settled, so a refused `TagResource`
/// and a refused `UntagResource` can each be shown to have changed nothing.
const SEEDED_KEY: &str = "seeded";
const SEEDED_VALUE: &str = "before_the_delete";

/// Which call accepts the deletion a row then writes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accepted {
    /// `DeleteKeyspace`: the keyspace and every table in it enter `DELETING`.
    ByDeletingTheKeyspace,
    /// `DeleteTable`: only that table enters `DELETING`; its keyspace stays `ACTIVE`.
    ByDeletingTheTable,
}

/// Which resource's ARN the tagging write addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addressed {
    Keyspace,
    Table,
}

/// One row: how the deletion is accepted, and which ARN the refused writes carry.
struct Row {
    /// The case name, which is also the keyspace the row works in, so rows never share a
    /// resource.
    case: &'static str,
    accepted: Accepted,
    addressed: Addressed,
}

/// The table under test: the three ways a taggable resource of this service is observable
/// in `DELETING`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "keyspace_arn_after_delete_keyspace",
            accepted: Accepted::ByDeletingTheKeyspace,
            addressed: Addressed::Keyspace,
        },
        Row {
            case: "table_arn_after_delete_keyspace",
            accepted: Accepted::ByDeletingTheKeyspace,
            addressed: Addressed::Table,
        },
        Row {
            case: "table_arn_after_delete_table",
            accepted: Accepted::ByDeletingTheTable,
            addressed: Addressed::Table,
        },
    ]
}

/// The table name every row creates in its own keyspace.
const TABLE: &str = "doomed_tbl";

/// A `TagList` request member built from key/value pairs.
fn tags_json(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// The tags `ListTagsForResource` reports for `resource_arn`, as key/value pairs in the
/// order reported, or the failure it answered with.
async fn list_tags(emulator: &Emulator, resource_arn: &str) -> (u16, Value) {
    emulator
        .call("ListTagsForResource", json!({"resourceArn": resource_arn}))
        .await
}

/// The reported tag set of a resource the reader must still answer 200 for.
async fn reported_tags(
    case: &str,
    emulator: &Emulator,
    resource_arn: &str,
) -> Vec<(String, String)> {
    let body = assert_ok(case, list_tags(emulator, resource_arn).await);
    body["tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag: &Value| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// RULE 1: a tagging write against a resource whose deletion is in progress is the modelled
/// conflict, changes nothing, and becomes the not-found once the deletion completes.
#[tokio::test]
async fn a_tagging_write_against_a_deleting_resource_is_refused_and_stores_nothing() {
    for row in rows() {
        let emulator = Emulator::start();
        let case = row.case;
        let keyspace = row.case;
        assert_ok(
            case,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
        assert_ok(
            case,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": TABLE,
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )
                .await,
        );
        let resource_arn = match row.addressed {
            Addressed::Keyspace => keyspace_arn(keyspace),
            Addressed::Table => table_arn(keyspace, TABLE),
        };

        // Both resources are settled before the deletion is accepted, so what the writes
        // below are refused for is the deletion and nothing else.
        tokio::time::sleep(settle_wait()).await;
        assert_ok(
            case,
            emulator
                .call(
                    "TagResource",
                    json!({
                        "resourceArn": resource_arn,
                        "tags": tags_json(&[(SEEDED_KEY, SEEDED_VALUE)]),
                    }),
                )
                .await,
        );

        match row.accepted {
            Accepted::ByDeletingTheKeyspace => assert_ok(
                case,
                emulator
                    .call("DeleteKeyspace", json!({"keyspaceName": keyspace}))
                    .await,
            ),
            Accepted::ByDeletingTheTable => assert_ok(
                case,
                emulator
                    .call(
                        "DeleteTable",
                        json!({"keyspaceName": keyspace, "tableName": TABLE}),
                    )
                    .await,
            ),
        };

        // Inside the span the deletion is in flight — nothing sleeps here, so the record
        // the writes address is still stored and still reported by the reader.
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({
                    "resourceArn": resource_arn,
                    "tags": tags_json(&[("added_during_the_delete", "v")]),
                }),
            )
            .await;
        assert_failure(
            &format!("TagResource {case}"),
            (status, body.clone()),
            CONFLICT_STATUS,
            CONFLICT_CODE,
        );
        // The refusal says which resource is on its way out and which status the caller is
        // waiting on, which is the only thing it can act on.
        let message = body["message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(keyspaces::KEYSPACE_STATUS_DELETING),
            "case {case:?}: expected the refusal to name the status {}, got {body}",
            keyspaces::KEYSPACE_STATUS_DELETING
        );
        assert!(
            message.contains(keyspace),
            "case {case:?}: expected the refusal to name keyspace {keyspace:?}, got {body}"
        );
        if row.accepted == Accepted::ByDeletingTheTable {
            assert!(
                message.contains(TABLE),
                "case {case:?}: the table is the resource being deleted, so the refusal \
                 names it; got {body}"
            );
        }

        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({
                    "resourceArn": resource_arn,
                    "tags": tags_json(&[(SEEDED_KEY, SEEDED_VALUE)]),
                }),
            )
            .await;
        assert_failure(
            &format!("UntagResource {case}"),
            (status, body),
            CONFLICT_STATUS,
            CONFLICT_CODE,
        );

        // Neither refusal is a half-performed write: the seeded tag is still there and the
        // key the refused call carried was never stored.
        assert_eq!(
            reported_tags(case, &emulator, &resource_arn).await,
            vec![(SEEDED_KEY.to_string(), SEEDED_VALUE.to_string())],
            "case {case:?}: the refused writes must leave the tag set exactly as it was"
        );

        // Once the clock settles the deletion the resource is gone, so the SAME calls now
        // answer the not-found rather than the conflict — which is what makes the conflict the
        // answer for the window, not a second spelling of absence.
        tokio::time::sleep(settle_wait()).await;
        for (operation, request_tags) in [
            ("TagResource", tags_json(&[("after", "v")])),
            ("UntagResource", tags_json(&[(SEEDED_KEY, SEEDED_VALUE)])),
        ] {
            assert_failure(
                &format!("{operation} {case} after the deletion completed"),
                emulator
                    .call(
                        operation,
                        json!({"resourceArn": resource_arn, "tags": request_tags}),
                    )
                    .await,
                400,
                "ResourceNotFoundException",
            );
        }
        assert_failure(
            &format!("ListTagsForResource {case} after the deletion completed"),
            list_tags(&emulator, &resource_arn).await,
            400,
            "ResourceNotFoundException",
        );
    }
}

/// One row of the statuses a tagging write must still be served in.
struct Served {
    case: &'static str,
    /// The status the resource is in when the write is made.
    status: &'static str,
    /// Whether the ARN under test is the table's (`true`) or the keyspace's (`false`).
    table: bool,
    /// Whether the row waits for the creation to settle before writing.
    settled: bool,
    /// Whether the row puts the table in `UPDATING` before writing.
    updating: bool,
}

/// The Regions a keyspace row replicates over, so its own status is observable: a
/// keyspace's status reaches a client through `ReplicationGroupStatus$keyspaceStatus`,
/// which a single-Region keyspace reports none of (`RegionList` is modelled len 2..25).
const REPLICATION_REGIONS: [&str; 2] = [REGION, "us-east-1"];

/// The status a reader reports for the resource a row addresses: `GetTable$status` for a
/// table, and the `keyspaceStatus` `GetKeyspace` reports for the request's own Region for
/// a keyspace.
async fn observed_status(case: &str, emulator: &Emulator, keyspace: &str, table: bool) -> String {
    if table {
        let body = assert_ok(
            case,
            emulator
                .call(
                    "GetTable",
                    json!({"keyspaceName": keyspace, "tableName": TABLE}),
                )
                .await,
        );
        return body["status"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: GetTable reported no status: {body}"))
            .to_string();
    }
    let body = assert_ok(
        case,
        emulator
            .call("GetKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    body["replicationGroupStatuses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|entry| entry["region"] == json!(REGION))
        .and_then(|entry| entry["keyspaceStatus"].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            panic!("case {case:?}: GetKeyspace reported no keyspaceStatus for {REGION}: {body}")
        })
}

/// RULE 2: `DELETING` is the only status that blocks a tagging write — every other status
/// these resources are observable in still takes one, and the reader reports it back.
#[tokio::test]
async fn every_status_other_than_deleting_still_takes_a_tagging_write() {
    let rows = [
        Served {
            case: "creating_keyspace",
            status: keyspaces::KEYSPACE_STATUS_CREATING,
            table: false,
            settled: false,
            updating: false,
        },
        Served {
            case: "active_keyspace",
            status: keyspaces::KEYSPACE_STATUS_ACTIVE,
            table: false,
            settled: true,
            updating: false,
        },
        Served {
            case: "creating_table",
            status: keyspaces::TABLE_STATUS_CREATING,
            table: true,
            settled: false,
            updating: false,
        },
        Served {
            case: "active_table",
            status: keyspaces::TABLE_STATUS_ACTIVE,
            table: true,
            settled: true,
            updating: false,
        },
        Served {
            case: "updating_table",
            status: keyspaces::TABLE_STATUS_UPDATING,
            table: true,
            settled: true,
            updating: true,
        },
    ];

    for row in rows {
        let emulator = Emulator::start();
        let case = row.case;
        let keyspace = row.case;
        // A keyspace row replicates over two Regions, so the keyspace's own status is
        // observable; a table row needs no replication for `GetTable` to report one.
        let mut create = json!({"keyspaceName": keyspace});
        if !row.table {
            create["replicationSpecification"] = json!({
                "replicationStrategy": "MULTI_REGION",
                "regionList": REPLICATION_REGIONS,
            });
        }
        assert_ok(case, emulator.call("CreateKeyspace", create).await);
        assert_ok(
            case,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": TABLE,
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )
                .await,
        );
        if row.settled {
            tokio::time::sleep(settle_wait()).await;
        }
        if row.updating {
            assert_ok(
                case,
                emulator
                    .call(
                        "UpdateTable",
                        json!({
                            "keyspaceName": keyspace,
                            "tableName": TABLE,
                            "defaultTimeToLive": 90,
                        }),
                    )
                    .await,
            );
        }

        // The row really is in the status it claims: a write served against a resource that
        // had already settled elsewhere would prove nothing.
        let resource_arn = match row.table {
            true => table_arn(keyspace, TABLE),
            false => keyspace_arn(keyspace),
        };
        assert_eq!(
            observed_status(case, &emulator, keyspace, row.table).await,
            row.status,
            "case {case:?}: the row's resource is not in the status it claims"
        );
        let status_set = match row.table {
            true => TABLE_STATUS_VALUES,
            false => KEYSPACE_STATUS_VALUES,
        };
        assert!(
            status_set.contains(&row.status),
            "case {case:?}: {} is not one of the modelled statuses {status_set:?}",
            row.status
        );

        assert_ok(
            case,
            emulator
                .call(
                    "TagResource",
                    json!({
                        "resourceArn": resource_arn,
                        "tags": tags_json(&[(SEEDED_KEY, SEEDED_VALUE)]),
                    }),
                )
                .await,
        );
        assert_eq!(
            reported_tags(case, &emulator, &resource_arn).await,
            vec![(SEEDED_KEY.to_string(), SEEDED_VALUE.to_string())],
            "case {case:?}: a resource that is not DELETING takes the write, and the \
             reader reports it"
        );
        assert_ok(
            case,
            emulator
                .call(
                    "UntagResource",
                    json!({
                        "resourceArn": resource_arn,
                        "tags": tags_json(&[(SEEDED_KEY, SEEDED_VALUE)]),
                    }),
                )
                .await,
        );
        assert!(
            reported_tags(case, &emulator, &resource_arn).await.is_empty(),
            "case {case:?}: the removal lands too, so the reader reports no tag"
        );
    }
}
