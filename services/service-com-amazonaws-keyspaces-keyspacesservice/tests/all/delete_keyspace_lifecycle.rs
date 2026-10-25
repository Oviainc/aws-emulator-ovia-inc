// SPDX-License-Identifier: CC0-1.0

//! `DeleteKeyspace`, driven end to end over the service's own `awsJson1_0` wire protocol
//! through the shared [`crate::keyspace_table_lifecycle::Emulator`] harness.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. A delete of a resource the model gives a `DELETING` status to is asynchronous and
//!    observable: an accepted `DeleteKeyspace` puts the keyspace — and every table and
//!    type it holds, which go with it ("The DeleteKeyspace operation deletes a keyspace
//!    and all of its tables") — in `DELETING`, every reader still reports it over that
//!    window, and only once the clock settles the deletion is it gone. `DELETING` is a
//!    member of the model's own `KeyspaceStatus` value set and reaches a client through
//!    `ReplicationGroupStatus$keyspaceStatus` ("The status of the keyspace").
//! 2. A keyspace is deletable only once it and everything in it has settled: while a
//!    change of its own is in flight, or a table or type of it is `CREATING`/`UPDATING`
//!    (the state in which `DeleteTable` itself refuses to drop a table), the delete is
//!    the modelled `ConflictException` — "you try to perform an action and the same or a
//!    different action is already in progress" — and nothing is written. A resource
//!    already `DELETING` is on its way out and blocks nothing.
//! 3. A keyspace whose deletion is in progress takes no new resource: the modelled
//!    `ConflictException` while it is still there, and the modelled
//!    `ResourceNotFoundException` once it is gone.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    KEYSPACE_STATUS_VALUES, TABLE_STATUS_VALUES, TYPE_STATUS_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, error_code, keyspace_arn, minimal_schema, settle_wait, Emulator,
    REGION,
};

/// The wire status and code the model declares for an action that cannot be completed
/// because another one is already in progress (`ConflictException`, HTTP 409).
const CONFLICT_STATUS: u16 = 409;
const CONFLICT_CODE: &str = "ConflictException";

/// Regions other than the request's own ([`REGION`]), each within `RegionList`'s member
/// length (2..25), so a `MULTI_REGION` keyspace can be created over two or three Regions.
const OTHER_REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// Create one keyspace, with `regions` when the row is `MULTI_REGION`, plus the tables and
/// types it holds, and wait for every one of them to settle into its usable status.
async fn seed(emulator: &Emulator, shape: &Shape) {
    let mut request = json!({"keyspaceName": shape.keyspace});
    if let Some(regions) = shape.regions {
        request["replicationSpecification"] =
            json!({"replicationStrategy": "MULTI_REGION", "regionList": regions});
    }
    assert_ok(shape.keyspace, emulator.call("CreateKeyspace", request).await);
    for table in shape.tables {
        assert_ok(
            shape.keyspace,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": shape.keyspace,
                        "tableName": table,
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )
                .await,
        );
    }
    for udt in shape.types {
        assert_ok(
            shape.keyspace,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": shape.keyspace,
                        "typeName": udt,
                        "fieldDefinitions": [{"name": "value", "type": "int"}],
                    }),
                )
                .await,
        );
    }
    tokio::time::sleep(settle_wait()).await;
}

/// One row of RULE 1: a keyspace shape to create and then delete.
struct Shape {
    /// The keyspace name, which is also the case name.
    keyspace: &'static str,
    /// The `regionList` of a `MULTI_REGION` keyspace, or `None` for a single-Region one —
    /// which is the only shape that reports no `replicationGroupStatuses` at all, the
    /// member's modelled minimum length being 2.
    regions: Option<&'static [&'static str]>,
    /// The tables the keyspace holds when the delete is called.
    tables: &'static [&'static str],
    /// The types it holds.
    types: &'static [&'static str],
}

/// The rows: the shapes differ in replication strategy, Region count and how many tables
/// and types go with the keyspace — including none of either.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            keyspace: "empty_single_ks",
            regions: None,
            tables: &[],
            types: &[],
        },
        Shape {
            keyspace: "tables_single_ks",
            regions: None,
            tables: &["first_tbl", "second_tbl", "third_tbl"],
            types: &[],
        },
        Shape {
            keyspace: "types_single_ks",
            regions: None,
            tables: &[],
            types: &["address"],
        },
        Shape {
            keyspace: "two_region_ks",
            regions: Some(&[REGION, OTHER_REGIONS[0]]),
            tables: &["only_tbl"],
            types: &[],
        },
        Shape {
            keyspace: "three_region_ks",
            regions: Some(&[REGION, OTHER_REGIONS[0], OTHER_REGIONS[1]]),
            tables: &["one_tbl", "two_tbl"],
            types: &["address", "contact"],
        },
    ]
}

/// `GetKeyspace`'s answer for one keyspace.
async fn get_keyspace(emulator: &Emulator, keyspace: &str) -> (u16, Value) {
    emulator
        .call("GetKeyspace", json!({"keyspaceName": keyspace}))
        .await
}

/// `GetTable`'s answer for one table.
async fn get_table(emulator: &Emulator, keyspace: &str, table: &str) -> (u16, Value) {
    emulator
        .call(
            "GetTable",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await
}

/// `GetType`'s answer for one type.
async fn get_type(emulator: &Emulator, keyspace: &str, type_name: &str) -> (u16, Value) {
    emulator
        .call(
            "GetType",
            json!({"keyspaceName": keyspace, "typeName": type_name}),
        )
        .await
}

/// Whether `ListKeyspaces` reports `keyspace`. Every suite here holds far fewer keyspaces
/// than one page, so one call reports them all.
async fn listed(emulator: &Emulator, keyspace: &str) -> bool {
    let body = assert_ok(keyspace, emulator.call("ListKeyspaces", json!({})).await);
    body["keyspaces"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .any(|summary| summary["keyspaceName"] == json!(keyspace))
        })
        .unwrap_or(false)
}

/// The `status` member of a 200 body, named for the case.
fn reported_status(case: &str, body: &Value) -> String {
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a reported status, got {body}"))
        .to_string()
}

/// RULE 1: whatever the keyspace holds, the delete is accepted, observable while it is in
/// flight, and settles into absence — after which the name is free again.
#[tokio::test]
async fn delete_keyspace_settles_every_keyspace_shape_into_absence() {
    // The status a deletion in flight reports is one the model declares, not an
    // invention: the catalog is the model's own `KeyspaceStatus` value set.
    for (set_name, values, status) in [
        (
            "KeyspaceStatus",
            KEYSPACE_STATUS_VALUES,
            keyspaces::KEYSPACE_STATUS_DELETING,
        ),
        (
            "TableStatus",
            TABLE_STATUS_VALUES,
            keyspaces::TABLE_STATUS_DELETING,
        ),
        (
            "TypeStatus",
            TYPE_STATUS_VALUES,
            keyspaces::TYPE_STATUS_DELETING,
        ),
    ] {
        assert!(
            values.contains(&status),
            "case {set_name:?}: {status} must be a modelled {set_name}, and the modelled \
             set is {values:?}"
        );
    }

    let emulator = Emulator::start();
    for shape in shapes() {
        let case = shape.keyspace;
        seed(&emulator, &shape).await;

        assert_ok(
            case,
            emulator
                .call("DeleteKeyspace", json!({"keyspaceName": case}))
                .await,
        );

        // The deletion is in flight, not instantaneous: every reader still reports the
        // keyspace and everything in it.
        let (status, body) = get_keyspace(&emulator, case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a keyspace whose deletion is in flight is still reported, got \
             {status} {body}"
        );
        assert!(
            listed(&emulator, case).await,
            "case {case:?}: ListKeyspaces must still report a keyspace that is DELETING"
        );
        match shape.regions {
            // "A list of all Regions the keyspace is replicated in ... and their status":
            // one entry per replication Region, each carrying the keyspace's own status,
            // which is where `DELETING` reaches a client.
            Some(regions) => {
                let statuses = body["replicationGroupStatuses"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: a MULTI_REGION keyspace reports a status per \
                             replication Region, got {body}"
                        )
                    });
                assert_eq!(
                    statuses.len(),
                    regions.len(),
                    "case {case:?}: expected one status per replication Region, got {body}"
                );
                for (entry, region) in statuses.iter().zip(regions) {
                    assert_eq!(
                        (entry["region"].clone(), entry["keyspaceStatus"].clone()),
                        (json!(region), json!(keyspaces::KEYSPACE_STATUS_DELETING)),
                        "case {case:?}: expected Region {region} to report {}, got {entry}",
                        keyspaces::KEYSPACE_STATUS_DELETING
                    );
                }
            }
            // The member's modelled minimum length is 2, which a single-Region keyspace's
            // one Region cannot satisfy, so it reports none — the keyspace's continued
            // presence is what a client observes for one.
            None => assert_eq!(
                body.get("replicationGroupStatuses"),
                None,
                "case {case:?}: a single-Region keyspace reports no replicationGroupStatuses, \
                 got {body}"
            ),
        }
        for table in shape.tables {
            let (status, body) = get_table(&emulator, case, table).await;
            assert_eq!(
                status, 200,
                "case {case:?}: table {table} goes with the keyspace, so it is still \
                 reported while the deletion is in flight, got {status} {body}"
            );
            assert_eq!(
                reported_status(case, &body),
                keyspaces::TABLE_STATUS_DELETING,
                "case {case:?}: expected table {table} to be {}, got {body}",
                keyspaces::TABLE_STATUS_DELETING
            );
        }
        for udt in shape.types {
            let (status, body) = get_type(&emulator, case, udt).await;
            assert_eq!(
                status, 200,
                "case {case:?}: type {udt} goes with the keyspace, so it is still reported \
                 while the deletion is in flight, got {status} {body}"
            );
            assert_eq!(
                reported_status(case, &body),
                keyspaces::TYPE_STATUS_DELETING,
                "case {case:?}: expected type {udt} to be {}, got {body}",
                keyspaces::TYPE_STATUS_DELETING
            );
        }

        // A second delete of a keyspace whose deletion is already in progress is the
        // modelled conflict, and it names the status the caller is waiting on.
        let (status, body) = emulator
            .call("DeleteKeyspace", json!({"keyspaceName": case}))
            .await;
        assert_failure(
            case,
            (status, body.clone()),
            CONFLICT_STATUS,
            CONFLICT_CODE,
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(case) && message.contains(keyspaces::KEYSPACE_STATUS_DELETING),
            "case {case:?}: expected the refusal to name the keyspace and the status {}, \
             got {body}",
            keyspaces::KEYSPACE_STATUS_DELETING
        );

        tokio::time::sleep(settle_wait()).await;

        // The deletion the clock owed has completed: the keyspace, its tables and its
        // types are gone from every reader.
        let (status, body) = get_keyspace(&emulator, case).await;
        assert_failure(case, (status, body.clone()), 404, "ResourceNotFoundException");
        assert_eq!(
            body["resourceArn"],
            json!(keyspace_arn(case)),
            "case {case:?}: the not-found names the keyspace that was looked for, got {body}"
        );
        assert!(
            !listed(&emulator, case).await,
            "case {case:?}: ListKeyspaces must not report a keyspace that is gone"
        );
        for table in shape.tables {
            assert_failure(
                case,
                get_table(&emulator, case, table).await,
                404,
                "ResourceNotFoundException",
            );
        }
        for udt in shape.types {
            assert_failure(
                case,
                get_type(&emulator, case, udt).await,
                404,
                "ResourceNotFoundException",
            );
        }
        assert_failure(
            case,
            emulator
                .call("DeleteKeyspace", json!({"keyspaceName": case}))
                .await,
            404,
            "ResourceNotFoundException",
        );

        // And the name is free again: a keyspace deleted is a keyspace creatable, holding
        // nothing the deleted one held.
        let recreated = assert_ok(
            case,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": case}))
                .await,
        );
        assert_eq!(
            recreated["resourceArn"],
            json!(keyspace_arn(case)),
            "case {case:?}: recreating a deleted keyspace mints its ARN again, got {recreated}"
        );
        let empty = assert_ok(
            case,
            emulator
                .call("ListTables", json!({"keyspaceName": case}))
                .await,
        );
        assert_eq!(
            empty["tables"],
            json!([]),
            "case {case:?}: the tables of a deleted keyspace are gone with it, got {empty}"
        );
    }
}

/// What a row of RULE 2 puts in flight, and how it is observed.
enum InFlight {
    /// The keyspace itself: the status `GetKeyspace` reports for it through its
    /// replication group.
    Keyspace,
    /// A table of the keyspace, by name.
    Table(&'static str),
    /// A type of the keyspace, by name.
    Type(&'static str),
}

/// One row of RULE 2: a keyspace with one change in flight, and the status that change
/// holds the resource in.
struct Pending {
    /// The case name, which is also the keyspace name.
    keyspace: &'static str,
    /// Which resource of the keyspace is mid-change.
    resource: InFlight,
    /// The status it is in when `DeleteKeyspace` is called.
    status: &'static str,
    /// The calls that put it there, each run in order; the keyspace is created, and
    /// allowed to settle, before the first of them.
    setup: fn(&'static str) -> Vec<(&'static str, Value)>,
}

/// The rows: every way a keyspace or a resource of it can be mid-change here — the create
/// that puts it in `CREATING`, and each update that puts it in `UPDATING`.
fn pending_rows() -> Vec<Pending> {
    vec![
        Pending {
            keyspace: "creating_ks",
            resource: InFlight::Keyspace,
            status: keyspaces::KEYSPACE_STATUS_CREATING,
            // Nothing to do: the keyspace is created by the harness and the delete
            // follows without waiting for it to settle.
            setup: |_| Vec::new(),
        },
        Pending {
            keyspace: "updating_ks",
            resource: InFlight::Keyspace,
            status: keyspaces::KEYSPACE_STATUS_UPDATING,
            setup: |keyspace| {
                vec![(
                    "UpdateKeyspace",
                    json!({
                        "keyspaceName": keyspace,
                        "replicationSpecification": {
                            "replicationStrategy": "MULTI_REGION",
                            "regionList": [REGION, OTHER_REGIONS[0]],
                        },
                    }),
                )]
            },
        },
        Pending {
            keyspace: "creating_tbl_ks",
            resource: InFlight::Table("pending_tbl"),
            status: keyspaces::TABLE_STATUS_CREATING,
            setup: |keyspace| {
                vec![(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": "pending_tbl",
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )]
            },
        },
        Pending {
            keyspace: "updating_tbl_ks",
            resource: InFlight::Table("pending_tbl"),
            status: keyspaces::TABLE_STATUS_UPDATING,
            setup: |keyspace| {
                vec![
                    (
                        "CreateTable",
                        json!({
                            "keyspaceName": keyspace,
                            "tableName": "pending_tbl",
                            "schemaDefinition": minimal_schema("pk"),
                        }),
                    ),
                    // The update is accepted only once the table is ACTIVE, which the
                    // runner waits out between the setup calls.
                    (
                        "UpdateTable",
                        json!({
                            "keyspaceName": keyspace,
                            "tableName": "pending_tbl",
                            "defaultTimeToLive": 120,
                        }),
                    ),
                ]
            },
        },
        Pending {
            keyspace: "creating_type_ks",
            resource: InFlight::Type("pending_type"),
            status: keyspaces::TYPE_STATUS_CREATING,
            setup: |keyspace| {
                vec![(
                    "CreateType",
                    json!({
                        "keyspaceName": keyspace,
                        "typeName": "pending_type",
                        "fieldDefinitions": [{"name": "value", "type": "int"}],
                    }),
                )]
            },
        },
    ]
}

/// The status the row's resource reports now, read back through the operation that reports
/// it.
async fn pending_status(emulator: &Emulator, row: &Pending) -> String {
    let case = row.keyspace;
    match row.resource {
        InFlight::Keyspace => {
            // A single-Region keyspace reports no status of its own, so the rows that
            // watch the keyspace make it MULTI_REGION and read the status its replication
            // group carries.
            let body = assert_ok(case, get_keyspace(emulator, case).await);
            body["replicationGroupStatuses"][0]["keyspaceStatus"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a keyspaceStatus, got {body}"))
                .to_string()
        }
        InFlight::Table(table) => {
            let body = assert_ok(case, get_table(emulator, case, table).await);
            reported_status(case, &body)
        }
        InFlight::Type(type_name) => {
            let body = assert_ok(case, get_type(emulator, case, type_name).await);
            reported_status(case, &body)
        }
    }
}

/// RULE 2: a keyspace with a change in flight — its own, or one of a table or type it
/// holds — is refused with the modelled conflict, nothing is deleted, and the same
/// request is accepted once the clock has settled that change.
#[tokio::test]
async fn deleting_a_keyspace_with_a_change_in_flight_answers_the_modelled_conflict() {
    let emulator = Emulator::start();
    for row in pending_rows() {
        let case = row.keyspace;
        // Every keyspace starts single-Region. The `updating_ks` row's own update is what
        // makes it `MULTI_REGION`, which is the only shape the model reports a keyspace
        // status for — so that row can read the status it claims, while `creating_ks` is
        // observed through the delete itself.
        assert_ok(
            case,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": case}))
                .await,
        );
        let setup = (row.setup)(case);
        for (operation, body) in setup {
            // Every setup call needs what it acts on to have settled first.
            tokio::time::sleep(settle_wait()).await;
            assert_ok(case, emulator.call(operation, body).await);
        }

        // The row really is mid-change: a refusal of a keyspace that had already settled
        // would prove nothing. The `creating_ks` row is read through its own delete
        // below, having no status on the wire.
        if !(matches!(row.resource, InFlight::Keyspace)
            && row.status == keyspaces::KEYSPACE_STATUS_CREATING)
        {
            assert_eq!(
                pending_status(&emulator, &row).await,
                row.status,
                "case {case:?}: expected the resource to be {} when the delete is called",
                row.status
            );
        }

        let (status, body) = emulator
            .call("DeleteKeyspace", json!({"keyspaceName": case}))
            .await;
        assert_failure(case, (status, body.clone()), CONFLICT_STATUS, CONFLICT_CODE);
        // The refusal says what the caller is waiting for, which is the only thing it can
        // act on. The text is not documented, so only what it names is asserted.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(case) && message.contains(row.status),
            "case {case:?}: expected the refusal to name the keyspace and the status {}, \
             got {body}",
            row.status
        );

        // A refused delete is not a half-performed one: the keyspace is still there, and
        // the change in flight settles into its usable status rather than into absence.
        let (status, body) = get_keyspace(&emulator, case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused delete removed the keyspace, got {status} {body}"
        );
        tokio::time::sleep(settle_wait()).await;
        let settled = assert_ok(case, get_keyspace(&emulator, case).await);
        assert_ne!(
            settled["keyspaceName"],
            Value::Null,
            "case {case:?}: the keyspace must still be reported after the refusal: {settled}"
        );

        // Once everything has settled the same request is accepted, and the keyspace is
        // then DELETING rather than gone.
        assert_ok(
            case,
            emulator
                .call("DeleteKeyspace", json!({"keyspaceName": case}))
                .await,
        );
        let (status, body) = get_keyspace(&emulator, case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an accepted delete leaves the keyspace observable, got \
             {status} {body}"
        );
    }
}

/// RULE 2's other half: a resource that is ALREADY `DELETING` is on its way out, so it
/// does not block the keyspace's own deletion.
#[tokio::test]
async fn a_resource_already_deleting_does_not_block_the_keyspace_delete() {
    let emulator = Emulator::start();
    let keyspace = "already_deleting_ks";
    seed(
        &emulator,
        &Shape {
            keyspace,
            regions: None,
            tables: &["going_tbl", "staying_tbl"],
            types: &["going_type"],
        },
    )
    .await;

    for (operation, request) in [
        (
            "DeleteTable",
            json!({"keyspaceName": keyspace, "tableName": "going_tbl"}),
        ),
        (
            "DeleteType",
            json!({"keyspaceName": keyspace, "typeName": "going_type"}),
        ),
    ] {
        assert_ok(operation, emulator.call(operation, request).await);
    }

    assert_ok(
        "delete the keyspace while a table and a type are DELETING",
        emulator
            .call("DeleteKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    let staying = assert_ok(keyspace, get_table(&emulator, keyspace, "staying_tbl").await);
    assert_eq!(
        reported_status(keyspace, &staying),
        keyspaces::TABLE_STATUS_DELETING,
        "the surviving table goes with its keyspace, so it is {} too: {staying}",
        keyspaces::TABLE_STATUS_DELETING
    );

    tokio::time::sleep(settle_wait()).await;
    assert_failure(
        keyspace,
        get_keyspace(&emulator, keyspace).await,
        404,
        "ResourceNotFoundException",
    );
}

/// RULE 3: while the keyspace is `DELETING` every request that would put a resource in it
/// is the modelled conflict, and once it is gone the same request is the modelled
/// not-found — never the scaffold's 501 and never a silently stored resource.
#[tokio::test]
async fn a_keyspace_being_deleted_takes_no_new_resource() {
    let emulator = Emulator::start();
    let doomed = "doomed_ks";
    let source = "restore_source_ks";

    // A keyspace that is NOT being deleted, holding a restorable table, so a restore into
    // the doomed keyspace is refused for the target rather than for its source.
    seed(
        &emulator,
        &Shape {
            keyspace: source,
            regions: None,
            tables: &[],
            types: &[],
        },
    )
    .await;
    assert_ok(
        source,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": source,
                    "tableName": "source_tbl",
                    "schemaDefinition": minimal_schema("pk"),
                    "pointInTimeRecovery": {"status": "ENABLED"},
                }),
            )
            .await,
    );
    seed(
        &emulator,
        &Shape {
            keyspace: doomed,
            regions: None,
            tables: &["existing_tbl"],
            types: &["existing_type"],
        },
    )
    .await;

    assert_ok(
        doomed,
        emulator
            .call("DeleteKeyspace", json!({"keyspaceName": doomed}))
            .await,
    );

    // (case, operation, request) — every way a resource reaches a keyspace.
    let writes: Vec<(&str, &str, Value)> = vec![
        (
            "CreateTable",
            "CreateTable",
            json!({
                "keyspaceName": doomed,
                "tableName": "late_tbl",
                "schemaDefinition": minimal_schema("pk"),
            }),
        ),
        (
            "CreateType",
            "CreateType",
            json!({
                "keyspaceName": doomed,
                "typeName": "late_type",
                "fieldDefinitions": [{"name": "value", "type": "int"}],
            }),
        ),
        (
            "RestoreTable",
            "RestoreTable",
            json!({
                "sourceKeyspaceName": source,
                "sourceTableName": "source_tbl",
                "targetKeyspaceName": doomed,
                "targetTableName": "late_restored_tbl",
            }),
        ),
        (
            "UpdateKeyspace",
            "UpdateKeyspace",
            json!({
                "keyspaceName": doomed,
                "replicationSpecification": {
                    "replicationStrategy": "MULTI_REGION",
                    "regionList": [REGION, OTHER_REGIONS[0]],
                },
            }),
        ),
    ];

    for (case, operation, request) in &writes {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_failure(case, (status, body.clone()), CONFLICT_STATUS, CONFLICT_CODE);
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(doomed) && message.contains(keyspaces::KEYSPACE_STATUS_DELETING),
            "case {case:?}: expected the refusal to name the keyspace and the status {}, \
             got {body}",
            keyspaces::KEYSPACE_STATUS_DELETING
        );
        assert_ne!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: the refusal must be the modelled conflict, not the scaffold: {body}"
        );
    }

    tokio::time::sleep(settle_wait()).await;

    // The keyspace is gone, so the very same requests now name a keyspace that does not
    // exist: a different refusal, and nothing was stored in between.
    for (case, operation, request) in &writes {
        assert_failure(
            case,
            emulator.call(operation, request.clone()).await,
            404,
            "ResourceNotFoundException",
        );
    }
    assert_failure(
        "the table a refused create would have added",
        get_table(&emulator, doomed, "late_tbl").await,
        404,
        "ResourceNotFoundException",
    );
}
