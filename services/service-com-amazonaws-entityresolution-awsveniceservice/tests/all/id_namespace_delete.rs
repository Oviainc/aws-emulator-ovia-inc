// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteIdNamespace`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `DELETE /idnamespaces/{idNamespaceName}`, through the harness the sibling
//! `schema_mapping_workflow_lifecycle` suite owns — so what is asserted is what an AWS
//! SDK client would see, not an internal function's return value.
//!
//! The rule under test: `DeleteIdNamespace` REMOVES the ID namespace stored under the
//! requested name, so every reader of that resource stops serving it (`GetIdNamespace`
//! by name and by ARN, `ListTagsForResource` and `GetPolicy` on its ARN all answer
//! `ResourceNotFoundException`, HTTP 404), the name becomes creatable again as a fresh
//! record, and the room it held under the enforced "ID namespaces" quota comes back; a
//! name that is not stored is refused with `ResourceNotFoundException` (HTTP 404) and
//! changes nothing, which is also what a repeated delete answers.
//!
//! Each table row differs along what the rule generalises over: the whole modelled
//! `IdNamespaceType` catalog (`SOURCE`, `TARGET`), the whole modelled `IdMappingType`
//! catalog (`RULE_BASED`, `PROVIDER`) plus an absent `idMappingWorkflowProperties`,
//! which optional members the record carries (`description`, `roleArn`,
//! `inputSourceConfig`, `tags`) over absent and present, the `inputSourceConfig` size
//! over the modelled `length 0..20` minimum, middle values and the maximum, and the
//! `idNamespaceName` over the character classes the modelled `EntityName` pattern
//! `^[a-zA-Z_0-9-]*$` admits at its `length` minimum of 1 and maximum of 255. The
//! recorded create/get/update round trip's delete is ONE row of these.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::id_mapping_workflow_lifecycle::ID_NAMESPACE_TYPES;
use crate::id_namespace_lifecycle::{input_sources, provider_properties, rule_based_properties};
use crate::resource_policy::{get_policy, put_policy};
use crate::resource_tagging::list_tags;
use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape as assert_error, expected_arn, role_arn, string_member, timestamp_member,
    Emulator, Reply,
};

/// One ID namespace to create and then delete.
struct DeleteCase {
    case: &'static str,
    name: String,
    /// The `CreateIdNamespace` body for this row.
    create: Value,
}

/// The `DeleteIdNamespace` table.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "TARGET, no optional member at all, a single-letter name at the \
                   modelled EntityName length minimum of 1",
            name: "a".to_string(),
            create: json!({ "idNamespaceName": "a", "type": "TARGET" }),
        },
        DeleteCase {
            case: "SOURCE with every optional member, one input source, RULE_BASED \
                   properties and two tags",
            name: "Customers".to_string(),
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
        },
        DeleteCase {
            case: "SOURCE with PROVIDER properties and the modelled inputSourceConfig \
                   minimum of zero entries",
            name: "crm_people-1".to_string(),
            create: json!({
                "idNamespaceName": "crm_people-1",
                "type": "SOURCE",
                "roleArn": role_arn(),
                "inputSourceConfig": input_sources(0),
                "idMappingWorkflowProperties": [provider_properties()],
            }),
        },
        DeleteCase {
            case: "TARGET at the modelled inputSourceConfig maximum of 20 entries and \
                   the modelled description maximum of 255 characters",
            name: "UPPER_lower-123".to_string(),
            create: json!({
                "idNamespaceName": "UPPER_lower-123",
                "type": "TARGET",
                "description": "d".repeat(255),
                "roleArn": role_arn(),
                "inputSourceConfig": input_sources(20),
                "tags": { "only": "" },
            }),
        },
        DeleteCase {
            case: "a digits-only name, three input sources and the many-to-many \
                   RULE_BASED models",
            name: "0".to_string(),
            create: json!({
                "idNamespaceName": "0",
                "type": "SOURCE",
                "description": "",
                "inputSourceConfig": input_sources(3),
                "idMappingWorkflowProperties": [rule_based_properties(
                    25,
                    "MANY_TO_MANY",
                    "MANY_SOURCE_TO_ONE_TARGET",
                )],
            }),
        },
        DeleteCase {
            case: "a name at the modelled EntityName length maximum of 255",
            name: "n".repeat(255),
            create: json!({
                "idNamespaceName": "n".repeat(255),
                "type": "TARGET",
                "inputSourceConfig": input_sources(2),
                "idMappingWorkflowProperties": [provider_properties()],
            }),
        },
    ]
}

/// Create the namespace `case` describes, and answer the created record.
async fn seed(emulator: &Emulator, case: &DeleteCase) -> Reply {
    let created = emulator.create_id_namespace(&case.create).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}",
        case.case
    );
    created
}

/// The `message` of a successful delete: modelled @required, so it is always there.
fn success_message(case: &str, deleted: &Reply) -> String {
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected HTTP 200 from DeleteIdNamespace, got {deleted:?}"
    );
    let message = string_member(&deleted.body, "message");
    assert!(
        !message.is_empty(),
        "case {case:?}: `message` is modelled @required, got {:?}",
        deleted.body
    );
    message
}

#[tokio::test]
async fn a_deleted_namespace_is_gone_from_the_reader_and_a_repeated_delete_is_not_found() {
    let emulator = Emulator::new();

    for case in delete_cases() {
        let label = case.case;
        let name = &case.name;
        let created = seed(&emulator, &case).await;
        let arn = string_member(&created.body, "idNamespaceArn");

        // It is there before the delete, so what the reader answers afterwards is the
        // delete's doing.
        let before = emulator.get_id_namespace(name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: expected HTTP 200 from GetIdNamespace, got {before:?}"
        );

        let deleted = emulator.delete_id_namespace(name).await;
        success_message(label, &deleted);

        // Gone from the reader, by name and by the ARN that named the same record.
        let read = emulator.get_id_namespace(name).await;
        assert_error(label, &read, 404, "ResourceNotFoundException");
        let by_arn = emulator.get_id_namespace(&arn.replace('/', "%2F")).await;
        assert_error(label, &by_arn, 404, "ResourceNotFoundException");

        // Only the HTTP 404 and the `ResourceNotFoundException` code are requirements
        // for the repeat: AWS documents no text for this refusal, so the message the
        // service answers is an example and is deliberately not asserted.
        let again = emulator.delete_id_namespace(name).await;
        assert_error(label, &again, 404, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn deleting_a_namespace_that_was_never_created_is_not_found_and_creates_nothing() {
    let emulator = Emulator::new();

    for case in delete_cases() {
        let label = case.case;
        let name = &case.name;

        let refused = emulator.delete_id_namespace(name).await;
        assert_error(label, &refused, 404, "ResourceNotFoundException");

        // A refused delete is not a create: the name is still absent.
        let read = emulator.get_id_namespace(name).await;
        assert_error(label, &read, 404, "ResourceNotFoundException");

        // ...and once the namespace exists, the SAME request succeeds, so the 404 is
        // decided by what the store holds and not by the request's shape.
        seed(&emulator, &case).await;
        let deleted = emulator.delete_id_namespace(name).await;
        success_message(label, &deleted);
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_namespace_it_names() {
    let emulator = Emulator::new();
    let cases = delete_cases();

    for case in &cases {
        seed(&emulator, case).await;
    }

    let mut still_there: Vec<&DeleteCase> = cases.iter().collect();
    while let Some(case) = still_there.pop() {
        let label = case.case;
        let untouched: Vec<(String, Value)> = {
            let mut records = Vec::new();
            for other in &still_there {
                let read = emulator.get_id_namespace(&other.name).await;
                assert_eq!(
                    read.status, 200,
                    "case {label:?}: {:?} must still be there, got {read:?}",
                    other.name
                );
                records.push((other.name.clone(), read.body));
            }
            records
        };

        let deleted = emulator.delete_id_namespace(&case.name).await;
        success_message(label, &deleted);

        for (name, body) in untouched {
            let read = emulator.get_id_namespace(&name).await;
            assert_eq!(
                read.body, body,
                "case {label:?}: deleting {:?} must leave {name:?} exactly as it was, \
                 got {:?} after {body:?}",
                case.name, read.body
            );
        }
    }
}

#[tokio::test]
async fn the_success_message_carries_nothing_of_the_record_that_was_removed() {
    let emulator = Emulator::new();
    let mut seen = BTreeSet::new();

    for case in delete_cases() {
        let label = case.case;
        seed(&emulator, &case).await;
        let deleted = emulator.delete_id_namespace(&case.name).await;
        let message = success_message(label, &deleted);
        seen.insert(message);
    }

    assert_eq!(
        seen.len(),
        1,
        "the success message must not depend on the namespace that was deleted, saw {seen:?}"
    );
}

#[tokio::test]
async fn a_deleted_name_is_creatable_again_as_a_fresh_record() {
    let emulator = Emulator::new();

    for case in delete_cases() {
        let label = case.case;
        let name = &case.name;
        let first = seed(&emulator, &case).await;
        let deleted = emulator.delete_id_namespace(name).await;
        success_message(label, &deleted);

        // The name is free again, so the create that would have conflicted now goes
        // through — and the record it makes is its OWN: the deleted namespace's
        // description, role, sources, properties and tags are not inherited.
        let minimal = json!({ "idNamespaceName": name, "type": "TARGET" });
        let recreated = emulator.create_id_namespace(&minimal).await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: a deleted name must be creatable again, got {recreated:?}"
        );
        for member in [
            "description",
            "roleArn",
            "inputSourceConfig",
            "idMappingWorkflowProperties",
            "tags",
        ] {
            assert!(
                recreated.body.get(member).is_none(),
                "case {label:?}: the re-created namespace must not inherit {member}, \
                 got {:?}",
                recreated.body
            );
        }
        assert_eq!(
            string_member(&recreated.body, "type"),
            "TARGET",
            "case {label:?}: the re-created namespace must carry its OWN type, got {:?}",
            recreated.body
        );
        assert!(
            timestamp_member(&recreated.body, "createdAt")
                >= timestamp_member(&first.body, "createdAt"),
            "case {label:?}: the re-created namespace is a later incarnation, got {:?} \
             after {:?}",
            recreated.body,
            first.body
        );

        // ...and the reader serves the new record, not the deleted one.
        let read = emulator.get_id_namespace(name).await;
        assert_eq!(
            read.body, recreated.body,
            "case {label:?}: GetIdNamespace must answer the re-created record, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_room_a_delete_releases_comes_back_under_the_enforced_quota() {
    let emulator = Emulator::new();

    // The bound this emulator enforces for the published, adjustable per-account,
    // per-Region "ID namespaces" quota (`crate::state::MAX_ID_NAMESPACES`), pinned here
    // as a literal so the suite checks the behaviour and not the constant.
    const MAX_ID_NAMESPACES: usize = 100;

    for index in 0..MAX_ID_NAMESPACES {
        let created = emulator
            .create_id_namespace(&json!({
                "idNamespaceName": format!("quota-{index}"),
                "type": ID_NAMESPACE_TYPES[index % ID_NAMESPACE_TYPES.len()],
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "namespace {index} is inside the quota, so it must be created, got {created:?}"
        );
    }

    let refused = emulator
        .create_id_namespace(&json!({ "idNamespaceName": "one-too-many", "type": "TARGET" }))
        .await;
    assert_error("a create at the quota", &refused, 402, "ExceedsLimitException");

    // Deleting one releases exactly one slot: the next create goes through, and the
    // one after it is refused again.
    let deleted = emulator.delete_id_namespace("quota-7").await;
    success_message("a delete that releases quota room", &deleted);

    let admitted = emulator
        .create_id_namespace(&json!({ "idNamespaceName": "one-too-many", "type": "TARGET" }))
        .await;
    assert_eq!(
        admitted.status, 200,
        "the room the delete released must admit one create, got {admitted:?}"
    );
    let refused_again = emulator
        .create_id_namespace(&json!({ "idNamespaceName": "two-too-many", "type": "SOURCE" }))
        .await;
    assert_error(
        "a create past the refilled quota",
        &refused_again,
        402,
        "ExceedsLimitException",
    );
}

#[tokio::test]
async fn the_delete_label_is_a_name_so_an_arn_names_no_namespace() {
    let emulator = Emulator::new();

    for case in delete_cases() {
        let label = case.case;
        let created = seed(&emulator, &case).await;

        // `DeleteIdNamespace`'s `idNamespaceName` is an `EntityName` (`^[a-zA-Z_0-9-]*$`),
        // not the `EntityNameOrIdNamespaceArn` `GetIdNamespace` takes, so an ARN — whose
        // `:` and `/` the pattern does not admit — is refused at the boundary with the
        // modelled `ValidationException` (HTTP 400).
        let arn = string_member(&created.body, "idNamespaceArn");
        let refused = emulator
            .delete_id_namespace(&arn.replace('/', "%2F"))
            .await;
        assert_error(label, &refused, 400, "ValidationException");

        // The refused call removed nothing: the namespace still reads as it was created.
        let read = emulator.get_id_namespace(&case.name).await;
        assert_eq!(
            read.body, created.body,
            "case {label:?}: the refused delete must leave the stored record alone, \
             got {:?} after {:?}",
            read.body, created.body
        );
    }
}

#[tokio::test]
async fn a_deleted_namespace_takes_its_tags_and_its_resource_policy_with_it() {
    let emulator = Emulator::new();
    // A minimal, well-formed policy document; this suite is about the delete, not about
    // what a document may contain (see the `resource_policy` suite for that).
    let document = r#"{"Version":"2012-10-17","Statement":[]}"#;

    for case in delete_cases() {
        let label = case.case;
        let created = seed(&emulator, &case).await;
        let arn = string_member(&created.body, "idNamespaceArn");

        let written = put_policy(&emulator, &arn, document, None).await;
        assert_eq!(
            written.status, 200,
            "case {label:?}: expected HTTP 200 from PutPolicy, got {written:?}"
        );
        let token = string_member(&written.body, "token");
        let tags_before = list_tags(&emulator, &arn).await;
        assert_eq!(
            tags_before.status, 200,
            "case {label:?}: expected HTTP 200 from ListTagsForResource, got {tags_before:?}"
        );
        let policy_before = get_policy(&emulator, &arn).await;
        assert_eq!(
            policy_before.status, 200,
            "case {label:?}: expected HTTP 200 from GetPolicy, got {policy_before:?}"
        );

        let deleted = emulator.delete_id_namespace(&case.name).await;
        success_message(label, &deleted);

        // Tags live on the record, and a resource-based policy belongs to the resource
        // it is attached to: with the namespace gone, neither is served.
        assert_error(
            label,
            &list_tags(&emulator, &arn).await,
            404,
            "ResourceNotFoundException",
        );
        assert_error(
            label,
            &get_policy(&emulator, &arn).await,
            404,
            "ResourceNotFoundException",
        );

        // A namespace created again under the same name is a NEW resource with the
        // same ARN, so it carries no policy and the deleted one's revision token is
        // not the current one.
        let recreated = emulator
            .create_id_namespace(&json!({ "idNamespaceName": case.name, "type": "SOURCE" }))
            .await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: expected HTTP 200 re-creating the namespace, got {recreated:?}"
        );
        assert_eq!(
            string_member(&recreated.body, "idNamespaceArn"),
            expected_arn("idnamespace", &case.name),
            "case {label:?}: the re-created namespace has the same ARN, got {:?}",
            recreated.body
        );
        assert_error(
            label,
            &get_policy(&emulator, &arn).await,
            404,
            "ResourceNotFoundException",
        );
        assert_error(
            label,
            &put_policy(&emulator, &arn, document, Some(&token)).await,
            400,
            "ConflictException",
        );
    }
}
