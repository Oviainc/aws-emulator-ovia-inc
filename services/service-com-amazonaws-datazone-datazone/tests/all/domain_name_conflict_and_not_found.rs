// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for two DataZone rules about refusals.
//!
//! 1. A domain name is UNIQUE within the account: a `CreateDomain` naming a name a
//!    live domain already holds is refused with `ConflictException` (HTTP 409,
//!    `smithy_query` operation="CreateDomain") and nothing is stored, so the account
//!    never holds two domains of one name. The `@idempotencyToken` `clientToken` is the
//!    only thing that turns a repeat into a replay of the first domain.
//! 2. A `ResourceNotFoundException` message names the RESOURCE KIND only and never
//!    echoes the caller-supplied identifier, on every not-found path this service
//!    serves (DeleteDomain, CreateProject, UpdateProject, DeleteProject, GetProject
//!    and GetAssetType inside a domain the store holds).
//!
//! A single-resource READ scoped to a domain the store holds no record of is not a
//! not-found path at all — authorization decides it before any lookup — so those rows
//! live in `domain_scoped_read_authorization`, not in the table below.
//!
//! Both are asserted over a TABLE of inputs: the names differ in form and length and
//! the repeat requests differ in their other members, and the not-found table covers
//! every operation/identifier combination that can miss, not just one request.
//!
//! The harness (`Emulator`, `Answer`) is the one the sibling lifecycle suite already
//! drives this service with.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};

/// A `DomainId` that no domain in the store has, satisfying the modelled pattern
/// `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId").
const MISSING_DOMAIN_ID: &str = "dzd_nosuchdomain000";

/// A `ProjectId` that no project in the store has, satisfying the modelled pattern
/// `^[a-zA-Z0-9_-]{1,36}$` (smithy_query shape="ProjectId").
const MISSING_PROJECT_ID: &str = "nosuchproject000";

/// An `AssetTypeIdentifier` that no asset type has, satisfying the modelled pattern
/// `^(?!\.)[\w\.]*\w$` with length 1..513 (smithy_query shape="AssetTypeIdentifier").
const MISSING_ASSET_TYPE_ID: &str = "NoSuchAssetType";

/// One duplicate-name case: the first request, then a SECOND request carrying the same
/// `name` but different other members, which must still be refused.
struct DuplicateCase {
    label: &'static str,
    name: String,
    /// Members the repeat carries beyond `name`, so the conflict is decided by the
    /// name alone and not by the request being byte-identical.
    repeat_extras: Value,
}

fn duplicate_cases() -> Vec<DuplicateCase> {
    vec![
        DuplicateCase {
            label: "plain ascii name, identical repeat",
            name: "alpha-domain".to_string(),
            repeat_extras: json!({}),
        },
        DuplicateCase {
            label: "name with spaces and punctuation, repeat adds a description",
            name: "beta domain: one!".to_string(),
            repeat_extras: json!({ "description": "a different description" }),
        },
        DuplicateCase {
            // `CreateDomain`'s `name` targets a plain `String` with no modelled length
            // bound (smithy_query operation="CreateDomain"), so a long name is as valid
            // as a short one and must conflict the same way.
            label: "long name, repeat asks for another domainVersion and tags",
            name: "d".repeat(128),
            repeat_extras: json!({ "domainVersion": "V2", "tags": { "team": "data" } }),
        },
        DuplicateCase {
            label: "digits and underscores, repeat carries a fresh clientToken",
            name: "1234_5678".to_string(),
            repeat_extras: json!({ "clientToken": "a-token-never-seen-before" }),
        },
    ]
}

/// Merge `name` with a case's extra members into one `CreateDomain` body.
fn create_body(name: &str, extras: &Value) -> Value {
    let mut body = json!({ "name": name });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extras.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// How many domains `ListDomains` reports carrying `name`.
async fn domains_named(dz: &Emulator, name: &str) -> usize {
    let listed = dz.call("GET", "/v2/domains", None).await;
    assert_eq!(
        listed.status, 200,
        "ListDomains: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    listed
        .body
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("name").and_then(Value::as_str) == Some(name))
                .count()
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn a_second_create_domain_with_an_in_use_name_is_refused_and_stores_nothing() {
    let dz = Emulator::new();

    for case in duplicate_cases() {
        let first = dz
            .call("POST", "/v2/domains", Some(create_body(&case.name, &json!({}))))
            .await;
        assert_eq!(
            first.status, 201,
            "case {:?}: the first create must be accepted, got {} {:?}",
            case.label, first.status, first.body
        );
        let id = first.s("id");

        let repeat = dz
            .call(
                "POST",
                "/v2/domains",
                Some(create_body(&case.name, &case.repeat_extras)),
            )
            .await;
        assert_eq!(
            (repeat.status, repeat.error_code.as_str()),
            (409, "ConflictException"),
            "case {:?}: a name already in use must be refused with the modelled \
             conflict, got {} {:?}",
            case.label,
            repeat.status,
            repeat.body
        );
        assert!(
            repeat.body.get("id").is_none(),
            "case {:?}: a refused create must mint no domain, got {:?}",
            case.label,
            repeat.body
        );
        let message = repeat.s("message");
        assert!(
            !message.is_empty() && message != "<absent>",
            "case {:?}: ConflictException.message is required by the model, got {:?}",
            case.label,
            repeat.body
        );
        assert!(
            !message.contains(&case.name),
            "case {:?}: the refusal must not echo the caller's name, got {message:?}",
            case.label
        );

        // The invariant: the account is left with exactly ONE domain of that name,
        // and it is the one the first call created.
        assert_eq!(
            domains_named(&dz, &case.name).await,
            1,
            "case {:?}: exactly one domain named {:?} must remain",
            case.label,
            case.name
        );
        let fetched = dz.call("GET", &format!("/v2/domains/{id}"), None).await;
        assert_eq!(
            (fetched.status, fetched.s("name")),
            (200, case.name.clone()),
            "case {:?}: the first domain must still be readable, got {} {:?}",
            case.label,
            fetched.status,
            fetched.body
        );
    }
}

#[tokio::test]
async fn a_name_differing_from_the_stored_ones_is_accepted() {
    let dz = Emulator::new();
    // The control row for the rule above: uniqueness refuses an EQUAL name, nothing
    // more, so each of these distinct names mints its own domain. The comparison is
    // exact, so a name differing only in case is a different name.
    let names = [
        "gamma-domain",
        "Gamma-Domain",
        "gamma-domain-2",
        "gamma domain",
        "gamma-domain ",
    ];
    let mut ids: Vec<String> = Vec::new();
    for name in names {
        let answer = dz
            .call("POST", "/v2/domains", Some(json!({ "name": name })))
            .await;
        assert_eq!(
            answer.status, 201,
            "name {name:?}: a name no live domain holds must be accepted, got {} {:?}",
            answer.status, answer.body
        );
        ids.push(answer.s("id"));
    }
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        names.len(),
        "each distinct name must mint its own domain, got ids {ids:?}"
    );
}

#[tokio::test]
async fn a_repeated_client_token_replays_the_first_domain_instead_of_conflicting() {
    let dz = Emulator::new();
    // The `@idempotencyToken` is checked BEFORE the uniqueness rule, so a retry of the
    // same request is a replay (201, the first domain) rather than a 409.
    let body = json!({ "name": "token-domain", "clientToken": "token-one" });
    let first = dz.call("POST", "/v2/domains", Some(body.clone())).await;
    let replay = dz.call("POST", "/v2/domains", Some(body)).await;
    assert_eq!(
        (replay.status, replay.s("id")),
        (201, first.s("id")),
        "a repeat carrying the same clientToken must replay the first domain, got {} {:?}",
        replay.status,
        replay.body
    );
    assert_eq!(
        domains_named(&dz, "token-domain").await,
        1,
        "the replay must not add a second domain"
    );
}

#[tokio::test]
async fn a_name_whose_domain_is_gone_can_be_used_again() {
    let dz = Emulator::new();
    let name = "recycled-domain";
    let id = dz.available_domain(name).await;

    let deleted = dz.call("DELETE", &format!("/v2/domains/{id}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "DeleteDomain: expected the modelled success status, got {} {:?}",
        deleted.status, deleted.body
    );
    // Once the deletion the clock owed has completed the domain is gone, so nothing
    // holds its name any more.
    tokio::time::sleep(SETTLED).await;
    assert_eq!(
        domains_named(&dz, name).await,
        0,
        "the deleted domain must be gone before the name is reused"
    );

    let again = dz
        .call("POST", "/v2/domains", Some(json!({ "name": name })))
        .await;
    assert_eq!(
        again.status, 201,
        "a name no live domain holds must be accepted again, got {} {:?}",
        again.status, again.body
    );
    assert_ne!(
        again.s("id"),
        id,
        "the re-created domain must be a new resource, not the deleted one"
    );
}

/// One not-found case: a request that cannot resolve, and the resource KIND whose name
/// the message must carry.
struct NotFoundCase {
    label: &'static str,
    method: &'static str,
    /// Built from the ids of the fixtures the test created: `{domain}`/`{project}` are
    /// substituted with a real domain id and a real project id.
    uri: String,
    body: Option<Value>,
    expect_kind: &'static str,
    /// The caller-supplied identifier the message must NOT echo.
    identifier: String,
}

#[tokio::test]
async fn resource_not_found_messages_name_the_kind_and_never_the_identifier() {
    let dz = Emulator::new();
    let domain_a = dz.available_domain("isolation-domain-a").await;
    let domain_b = dz.available_domain("isolation-domain-b").await;
    let project = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain_a}/projects"),
            Some(json!({ "name": "isolated project" })),
        )
        .await;
    assert_eq!(
        project.status, 201,
        "CreateProject: expected 201, got {} {:?}",
        project.status, project.body
    );
    let project_id = project.s("id");
    // One real asset type in domain A, so a wrong-revision lookup is a miss on an
    // EXISTING resource rather than on an unknown identifier. `formsInput` is required
    // and documented as allowed to be empty (smithy_query query="doc",
    // shape="CreateAssetType").
    let asset_type = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain_a}/asset-types"),
            Some(json!({
                "name": "IsolatedType",
                "owningProjectIdentifier": project_id,
                "formsInput": {},
            })),
        )
        .await;
    assert_eq!(
        asset_type.status, 201,
        "CreateAssetType: expected 201, got {} {:?}",
        asset_type.status, asset_type.body
    );
    let asset_type_id = asset_type.s("name");

    let cases = vec![
        NotFoundCase {
            label: "GetProject with an unknown project in a real domain",
            method: "GET",
            uri: format!("/v2/domains/{domain_a}/projects/{MISSING_PROJECT_ID}"),
            body: None,
            expect_kind: "Project",
            identifier: MISSING_PROJECT_ID.to_string(),
        },
        NotFoundCase {
            // The cross-domain isolation case: the project exists, but not in the
            // domain the request scoped it to.
            label: "GetProject scoped to an unrelated domain",
            method: "GET",
            uri: format!("/v2/domains/{domain_b}/projects/{project_id}"),
            body: None,
            expect_kind: "Project",
            identifier: project_id.clone(),
        },
        NotFoundCase {
            label: "DeleteProject scoped to an unrelated domain",
            method: "DELETE",
            uri: format!("/v2/domains/{domain_b}/projects/{project_id}"),
            body: None,
            expect_kind: "Project",
            identifier: project_id.clone(),
        },
        NotFoundCase {
            label: "UpdateProject scoped to an unrelated domain",
            method: "PATCH",
            uri: format!("/v2/domains/{domain_b}/projects/{project_id}"),
            body: Some(json!({ "description": "wrong domain" })),
            expect_kind: "Project",
            identifier: project_id.clone(),
        },
        NotFoundCase {
            label: "UpdateProject with an unknown project in a real domain",
            method: "PATCH",
            uri: format!("/v2/domains/{domain_a}/projects/{MISSING_PROJECT_ID}"),
            body: Some(json!({ "description": "no such project" })),
            expect_kind: "Project",
            identifier: MISSING_PROJECT_ID.to_string(),
        },
        NotFoundCase {
            label: "GetAssetType with an unknown asset type in a real domain",
            method: "GET",
            uri: format!("/v2/domains/{domain_a}/asset-types/{MISSING_ASSET_TYPE_ID}"),
            body: None,
            expect_kind: "Asset type",
            identifier: MISSING_ASSET_TYPE_ID.to_string(),
        },
        NotFoundCase {
            label: "GetAssetType asking for a revision the type does not have",
            method: "GET",
            uri: format!("/v2/domains/{domain_a}/asset-types/{asset_type_id}?revision=99"),
            body: None,
            expect_kind: "Asset type",
            identifier: asset_type_id.clone(),
        },
        NotFoundCase {
            label: "GetAssetType scoped to an unrelated domain",
            method: "GET",
            uri: format!("/v2/domains/{domain_b}/asset-types/{asset_type_id}"),
            body: None,
            expect_kind: "Asset type",
            identifier: asset_type_id.clone(),
        },
    ];

    for case in cases {
        let answer = dz.call(case.method, &case.uri, case.body.clone()).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {:?}: expected the modelled not-found error, got {} {:?}",
            case.label,
            answer.status,
            answer.body
        );
        let message = answer.s("message");
        assert_eq!(
            message,
            format!("{} not found", case.expect_kind),
            "case {:?}: the message must name the resource kind only, got {message:?}",
            case.label
        );
        assert!(
            !message.contains(&case.identifier),
            "case {:?}: the message must not echo the requested identifier {:?}, got \
             {message:?}",
            case.label,
            case.identifier
        );
    }

    // A call scoped to a domain the store serves no record of never reaches an
    // existence answer: a permission is held ON a domain, so the caller holds none
    // there and authorization decides the call first. The denial names the OPERATION
    // and nothing the caller sent, and the one whose own identifier IS the domain scope
    // (`DeleteDomain`) carries no message at all.
    let denied: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "DeleteDomain with a domain nothing created",
            "DELETE",
            format!("/v2/domains/{MISSING_DOMAIN_ID}"),
            None,
            String::new(),
        ),
        (
            "CreateProject under a domain nothing created",
            "POST",
            format!("/v2/domains/{MISSING_DOMAIN_ID}/projects"),
            Some(json!({ "name": "orphan project" })),
            "User is not permitted to perform operation: CreateProject".to_string(),
        ),
        (
            "DeleteProject under a domain nothing created",
            "DELETE",
            format!("/v2/domains/{MISSING_DOMAIN_ID}/projects/{project_id}"),
            None,
            "User is not permitted to perform operation: DeleteProject".to_string(),
        ),
        (
            "UpdateProject under a domain nothing created",
            "PATCH",
            format!("/v2/domains/{MISSING_DOMAIN_ID}/projects/{project_id}"),
            Some(json!({ "description": "no such domain" })),
            "User is not permitted to perform operation: UpdateProject".to_string(),
        ),
    ];
    for (label, method, uri, body, message) in denied {
        let answer = dz.call(method, &uri, body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str(), answer.s("message")),
            (403, "AccessDeniedException", message),
            "case {label:?}: expected the modelled denial, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            !answer.s("message").contains(MISSING_DOMAIN_ID),
            "case {label:?}: the denial must not echo the requested domain, got {:?}",
            answer.body
        );
    }

    // The isolation itself still holds: each project is readable only under its own
    // domain, and it is still there after every refusal above.
    let readable = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain_a}/projects/{project_id}"),
            None,
        )
        .await;
    assert_eq!(
        (readable.status, readable.s("id")),
        (200, project_id.clone()),
        "the project must still be readable under its own domain, got {} {:?}",
        readable.status,
        readable.body
    );
}
