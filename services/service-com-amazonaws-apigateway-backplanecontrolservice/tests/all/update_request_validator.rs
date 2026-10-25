// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateRequestValidator`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the validator
//! create/read helpers and the PATCH wrappers are this crate's existing harness, reused
//! from `rest_api_lifecycle`, `request_validator_lifecycle` and `patch_updates`.
//!
//! The RULE under test: `UpdateRequestValidator` applies every entry of `patchOperations`
//! to the stored `RequestValidator` record of the named API, in request order, and reports
//! the patched record — which a later `GetRequestValidator` reads back member for member —
//! while a patch the service cannot apply is refused with a modelled error and leaves the
//! stored record exactly as it was.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: a table of
//! members, member types and ops; the boolean value spellings the two modelled flags take;
//! the ops and paths the resource does not support; the `name` character bound and one past
//! it; repeated application of one list; and every identifier the operation resolves.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::MAX_STRING_CHARS;

use crate::patch_updates::{patch, patch_ok, replace};
use crate::request_validator_lifecycle::{created_validator, get_validator};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};

/// One `patchOperations` entry, as a client spells it on the wire; `None` is an op that
/// carries no `value`.
fn op(kind: &str, path: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "op": kind, "path": path, "value": value }),
        None => json!({ "op": kind, "path": path }),
    }
}

/// The `UpdateRequestValidator`/`GetRequestValidator` URI for one validator of one API.
fn validator_uri(api: &str, validator: &str) -> String {
    format!("/restapis/{api}/requestvalidators/{validator}")
}

/// The baseline validator every row that patches a named record starts from: a name and
/// both flags at known values, so a replace has something to move.
fn baseline_request() -> Value {
    json!({
        "name": "baseline-validator",
        "validateRequestBody": true,
        "validateRequestParameters": false,
    })
}

/// Create a validator on `api` from `request` and answer its id.
async fn baseline(gateway: &Gateway, api: &str, case: &str, request: Value) -> String {
    let created = created_validator(gateway, api, case, request).await;
    string_member(&created, "id", case)
}

/// `UpdateRequestValidator` on one validator of `api`.
async fn update(gateway: &Gateway, api: &str, validator: &str, operations: Vec<Value>) -> Answer {
    patch(gateway, &validator_uri(api, validator), operations).await
}

/// Assert every `(member, expected)` pair holds in `body`; a `Value::Null` expectation is
/// the member being ABSENT, which is what a cleared optional member looks like on the wire.
fn assert_members(body: &Value, expectations: &[(&str, Value)], case: &str, source: &str) {
    for (member, expected) in expectations {
        assert_eq!(
            body[*member], *expected,
            "case {case}: {source} must report {member} as {expected:?}, got {:?}",
            body[*member]
        );
    }
}

#[tokio::test]
async fn a_patched_request_validator_member_is_stored_and_read_back() {
    // Rows differ along what the rule generalises over: which member the path names, that
    // member's type (an optional string, each of the two flags modelled `@default false`
    // per `smithy_query(query="operation", operation="UpdateRequestValidator")`), which op
    // the row uses, whether the record started with the member set, how many ops one
    // request carries, and whether two ops target one member.
    type Row = (
        &'static str,
        Value,
        Vec<Value>,
        Vec<(&'static str, Value)>,
    );
    let rows: Vec<Row> = vec![
        (
            "replace the name",
            baseline_request(),
            vec![replace("/name", "renamed")],
            vec![
                ("name", json!("renamed")),
                ("validateRequestBody", json!(true)),
                ("validateRequestParameters", json!(false)),
            ],
        ),
        (
            "add a name the record had none of",
            json!({}),
            vec![op("add", "/name", Some("first-name"))],
            vec![
                ("name", json!("first-name")),
                ("validateRequestBody", json!(false)),
                ("validateRequestParameters", json!(false)),
            ],
        ),
        (
            "remove the name of an optional member",
            baseline_request(),
            vec![op("remove", "/name", None)],
            vec![
                ("name", Value::Null),
                ("validateRequestBody", json!(true)),
                ("validateRequestParameters", json!(false)),
            ],
        ),
        (
            "turn the body flag off",
            baseline_request(),
            vec![replace("/validateRequestBody", "false")],
            vec![
                ("name", json!("baseline-validator")),
                ("validateRequestBody", json!(false)),
                ("validateRequestParameters", json!(false)),
            ],
        ),
        (
            "turn the parameters flag on",
            baseline_request(),
            vec![replace("/validateRequestParameters", "true")],
            vec![
                ("name", json!("baseline-validator")),
                ("validateRequestBody", json!(true)),
                ("validateRequestParameters", json!(true)),
            ],
        ),
        (
            "add on a flag sets it just as replace does",
            baseline_request(),
            vec![op("add", "/validateRequestParameters", Some("true"))],
            vec![
                ("name", json!("baseline-validator")),
                ("validateRequestBody", json!(true)),
                ("validateRequestParameters", json!(true)),
            ],
        ),
        (
            "both flags and the name in one request",
            baseline_request(),
            vec![
                replace("/name", "all-three"),
                replace("/validateRequestBody", "false"),
                replace("/validateRequestParameters", "true"),
            ],
            vec![
                ("name", json!("all-three")),
                ("validateRequestBody", json!(false)),
                ("validateRequestParameters", json!(true)),
            ],
        ),
        (
            "two ops on one member apply in request order",
            baseline_request(),
            vec![
                replace("/validateRequestParameters", "true"),
                replace("/validateRequestParameters", "false"),
            ],
            vec![
                ("name", json!("baseline-validator")),
                ("validateRequestBody", json!(true)),
                ("validateRequestParameters", json!(false)),
            ],
        ),
        (
            "a name cleared and then set again",
            baseline_request(),
            vec![op("remove", "/name", None), replace("/name", "set-again")],
            vec![
                ("name", json!("set-again")),
                ("validateRequestBody", json!(true)),
                ("validateRequestParameters", json!(false)),
            ],
        ),
        (
            "a name outside ASCII",
            baseline_request(),
            vec![replace("/name", "валидатор-✓")],
            vec![("name", json!("валидатор-✓"))],
        ),
        (
            "a name at the character bound",
            baseline_request(),
            vec![replace("/name", &"n".repeat(MAX_STRING_CHARS))],
            vec![("name", json!("n".repeat(MAX_STRING_CHARS)))],
        ),
        (
            "an empty list reports the record unchanged",
            baseline_request(),
            vec![],
            vec![
                ("name", json!("baseline-validator")),
                ("validateRequestBody", json!(true)),
                ("validateRequestParameters", json!(false)),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-request-validator-rows").await;

    for (case, request, operations, expected) in rows {
        let id = baseline(&gateway, &api, case, request).await;
        let updated = patch_ok(&gateway, case, &validator_uri(&api, &id), operations).await;
        assert_members(&updated, &expected, case, "UpdateRequestValidator");
        assert_eq!(
            updated["id"],
            json!(id),
            "case {case}: the update must report the record's own id, got {:?}",
            updated["id"]
        );

        // The patched record is what is STORED, not just what the update answered.
        let read = get_validator(&gateway, &api, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetRequestValidator, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, updated,
            "case {case}: GetRequestValidator must read back the patched record, got {:?} \
             after {:?}",
            read.body, updated
        );
    }
}

#[tokio::test]
async fn the_flag_value_spellings_a_boolean_member_takes() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-request-validator-flag-values").await;

    // A patch `value` is a bare string on the wire, so each flag's two modelled values are
    // read from text: the spelling the SDK sends, and the ones that differ only in case or
    // surrounding space. Anything else is not a boolean and is refused.
    for (spelling, expected) in [
        ("true", true),
        ("false", false),
        ("TRUE", true),
        ("False", false),
        (" true ", true),
    ] {
        let case = format!("UpdateRequestValidator(value {spelling:?})");
        let id = baseline(&gateway, &api, &case, json!({ "name": "flags" })).await;
        let updated = patch_ok(
            &gateway,
            &case,
            &validator_uri(&api, &id),
            vec![
                replace("/validateRequestBody", spelling),
                replace("/validateRequestParameters", spelling),
            ],
        )
        .await;
        assert_members(
            &updated,
            &[
                ("validateRequestBody", json!(expected)),
                ("validateRequestParameters", json!(expected)),
            ],
            &case,
            "UpdateRequestValidator",
        );
    }

    for spelling in ["yes", "1", "", "tru", "null"] {
        let case = format!("UpdateRequestValidator(non-boolean {spelling:?})");
        let id = baseline(&gateway, &api, &case, json!({ "name": "flags" })).await;
        let answer = update(
            &gateway,
            &api,
            &id,
            vec![replace("/validateRequestBody", spelling)],
        )
        .await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        let read = get_validator(&gateway, &api, &id).await;
        assert_members(
            &read.body,
            &[("validateRequestBody", json!(false))],
            &case,
            "GetRequestValidator",
        );
    }
}

#[tokio::test]
async fn a_patch_this_operation_cannot_apply_is_refused_and_changes_nothing() {
    // Rows differ along the KIND of refusal: an op no single-valued member supports, an op
    // missing its value, a path naming a member that is not updateable, a path that is not
    // a JSON Pointer, a path reaching into a single-valued member, a remove of a member
    // that always has a value, a value past the character bound, and a list whose LAST op
    // is refused (so the earlier ops must not be kept either).
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("move is not supported", vec![op("move", "/name", None)]),
        (
            "copy is not supported",
            vec![json!({ "op": "copy", "path": "/name", "from": "/name" })],
        ),
        ("test is not supported", vec![op("test", "/name", None)]),
        ("a replace with no value", vec![op("replace", "/name", None)]),
        ("an entry with no path", vec![json!({ "op": "replace" })]),
        ("an entry with no op", vec![json!({ "path": "/name" })]),
        (
            "the id is reported but not patchable",
            vec![replace("/id", "zzzzzzzzzz")],
        ),
        (
            "a path naming no updateable member",
            vec![replace("/nosuchmember", "x")],
        ),
        (
            "a path that is not a JSON Pointer",
            vec![replace("name", "x")],
        ),
        ("an empty path", vec![replace("", "x")]),
        (
            "a path reaching into a single-valued member",
            vec![replace("/name/first", "x")],
        ),
        (
            "remove of a flag that always has a value",
            vec![op("remove", "/validateRequestBody", None)],
        ),
        (
            "remove of the other flag",
            vec![op("remove", "/validateRequestParameters", None)],
        ),
        (
            "a name one character past the bound",
            vec![replace("/name", &"n".repeat(MAX_STRING_CHARS + 1))],
        ),
        (
            "a list whose last op is refused",
            vec![
                replace("/name", "never stored"),
                replace("/nosuchmember", "x"),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-request-validator-refusals").await;
    let baseline_members = vec![
        ("name", json!("baseline-validator")),
        ("validateRequestBody", json!(true)),
        ("validateRequestParameters", json!(false)),
    ];

    for (case, operations) in &cases {
        let id = baseline(&gateway, &api, case, baseline_request()).await;
        let answer = update(&gateway, &api, &id, operations.clone()).await;
        assert_failure(&answer, 400, "BadRequestException", case);

        // Nothing was applied: the stored record is the one the validator was created with.
        let read = get_validator(&gateway, &api, &id).await;
        assert_members(&read.body, &baseline_members, case, "GetRequestValidator");
    }

    // A path the resource has no updateable member for is refused with the fixed sentence
    // and the path the request sent, and nothing else.
    let id = baseline(&gateway, &api, "the message of an unsupported path", baseline_request()).await;
    let answer = update(&gateway, &api, &id, vec![replace("/id", "zzzzzzzzzz")]).await;
    let message = answer.body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, "Invalid patch path /id",
        "case the message of an unsupported path: expected the fixed sentence and the path, \
         got {message:?}"
    );
}

#[tokio::test]
async fn every_identifier_the_update_resolves_is_a_not_found_when_it_names_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-request-validator-notfound").await;
    let missing = "zzzzzzzzzz";
    let id = baseline(&gateway, &api, "the validator to patch", baseline_request()).await;
    let operations = vec![replace("/name", "never applied")];

    // Rows span each identifier the operation resolves, one unknown at a time.
    assert_failure(
        &update(&gateway, missing, &id, operations.clone()).await,
        404,
        "NotFoundException",
        "UpdateRequestValidator on an unknown API",
    );
    assert_failure(
        &update(&gateway, &api, missing, operations.clone()).await,
        404,
        "NotFoundException",
        "UpdateRequestValidator for an unknown validator",
    );

    // A validator belongs to its own API: it cannot be patched through another one.
    let (other, _other_root) = create_api(&gateway, "update-request-validator-notfound-other").await;
    assert_failure(
        &update(&gateway, &other, &id, operations.clone()).await,
        404,
        "NotFoundException",
        "UpdateRequestValidator through another API",
    );

    // None of those refusals touched the record.
    let read = get_validator(&gateway, &api, &id).await;
    assert_members(
        &read.body,
        &[("name", json!("baseline-validator"))],
        "after the refusals",
        "GetRequestValidator",
    );

    // The validators of a deleted API go with it, patch included.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &update(&gateway, &api, &id, operations).await,
        404,
        "NotFoundException",
        "UpdateRequestValidator after DeleteRestApi",
    );
}

#[tokio::test]
async fn one_validator_is_patched_and_its_siblings_are_left_alone() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-request-validator-isolation").await;

    let target = baseline(&gateway, &api, "the validator to patch", baseline_request()).await;
    let sibling = baseline(
        &gateway,
        &api,
        "the sibling validator",
        json!({ "name": "sibling", "validateRequestParameters": true }),
    )
    .await;

    // Applying one list repeatedly lands on the same record: the patch sets members, it
    // does not accumulate.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let case = format!("UpdateRequestValidator(round {round})");
        seen.push(
            patch_ok(
                &gateway,
                &case,
                &validator_uri(&api, &target),
                vec![
                    replace("/name", "settled"),
                    replace("/validateRequestBody", "false"),
                    replace("/validateRequestParameters", "true"),
                ],
            )
            .await,
        );
    }
    assert_eq!(
        seen[0], seen[2],
        "case UpdateRequestValidator repeated: the same list must land on the same record, \
         got {:?} then {:?}",
        seen[0], seen[2]
    );

    // The sibling is the record its own create left behind.
    let read = get_validator(&gateway, &api, &sibling).await;
    assert_members(
        &read.body,
        &[
            ("name", json!("sibling")),
            ("validateRequestBody", json!(false)),
            ("validateRequestParameters", json!(true)),
        ],
        "the sibling validator",
        "GetRequestValidator",
    );
    assert_eq!(
        read.body["id"],
        json!(sibling),
        "case the sibling validator: expected its own id, got {:?}",
        read.body["id"]
    );
}
