// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateIdNamespace`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `PUT /idnamespaces/{idNamespaceName}`, through the harness the sibling
//! `schema_mapping_workflow_lifecycle` suite owns — so what is asserted is what an AWS
//! SDK client would see, not an internal function's return value.
//!
//! The rule under test: `UpdateIdNamespace` REPLACES the configuration members of the
//! ID namespace already persisted under the requested name and re-stamps its
//! `updatedAt`, while the resource's IDENTITY — the `idNamespaceName` key, the
//! `idNamespaceArn` minted at create time, the `type`, the `createdAt` stamp — and its
//! tags, none of which the request carries, survive unchanged; a name that is not
//! stored is refused with `ResourceNotFoundException` (HTTP 404) and persists nothing.
//! Because the request carries the WHOLE configuration, an optional member it omits is
//! cleared, not kept.
//!
//! Each table row differs along what the rule generalises over: which optional members
//! the configuration carries (`description`, `roleArn`, `inputSourceConfig`,
//! `idMappingWorkflowProperties`) over absent and present in BOTH directions, the
//! `inputSourceConfig` size over the modelled `length 0..20` minimum, middle values and
//! the maximum, the whole modelled `IdMappingType` catalog (`RULE_BASED`, `PROVIDER`) in
//! both directions, the whole modelled `IdNamespaceType` catalog (`SOURCE`, `TARGET`) as
//! the identity the update must not touch, whether the namespace carries tags, and the
//! `idNamespaceName` over the character classes the modelled `EntityName` pattern
//! `^[a-zA-Z_0-9-]*$` admits at its `length` minimum of 1 and maximum of 255. The
//! recorded create/get/update round trip is ONE row of these.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use crate::id_namespace_lifecycle::{
    input_sources, optional_member, provider_properties, rule_based_properties,
};
use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape as assert_error, expected_arn, role_arn, string_member, timestamp_member,
    Emulator, Reply,
};

/// The members `UpdateIdNamespace` carries on BOTH its input and its output, i.e. the
/// namespace's configuration: exactly what the update replaces
/// (`smithy_query(query="operation",operation="UpdateIdNamespace")`). `idNamespaceName`
/// is excluded because it is the `@httpLabel` that names the resource rather than part
/// of the configuration, and so are the identity members the request cannot carry
/// (`idNamespaceArn`, `type`, `createdAt`, `updatedAt`).
const CONFIG_MEMBERS: &[&str] = &[
    "description",
    "inputSourceConfig",
    "idMappingWorkflowProperties",
    "roleArn",
];

/// Long enough that the millisecond-resolution `updatedAt` stamp of a later write is
/// strictly greater than an earlier one, so a re-stamp is observable on the wire.
///
/// Slept with `std::thread::sleep` rather than `tokio::time::sleep`, because the
/// workspace's `tokio` carries only the `macros`/`rt`/`rt-multi-thread` features and
/// `tokio::time` is gated behind `time`: a test-only pause is not a reason to widen a
/// runtime dependency's feature set. Each test has its own runtime and no concurrent
/// task, so blocking it for five milliseconds starves nothing.
const A_LATER_MILLISECOND: Duration = Duration::from_millis(5);

/// One ID namespace, the request it is created with, and the `UpdateIdNamespace` body
/// that replaces its configuration.
struct UpdateCase {
    case: &'static str,
    name: String,
    /// `type` — @required on the create, and NOT a member of the update.
    namespace_type: &'static str,
    /// The `CreateIdNamespace` body, carrying `idNamespaceName` and `type`.
    create: Value,
    /// The `UpdateIdNamespace` body: `idNamespaceName` is the `@httpLabel`, so it is
    /// not a body member.
    update: Value,
}

/// The `UpdateIdNamespace` table.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "every optional member present, replaced by a request that carries none",
            name: "Customers".to_string(),
            namespace_type: "SOURCE",
            create: json!({
                "idNamespaceName": "Customers",
                "type": "SOURCE",
                "description": "the CRM people namespace",
                "roleArn": role_arn(),
                "inputSourceConfig": input_sources(1),
                "idMappingWorkflowProperties": [rule_based_properties(
                    1,
                    "ONE_TO_ONE",
                    "ONE_SOURCE_TO_ONE_TARGET",
                )],
                "tags": { "team": "data", "env": "test" },
            }),
            // Every configuration member is optional, so the whole-configuration PUT
            // may legitimately carry none of them.
            update: json!({}),
        },
        UpdateCase {
            case: "no optional member at all, replaced by every one at its modelled \
                   maximum (description 255, inputSourceConfig 20)",
            name: "a".to_string(),
            namespace_type: "TARGET",
            create: json!({ "idNamespaceName": "a", "type": "TARGET" }),
            update: json!({
                "description": "d".repeat(255),
                "roleArn": role_arn(),
                "inputSourceConfig": input_sources(20),
                "idMappingWorkflowProperties": [provider_properties()],
            }),
        },
        UpdateCase {
            case: "RULE_BASED properties replaced by PROVIDER properties, three input \
                   sources shrinking to the modelled minimum of zero",
            name: "crm_people-1".to_string(),
            namespace_type: "SOURCE",
            create: json!({
                "idNamespaceName": "crm_people-1",
                "type": "SOURCE",
                "roleArn": role_arn(),
                "inputSourceConfig": input_sources(3),
                "idMappingWorkflowProperties": [rule_based_properties(
                    2,
                    "ONE_TO_ONE",
                    "ONE_SOURCE_TO_ONE_TARGET",
                )],
            }),
            update: json!({
                "description": "",
                "inputSourceConfig": input_sources(0),
                "idMappingWorkflowProperties": [provider_properties()],
            }),
        },
        UpdateCase {
            case: "PROVIDER properties replaced by the many-to-many RULE_BASED models, \
                   on a name at the modelled EntityName length maximum of 255",
            name: "n".repeat(255),
            namespace_type: "TARGET",
            create: json!({
                "idNamespaceName": "n".repeat(255),
                "type": "TARGET",
                "description": "before",
                "inputSourceConfig": input_sources(2),
                "idMappingWorkflowProperties": [provider_properties()],
                "tags": { "only": "" },
            }),
            update: json!({
                "roleArn": role_arn(),
                "inputSourceConfig": input_sources(5),
                "idMappingWorkflowProperties": [rule_based_properties(
                    25,
                    "MANY_TO_MANY",
                    "MANY_SOURCE_TO_ONE_TARGET",
                )],
            }),
        },
        UpdateCase {
            case: "a digits-only name whose description alone is replaced",
            name: "0".to_string(),
            namespace_type: "SOURCE",
            create: json!({
                "idNamespaceName": "0",
                "type": "SOURCE",
                "description": "before",
                "roleArn": role_arn(),
            }),
            update: json!({ "description": "after" }),
        },
        UpdateCase {
            case: "an UPPER_lower-123 name gaining an inputSourceConfig and losing its \
                   roleArn",
            name: "UPPER_lower-123".to_string(),
            namespace_type: "TARGET",
            create: json!({
                "idNamespaceName": "UPPER_lower-123",
                "type": "TARGET",
                "roleArn": role_arn(),
                "tags": { "env": "test" },
            }),
            update: json!({
                "description": "gained a source",
                "inputSourceConfig": input_sources(4),
            }),
        },
    ]
}

/// Assert `body` carries exactly the configuration `update` asked for: a member the
/// request carried comes back as it was sent, and one it omitted is ABSENT, because the
/// whole configuration is replaced rather than merged.
fn assert_configuration(case: &str, from: &str, update: &Value, body: &Value) {
    for member in CONFIG_MEMBERS {
        assert_eq!(
            optional_member(body, member),
            update.get(*member).cloned(),
            "case {case:?}: {from} must answer the updated {member}, expected {:?}, \
             got {body:?}",
            update.get(*member)
        );
    }
}

/// Create the namespace `case` describes, and answer the created record.
async fn seed(emulator: &Emulator, case: &UpdateCase) -> Reply {
    let created = emulator.create_id_namespace(&case.create).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}",
        case.case
    );
    created
}

#[tokio::test]
async fn an_update_replaces_the_stored_configuration_and_keeps_the_resources_identity() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let label = case.case;
        let name = &case.name;
        let before = seed(&emulator, &case).await;

        sleep(A_LATER_MILLISECOND);
        let updated = emulator.update_id_namespace(name, &case.update).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateIdNamespace, got {updated:?}"
        );
        assert_configuration(label, "UpdateIdNamespace", &case.update, &updated.body);

        // The round trip: the reader answers what the update persisted, not what the
        // create did.
        let after = emulator.get_id_namespace(name).await;
        assert_eq!(
            after.status, 200,
            "case {label:?}: expected HTTP 200 from GetIdNamespace, got {after:?}"
        );
        assert_configuration(label, "GetIdNamespace", &case.update, &after.body);

        // Identity is not carried by the update request, so it survives it.
        for (member, expected) in [
            ("idNamespaceName", name.clone()),
            ("idNamespaceArn", expected_arn("idnamespace", name)),
            ("type", case.namespace_type.to_string()),
        ] {
            assert_eq!(
                string_member(&updated.body, member),
                expected,
                "case {label:?}: the update must not change {member}, got {updated:?}"
            );
            assert_eq!(
                string_member(&after.body, member),
                expected,
                "case {label:?}: the stored {member} must stay {expected:?}, got {after:?}"
            );
        }
        let created_at = timestamp_member(&before.body, "createdAt");
        assert!(
            (timestamp_member(&after.body, "createdAt") - created_at).abs() < f64::EPSILON,
            "case {label:?}: expected createdAt to stay {created_at}, got {:?}",
            after.body
        );
        // Tags are not a member of this request (nor of its output), so they stay as
        // the create and `TagResource` left them.
        assert_eq!(
            after.body.get("tags"),
            before.body.get("tags"),
            "case {label:?}: the update carries no tags, so it must not change them, \
             got {after:?}"
        );

        // `updatedAt` is re-stamped by the write, and never precedes `createdAt`.
        let was_updated_at = timestamp_member(&before.body, "updatedAt");
        let now_updated_at = timestamp_member(&updated.body, "updatedAt");
        assert!(
            now_updated_at > was_updated_at,
            "case {label:?}: expected updatedAt to advance past {was_updated_at}, \
             got {now_updated_at}"
        );
        assert!(
            now_updated_at >= created_at,
            "case {label:?}: expected updatedAt {now_updated_at} not to precede \
             createdAt {created_at}"
        );
        assert!(
            (timestamp_member(&after.body, "updatedAt") - now_updated_at).abs() < f64::EPSILON,
            "case {label:?}: the reader must answer the stamp the update stored \
             ({now_updated_at}), got {:?}",
            after.body
        );
    }
}

#[tokio::test]
async fn updating_a_namespace_that_does_not_exist_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let label = case.case;
        let name = &case.name;

        // Only the HTTP 404 and the `ResourceNotFoundException` code are requirements
        // here: AWS documents no text for this refusal, so the message the service
        // answers is an example and is deliberately not asserted.
        let refused = emulator.update_id_namespace(name, &case.update).await;
        assert_error(label, &refused, 404, "ResourceNotFoundException");

        // The refusal wrote nothing: an update is not a create.
        let read = emulator.get_id_namespace(name).await;
        assert_error(label, &read, 404, "ResourceNotFoundException");

        // ...and once the namespace exists, the SAME request succeeds, so the 404 is
        // decided by what the store holds and not by the request's shape.
        seed(&emulator, &case).await;
        let updated = emulator.update_id_namespace(name, &case.update).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 once the namespace exists, got {updated:?}"
        );
    }
}

#[tokio::test]
async fn an_update_touches_only_the_namespace_it_names() {
    let emulator = Emulator::new();
    let cases = update_cases();

    for case in &cases {
        seed(&emulator, case).await;
    }

    for case in &cases {
        let label = case.case;
        let mut untouched = Vec::new();
        for other in cases.iter().filter(|other| other.name != case.name) {
            let read = emulator.get_id_namespace(&other.name).await;
            untouched.push((other.name.clone(), read.body));
        }

        sleep(A_LATER_MILLISECOND);
        let updated = emulator.update_id_namespace(&case.name, &case.update).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateIdNamespace, got {updated:?}"
        );

        for (name, body) in untouched {
            let read = emulator.get_id_namespace(&name).await;
            assert_eq!(
                read.body, body,
                "case {label:?}: updating {:?} must leave {name:?} exactly as it was, \
                 got {:?} after {body:?}",
                case.name, read.body
            );
        }
    }

    // Every namespace is still its own record, reachable under its own ARN.
    let arns: BTreeSet<String> = cases
        .iter()
        .map(|case| expected_arn("idnamespace", &case.name))
        .collect();
    assert_eq!(
        arns.len(),
        cases.len(),
        "each namespace must keep its own ARN, saw {arns:?}"
    );
}

#[tokio::test]
async fn applying_the_same_update_twice_leaves_the_same_configuration() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let label = case.case;
        let name = &case.name;
        seed(&emulator, &case).await;

        let first = emulator.update_id_namespace(name, &case.update).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateIdNamespace, got {first:?}"
        );
        let once = emulator.get_id_namespace(name).await;

        sleep(A_LATER_MILLISECOND);
        let second = emulator.update_id_namespace(name, &case.update).await;
        assert_eq!(
            second.status, 200,
            "case {label:?}: expected HTTP 200 from a repeated UpdateIdNamespace, \
             got {second:?}"
        );
        let twice = emulator.get_id_namespace(name).await;

        // `UpdateIdNamespace` is `@idempotent`: the same request applied again leaves
        // the same configuration, the same identity and the same tags behind.
        assert_configuration(label, "a repeated UpdateIdNamespace", &case.update, &twice.body);
        for member in ["idNamespaceName", "idNamespaceArn", "type"] {
            assert_eq!(
                string_member(&twice.body, member),
                string_member(&once.body, member),
                "case {label:?}: a repeated update must not change {member}, got {:?}",
                twice.body
            );
        }
        assert_eq!(
            twice.body.get("tags"),
            once.body.get("tags"),
            "case {label:?}: the tags must survive a repeated update, got {:?}",
            twice.body
        );
        let created_at = timestamp_member(&once.body, "createdAt");
        assert!(
            (timestamp_member(&twice.body, "createdAt") - created_at).abs() < f64::EPSILON,
            "case {label:?}: expected createdAt to stay {created_at}, got {:?}",
            twice.body
        );
        assert!(
            timestamp_member(&twice.body, "updatedAt") >= timestamp_member(&once.body, "updatedAt"),
            "case {label:?}: expected updatedAt not to go backwards, got {:?}",
            twice.body
        );
    }
}

#[tokio::test]
async fn the_update_label_is_a_name_so_an_arn_names_no_namespace() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let label = case.case;
        let created = seed(&emulator, &case).await;

        // `UpdateIdNamespace`'s `idNamespaceName` is an `EntityName`
        // (`^[a-zA-Z_0-9-]*$`), not the `EntityNameOrIdNamespaceArn` `GetIdNamespace`
        // takes, so an ARN — whose `:` and `/` the pattern does not admit — is refused
        // at the boundary with the modelled `ValidationException` (HTTP 400).
        let arn = string_member(&created.body, "idNamespaceArn");
        let refused = emulator
            .update_id_namespace(&arn.replace('/', "%2F"), &case.update)
            .await;
        assert_error(label, &refused, 400, "ValidationException");

        // The refused call wrote nothing: the namespace still reads as it was created.
        let read = emulator.get_id_namespace(&case.name).await;
        assert_eq!(
            read.body, created.body,
            "case {label:?}: the refused update must leave the stored record alone, \
             got {:?} after {:?}",
            read.body, created.body
        );
    }
}
