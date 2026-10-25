// SPDX-License-Identifier: CC0-1.0

//! `DeleteKeyspace` against a keyspace that still holds a resource of its own whose
//! change is in flight, driven end to end over the service's `awsJson1_0` wire protocol.
//!
//! The rule under test: a delete that drops a container drops everything inside it
//! ("The DeleteKeyspace operation deletes a keyspace and all of its tables"), so it is
//! refused — with the `ConflictException` the model declares on this operation, "you try
//! to perform an action and the same or a different action is already in progress"
//! (HTTP 409) — for as long as ANY resource it would drop still owes the clock a
//! transition towards a usable status, which is exactly the state in which dropping that
//! resource on its own is refused; and a refused delete writes nothing, in this service's
//! state or in the Application Auto Scaling resources the accepted one retires.
//!
//! So the two deletes agree about the same resource: `DeleteTable` refuses a table that is
//! `CREATING` or `UPDATING` ("then Amazon Keyspaces returns a ResourceInUseException") and
//! `DeleteType` refuses a type that is not yet `ACTIVE`, and neither resource can be
//! dropped through its keyspace one call later either.
//!
//! Each assertion below spans a SET of inputs rather than one example:
//!
//! 1. Every way a resource of a keyspace can be mid-change here — which resource it is (a
//!    table, a type), the create that puts it in `CREATING`, each kind of update that puts
//!    a table in `UPDATING`, and whether settled resources sit beside it — is refused the
//!    same way, the resource's own delete is refused in the same breath, and nothing moves:
//!    the resource settles into its usable status rather than into absence, after which the
//!    same `DeleteKeyspace` is accepted.
//! 2. The Application Auto Scaling settings of a table whose creation is still in flight
//!    survive the refused delete unchanged, and go only with the accepted one.
//!
//! The other half of the rule — a resource that is already `DELETING` is on its way out
//! and blocks nothing — is pinned by `super::delete_keyspace_lifecycle`.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    TABLE_STATUS_VALUES, TYPE_STATUS_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::column_types_and_auto_scaling::assert_member;
use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, Emulator,
};

/// The wire status and code the model declares for an action that cannot be completed
/// because another one is already in progress (`ConflictException`).
const CONFLICT_STATUS: u16 = 409;
const CONFLICT_CODE: &str = "ConflictException";

/// The wire status and code `DeleteTable`'s own documentation names for a table that is
/// `CREATING` or `UPDATING`.
const IN_USE_STATUS: u16 = 400;
const IN_USE_CODE: &str = "ResourceInUseException";

/// Which resource of the keyspace is mid-change when the deletes are called.
#[derive(Clone, Copy)]
enum Resource {
    /// A table of the keyspace, by name.
    Table(&'static str),
    /// A user-defined type of the keyspace, by name.
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

    /// The operation that reports this resource's status, and its request.
    fn read(self, keyspace: &str) -> (&'static str, Value) {
        match self {
            Self::Table(table) => (
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            ),
            Self::Type(type_name) => (
                "GetType",
                json!({"keyspaceName": keyspace, "typeName": type_name}),
            ),
        }
    }

    /// This resource's OWN delete: the operation, its request, and the refusal that
    /// operation answers while the resource is mid-change.
    fn delete(self, keyspace: &str) -> (&'static str, Value, (u16, &'static str)) {
        match self {
            // "If a table is either in the CREATING or UPDATING states, then Amazon
            // Keyspaces returns a ResourceInUseException."
            Self::Table(table) => (
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
                (IN_USE_STATUS, IN_USE_CODE),
            ),
            // A type whose own action is still in progress is this model's
            // `ConflictException`.
            Self::Type(type_name) => (
                "DeleteType",
                json!({"keyspaceName": keyspace, "typeName": type_name}),
                (CONFLICT_STATUS, CONFLICT_CODE),
            ),
        }
    }

    /// The status the clock owes this resource once its change has settled.
    fn settled_status(self) -> &'static str {
        match self {
            Self::Table(_) => keyspaces::TABLE_STATUS_ACTIVE,
            Self::Type(_) => keyspaces::TYPE_STATUS_ACTIVE,
        }
    }

    /// The status an accepted `DeleteKeyspace` puts this resource in, since it goes with
    /// the keyspace.
    fn deleting_status(self) -> &'static str {
        match self {
            Self::Table(_) => keyspaces::TABLE_STATUS_DELETING,
            Self::Type(_) => keyspaces::TYPE_STATUS_DELETING,
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

/// One step of a row's setup: the calls to make together, each body carrying everything
/// but `keyspaceName`, which the runner fills in per keyspace.
type Step = Vec<(&'static str, Value)>;

/// One row: a keyspace shape with exactly one resource left mid-change.
struct Row {
    /// The case name, which also names the two keyspaces the row drives.
    case: &'static str,
    /// The resource that is mid-change when the deletes are called.
    resource: Resource,
    /// The setup, step by step: the keyspace has settled before the first step, the runner
    /// waits out the previous step's transitions before each later one, and the LAST step
    /// is what leaves `resource` in flight.
    setup: fn() -> Vec<Step>,
    /// The status that resource is in when the deletes are called.
    status: &'static str,
}

/// A `CreateTable` body, minus `keyspaceName`.
fn create_table(table: &'static str) -> (&'static str, Value) {
    (
        "CreateTable",
        json!({"tableName": table, "schemaDefinition": minimal_schema("pk")}),
    )
}

/// An `UpdateTable` body, minus `keyspaceName`, carrying `change`.
fn update_table(table: &'static str, change: Value) -> (&'static str, Value) {
    let mut request = change;
    request["tableName"] = json!(table);
    ("UpdateTable", request)
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

/// The rows: every way a resource of a keyspace is mid-change here — each kind of resource,
/// the create and each kind of update, and a keyspace whose other resources have settled.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "table_creating",
            resource: Resource::Table("pending_tbl"),
            setup: || vec![vec![create_table("pending_tbl")]],
            status: keyspaces::TABLE_STATUS_CREATING,
        },
        Row {
            // A create carrying every optional setting: still `CREATING`, still blocking.
            case: "table_creating_fully_specified",
            resource: Resource::Table("pending_tbl"),
            setup: || {
                vec![vec![(
                    "CreateTable",
                    json!({
                        "tableName": "pending_tbl",
                        "schemaDefinition": {
                            "allColumns": [
                                {"name": "pk", "type": "text"},
                                {"name": "ck", "type": "timestamp"},
                                {"name": "payload", "type": "blob"},
                            ],
                            "partitionKeys": [{"name": "pk"}],
                            "clusteringKeys": [{"name": "ck", "orderBy": "ASC"}],
                        },
                        "comment": {"message": "in flight"},
                        "pointInTimeRecovery": {"status": "ENABLED"},
                        "defaultTimeToLive": 600,
                    }),
                )]]
            },
            status: keyspaces::TABLE_STATUS_CREATING,
        },
        Row {
            case: "table_updating_capacity",
            resource: Resource::Table("pending_tbl"),
            setup: || {
                vec![
                    vec![create_table("pending_tbl")],
                    vec![update_table(
                        "pending_tbl",
                        json!({
                            "capacitySpecification": {
                                "throughputMode": "PROVISIONED",
                                "readCapacityUnits": 5,
                                "writeCapacityUnits": 2,
                            }
                        }),
                    )],
                ]
            },
            status: keyspaces::TABLE_STATUS_UPDATING,
        },
        Row {
            case: "table_updating_added_columns",
            resource: Resource::Table("pending_tbl"),
            setup: || {
                vec![
                    vec![create_table("pending_tbl")],
                    vec![update_table(
                        "pending_tbl",
                        json!({"addColumns": [{"name": "extra", "type": "int"}]}),
                    )],
                ]
            },
            status: keyspaces::TABLE_STATUS_UPDATING,
        },
        Row {
            case: "type_creating",
            resource: Resource::Type("pending_type"),
            setup: || vec![vec![create_type("pending_type")]],
            status: keyspaces::TYPE_STATUS_CREATING,
        },
        Row {
            // One resource in flight beside resources that have settled: the settled ones
            // block nothing, and the refusal names the one that does.
            case: "table_creating_beside_settled_resources",
            resource: Resource::Table("pending_tbl"),
            setup: || {
                vec![
                    vec![create_table("settled_tbl"), create_type("settled_type")],
                    vec![create_table("pending_tbl")],
                ]
            },
            status: keyspaces::TABLE_STATUS_CREATING,
        },
    ]
}

/// The status the row's resource reports now, read back through the operation that
/// reports it.
async fn reported_status(
    emulator: &Emulator,
    case: &str,
    keyspace: &str,
    resource: Resource,
) -> String {
    let (operation, request) = resource.read(keyspace);
    let body = assert_ok(case, emulator.call(operation, request).await);
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {operation} reported no status: {body}"))
        .to_string()
}

/// RULE 1: a keyspace holding a resource whose own change is in flight is refused the
/// modelled conflict, exactly as that resource's own delete is, and the refusal writes
/// nothing.
#[tokio::test]
async fn delete_keyspace_is_refused_while_a_resource_it_would_drop_is_in_flight() {
    for row in rows() {
        let case = row.case;
        // The row's status is one the model declares for that resource, not an invention.
        assert!(
            row.resource.modelled_statuses().contains(&row.status),
            "case {case:?}: {} is not a modelled status ({:?})",
            row.status,
            row.resource.modelled_statuses()
        );

        // One emulator per row, and two identically seeded keyspaces in it: one the
        // resource's own delete is observed on, one `DeleteKeyspace` is called on, so
        // neither call is what the other observed.
        let emulator = Emulator::start();
        let own_keyspace = format!("{case}_own");
        let whole_keyspace = format!("{case}_whole");
        let keyspaces_under_test = [own_keyspace.clone(), whole_keyspace.clone()];
        for keyspace in &keyspaces_under_test {
            assert_ok(
                case,
                emulator
                    .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
                    .await,
            );
        }
        // A resource is only added to a keyspace that has settled.
        tokio::time::sleep(settle_wait()).await;

        // Both keyspaces are driven through each step TOGETHER, so their resources are in
        // flight at the same instant: neither settles while the other is being seeded.
        for (index, step) in (row.setup)().into_iter().enumerate() {
            if index > 0 {
                tokio::time::sleep(settle_wait()).await;
            }
            for (operation, body) in &step {
                for keyspace in &keyspaces_under_test {
                    let mut request = body.clone();
                    request["keyspaceName"] = json!(keyspace);
                    assert_ok(case, emulator.call(operation, request).await);
                }
            }
        }

        // The row really is mid-change: a refusal of a keyspace whose resources had all
        // settled would prove nothing.
        for keyspace in &keyspaces_under_test {
            assert_eq!(
                reported_status(&emulator, case, keyspace, row.resource).await,
                row.status,
                "case {case:?}: expected {} to be {} in {keyspace} when the deletes are called",
                row.resource.name(),
                row.status
            );
        }

        // The resource's own delete is refused, with the code that operation answers for a
        // resource of its own that is mid-change.
        let (operation, request, (expected_status, expected_code)) =
            row.resource.delete(&own_keyspace);
        assert_failure(
            case,
            emulator.call(operation, request).await,
            expected_status,
            expected_code,
        );

        // And so is the delete of the keyspace that holds it — the conflict this operation
        // models, never a 200 that drops the resource the call above could not.
        let (status, body) = emulator
            .call("DeleteKeyspace", json!({"keyspaceName": whole_keyspace}))
            .await;
        assert_failure(case, (status, body.clone()), CONFLICT_STATUS, CONFLICT_CODE);
        // The refusal says what the caller is waiting for, which is the only thing it can
        // act on. The text is not documented, so only what it names is asserted.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&whole_keyspace)
                && message.contains(row.resource.name())
                && message.contains(row.status),
            "case {case:?}: expected the refusal to name {whole_keyspace:?}, {:?} and {}, got \
             {body}",
            row.resource.name(),
            row.status
        );

        // A refused delete is not a half-performed one: the resource is still in that
        // status, and the change in flight settles into its usable status rather than into
        // absence.
        assert_eq!(
            reported_status(&emulator, case, &whole_keyspace, row.resource).await,
            row.status,
            "case {case:?}: the refused delete moved {}",
            row.resource.name()
        );
        tokio::time::sleep(settle_wait()).await;
        assert_eq!(
            reported_status(&emulator, case, &whole_keyspace, row.resource).await,
            row.resource.settled_status(),
            "case {case:?}: {} settles into {} rather than into absence",
            row.resource.name(),
            row.resource.settled_status()
        );

        // Once everything has settled the same request is accepted, and it takes the
        // resource with it.
        assert_ok(
            case,
            emulator
                .call("DeleteKeyspace", json!({"keyspaceName": whole_keyspace}))
                .await,
        );
        assert_eq!(
            reported_status(&emulator, case, &whole_keyspace, row.resource).await,
            row.resource.deleting_status(),
            "case {case:?}: an accepted delete puts {} in {}",
            row.resource.name(),
            row.resource.deleting_status()
        );
    }
}

/// RULE 2: the Application Auto Scaling settings of a table whose creation is still in
/// flight are untouched by the refused delete — a refused call retires nothing — and go
/// with the accepted one.
#[tokio::test]
async fn a_refused_delete_keyspace_retires_no_auto_scaling_settings() {
    let case = "auto scaling of a table whose creation is in flight";
    let emulator = Emulator::start();
    let keyspace = "scaled_inflight_ks";
    let table = "scaled_tbl";
    assert_ok(
        case,
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    // A provisioned table with both capacities scaled: the create registers the scalable
    // targets the settings ARE, while the table is still `CREATING`.
    assert_ok(
        case,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": minimal_schema("pk"),
                    "capacitySpecification": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 5,
                        "writeCapacityUnits": 5,
                    },
                    "autoScalingSpecification": {
                        "readCapacityAutoScaling": {
                            "autoScalingDisabled": false,
                            "minimumUnits": 5,
                            "maximumUnits": 50,
                            "scalingPolicy": {
                                "targetTrackingScalingPolicyConfiguration": {"targetValue": 60.0},
                            },
                        },
                        "writeCapacityAutoScaling": {
                            "autoScalingDisabled": false,
                            "minimumUnits": 2,
                            "maximumUnits": 20,
                            "scalingPolicy": {
                                "targetTrackingScalingPolicyConfiguration": {"targetValue": 40.0},
                            },
                        },
                    },
                }),
            )
            .await,
    );
    let scaling_request = json!({"keyspaceName": keyspace, "tableName": table});
    let before = assert_ok(
        case,
        emulator
            .call("GetTableAutoScalingSettings", scaling_request.clone())
            .await,
    );
    for (pointer, expected) in [
        ("/readCapacityAutoScaling/minimumUnits", json!(5)),
        ("/readCapacityAutoScaling/maximumUnits", json!(50)),
        ("/writeCapacityAutoScaling/minimumUnits", json!(2)),
        ("/writeCapacityAutoScaling/maximumUnits", json!(20)),
    ] {
        assert_member(
            case,
            &before,
            &format!("/autoScalingSpecification{pointer}"),
            &expected,
        );
    }

    // The delete is refused while the table's creation is in flight, so nothing it would
    // retire is retired.
    let (status, body) = emulator
        .call("DeleteKeyspace", json!({"keyspaceName": keyspace}))
        .await;
    assert_failure(case, (status, body), CONFLICT_STATUS, CONFLICT_CODE);
    let after = assert_ok(
        case,
        emulator
            .call("GetTableAutoScalingSettings", scaling_request.clone())
            .await,
    );
    assert_eq!(
        after, before,
        "case {case:?}: the refused delete changed the table's auto scaling settings, got \
         {after} instead of {before}"
    );

    // Settled, the delete is accepted — and the capacities the targets scaled go with the
    // table, so the settings are gone, while the table itself is still reported on its way
    // out.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        case,
        emulator
            .call("DeleteKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    let retired = assert_ok(
        case,
        emulator
            .call("GetTableAutoScalingSettings", scaling_request)
            .await,
    );
    assert!(
        retired.pointer("/autoScalingSpecification").is_none(),
        "case {case:?}: expected the retired settings to be gone, got {retired}"
    );
}
