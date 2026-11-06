// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a `DeleteRequestValidator` a `Method` of the same API blocks.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the `CreateRestApi` / `PutMethod` /
//! `CreateRequestValidator` / `GetRequestValidator` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle`, `execute_api_data_plane`,
//! `request_validator_lifecycle` and `delete_request_validator`.
//!
//! The RULE under test: a record another record of the same API still REFERENCES cannot be
//! deleted — a `RequestValidator` a `Method` names in its `requestValidatorId` is refused
//! with the modelled `ConflictException` (HTTP 409), whose message names the blocked
//! validator and the referencing method, and the addressed record is left exactly as it
//! was; clearing the reference — by patching the method's `requestValidatorId` or by
//! deleting the method — is what frees the same delete, which then answers the modelled 202
//! with an empty body.
//!
//! The rows vary what the rule generalises over: which resource the referencing method sits
//! on (the root resource and a child), its HTTP verb, what the blocked validator is NAMED
//! (an ordinary name, a name outside ASCII, no name at all — `name` is optional on
//! `CreateRequestValidator`), and how the reference is cleared (a patch of the method and a
//! delete of it).

use serde_json::{json, Value};

use crate::delete_request_validator::{delete_validator, deleted_validator};
use crate::execute_api_data_plane::put_method;
use crate::patch_updates::{op, patch};
use crate::request_validator_lifecycle::{created_validator, get_validator};
use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Answer, Gateway};

/// How a row clears the reference that blocks the delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clear {
    /// `UpdateMethod` removing the method's `requestValidatorId`.
    PatchTheMethod,
    /// `DeleteMethod` taking the referencing method away whole.
    DeleteTheMethod,
}

/// One blocked delete: where the referencing method sits, what the validator is named, and
/// how the reference is cleared afterwards.
struct Blocked {
    case: &'static str,
    /// The path part of the resource the referencing method sits on, or `None` for the
    /// API's root resource.
    path_part: Option<&'static str>,
    /// The verb the referencing method is put under.
    verb: &'static str,
    /// The `name` the blocked validator is created with, or `None` for one created without.
    name: Option<&'static str>,
    /// The resource path the refusal must name, which is the root's `/` or the child's.
    resource_path: &'static str,
    clear: Clear,
}

/// The table every case runs over.
const BLOCKED: &[Blocked] = &[
    Blocked {
        case: "a POST on the root resource",
        path_part: None,
        verb: "POST",
        name: Some("body-validator"),
        resource_path: "/",
        clear: Clear::PatchTheMethod,
    },
    Blocked {
        case: "a PUT on a child resource",
        path_part: Some("widgets"),
        verb: "PUT",
        name: Some("widgets-validator"),
        resource_path: "/widgets",
        clear: Clear::PatchTheMethod,
    },
    Blocked {
        case: "a PATCH on a child resource, cleared by deleting the method",
        path_part: Some("items"),
        verb: "PATCH",
        name: Some("items-validator"),
        resource_path: "/items",
        clear: Clear::DeleteTheMethod,
    },
    Blocked {
        case: "a GET on a child resource whose validator is named outside ASCII",
        path_part: Some("reports"),
        verb: "GET",
        name: Some("валидатор-✓"),
        resource_path: "/reports",
        clear: Clear::PatchTheMethod,
    },
    Blocked {
        case: "a DELETE on the root resource whose validator carries no name",
        path_part: None,
        verb: "DELETE",
        name: None,
        resource_path: "/",
        clear: Clear::DeleteTheMethod,
    },
];

/// The request a row's validator is created with.
fn validator_request(row: &Blocked) -> Value {
    let mut request = json!({ "validateRequestBody": true, "validateRequestParameters": true });
    if let Some(name) = row.name {
        request["name"] = json!(name);
    }
    request
}

/// The `ConflictException` message a row's blocked delete must state: the fixed text, the
/// validator's own name as state holds it (empty for a validator created without one) and
/// the referencing method, which API Gateway spells `<VERB>/<resource path>`.
fn in_use_message(row: &Blocked) -> String {
    format!(
        "Cannot delete Request Validator '{}' referenced in method: {}/{}",
        row.name.unwrap_or_default(),
        row.verb,
        row.resource_path
    )
}

/// Assert `answer` is the modelled 409 `ConflictException` stating `message`.
fn assert_conflict(answer: &Answer, message: &str, case: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case}: expected 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ConflictException"),
        "case {case}: expected ConflictException, got {:?}",
        answer.error
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

#[tokio::test]
async fn a_validator_a_method_still_references_cannot_be_deleted() {
    for row in BLOCKED {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "request-validator-in-use").await;
        let resource = match row.path_part {
            None => root.clone(),
            Some(path_part) => resource_under(&gateway, &api, &root, path_part).await,
        };
        let blocked = created_validator(&gateway, &api, row.case, validator_request(row)).await;
        let blocked_id = string_member(&blocked, "id", row.case);
        // A second validator NOTHING references, so the refusal is about the record
        // addressed rather than about the API.
        let spare = created_validator(
            &gateway,
            &api,
            &format!("{}: the spare validator", row.case),
            json!({ "name": "spare" }),
        )
        .await;
        let spare_id = string_member(&spare, "id", row.case);
        put_method(
            &gateway,
            &api,
            &resource,
            row.verb,
            json!({
                "authorizationType": "NONE",
                "requestValidatorId": blocked_id.clone(),
            }),
        )
        .await;

        // While the reference stands the delete is refused, and repeating it is refused the
        // same way: the refusal takes nothing away.
        let expected = in_use_message(row);
        for round in 0..2 {
            assert_conflict(
                &delete_validator(&gateway, &api, &blocked_id).await,
                &expected,
                &format!("{} (round {round})", row.case),
            );
        }
        let read = get_validator(&gateway, &api, &blocked_id).await;
        assert_eq!(
            read.body, blocked,
            "case {}: expected the blocked validator {blocked:?} to be untouched, got {:?}",
            row.case, read.body
        );

        // A validator no method references is deleted straight away, even while a sibling
        // of the same API is blocked.
        deleted_validator(
            &gateway,
            &api,
            &spare_id,
            &format!("{}: the spare validator", row.case),
        )
        .await;

        // Clearing the reference is what frees the delete.
        match row.clear {
            Clear::PatchTheMethod => {
                let detached = patch(
                    &gateway,
                    &format!("/restapis/{api}/resources/{resource}/methods/{}", row.verb),
                    vec![op("remove", "/requestValidatorId", None)],
                )
                .await;
                assert_eq!(
                    detached.status, 200,
                    "case {}: expected UpdateMethod to clear the reference, got {} with {:?}",
                    row.case, detached.status, detached.body
                );
            }
            Clear::DeleteTheMethod => {
                let removed = gateway
                    .call(
                        "DELETE",
                        &format!("/restapis/{api}/resources/{resource}/methods/{}", row.verb),
                        None,
                    )
                    .await;
                assert_eq!(
                    removed.status, 204,
                    "case {}: expected DeleteMethod to take the method away, got {} with {:?}",
                    row.case, removed.status, removed.body
                );
            }
        }
        deleted_validator(
            &gateway,
            &api,
            &blocked_id,
            &format!("{}: once nothing references it", row.case),
        )
        .await;
        assert_eq!(
            get_validator(&gateway, &api, &blocked_id).await.status,
            404,
            "case {}: expected the deleted validator to be gone",
            row.case
        );
    }
}

#[tokio::test]
async fn only_the_method_that_names_the_validator_blocks_its_delete() {
    // One API, two validators and two methods: each method names ONE of the validators, so
    // a delete is blocked by the method that names the validator it addresses and by no
    // other. The methods sit on different resources and carry different verbs, so the
    // refusal has to name the right one of them.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "request-validator-in-use-pairs").await;
    let child = resource_under(&gateway, &api, &root, "orders").await;

    let first = created_validator(
        &gateway,
        &api,
        "the first validator",
        json!({ "name": "first", "validateRequestBody": true }),
    )
    .await;
    let first_id = string_member(&first, "id", "the first validator");
    let second = created_validator(
        &gateway,
        &api,
        "the second validator",
        json!({ "name": "second", "validateRequestParameters": true }),
    )
    .await;
    let second_id = string_member(&second, "id", "the second validator");

    put_method(
        &gateway,
        &api,
        &root,
        "POST",
        json!({ "authorizationType": "NONE", "requestValidatorId": first_id.clone() }),
    )
    .await;
    put_method(
        &gateway,
        &api,
        &child,
        "GET",
        json!({ "authorizationType": "NONE", "requestValidatorId": second_id.clone() }),
    )
    .await;

    for (case, id, name, verb, resource_path) in [
        ("the first validator", &first_id, "first", "POST", "/"),
        (
            "the second validator",
            &second_id,
            "second",
            "GET",
            "/orders",
        ),
    ] {
        let expected =
            format!("Cannot delete Request Validator '{name}' referenced in method: {verb}/{resource_path}");
        assert_conflict(
            &delete_validator(&gateway, &api, id).await,
            &expected,
            case,
        );
    }

    // A validator of ANOTHER API is not referenced by either method, so nothing blocks it:
    // a reference is one of the same API.
    let (other, _other_root) = create_api(&gateway, "request-validator-in-use-other").await;
    let elsewhere = created_validator(
        &gateway,
        &other,
        "the other API's validator",
        json!({ "name": "first" }),
    )
    .await;
    let elsewhere_id = string_member(&elsewhere, "id", "the other API's validator");
    deleted_validator(
        &gateway,
        &other,
        &elsewhere_id,
        "the other API's validator",
    )
    .await;
}
