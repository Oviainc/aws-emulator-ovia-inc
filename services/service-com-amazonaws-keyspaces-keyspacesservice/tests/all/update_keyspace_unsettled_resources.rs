// SPDX-License-Identifier: CC0-1.0

//! `UpdateKeyspace` against a keyspace that still holds a resource of its own that is not
//! settled, driven end to end over the service's `awsJson1_0` wire protocol.
//!
//! The rule under test: adding an Amazon Web Services Region replicates the whole keyspace
//! — "Amazon Keyspaces is going to replicate all tables in the keyspace to the new Region"
//! — so the addition is accepted only while every resource the keyspace holds, each of its
//! tables AND each of its user-defined types, is `ACTIVE`; a resource in any other status
//! is the `ConflictException` the model declares on this operation ("you try to perform an
//! action and the same or a different action is already in progress"), naming that resource
//! and the status the caller is waiting on, and the refused call writes nothing. Once the
//! clock owes that resource its transition the same request is accepted and the keyspace is
//! replicated over the Regions the request named.
//!
//! The rows span which kind of resource is unsettled (a table, a user-defined type), the
//! status it is in and the call that put it there (`CREATING` from each create, `UPDATING`
//! from an update, `DELETING` from each delete), whether settled resources of both kinds sit
//! beside it, and the replication strategy the keyspace already had (single-Region, and
//! already multi-Region so the call widens an existing replication group).
//!
//! The keyspace's own status is the other half of the same rule and is pinned by
//! `super::update_keyspace`; the sibling keyspace-wide write answers its own question about
//! the keyspace's resources in `super::delete_keyspace_in_flight_resources`.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    TABLE_STATUS_VALUES, TYPE_STATUS_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, Emulator, REGION,
};

/// The wire status and code this service answers an action that cannot be completed because
/// another one is already in progress with: the modelled `ConflictException`, on this RPC
/// service's client-error status
/// (`service_..._keyspacesservice::scaffolded::CLIENT_ERROR_STATUS`).
const CONFLICT_STATUS: u16 = 400;
const CONFLICT_CODE: &str = "ConflictException";

/// Regions other than the request's own ([`REGION`]), each a member of the Region catalog
/// `emulator_telemetry::policy::KNOWN_REGIONS` that `keyspaces::is_aws_region` checks a
/// `regionList` entry against, and each inside `region`'s modelled length (2..25).
const OTHER_REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// A `MULTI_REGION` replication specification over `regions`.
fn multi_region(regions: &[&str]) -> Value {
    json!({"replicationStrategy": "MULTI_REGION", "regionList": regions})
}

/// The `clientSideTimestamps` member as a request carries it: `ClientSideTimestampsStatus`
/// models exactly one value, `ENABLED`
/// (`service_..._keyspacesservice::ops::types::CLIENT_SIDE_TIMESTAMPS_STATUS_VALUES`). Every
/// update below carries it, so a table that does not use client-side timestamps is never
/// what the request is refused for.
fn client_side_timestamps() -> Value {
    json!({"status": "ENABLED"})
}

/// A `CreateTable` body, minus `keyspaceName`.
fn create_table(table: &'static str) -> (&'static str, Value) {
    (
        "CreateTable",
        json!({"tableName": table, "schemaDefinition": minimal_schema("pk")}),
    )
}

/// A `CreateType` body, minus `keyspaceName`.
fn create_type(type_name: &'static str) -> (&'static str, Value) {
    (
        "CreateType",
        json!({
            "typeName": type_name,
            "fieldDefinitions": [{"name": "value", "type": "int"}],
        }),
    )
}

/// Which resource of the keyspace is unsettled when `UpdateKeyspace` is called.
#[derive(Clone, Copy)]
enum Resource {
    /// A table of the keyspace, by name.
    Table(&'static str),
    /// A user-defined type of the keyspace, by its formatted (lower-case) name.
    Type(&'static str),
}

impl Resource {
    /// The resource's own name, which the refusal is expected to name.
    fn name(self) -> &'static str {
        match self {
            Self::Table(table) => table,
            Self::Type(type_name) => type_name,
        }
    }

    /// The operation that reports this resource's status, and its request minus
    /// `keyspaceName`.
    fn read(self) -> (&'static str, Value) {
        match self {
            Self::Table(table) => ("GetTable", json!({"tableName": table})),
            Self::Type(type_name) => ("GetType", json!({"typeName": type_name})),
        }
    }

    /// The model's own value set for this resource's status member, so a row claiming a
    /// status the model does not declare fails here.
    fn modelled_statuses(self) -> &'static [&'static str] {
        match self {
            Self::Table(_) => TABLE_STATUS_VALUES,
            Self::Type(_) => TYPE_STATUS_VALUES,
        }
    }
}

/// One row: a keyspace with exactly one resource left unsettled when the addition is called.
struct Row {
    /// The case name, which is also the keyspace name (inside the modelled `keyspaceName`
    /// pattern `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`).
    case: &'static str,
    /// The `replicationSpecification` the keyspace is created with, if any.
    create: Option<Value>,
    /// The resource that is unsettled when `UpdateKeyspace` is called.
    resource: Resource,
    /// The status that resource is in then.
    status: &'static str,
    /// The setup, step by step: the keyspace has settled before the first step, the runner
    /// waits out the previous step's transitions before each later one, and the LAST step is
    /// what leaves `resource` unsettled.
    setup: Vec<Vec<(&'static str, Value)>>,
    /// Names of resources that HAVE settled, which the refusal must therefore not name.
    settled: &'static [&'static str],
    /// The `regionList` the `UpdateKeyspace` request carries, which is also the Region list
    /// `GetKeyspace` must report once the call is accepted.
    request: &'static [&'static str],
}

/// The rows: every way one resource of a keyspace is left unsettled here, over both
/// replication strategies a keyspace can already have.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "table_creating",
            create: None,
            resource: Resource::Table("pending_tbl"),
            status: keyspaces::TABLE_STATUS_CREATING,
            setup: vec![vec![create_table("pending_tbl")]],
            settled: &[],
            request: &[REGION, OTHER_REGIONS[0]],
        },
        Row {
            case: "table_updating",
            create: None,
            resource: Resource::Table("pending_tbl"),
            status: keyspaces::TABLE_STATUS_UPDATING,
            setup: vec![
                vec![create_table("pending_tbl")],
                vec![(
                    "UpdateTable",
                    json!({
                        "tableName": "pending_tbl",
                        "addColumns": [{"name": "extra", "type": "int"}],
                    }),
                )],
            ],
            settled: &[],
            request: &[REGION, OTHER_REGIONS[0]],
        },
        Row {
            // A resource on its way out is not settled either: it is still a resource of
            // the keyspace until the clock removes it.
            case: "table_deleting",
            create: None,
            resource: Resource::Table("pending_tbl"),
            status: keyspaces::TABLE_STATUS_DELETING,
            setup: vec![
                vec![create_table("pending_tbl")],
                vec![("DeleteTable", json!({"tableName": "pending_tbl"}))],
            ],
            settled: &[],
            request: &[REGION, OTHER_REGIONS[0]],
        },
        Row {
            case: "type_creating",
            create: None,
            resource: Resource::Type("pending_type"),
            status: keyspaces::TYPE_STATUS_CREATING,
            setup: vec![vec![create_type("pending_type")]],
            settled: &[],
            request: &[REGION, OTHER_REGIONS[0]],
        },
        Row {
            case: "type_deleting",
            create: None,
            resource: Resource::Type("pending_type"),
            status: keyspaces::TYPE_STATUS_DELETING,
            setup: vec![
                vec![create_type("pending_type")],
                vec![("DeleteType", json!({"typeName": "pending_type"}))],
            ],
            settled: &[],
            request: &[REGION, OTHER_REGIONS[0]],
        },
        Row {
            // One resource unsettled beside resources of both kinds that have settled: the
            // settled ones block nothing, and the refusal names the one that does.
            case: "type_creating_beside_others",
            create: None,
            resource: Resource::Type("pending_type"),
            status: keyspaces::TYPE_STATUS_CREATING,
            setup: vec![
                vec![create_table("done_tbl"), create_type("done_type")],
                vec![create_type("pending_type")],
            ],
            settled: &["done_tbl", "done_type"],
            request: &[REGION, OTHER_REGIONS[0]],
        },
        Row {
            // A keyspace that is already replicated in two Regions and is being widened to
            // a third: the same rule decides the addition.
            case: "type_creating_in_multi_region_keyspace",
            create: Some(multi_region(&[REGION, OTHER_REGIONS[0]])),
            resource: Resource::Type("pending_type"),
            status: keyspaces::TYPE_STATUS_CREATING,
            setup: vec![vec![create_type("pending_type")]],
            settled: &[],
            request: &[REGION, OTHER_REGIONS[0], OTHER_REGIONS[1]],
        },
    ]
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

/// The rule: a keyspace is replicated to a further Region only while every table and every
/// user-defined type it holds is `ACTIVE`; until then the addition is the modelled conflict
/// and nothing is written, and once the clock has settled that resource the same request is
/// accepted.
#[tokio::test]
async fn update_keyspace_is_refused_while_a_resource_of_the_keyspace_is_not_active() {
    for row in rows() {
        let case = row.case;
        // The row's status is one the model declares for that resource, not an invention.
        assert!(
            row.resource.modelled_statuses().contains(&row.status),
            "case {case:?}: {} is not a modelled status ({:?})",
            row.status,
            row.resource.modelled_statuses()
        );
        for region in row.request {
            assert!(
                keyspaces::is_aws_region(region),
                "case {case:?}: {region:?} is not an Amazon Web Services Region"
            );
        }

        let emulator = Emulator::start();
        let keyspace = row.case;
        let mut create = json!({"keyspaceName": keyspace});
        if let Some(specification) = &row.create {
            create["replicationSpecification"] = specification.clone();
        }
        assert_ok(case, emulator.call("CreateKeyspace", create).await);
        // A resource is only added to a keyspace that has settled.
        tokio::time::sleep(settle_wait()).await;

        for (index, step) in row.setup.iter().enumerate() {
            if index > 0 {
                tokio::time::sleep(settle_wait()).await;
            }
            for (operation, body) in step {
                let mut request = body.clone();
                request["keyspaceName"] = json!(keyspace);
                assert_ok(case, emulator.call(operation, request).await);
            }
        }

        // The row's resource really is in that status: a refusal from a keyspace whose
        // resources had all settled would prove nothing.
        let (read_operation, mut read_request) = row.resource.read();
        read_request["keyspaceName"] = json!(keyspace);
        let reported = assert_ok(case, emulator.call(read_operation, read_request).await);
        assert_eq!(
            reported["status"],
            json!(row.status),
            "case {case:?}: expected {} to be {} when the addition is called, got {reported}",
            row.resource.name(),
            row.status
        );

        // What the keyspace looks like before the refused call, so "nothing was written" is
        // checked against the whole report rather than one member of it.
        let before = get_keyspace(&emulator, case, keyspace).await;

        let update = || {
            json!({
                "keyspaceName": keyspace,
                "replicationSpecification": multi_region(row.request),
                "clientSideTimestamps": client_side_timestamps(),
            })
        };
        let (status, body) = emulator.call("UpdateKeyspace", update()).await;
        assert_failure(case, (status, body.clone()), CONFLICT_STATUS, CONFLICT_CODE);
        // The refusal says what the caller is waiting for, which is the only thing it can
        // act on. The text is not documented, so only what it names is asserted.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(keyspace)
                && message.contains(row.resource.name())
                && message.contains(row.status),
            "case {case:?}: expected the refusal to name {keyspace:?}, {:?} and {}, got {body}",
            row.resource.name(),
            row.status
        );
        for settled in row.settled {
            assert!(
                !message.contains(settled),
                "case {case:?}: a settled resource {settled:?} blocks nothing, got {body}"
            );
        }

        // A refused addition is not a half-performed one: the keyspace is exactly as it
        // was, so no later read reports a replication group this call did not make.
        let after = get_keyspace(&emulator, case, keyspace).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused addition changed the keyspace, got {after} instead of \
             {before}"
        );

        // Once the clock owes the resource its transition the same request is accepted, and
        // the keyspace is replicated over exactly the Regions the request named — so the
        // refusal above was the unsettled resource, not a keyspace that takes no addition.
        tokio::time::sleep(settle_wait()).await;
        assert_ok(case, emulator.call("UpdateKeyspace", update()).await);
        let widened = get_keyspace(&emulator, case, keyspace).await;
        assert_eq!(
            widened["replicationStrategy"],
            json!(keyspaces::MULTI_REGION_STRATEGY),
            "case {case:?}: an accepted addition makes the keyspace {}, got {widened}",
            keyspaces::MULTI_REGION_STRATEGY
        );
        assert_eq!(
            widened["replicationRegions"],
            json!(row.request),
            "case {case:?}: expected the keyspace to be replicated in {:?}, got {widened}",
            row.request
        );
    }
}
