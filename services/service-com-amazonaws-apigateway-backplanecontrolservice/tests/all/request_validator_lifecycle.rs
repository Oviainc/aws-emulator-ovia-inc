// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an API's request validators: `CreateRequestValidator` and
//! `GetRequestValidator`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` helper are this crate's existing harness, reused from
//! `rest_api_lifecycle`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the create/read
//! round trip over a table spanning every combination of the two modelled validation
//! flags, whether the request names the validator at all, and what its name is made of;
//! the `name` bound at the character bound and one past it; that a name does not identify
//! a validator (two identical requests make two records); that a read changes nothing a
//! later caller sees; every identifier these operations resolve, over the records that do
//! and do not exist; and the per-API bound on how many validators the store keeps.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_REQUEST_VALIDATORS_PER_API, MAX_STRING_CHARS,
};

use crate::rest_api_lifecycle::{
    assert_failure, create_api, string_member, without, Answer, Gateway,
};

/// `CreateRequestValidator` on `api` with `request`.
async fn create_validator(gateway: &Gateway, api: &str, request: Value) -> Answer {
    gateway
        .call(
            "POST",
            &format!("/restapis/{api}/requestvalidators"),
            Some(request),
        )
        .await
}

/// `GetRequestValidator` for `validator` of `api`.
async fn get_validator(gateway: &Gateway, api: &str, validator: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/requestvalidators/{validator}"),
            None,
        )
        .await
}

/// `CreateRequestValidator` with `request`, asserting the modelled 201 and answering the
/// body.
async fn created_validator(gateway: &Gateway, api: &str, case: &str, request: Value) -> Value {
    let answer = create_validator(gateway, api, request).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateRequestValidator, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The response body with `id` removed, so a case can state the record the request asked
/// for without naming the id the service minted for it.
fn without_id(body: &Value, case: &str) -> Value {
    assert!(
        body.is_object(),
        "case {case}: expected an object, got {body:?}"
    );
    without(body, &["id"])
}

#[tokio::test]
async fn a_created_request_validator_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: every combination of the two
    // modelled flags (`validateRequestBody` and `validateRequestParameters`, each
    // `@default false` per
    // `smithy_query(query="operation", operation="CreateRequestValidator")`), whether a
    // flag is set explicitly to its default or left out of the request altogether, and
    // whether the request names the validator — and if so, what the name is made of.
    let long_name: String = "n".repeat(MAX_STRING_CHARS);
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a request that sets nothing at all",
            json!({}),
            json!({ "validateRequestBody": false, "validateRequestParameters": false }),
        ),
        (
            "a named validator with both flags left out",
            json!({ "name": "neither" }),
            json!({
                "name": "neither",
                "validateRequestBody": false,
                "validateRequestParameters": false,
            }),
        ),
        (
            "both flags set explicitly to their default",
            json!({
                "name": "explicit-defaults",
                "validateRequestBody": false,
                "validateRequestParameters": false,
            }),
            json!({
                "name": "explicit-defaults",
                "validateRequestBody": false,
                "validateRequestParameters": false,
            }),
        ),
        (
            "the body only",
            json!({ "name": "body-only", "validateRequestBody": true }),
            json!({
                "name": "body-only",
                "validateRequestBody": true,
                "validateRequestParameters": false,
            }),
        ),
        (
            "the parameters only",
            json!({ "name": "params-only", "validateRequestParameters": true }),
            json!({
                "name": "params-only",
                "validateRequestBody": false,
                "validateRequestParameters": true,
            }),
        ),
        (
            "both the body and the parameters",
            json!({
                "name": "body-and-params",
                "validateRequestBody": true,
                "validateRequestParameters": true,
            }),
            json!({
                "name": "body-and-params",
                "validateRequestBody": true,
                "validateRequestParameters": true,
            }),
        ),
        (
            "a name outside ASCII",
            json!({ "name": "валидатор-✓", "validateRequestBody": true }),
            json!({
                "name": "валидатор-✓",
                "validateRequestBody": true,
                "validateRequestParameters": false,
            }),
        ),
        (
            "a name at the character bound",
            json!({ "name": long_name.clone(), "validateRequestParameters": true }),
            json!({
                "name": long_name,
                "validateRequestBody": false,
                "validateRequestParameters": true,
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "request-validator-round-trip").await;

    let mut ids: Vec<String> = Vec::new();
    for (case, request, expected) in &cases {
        let created = created_validator(&gateway, &api, case, request.clone()).await;
        let id = string_member(&created, "id", case);
        assert_eq!(
            id.chars().count(),
            10,
            "case {case}: a request validator id is 10 characters, got {id:?}"
        );
        assert!(
            id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case}: a request validator id is lowercase alphanumeric, got {id:?}"
        );

        // The body is the stored record: what the request set, the flags the model
        // defaults, the id the service minted, and nothing else.
        let reported = without_id(&created, case);
        assert_eq!(
            &reported, expected,
            "case {case}: expected the validator {expected:?}, got {reported:?}"
        );

        let read = get_validator(&gateway, &api, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetRequestValidator, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created,
            "case {case}: GetRequestValidator must report the stored validator, \
             got {:?} after {:?}",
            read.body, created
        );
        ids.push(id);
    }

    // Each create minted its own id, so no row overwrote another row's record.
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(
            ids.iter().filter(|other| *other == id).count(),
            1,
            "case {}: expected a distinct id, {id:?} was minted twice",
            cases[index].0
        );
    }
}

#[tokio::test]
async fn a_name_past_the_character_bound_is_refused_and_stores_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "request-validator-name-bound").await;

    // The bound is this emulator's own caller-sized-string guard
    // (`state::MAX_STRING_CHARS`): a name of exactly that many characters is accepted
    // (asserted by the round-trip table above), one character past it is refused, and so
    // is anything further outside it.
    for over in [MAX_STRING_CHARS + 1, MAX_STRING_CHARS * 2, 10_000] {
        let case = format!("CreateRequestValidator(name of {over} characters)");
        let answer = create_validator(&gateway, &api, json!({ "name": "n".repeat(over) })).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
    }

    // A refused request stored nothing, so the next accepted one is the API's first
    // validator.
    let created = created_validator(
        &gateway,
        &api,
        "CreateRequestValidator after the refusals",
        json!({ "name": "accepted" }),
    )
    .await;
    let id = string_member(&created, "id", "CreateRequestValidator after the refusals");
    let read = get_validator(&gateway, &api, &id).await;
    assert_eq!(
        read.body, created,
        "case GetRequestValidator after the refusals: expected the stored validator, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_name_does_not_identify_a_validator_and_a_read_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "request-validator-no-dedup").await;

    // Two requests carrying the SAME name and flags make two records: a validator is
    // identified by the id the service mints, which is what a method's
    // `requestValidatorId` names.
    let request = json!({ "name": "same", "validateRequestBody": true });
    let first = created_validator(&gateway, &api, "the first create", request.clone()).await;
    let second = created_validator(&gateway, &api, "the second create", request).await;
    let first_id = string_member(&first, "id", "the first create");
    let second_id = string_member(&second, "id", "the second create");
    assert_ne!(
        first_id, second_id,
        "case two identical creates: expected distinct ids, both were {first_id:?}"
    );
    for (case, id, created) in [
        ("the first create", &first_id, &first),
        ("the second create", &second_id, &second),
    ] {
        let read = get_validator(&gateway, &api, id).await;
        assert_eq!(
            &read.body, created,
            "case {case}: both validators must stand, got {:?} for {id}",
            read.body
        );
    }

    // Reading a validator does not change what a later caller sees.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_validator(&gateway, &api, &first_id).await;
        assert_eq!(
            read.status, 200,
            "case GetRequestValidator(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetRequestValidator repeated: a read must not change the record, \
         got {:?} then {:?}",
        seen[0], seen[2]
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_every_request_validator_operation() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "request-validator-notfound").await;
    let missing = "zzzzzzzzzz";
    let created = created_validator(
        &gateway,
        &api,
        "the validator to read",
        json!({ "name": "present" }),
    )
    .await;
    let id = string_member(&created, "id", "the validator to read");

    // Rows span each identifier these operations resolve, one unknown at a time.
    assert_failure(
        &create_validator(&gateway, missing, json!({ "name": "orphan" })).await,
        404,
        "NotFoundException",
        "CreateRequestValidator on an unknown API",
    );
    assert_failure(
        &get_validator(&gateway, missing, &id).await,
        404,
        "NotFoundException",
        "GetRequestValidator on an unknown API",
    );
    assert_failure(
        &get_validator(&gateway, &api, missing).await,
        404,
        "NotFoundException",
        "GetRequestValidator for an unknown validator",
    );

    // A validator belongs to its own API: another API does not report it.
    let (other, _other_root) = create_api(&gateway, "request-validator-notfound-other").await;
    assert_failure(
        &get_validator(&gateway, &other, &id).await,
        404,
        "NotFoundException",
        "GetRequestValidator through another API",
    );

    // The validators of a deleted API go with it.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_validator(&gateway, &api, &id).await,
        404,
        "NotFoundException",
        "GetRequestValidator after DeleteRestApi",
    );
}

#[tokio::test]
async fn an_api_holds_at_most_the_bound_of_request_validators() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "request-validator-bound").await;

    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_REQUEST_VALIDATORS_PER_API {
        let case = format!("CreateRequestValidator(#{index})");
        let created =
            created_validator(&gateway, &api, &case, json!({ "name": format!("v-{index}") }))
                .await;
        ids.push(string_member(&created, "id", &case));
    }
    let over = create_validator(&gateway, &api, json!({ "name": "one-too-many" })).await;
    assert_failure(
        &over,
        429,
        "LimitExceededException",
        "CreateRequestValidator past the bound",
    );

    // The refusal stored nothing and took nothing away: the first and the last validator
    // created up to the bound are both still readable.
    for (case, id) in [
        ("the first validator", ids.first()),
        ("the last validator", ids.last()),
    ] {
        let id = id.unwrap_or_else(|| panic!("case {case}: expected a created validator"));
        let read = get_validator(&gateway, &api, id).await;
        assert_eq!(
            read.status, 200,
            "case GetRequestValidator({case}): expected 200, got {} with {:?}",
            read.status, read.body
        );
    }

    // The bound is per API: another API takes its own validators.
    let (other, _other_root) = create_api(&gateway, "request-validator-bound-other").await;
    let elsewhere = created_validator(
        &gateway,
        &other,
        "CreateRequestValidator on another API",
        json!({ "name": "elsewhere" }),
    )
    .await;
    assert!(
        elsewhere["id"].is_string(),
        "case CreateRequestValidator on another API: expected an id, got {elsewhere:?}"
    );
}
