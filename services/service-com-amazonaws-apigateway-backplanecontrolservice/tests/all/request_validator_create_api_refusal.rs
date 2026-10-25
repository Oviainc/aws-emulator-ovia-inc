// SPDX-License-Identifier: CC0-1.0

//! How `CreateRequestValidator` CLASSIFIES a `restApiId` that resolves to no stored API,
//! asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateRequestValidator` helpers are this crate's existing harness,
//! reused from `rest_api_lifecycle` and `request_validator_lifecycle`.
//!
//! The RULE under test: where API Gateway reads a REST API identifier while VALIDATING
//! the request it was handed — which is what this create does with the identifier its URI
//! carries — an identifier that resolves to no API is the modelled `BadRequestException`
//! (HTTP 400) carrying the service's own fixed invalid-identifier sentence, with nothing
//! interpolated from the request; where the API is instead the record an operation reads
//! or patches, the same identifier stays the modelled `NotFoundException` (HTTP 404) that
//! names the identifier back. Both classes are modelled on this operation
//! (`smithy_query(query="operation", operation="CreateRequestValidator")`:
//! `BadRequestException` httpError 400, `NotFoundException` httpError 404), so the choice
//! between them is the operation's own contract.
//!
//! The rows span what the rule generalises over rather than repeating one recorded call:
//! identifiers differing in length, alphabet and case (so the sentence is shown to be
//! fixed rather than built from the request), every form the request body takes (no
//! member at all, a name, each validation flag), an identifier that resolved to an API
//! until it was deleted, and — for the rule's other side — the operations of the same
//! slice that resolve the API as a record they read or patch.

use serde_json::{json, Value};

use crate::request_validator_lifecycle::{create_validator, get_validator};
use crate::rest_api_identifier_refusals::no_such_api;
use crate::rest_api_lifecycle::{assert_failure, create_api, page_items, string_member, Gateway};

/// The sentence this service answers a REST API identifier it could not resolve WHILE
/// VALIDATING a request with. Fixed text: it quotes neither the identifier nor the
/// account, which is what distinguishes it from the not-found sentence `no_such_api`
/// states, which this crate's `rest_api_identifier_refusals` suite already defines and
/// which is reused here rather than spelled again.
const INVALID_REST_API_IDENTIFIER: &str = "Invalid REST API identifier specified";

/// Identifiers no stored API has. The rows differ along what the rule generalises over:
/// the length (one character, the ten characters API Gateway mints, longer), the alphabet
/// (letters, digits, a mix, a hyphen) and the case — `restApiId` is a plain `String` with
/// no `@pattern` (`smithy_query(query="operation", operation="CreateRequestValidator")`),
/// so every row reaches the handler's own validation rather than the boundary validator.
const UNKNOWN_IDS: &[&str] = &[
    "q",
    "zzzzzzzzzz",
    "0123456789",
    "Ab3Xy9Zq1W",
    "no-such-api-00000001",
];

/// Every form the create's body takes, so the classification is shown to be about the API
/// identifier and not about any body member: nothing at all, a name, and each modelled
/// validation flag on its own and together.
fn bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("an empty body", json!({})),
        ("a named validator", json!({ "name": "orphan-validator" })),
        (
            "the body flag",
            json!({ "name": "orphan-body", "validateRequestBody": true }),
        ),
        (
            "the parameters flag",
            json!({ "name": "orphan-params", "validateRequestParameters": true }),
        ),
        (
            "both flags",
            json!({
                "name": "orphan-both",
                "validateRequestBody": true,
                "validateRequestParameters": true,
            }),
        ),
    ]
}

/// The ids `GetRequestValidators` reports for `api`, asserting the modelled 200.
async fn listed_ids(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
    let answer = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/requestvalidators?limit=500"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetRequestValidators, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

#[tokio::test]
async fn an_unresolvable_api_identifier_is_a_bad_request_with_the_services_fixed_sentence() {
    let gateway = Gateway::new();
    // One stored API is the positive control: the same bodies are accepted there, so the
    // refusals below are about the identifier and nothing else.
    let (stored, _root) = create_api(&gateway, "validator-create-api-refusal").await;

    let mut accepted: Vec<String> = Vec::new();
    for (body_case, body) in bodies() {
        let created = create_validator(&gateway, &stored, body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {body_case} on the stored API: expected 201, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", body_case);
        // The accepted record reads back through the implemented reader, so the stored
        // side of the contract is proven and not assumed.
        let read = get_validator(&gateway, &stored, &id).await;
        assert_eq!(
            read.body, created.body,
            "case {body_case} on the stored API: GetRequestValidator must report the \
             stored validator, got {:?} after {:?}",
            read.body, created.body
        );
        accepted.push(id);

        for missing in UNKNOWN_IDS {
            let case = format!("{body_case} on the unknown API {missing:?}");
            let answer = create_validator(&gateway, missing, body.clone()).await;
            assert_failure(&answer, 400, "BadRequestException", &case);
            let message = answer.body["message"].as_str();
            assert_eq!(
                message,
                Some(INVALID_REST_API_IDENTIFIER),
                "case {case}: expected the message {INVALID_REST_API_IDENTIFIER:?}, \
                 got {message:?}"
            );
        }
    }

    // The sentence is fixed, so no refusal quoted the identifier or the account back —
    // asserted on the message of a row whose identifier could not appear by accident.
    let echoed = create_validator(&gateway, "no-such-api-00000001", json!({})).await;
    let message = echoed.body["message"].as_str().unwrap_or_default();
    let account = engine::emulator_account_id();
    assert!(
        !message.contains("no-such-api-00000001") && !message.contains(account),
        "case CreateRequestValidator(unknown API): the sentence is fixed, \
         got {message:?}"
    );

    // No refused request created a validator: the stored API holds exactly the records
    // its own accepted creates made, in creation order.
    let listed = listed_ids(&gateway, &stored, "after the refusals").await;
    assert_eq!(
        listed, accepted,
        "case GetRequestValidators after the refusals: expected {accepted:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn an_identifier_that_stops_resolving_is_refused_the_same_way() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "validator-create-after-delete").await;

    // While the API stands, the create is served.
    let served = create_validator(&gateway, &api, json!({ "name": "while-it-stands" })).await;
    assert_eq!(
        served.status, 201,
        "case CreateRequestValidator(stored API): expected 201, got {} with {:?}",
        served.status, served.body
    );

    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );

    // The identifier is well formed and was real; what matters is that it resolves to no
    // API now, so the refusal is the same one every unknown identifier earns.
    for (body_case, body) in bodies() {
        let case = format!("{body_case} on the deleted API");
        let answer = create_validator(&gateway, &api, body).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(INVALID_REST_API_IDENTIFIER),
            "case {case}: expected the message {INVALID_REST_API_IDENTIFIER:?}, got {:?}",
            answer.body["message"]
        );
    }
}

#[tokio::test]
async fn resolving_the_api_as_a_record_to_read_or_patch_stays_a_not_found() {
    let gateway = Gateway::new();
    let (stored, _root) = create_api(&gateway, "validator-refusal-other-side").await;
    let created = create_validator(&gateway, &stored, json!({ "name": "present" })).await;
    let validator = string_member(&created.body, "id", "CreateRequestValidator(stored)");

    // The other side of the rule: these operations resolve the API as the record they
    // report or patch, so the same unknown identifier is the identifier-naming 404.
    for missing in UNKNOWN_IDS {
        let rows: Vec<(String, &str, String, Option<Value>)> = vec![
            (
                format!("GetRequestValidator on the unknown API {missing:?}"),
                "GET",
                format!("/restapis/{missing}/requestvalidators/{validator}"),
                None,
            ),
            (
                format!("GetRequestValidators on the unknown API {missing:?}"),
                "GET",
                format!("/restapis/{missing}/requestvalidators"),
                None,
            ),
            (
                format!("UpdateRequestValidator on the unknown API {missing:?}"),
                "PATCH",
                format!("/restapis/{missing}/requestvalidators/{validator}"),
                Some(json!({
                    "patchOperations": [
                        { "op": "replace", "path": "/name", "value": "renamed" }
                    ]
                })),
            ),
            (
                format!("GetRestApi on the unknown API {missing:?}"),
                "GET",
                format!("/restapis/{missing}"),
                None,
            ),
        ];
        for (case, method, uri, body) in rows {
            let answer = gateway.call(method, &uri, body).await;
            assert_failure(&answer, 404, "NotFoundException", &case);
            assert_eq!(
                answer.body["message"].as_str(),
                Some(no_such_api(missing).as_str()),
                "case {case}: expected the message {:?}, got {:?}",
                no_such_api(missing),
                answer.body["message"]
            );
        }
    }

    // The stored validator is untouched by every refusal above.
    let read = get_validator(&gateway, &stored, &validator).await;
    assert_eq!(
        read.body, created.body,
        "case GetRequestValidator(stored) after the refusals: expected {:?}, got {:?}",
        created.body, read.body
    );
}
