// SPDX-License-Identifier: CC0-1.0

//! A keyspace takes a new table, type or restored table AS SOON AS it is stored — the
//! transient `CREATING` its own create leaves it in, and the `UPDATING` a replication
//! change leaves it in, do not hold a resource out — while a keyspace whose DELETION is in
//! flight takes none. Driven end to end over the service's own `awsJson1_0` wire protocol
//! through the shared [`crate::keyspace_table_lifecycle::Emulator`] harness, so what is
//! pinned is the emulator's observable behaviour rather than its internals.
//!
//! The rule under test: what decides whether a keyspace accepts a resource added to it is
//! whether that keyspace is on its way OUT, not whether a change of its own is still
//! settling. `CreateKeyspace` is asynchronous and the keyspace it stores is usable
//! immediately — Amazon Keyspaces serves `CreateTable`, `CreateType` and `RestoreTable`
//! against a keyspace it still reports as `CREATING` — and `DELETING` is the one status
//! that is the modelled `ConflictException` ("you try to perform an action and the same or
//! a different action is already in progress"), because a resource stored then would
//! vanish with the keyspace that is about to go.
//!
//! The rows span:
//!
//! * every operation that adds a resource to a keyspace — `CreateTable`, `CreateType` and
//!   `RestoreTable`, whose TARGET keyspace is the one the restored table is added to;
//! * every `KeyspaceStatus` value (`crate::ops::types::KEYSPACE_STATUS_VALUES` = `ACTIVE`,
//!   `CREATING`, `UPDATING`, `DELETING`), each reached through the operation that leaves
//!   the keyspace in it: `CreateKeyspace` (`CREATING`), the elapsed transient span
//!   (`ACTIVE`), `UpdateKeyspace` (`UPDATING`) and `DeleteKeyspace` (`DELETING`);
//! * each added resource read back through `GetTable`/`GetType`, so an accepted add is
//!   believed only once a reader reports what it stored.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::KEYSPACE_STATUS_VALUES;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, Emulator, REGION,
};

/// The status every refusal of this RPC service travels with: the error TYPE in the body
/// discriminates the failure, not the transport status, which
/// `crate::client_error_status` pins for every modelled exception of this service.
const CLIENT_ERROR_STATUS: u16 = 400;
const CONFLICT_CODE: &str = "ConflictException";
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// A Region other than the request's own ([`REGION`]), so a keyspace can be widened into
/// one: `RegionList` is modelled `len 2..25` and "requires the current Region and at least
/// one additional Amazon Web Services Region".
const OTHER_REGION: &str = "us-east-1";

/// The keyspace every restore row reads its source table out of, which stays `ACTIVE`
/// throughout: what these rows vary is the TARGET keyspace's status.
const SOURCE_KEYSPACE: &str = "takes_src_ks";
/// The source table of every restore row.
const SOURCE_TABLE: &str = "takes_src_tbl";

/// One operation that adds a resource to a keyspace: the request that adds it, and the
/// read that reports what was stored.
struct Adder {
    /// The operation's name on the wire, which is also part of each case name.
    operation: &'static str,
    /// The request body that adds `name` to `keyspace`.
    request: fn(keyspace: &str, name: &str) -> Value,
    /// The read that reports the resource the request stores, as (operation, request).
    read: fn(keyspace: &str, name: &str) -> (&'static str, Value),
}

/// The three paths that add a resource to a keyspace.
fn adders() -> Vec<Adder> {
    vec![
        Adder {
            operation: "CreateTable",
            request: |keyspace, name| {
                json!({
                    "keyspaceName": keyspace,
                    "tableName": name,
                    "schemaDefinition": minimal_schema("pk"),
                })
            },
            read: |keyspace, name| {
                (
                    "GetTable",
                    json!({"keyspaceName": keyspace, "tableName": name}),
                )
            },
        },
        Adder {
            operation: "CreateType",
            request: |keyspace, name| {
                json!({
                    "keyspaceName": keyspace,
                    "typeName": name,
                    "fieldDefinitions": [{"name": "f", "type": "text"}],
                })
            },
            read: |keyspace, name| {
                (
                    "GetType",
                    json!({"keyspaceName": keyspace, "typeName": name}),
                )
            },
        },
        Adder {
            operation: "RestoreTable",
            request: |keyspace, name| {
                json!({
                    "sourceKeyspaceName": SOURCE_KEYSPACE,
                    "sourceTableName": SOURCE_TABLE,
                    "targetKeyspaceName": keyspace,
                    "targetTableName": name,
                })
            },
            read: |keyspace, name| {
                (
                    "GetTable",
                    json!({"keyspaceName": keyspace, "tableName": name}),
                )
            },
        },
    ]
}

/// The source keyspace and the settled source table every restore row restores from.
async fn given_a_restorable_source(emulator: &Emulator) {
    assert_ok(
        SOURCE_KEYSPACE,
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": SOURCE_KEYSPACE}))
            .await,
    );
    assert_ok(
        SOURCE_TABLE,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": SOURCE_KEYSPACE,
                    "tableName": SOURCE_TABLE,
                    "schemaDefinition": minimal_schema("pk"),
                    "pointInTimeRecovery": {"status": "ENABLED"},
                }),
            )
            .await,
    );
    // A restore reads its source only in `ACTIVE`, so this one table — and nothing about
    // the target keyspace below — is waited out.
    tokio::time::sleep(settle_wait()).await;
}

/// The `message` member of a failure body, as a string.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message member>")
        .to_string()
}

/// The call that moves a keyspace out of the status its create left it in: the operation's
/// name on the wire, and the request it takes for one keyspace.
type Transition = (&'static str, fn(&str) -> Value);

/// One row: the `KeyspaceStatus` the keyspace is in when the add is made, and how it was
/// left in it.
struct Row {
    /// The status the keyspace holds when the add is attempted.
    status: &'static str,
    /// Whether the create's own transient span is waited out before the add.
    settled: bool,
    /// The call made (after that wait) to put the keyspace in `status`, or `None` when the
    /// create itself leaves it there.
    transition: Option<Transition>,
    /// Whether the keyspace accepts the add in this status.
    accepts: bool,
}

/// Every modelled `KeyspaceStatus`, each reached through the operation that leaves the
/// keyspace in it.
fn rows() -> Vec<Row> {
    vec![
        // "CreateKeyspace is an asynchronous operation. You can monitor the creation
        // status of the new keyspace by using the GetKeyspace operation." The keyspace is
        // `CREATING` until the clock settles it — and already holds resources.
        Row {
            status: keyspaces::KEYSPACE_STATUS_CREATING,
            settled: false,
            transition: None,
            accepts: true,
        },
        // The same keyspace once that span has elapsed.
        Row {
            status: keyspaces::KEYSPACE_STATUS_ACTIVE,
            settled: true,
            transition: None,
            accepts: true,
        },
        // UpdateKeyspace "adds a new Amazon Web Services Region to the keyspace ... Amazon
        // Keyspaces is going to replicate all tables in the keyspace to the new Region", so
        // the keyspace is `UPDATING` while that replication is in flight.
        Row {
            status: keyspaces::KEYSPACE_STATUS_UPDATING,
            settled: true,
            transition: Some(("UpdateKeyspace", |keyspace| {
                json!({
                    "keyspaceName": keyspace,
                    "replicationSpecification": {
                        "replicationStrategy": "MULTI_REGION",
                        "regionList": [REGION, OTHER_REGION],
                    },
                })
            })),
            accepts: true,
        },
        // "DeleteKeyspace ... deletes a keyspace and all of its tables": the keyspace and
        // everything in it are on their way out, so it takes nothing more.
        Row {
            status: keyspaces::KEYSPACE_STATUS_DELETING,
            settled: true,
            transition: Some(("DeleteKeyspace", |keyspace| {
                json!({"keyspaceName": keyspace})
            })),
            accepts: false,
        },
    ]
}

/// Put one keyspace in the row's status and hand back its name.
async fn given_a_keyspace_in(emulator: &Emulator, case: &str, row: &Row) -> String {
    let keyspace = format!("takes_{}_ks", row.status.to_ascii_lowercase());
    assert_ok(
        case,
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    if row.settled {
        tokio::time::sleep(settle_wait()).await;
    }
    if let Some((operation, request)) = row.transition {
        assert_ok(case, emulator.call(operation, request(&keyspace)).await);
    }
    keyspace
}

/// The rule: a keyspace takes every resource an operation adds to it while a change of its
/// OWN is still in flight, and takes none once its deletion is.
#[tokio::test]
async fn a_keyspace_takes_a_new_resource_in_every_status_but_the_one_that_is_going_away() {
    for row in rows() {
        assert!(
            KEYSPACE_STATUS_VALUES.contains(&row.status),
            "case {:?}: this row is meant to name a modelled status, got the modelled set \
             {KEYSPACE_STATUS_VALUES:?}",
            row.status
        );

        for adder in adders() {
            let case = format!("{} into a {} keyspace", adder.operation, row.status);
            // One emulator per case, so the status of the keyspace under test is the only
            // thing that differs between them.
            let emulator = Emulator::start();
            given_a_restorable_source(&emulator).await;
            let keyspace = given_a_keyspace_in(&emulator, &case, &row).await;

            let name = "added";
            let (status, body) = emulator
                .call(adder.operation, (adder.request)(&keyspace, name))
                .await;
            let (read, read_request) = (adder.read)(&keyspace, name);
            if !row.accepts {
                assert_failure(&case, (status, body.clone()), CLIENT_ERROR_STATUS, CONFLICT_CODE);
                let expected = keyspaces::keyspace_deleting_rejection(&keyspace);
                assert_eq!(
                    message(&body), expected,
                    "case {case:?}: expected the refusal to state {expected:?}, got {body}"
                );
                // The refusal stored nothing, and the deletion the clock owed completed.
                tokio::time::sleep(settle_wait()).await;
                assert_failure(
                    &format!("{case}: {read} after the refusal"),
                    emulator.call(read, read_request).await,
                    CLIENT_ERROR_STATUS,
                    NOT_FOUND_CODE,
                );
                continue;
            }

            assert_eq!(
                status, 200,
                "case {case:?}: a keyspace in status {} takes the resource, got {status} \
                 {body}",
                row.status
            );
            // The add is believed only once a reader reports what it stored, under the
            // keyspace it was added to.
            let stored = assert_ok(
                &format!("{case}: {read}"),
                emulator.call(read, read_request.clone()).await,
            );
            assert_eq!(
                stored["keyspaceName"], json!(keyspace),
                "case {case:?}: the resource reads back under the keyspace it was added to, \
                 got {stored}"
            );
            // And it is still there once every transient span of that keyspace and of the
            // resource itself has elapsed: an add served against an unsettled keyspace is
            // stored, not swallowed by the transition the clock still owed.
            tokio::time::sleep(settle_wait()).await;
            let settled = assert_ok(
                &format!("{case}: {read} once every span has elapsed"),
                emulator.call(read, read_request).await,
            );
            assert_eq!(
                settled["status"], json!(keyspaces::KEYSPACE_STATUS_ACTIVE),
                "case {case:?}: the added resource settles into {}, got {settled}",
                keyspaces::KEYSPACE_STATUS_ACTIVE
            );
        }
    }
}

/// The same rule from the keyspace's side: a keyspace created and never waited for holds
/// EVERY resource added to it, and reports them all through its own listings.
#[tokio::test]
async fn resources_added_before_a_keyspace_settles_are_all_there_afterwards() {
    let emulator = Emulator::start();
    given_a_restorable_source(&emulator).await;
    let keyspace = "takes_unsettled_ks";
    assert_ok(
        keyspace,
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    // Every add is made inside the keyspace's own transient span, with nothing waited out
    // in between.
    for (index, adder) in adders().into_iter().enumerate() {
        let case = format!("{} into a keyspace that has not settled", adder.operation);
        let name = format!("added_{index}");
        assert_ok(
            &case,
            emulator
                .call(adder.operation, (adder.request)(keyspace, &name))
                .await,
        );
    }
    tokio::time::sleep(settle_wait()).await;

    let tables = assert_ok(
        "ListTables",
        emulator
            .call("ListTables", json!({"keyspaceName": keyspace}))
            .await,
    );
    let table_names: Vec<String> = tables["tables"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|summary| summary["tableName"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        table_names,
        vec!["added_0".to_string(), "added_2".to_string()],
        "case \"ListTables\": the created and the restored table are both in the keyspace, \
         got {tables}"
    );
    let types = assert_ok(
        "ListTypes",
        emulator
            .call("ListTypes", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_eq!(
        types["types"],
        json!(["added_1"]),
        "case \"ListTypes\": the created type is in the keyspace, got {types}"
    );
}
