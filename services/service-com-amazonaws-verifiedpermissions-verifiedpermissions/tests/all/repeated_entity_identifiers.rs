// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for an `entityList` that names one `identifier` more than
//! once.
//!
//! `EntitiesDefinition$entityList` makes that a SUCCESS path, keyed by the
//! identifier: "If you include multiple entities with the same `identifier`, only
//! the last one is processed in the request"
//! (`smithy_query(query="shape", shape="EntitiesDefinition")`, member
//! `entityList`). So the list is a mapping from identifier to entity, built in
//! order with the later entry replacing the earlier: the repeat is neither
//! refused nor merged with what came before it, and an identifier that differs in
//! EITHER of its two parts (`entityType`, `entityId`) is a different entity that
//! survives beside it.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the shared harness in [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{ok, router, store_with_validation_off};

/// Permit on an ATTRIBUTE of the principal entity, so which `User` entity the
/// request's entity list ends up holding decides the call.
const PERMIT_ADMIN_ATTRIBUTE: &str =
    "permit(principal, action, resource) when { principal has level && principal.level == \"admin\" };";

/// Permit on the principal's PARENTS, so which hierarchy the surviving `User`
/// entity carries decides the call.
const PERMIT_ADMIN_GROUP: &str = "permit(principal in Group::\"admins\", action, resource);";

/// Permit on an attribute of the RESOURCE entity, so the rule is exercised on an
/// entity that is not the principal too.
const PERMIT_PUBLIC_RESOURCE: &str =
    "permit(principal, action, resource) when { resource has visibility && resource.visibility == \"public\" };";

/// One `EntityItem` with a single string attribute.
fn with_attribute(entity_type: &str, entity_id: &str, key: &str, value: &str) -> Value {
    json!({
        "identifier": { "entityType": entity_type, "entityId": entity_id },
        "attributes": { key: { "string": value } },
    })
}

/// One `EntityItem` carrying nothing but its identifier.
fn bare(entity_type: &str, entity_id: &str) -> Value {
    json!({ "identifier": { "entityType": entity_type, "entityId": entity_id } })
}

/// One `EntityItem` whose only description is its parent entity.
fn with_parent(entity_type: &str, entity_id: &str, parent_id: &str) -> Value {
    json!({
        "identifier": { "entityType": entity_type, "entityId": entity_id },
        "parents": [{ "entityType": "Group", "entityId": parent_id }],
    })
}

/// A store holding the three permits above, each deciding on a different part of
/// the entity slice.
async fn store(router: &smithy_server_runtime::MultiServiceRouter) -> String {
    let store_id = store_with_validation_off(router).await;
    for statement in [
        PERMIT_ADMIN_ATTRIBUTE,
        PERMIT_ADMIN_GROUP,
        PERMIT_PUBLIC_RESOURCE,
    ] {
        ok(
            router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
    }
    store_id
}

/// One row of the table: the entity list sent, the triple's principal and
/// resource ids, and the decision the surviving entities produce.
struct Case {
    /// What makes this row different from the others.
    name: &'static str,
    /// `entityId` of the `User` the request authorizes.
    principal_id: &'static str,
    /// `entityId` of the `Photo` the request authorizes.
    resource_id: &'static str,
    /// The `entityList` the request carries, in the order it is sent.
    entities: Vec<Value>,
    /// `ALLOW` only when the LAST entity for each repeated identifier is the one
    /// evaluated.
    decision: &'static str,
}

/// A repeated `identifier` is accepted and only the LAST entity given for it is
/// processed — on either side of the triple, however many times it repeats, and
/// whatever it describes.
///
/// The rows differ along what the repeat changes (a string attribute, a parent
/// entity, an attribute DROPPED by the later entry), how many times the
/// identifier repeats, which member of the triple the repeated entity is, and —
/// as the control rows — which PART of the identifier differs when two entities
/// are not the same entity at all.
#[tokio::test]
async fn a_repeated_entity_identifier_keeps_the_last_entity() {
    let router = router();
    let store_id = store(&router).await;

    let cases = vec![
        Case {
            name: "two entities for one identifier, the admin one last",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_attribute("User", "alice", "level", "guest"),
                with_attribute("User", "alice", "level", "admin"),
            ],
            decision: "ALLOW",
        },
        Case {
            name: "the same two entities in the other order",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_attribute("User", "alice", "level", "admin"),
                with_attribute("User", "alice", "level", "guest"),
            ],
            decision: "DENY",
        },
        Case {
            name: "three entities for one identifier, the admin one last",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_attribute("User", "alice", "level", "admin"),
                with_attribute("User", "alice", "level", "guest"),
                with_attribute("User", "alice", "level", "admin"),
            ],
            decision: "ALLOW",
        },
        Case {
            name: "three entities for one identifier, the guest one last",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_attribute("User", "alice", "level", "admin"),
                with_attribute("User", "alice", "level", "admin"),
                with_attribute("User", "alice", "level", "guest"),
            ],
            decision: "DENY",
        },
        Case {
            name: "the last entity drops the attribute, so it is not merged in",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_attribute("User", "alice", "level", "admin"),
                bare("User", "alice"),
            ],
            decision: "DENY",
        },
        Case {
            name: "the repeat changes the parent entity, the admin group last",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_parent("User", "alice", "guests"),
                with_parent("User", "alice", "admins"),
            ],
            decision: "ALLOW",
        },
        Case {
            name: "the repeat changes the parent entity, another group last",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_parent("User", "alice", "admins"),
                with_parent("User", "alice", "guests"),
            ],
            decision: "DENY",
        },
        Case {
            name: "the repeated entity is the resource, the public one last",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_attribute("Photo", "p", "visibility", "private"),
                with_attribute("Photo", "p", "visibility", "public"),
            ],
            decision: "ALLOW",
        },
        Case {
            name: "the repeated entity is the resource, the private one last",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_attribute("Photo", "p", "visibility", "public"),
                with_attribute("Photo", "p", "visibility", "private"),
            ],
            decision: "DENY",
        },
        Case {
            name: "a repeat between two entities of the same identifier only",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_attribute("User", "bob", "level", "admin"),
                with_attribute("User", "alice", "level", "guest"),
                with_attribute("User", "alice", "level", "admin"),
                with_attribute("User", "bob", "level", "guest"),
            ],
            decision: "ALLOW",
        },
        Case {
            name: "a different entityId is a different entity, kept beside it",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![
                with_attribute("User", "alice", "level", "admin"),
                with_attribute("User", "bob", "level", "guest"),
            ],
            decision: "ALLOW",
        },
        Case {
            name: "a different entityType is a different entity, kept beside it",
            principal_id: "alice",
            resource_id: "alice",
            entities: vec![
                with_attribute("Photo", "alice", "visibility", "public"),
                with_attribute("User", "alice", "level", "guest"),
            ],
            decision: "ALLOW",
        },
        Case {
            name: "no repeat at all still decides on the one entity sent",
            principal_id: "alice",
            resource_id: "p",
            entities: vec![with_attribute("User", "alice", "level", "guest")],
            decision: "DENY",
        },
    ];

    for case in &cases {
        let answer = ok(
            &router,
            "IsAuthorized",
            &json!({
                "policyStoreId": store_id,
                "principal": { "entityType": "User", "entityId": case.principal_id },
                "action": { "actionType": "Action", "actionId": "view" },
                "resource": { "entityType": "Photo", "entityId": case.resource_id },
                "entities": { "entityList": case.entities.clone() },
            }),
        )
        .await;
        assert_eq!(
            answer["decision"].as_str().unwrap_or_default(),
            case.decision,
            "case {:?}: expected decision {}, got {answer}",
            case.name,
            case.decision
        );
        assert!(
            answer["errors"]
                .as_array()
                .is_some_and(|errors| errors.is_empty()),
            "case {:?}: expected no evaluation errors, got {answer}",
            case.name
        );
    }

    // The same list, evaluated by the batch operation against the same store: the
    // entity slice a batch shares is read the same way, so the repeat decides the
    // batch's results and not a request-level refusal.
    let answer = ok(
        &router,
        "BatchIsAuthorized",
        &json!({
            "policyStoreId": store_id,
            "requests": [
                {
                    "principal": { "entityType": "User", "entityId": "alice" },
                    "action": { "actionType": "Action", "actionId": "view" },
                    "resource": { "entityType": "Photo", "entityId": "p" },
                },
                {
                    "principal": { "entityType": "User", "entityId": "bob" },
                    "action": { "actionType": "Action", "actionId": "view" },
                    "resource": { "entityType": "Photo", "entityId": "p" },
                },
            ],
            "entities": { "entityList": [
                with_attribute("User", "alice", "level", "guest"),
                with_attribute("User", "alice", "level", "admin"),
            ] },
        }),
    )
    .await;
    let decisions: Vec<String> = answer["results"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|result| {
            result["decision"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        decisions,
        vec!["ALLOW".to_string(), "DENY".to_string()],
        "case \"batch sharing a repeated identifier\": expected the last entity to decide, got {answer}"
    );
}
