// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteRequestValidator`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the `CreateRestApi` helper and the
//! `CreateRequestValidator`/`GetRequestValidator` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle` and `request_validator_lifecycle`.
//!
//! The RULE under test: a delete removes exactly the one stored record its
//! `restApiId`/`requestValidatorId` pair names and leaves every other record standing, so
//! a later read of that id finds nothing while every sibling still reads back member for
//! member; an identifier that names no stored record is the modelled 404
//! `NotFoundException`, stating the sentence that identifier earns.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: a table
//! spanning every combination of the two modelled validation flags and whether the
//! validator carries a name; every identifier the operation resolves, one unknown at a
//! time (an unknown API, an unknown validator, a validator of another API, a second delete
//! of the same id, and a delete after the API itself is gone); that the collection report
//! drops the deleted record and keeps the creation order of the rest; and that the per-API
//! validator bound counts only the validators that still exist, so a delete really takes a
//! record away rather than marking it.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_REQUEST_VALIDATORS_PER_API, NO_SUCH_REQUEST_VALIDATOR,
};

use crate::request_validator_lifecycle::{create_validator, created_validator, get_validator};
use crate::rest_api_identifier_refusals::no_such_api;
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};

/// `DeleteRequestValidator` for `validator` of `api`.
///
/// `pub(crate)` so the sibling `request_validator_in_use` suite drives the delete through
/// the same wire helper rather than keeping its own copy of the URI.
pub(crate) async fn delete_validator(gateway: &Gateway, api: &str, validator: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/requestvalidators/{validator}"),
            None,
        )
        .await
}

/// `DeleteRequestValidator`, asserting the modelled 202 and an empty body.
///
/// `pub(crate)` for the reason [`delete_validator`] is.
pub(crate) async fn deleted_validator(gateway: &Gateway, api: &str, validator: &str, case: &str) {
    let answer = delete_validator(gateway, api, validator).await;
    assert_eq!(
        answer.status, 202,
        "case {case}: expected 202 from DeleteRequestValidator, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "case {case}: DeleteRequestValidator answers no body, got {:?}",
        answer.body
    );
}

/// Assert `answer` is the modelled 404 `NotFoundException` stating `message`.
///
/// The message is asserted and not only the code, because the two identifiers this
/// operation resolves earn DIFFERENT sentences — the API is named back
/// (`crate::rest_api_identifier_refusals::no_such_api`, which states the one sentence this
/// crate's suites share) while a validator that names no record earns the service's fixed
/// `NO_SUCH_REQUEST_VALIDATOR`, taken from the crate rather than spelled again here — so a
/// refusal that resolved the wrong identifier would otherwise pass.
fn assert_not_found(answer: &Answer, message: &str, case: &str) {
    assert_failure(answer, 404, "NotFoundException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// `GetRequestValidators` for `api`, asserting the modelled 200 and answering the items it
/// reports.
async fn listed_ids(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/requestvalidators"), None)
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

/// The table every "delete one of these" case runs over. Rows differ along what the rule
/// generalises over: every combination of the two modelled flags (`validateRequestBody`
/// and `validateRequestParameters`, each `@default false` per
/// `smithy_query(query="operation", operation="CreateRequestValidator")`), whether the
/// request names the validator at all (`name` is optional there), and what a name is made
/// of.
fn validator_table() -> Vec<(&'static str, Value)> {
    vec![
        ("a validator that sets nothing at all", json!({})),
        (
            "a named validator with both flags at their default",
            json!({
                "name": "neither",
                "validateRequestBody": false,
                "validateRequestParameters": false,
            }),
        ),
        (
            "a validator over the body only",
            json!({ "name": "body-only", "validateRequestBody": true }),
        ),
        (
            "a validator over the parameters only",
            json!({ "name": "params-only", "validateRequestParameters": true }),
        ),
        (
            "a validator over both the body and the parameters",
            json!({
                "name": "body-and-params",
                "validateRequestBody": true,
                "validateRequestParameters": true,
            }),
        ),
        (
            "an unnamed validator with both flags on",
            json!({ "validateRequestBody": true, "validateRequestParameters": true }),
        ),
        (
            "a validator whose name is outside ASCII",
            json!({ "name": "валидатор-✓", "validateRequestParameters": true }),
        ),
    ]
}

#[tokio::test]
async fn a_delete_removes_the_named_validator_and_leaves_every_sibling_standing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "request-validator-delete").await;

    // Every row of the table is created on ONE API, so each delete runs against a store
    // that holds validators of every other shape beside the one it names.
    let mut records: Vec<(&str, String, Value)> = Vec::new();
    for (case, request) in validator_table() {
        let created = created_validator(&gateway, &api, case, request).await;
        let id = string_member(&created, "id", case);
        records.push((case, id, created));
    }
    let created_order: Vec<String> = records.iter().map(|(_, id, _)| id.clone()).collect();
    assert_eq!(
        listed_ids(&gateway, &api, "GetRequestValidators before any delete").await,
        created_order,
        "case GetRequestValidators before any delete: expected the created validators in \
         creation order"
    );

    // Delete them one at a time. After each delete the named validator is gone, the report
    // lists exactly the records not yet deleted in creation order, and every one of those
    // still reads back member for member.
    for index in 0..records.len() {
        let (case, id, _) = &records[index];
        deleted_validator(&gateway, &api, id, case).await;
        assert_not_found(
            &get_validator(&gateway, &api, id).await,
            NO_SUCH_REQUEST_VALIDATOR,
            &format!("GetRequestValidator after deleting {case}"),
        );

        let expected: Vec<String> = created_order[index + 1..].to_vec();
        let listed = listed_ids(
            &gateway,
            &api,
            &format!("GetRequestValidators after deleting {case}"),
        )
        .await;
        assert_eq!(
            listed, expected,
            "case GetRequestValidators after deleting {case}: expected {expected:?}, \
             got {listed:?}"
        );

        for (other_case, other_id, created) in &records[index + 1..] {
            let read = get_validator(&gateway, &api, other_id).await;
            assert_eq!(
                read.status, 200,
                "case GetRequestValidator({other_case}) after deleting {case}: expected \
                 200, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                &read.body, created,
                "case GetRequestValidator({other_case}) after deleting {case}: expected \
                 the stored validator {created:?}, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_that_names_no_record_is_a_not_found_on_delete() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "request-validator-delete-notfound").await;
    let (other, _other_root) =
        create_api(&gateway, "request-validator-delete-notfound-other").await;
    let missing = "zzzzzzzzzz";

    let kept = created_validator(
        &gateway,
        &api,
        "the validator kept",
        json!({ "name": "kept", "validateRequestBody": true }),
    )
    .await;
    let kept_id = string_member(&kept, "id", "the validator kept");
    let elsewhere = created_validator(
        &gateway,
        &other,
        "the validator of the other API",
        json!({ "name": "elsewhere" }),
    )
    .await;
    let elsewhere_id = string_member(&elsewhere, "id", "the validator of the other API");

    // Rows span each identifier this operation resolves, one unknown at a time, each
    // asserted on the sentence its OWN identifier earns: the API is resolved first, so a
    // request whose API names nothing is refused for the API however its other identifier
    // resolves, and a request whose API does resolve is refused for the validator.
    let unknown_api = no_such_api(missing);
    for (case, target_api, target_validator, message) in [
        (
            "DeleteRequestValidator on an unknown API",
            missing,
            kept_id.as_str(),
            unknown_api.as_str(),
        ),
        (
            "DeleteRequestValidator for an unknown validator",
            api.as_str(),
            missing,
            NO_SUCH_REQUEST_VALIDATOR,
        ),
        (
            "DeleteRequestValidator on an unknown API and validator",
            missing,
            missing,
            unknown_api.as_str(),
        ),
        (
            "DeleteRequestValidator through another API",
            api.as_str(),
            elsewhere_id.as_str(),
            NO_SUCH_REQUEST_VALIDATOR,
        ),
    ] {
        assert_not_found(
            &delete_validator(&gateway, target_api, target_validator).await,
            message,
            case,
        );
    }

    // None of those refusals took anything away: both validators still read back.
    for (case, target_api, target_validator, created) in [
        ("the validator kept", api.as_str(), kept_id.as_str(), &kept),
        (
            "the validator of the other API",
            other.as_str(),
            elsewhere_id.as_str(),
            &elsewhere,
        ),
    ] {
        let read = get_validator(&gateway, target_api, target_validator).await;
        assert_eq!(
            &read.body, created,
            "case GetRequestValidator({case}) after the refusals: expected {created:?}, \
             got {:?}",
            read.body
        );
    }

    // A delete is not idempotent: the second one names no validator, because the first
    // took the record away.
    let doomed = created_validator(
        &gateway,
        &api,
        "the validator deleted twice",
        json!({ "name": "twice" }),
    )
    .await;
    let doomed_id = string_member(&doomed, "id", "the validator deleted twice");
    deleted_validator(&gateway, &api, &doomed_id, "the first delete").await;
    for round in 0..2 {
        assert_not_found(
            &delete_validator(&gateway, &api, &doomed_id).await,
            NO_SUCH_REQUEST_VALIDATOR,
            &format!("DeleteRequestValidator again (round {round})"),
        );
    }

    // The validators of a deleted API go with it, so a delete addressed through that API
    // resolves nothing either.
    let dropped = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        dropped.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        dropped.status, dropped.body
    );
    // The API itself is what no longer resolves, so this is the API's own sentence.
    assert_not_found(
        &delete_validator(&gateway, &api, &kept_id).await,
        &no_such_api(&api),
        "DeleteRequestValidator after DeleteRestApi",
    );
    // The other API is untouched by either delete.
    let read = get_validator(&gateway, &other, &elsewhere_id).await;
    assert_eq!(
        read.body, elsewhere,
        "case GetRequestValidator(the other API) after DeleteRestApi: expected \
         {elsewhere:?}, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn the_per_api_bound_counts_only_the_validators_that_still_exist() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "request-validator-delete-bound").await;

    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_REQUEST_VALIDATORS_PER_API {
        let case = format!("CreateRequestValidator(#{index})");
        let created =
            created_validator(&gateway, &api, &case, json!({ "name": format!("v-{index}") }))
                .await;
        ids.push(string_member(&created, "id", &case));
    }
    assert_failure(
        &create_validator(&gateway, &api, json!({ "name": "one-too-many" })).await,
        429,
        "LimitExceededException",
        "CreateRequestValidator past the bound",
    );

    // Deleting validators frees exactly that many slots: the record is gone from the
    // store, not merely hidden from a read.
    let freed = 3;
    for (index, id) in ids.iter().take(freed).enumerate() {
        deleted_validator(
            &gateway,
            &api,
            id,
            &format!("DeleteRequestValidator(#{index})"),
        )
        .await;
    }
    let mut replacements: Vec<String> = Vec::new();
    for index in 0..freed {
        let case = format!("CreateRequestValidator(replacement #{index})");
        let created = created_validator(
            &gateway,
            &api,
            &case,
            json!({ "name": format!("replacement-{index}") }),
        )
        .await;
        replacements.push(string_member(&created, "id", &case));
    }
    assert_failure(
        &create_validator(&gateway, &api, json!({ "name": "one-too-many-again" })).await,
        429,
        "LimitExceededException",
        "CreateRequestValidator past the bound again",
    );

    // Each replacement is its own record under its own minted id, and every validator that
    // was not deleted is still there.
    for (index, id) in replacements.iter().enumerate() {
        assert!(
            !ids[..freed].contains(id),
            "case CreateRequestValidator(replacement #{index}): expected a freshly minted \
             id, got the deleted {id:?}"
        );
        let read = get_validator(&gateway, &api, id).await;
        assert_eq!(
            read.status, 200,
            "case GetRequestValidator(replacement #{index}): expected 200, got {} with {:?}",
            read.status, read.body
        );
    }
    for (index, id) in ids.iter().enumerate().skip(freed) {
        let read = get_validator(&gateway, &api, id).await;
        assert_eq!(
            read.status, 200,
            "case GetRequestValidator(#{index}) after the deletes: expected 200, got {} \
             with {:?}",
            read.status, read.body
        );
    }
}
