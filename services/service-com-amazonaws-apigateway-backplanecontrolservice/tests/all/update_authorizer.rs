// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateAuthorizer`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the authorizer
//! create/read helpers and the PATCH wrappers are this crate's existing harness, reused
//! from `rest_api_lifecycle`, `authorizer_lifecycle` and `patch_updates`.
//!
//! The RULE under test: `UpdateAuthorizer` applies every entry of `patchOperations` to the
//! stored `Authorizer` record, in request order, and reports the patched record — which a
//! later `GetAuthorizer` reads back member for member — while a patch the service cannot
//! apply is refused with a modelled error and leaves the stored record exactly as it was.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: a table
//! of members, member types and ops; the published window of
//! `authorizerResultTtlInSeconds` (each bound, values inside it, one past each bound); the
//! modelled `AuthorizerType` catalog; the ops and paths the resource does not support; and
//! every identifier the operation resolves.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS, MAX_AUTHORIZER_RESULT_TTL_SECONDS,
    MIN_AUTHORIZER_RESULT_TTL_SECONDS,
};

use crate::authorizer_lifecycle::{
    create_authorizer, created_authorizer, get_authorizer, lambda_uri, token_request, user_pool_arn,
};
use crate::patch_updates::{patch, patch_ok, replace};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};

/// One row of the type-coherence table: the case name, the create request the row starts
/// from, and the `patchOperations` it then sends.
type CoherenceRow = (&'static str, fn(&str) -> Value, Vec<Value>);

/// One `patchOperations` entry carrying no `value`, for the ops that take none.
fn bare(kind: &str, path: &str) -> Value {
    json!({ "op": kind, "path": path })
}

/// An `add` entry.
fn add(path: &str, value: &str) -> Value {
    json!({ "op": "add", "path": path, "value": value })
}

/// The `UpdateAuthorizer` URI for one authorizer of one API.
fn authorizer_uri(api: &str, authorizer: &str) -> String {
    format!("/restapis/{api}/authorizers/{authorizer}")
}

/// A COGNITO_USER_POOLS authorizer request over one pool, so a row can vary the
/// `providerARNs` list a TOKEN authorizer does not carry.
fn cognito_request(name: &str) -> Value {
    json!({
        "name": name,
        "type": "COGNITO_USER_POOLS",
        "identitySource": "method.request.header.Authorization",
        "providerARNs": [user_pool_arn("us-east-1_base")],
    })
}

/// Create a baseline authorizer on `api` from `request` and answer its id.
async fn baseline(gateway: &Gateway, api: &str, case: &str, request: Value) -> String {
    let created = created_authorizer(gateway, api, case, request).await;
    string_member(&created, "id", case)
}

/// `UpdateAuthorizer` on an authorizer of `api`.
async fn update(gateway: &Gateway, api: &str, authorizer: &str, operations: Vec<Value>) -> Answer {
    patch(gateway, &authorizer_uri(api, authorizer), operations).await
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
async fn a_patched_authorizer_member_is_stored_and_read_back() {
    // Rows differ along what the rule generalises over: which member the path names, that
    // member's type (required string, optional string, enum-valued string, bounded
    // integer, list of strings), which op the row uses, how many ops one request carries,
    // and whether two ops target one member.
    type Row = (
        &'static str,
        fn(&str) -> Value,
        Vec<Value>,
        Vec<(&'static str, Value)>,
    );
    let rows: Vec<Row> = vec![
        (
            "replace the name",
            token_request,
            vec![replace("/name", "renamed")],
            vec![("name", json!("renamed"))],
        ),
        (
            "replace the authorizer URI",
            token_request,
            vec![replace("/authorizerUri", &lambda_uri("other"))],
            vec![("authorizerUri", json!(lambda_uri("other")))],
        ),
        (
            "add the credentials an absent member had none of",
            token_request,
            vec![add(
                "/authorizerCredentials",
                "arn:aws:iam::123456789012:role/invoke",
            )],
            vec![(
                "authorizerCredentials",
                json!("arn:aws:iam::123456789012:role/invoke"),
            )],
        ),
        (
            "remove the credentials, which the member documents as nullable",
            token_request,
            vec![
                replace(
                    "/authorizerCredentials",
                    "arn:aws:iam::123456789012:role/invoke",
                ),
                bare("remove", "/authorizerCredentials"),
            ],
            vec![("authorizerCredentials", Value::Null)],
        ),
        (
            "replace the identity source",
            token_request,
            vec![replace("/identitySource", "method.request.header.Token")],
            vec![("identitySource", json!("method.request.header.Token"))],
        ),
        (
            "replace then remove the identity validation expression",
            token_request,
            vec![
                replace("/identityValidationExpression", "^Bearer .+$"),
                bare("remove", "/identityValidationExpression"),
            ],
            vec![("identityValidationExpression", Value::Null)],
        ),
        (
            "replace the authType",
            token_request,
            vec![replace("/authType", "custom")],
            vec![("authType", json!("custom"))],
        ),
        (
            "replace the cached result TTL",
            token_request,
            vec![replace("/authorizerResultTtlInSeconds", "42")],
            vec![("authorizerResultTtlInSeconds", json!(42))],
        ),
        (
            "two ops on one member: the last one in request order stands",
            token_request,
            vec![
                replace("/authorizerResultTtlInSeconds", "11"),
                replace("/authorizerResultTtlInSeconds", "22"),
            ],
            vec![("authorizerResultTtlInSeconds", json!(22))],
        ),
        (
            "four ops across four members in one request",
            token_request,
            vec![
                replace("/name", "four-ops"),
                replace("/authType", "custom"),
                replace("/authorizerUri", &lambda_uri("four")),
                replace("/authorizerResultTtlInSeconds", "0"),
            ],
            vec![
                ("name", json!("four-ops")),
                ("authType", json!("custom")),
                ("authorizerUri", json!(lambda_uri("four"))),
                ("authorizerResultTtlInSeconds", json!(0)),
            ],
        ),
        (
            "replace the type with another the record's members admit",
            token_request,
            vec![replace("/type", "REQUEST")],
            vec![("type", json!("REQUEST"))],
        ),
        (
            "add a second provider ARN to the list",
            cognito_request,
            vec![add("/providerARNs", &user_pool_arn("us-east-1_second"))],
            vec![(
                "providerARNs",
                json!([
                    user_pool_arn("us-east-1_base"),
                    user_pool_arn("us-east-1_second")
                ]),
            )],
        ),
        (
            "add a provider ARN addressed by the path rather than the value",
            cognito_request,
            vec![add(
                &format!("/providerARNs/{}", user_pool_arn("us-east-1_pathed")),
                "",
            )],
            vec![(
                "providerARNs",
                json!([
                    user_pool_arn("us-east-1_base"),
                    user_pool_arn("us-east-1_pathed")
                ]),
            )],
        ),
        (
            "add then remove a provider ARN, leaving the list as it was",
            cognito_request,
            vec![
                add("/providerARNs", &user_pool_arn("us-east-1_transient")),
                bare(
                    "remove",
                    &format!("/providerARNs/{}", user_pool_arn("us-east-1_transient")),
                ),
            ],
            vec![("providerARNs", json!([user_pool_arn("us-east-1_base")]))],
        ),
        (
            "an empty patch list changes nothing",
            token_request,
            vec![],
            vec![
                ("name", json!("baseline")),
                (
                    "authorizerResultTtlInSeconds",
                    json!(DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS),
                ),
            ],
        ),
    ];

    let gateway = Gateway::new();

    for (index, (case, request, operations, expectations)) in rows.into_iter().enumerate() {
        // One API per row: an API holds at most the documented quota of authorizers, and
        // the table is longer than that.
        let (api, _root) =
            create_api(&gateway, &format!("update-authorizer-members-{index}")).await;
        let id = baseline(&gateway, &api, case, request("baseline")).await;
        let patched = patch_ok(&gateway, case, &authorizer_uri(&api, &id), operations).await;
        assert_members(&patched, &expectations, case, "UpdateAuthorizer");
        assert_eq!(
            patched["id"].as_str(),
            Some(id.as_str()),
            "case {case}: the patched authorizer keeps its id, got {:?}",
            patched["id"]
        );

        // The response IS the stored record: the reader reports it member for member.
        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetAuthorizer, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, patched,
            "case {case}: GetAuthorizer must read back the patched authorizer, got {:?} after {:?}",
            read.body, patched
        );
    }
}

#[tokio::test]
async fn a_patched_ttl_takes_the_window_the_member_documents() {
    // From `Authorizer$authorizerResultTtlInSeconds`: "If it equals 0, authorization
    // caching is disabled", "If this field is not set, the default value is 300", "The
    // maximum value is 3600, or 1 hour." A patch is bounded by the same window a create
    // is, so the accepted rows are each published bound and values inside it, and the
    // refused rows are one past each bound and further outside it.
    let accepted = [
        MIN_AUTHORIZER_RESULT_TTL_SECONDS,
        MIN_AUTHORIZER_RESULT_TTL_SECONDS + 1,
        DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS,
        MAX_AUTHORIZER_RESULT_TTL_SECONDS - 1,
        MAX_AUTHORIZER_RESULT_TTL_SECONDS,
    ];
    let refused = [
        MIN_AUTHORIZER_RESULT_TTL_SECONDS - 1,
        MAX_AUTHORIZER_RESULT_TTL_SECONDS + 1,
        -300,
        86_400,
        i32::MAX,
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-authorizer-ttl").await;

    // A create that sets no TTL stores the documented default, and the update is what
    // moves it: a TOKEN authorizer needs an identity source at every TTL, so the row
    // varies only the TTL.
    let created = created_authorizer(
        &gateway,
        &api,
        "the baseline authorizer",
        token_request("ttl-baseline"),
    )
    .await;
    assert_eq!(
        created["authorizerResultTtlInSeconds"].as_i64(),
        Some(i64::from(DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS)),
        "case CreateAuthorizer(no ttl): expected the documented default, got {:?}",
        created["authorizerResultTtlInSeconds"]
    );
    let id = string_member(&created, "id", "the baseline authorizer");

    for ttl in accepted {
        let case = format!("UpdateAuthorizer(ttl={ttl})");
        let patched = patch_ok(
            &gateway,
            &case,
            &authorizer_uri(&api, &id),
            vec![replace("/authorizerResultTtlInSeconds", &ttl.to_string())],
        )
        .await;
        assert_eq!(
            patched["authorizerResultTtlInSeconds"].as_i64(),
            Some(i64::from(ttl)),
            "case {case}: expected the patched TTL back, got {:?}",
            patched["authorizerResultTtlInSeconds"]
        );
        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.body["authorizerResultTtlInSeconds"].as_i64(),
            Some(i64::from(ttl)),
            "case GetAuthorizer(ttl={ttl}): expected the stored TTL, got {:?}",
            read.body
        );
    }

    // The last accepted row stands as the stored TTL every refusal below must leave alone.
    let held = MAX_AUTHORIZER_RESULT_TTL_SECONDS;
    for ttl in refused {
        let case = format!("UpdateAuthorizer(ttl={ttl})");
        let answer = update(
            &gateway,
            &api,
            &id,
            vec![replace("/authorizerResultTtlInSeconds", &ttl.to_string())],
        )
        .await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.body["authorizerResultTtlInSeconds"].as_i64(),
            Some(i64::from(held)),
            "case {case}: a refused patch must leave the stored TTL alone, got {:?}",
            read.body["authorizerResultTtlInSeconds"]
        );
    }

    // A stored authorizer always carries a TTL — the service fills one in — so the member
    // is replaced, never cleared, and a value that is not an integer is not a TTL.
    for (case, operations) in [
        (
            "remove the TTL",
            vec![bare("remove", "/authorizerResultTtlInSeconds")],
        ),
        (
            "a TTL that is not an integer",
            vec![replace("/authorizerResultTtlInSeconds", "not-a-number")],
        ),
        (
            "a replace of the TTL carrying no value",
            vec![bare("replace", "/authorizerResultTtlInSeconds")],
        ),
    ] {
        let answer = update(&gateway, &api, &id, operations).await;
        assert_failure(&answer, 400, "BadRequestException", case);
    }
}

#[tokio::test]
async fn the_patched_record_must_still_be_an_authorizer_its_own_type_admits() {
    // The members each `type` requires are the ones the members document, and the rule
    // holds of the RECORD: whichever member the patch moved, the authorizer that is stored
    // has to be one its own type admits. Rows span the catalog from
    // `smithy_query(query="shape", shape="com.amazonaws.apigateway#AuthorizerType")`.
    let refused: Vec<CoherenceRow> = vec![
        (
            "a TOKEN authorizer losing its identity source",
            token_request,
            vec![bare("remove", "/identitySource")],
        ),
        (
            "a TOKEN authorizer losing its authorizer URI",
            token_request,
            vec![bare("remove", "/authorizerUri")],
        ),
        (
            "a TOKEN authorizer becoming COGNITO_USER_POOLS with no provider ARNs",
            token_request,
            vec![replace("/type", "COGNITO_USER_POOLS")],
        ),
        (
            "a COGNITO_USER_POOLS authorizer losing its last provider ARN",
            cognito_request,
            vec![bare(
                "remove",
                &format!("/providerARNs/{}", user_pool_arn("us-east-1_base")),
            )],
        ),
        (
            "a COGNITO_USER_POOLS authorizer becoming TOKEN with no authorizer URI",
            cognito_request,
            vec![replace("/type", "TOKEN")],
        ),
    ];
    let accepted: Vec<CoherenceRow> = vec![
        (
            "a TOKEN authorizer losing the identity source it keeps as REQUEST with caching off",
            token_request,
            vec![
                replace("/type", "REQUEST"),
                replace("/authorizerResultTtlInSeconds", "0"),
                bare("remove", "/identitySource"),
            ],
        ),
        (
            "a TOKEN authorizer becoming COGNITO_USER_POOLS with a provider ARN added first",
            token_request,
            vec![
                add("/providerARNs", &user_pool_arn("us-east-1_added")),
                replace("/type", "COGNITO_USER_POOLS"),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-authorizer-coherence").await;

    for (case, request, operations) in refused {
        let id = baseline(&gateway, &api, case, request("coherence")).await;
        let before = get_authorizer(&gateway, &api, &id).await;
        let answer = update(&gateway, &api, &id, operations).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        let after = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            after.body, before.body,
            "case {case}: a refused patch must leave the record untouched, got {:?} instead of {:?}",
            after.body, before.body
        );
    }
    for (case, request, operations) in accepted {
        let id = baseline(&gateway, &api, case, request("coherence-ok")).await;
        let patched = patch_ok(&gateway, case, &authorizer_uri(&api, &id), operations).await;
        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.body, patched,
            "case {case}: GetAuthorizer must read back the patched authorizer, got {:?} after {:?}",
            read.body, patched
        );
    }
}

#[tokio::test]
async fn an_op_or_path_the_resource_does_not_support_is_refused_and_changes_nothing() {
    // Rows span the ops the resource does not support, the paths it has no updateable
    // member for, a pointer reaching into a single-valued member, a `type` outside the
    // modelled catalog, an empty value for a member that always has one, and a list whose
    // LAST op is the refused one — which is what proves the whole list is applied or none
    // of it is.
    let rows: Vec<(&str, Vec<Value>)> = vec![
        ("a move", vec![json!({
            "op": "move",
            "path": "/name",
            "from": "/authType",
        })]),
        ("a copy", vec![json!({
            "op": "copy",
            "path": "/name",
            "from": "/authType",
        })]),
        ("a test", vec![replace("/name", "settled")]
            .into_iter()
            .chain([json!({ "op": "test", "path": "/name", "value": "settled" })])
            .collect()),
        ("an op outside the modelled Op catalog", vec![json!({
            "op": "increment",
            "path": "/authorizerResultTtlInSeconds",
            "value": "1",
        })]),
        ("an entry carrying no op", vec![json!({ "path": "/name", "value": "x" })]),
        ("an entry carrying no path", vec![json!({ "op": "replace", "value": "x" })]),
        ("a path that is not a JSON Pointer", vec![replace("name", "x")]),
        ("the empty path", vec![replace("", "x")]),
        ("the service-assigned id", vec![replace("/id", "0123456789")]),
        ("a member no Authorizer shape declares", vec![replace("/enabled", "true")]),
        ("a pointer into a single-valued member", vec![replace("/name/first", "x")]),
        ("a type outside the modelled catalog", vec![replace("/type", "MUTUAL_TLS")]),
        ("a name replaced with the empty string", vec![replace("/name", "")]),
        ("a removed name, which an authorizer always reports", vec![bare("remove", "/name")]),
        (
            "a supported op followed by a refused one",
            vec![
                replace("/name", "applied-then-refused"),
                replace("/id", "0123456789"),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-authorizer-refusals").await;
    let id = baseline(
        &gateway,
        &api,
        "the authorizer every refusal targets",
        token_request("refusals"),
    )
    .await;
    let before = get_authorizer(&gateway, &api, &id).await;

    for (case, operations) in rows {
        let answer = update(&gateway, &api, &id, operations).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        let after = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            after.body, before.body,
            "case {case}: a refused patch must leave the record untouched, got {:?} instead of {:?}",
            after.body, before.body
        );
    }

    // The refusal names the path, the op and the paths the resource does have, so a
    // client can fix the request from the message alone.
    let answer = update(&gateway, &api, &id, vec![replace("/id", "0123456789")]).await;
    let message = answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case the unsupported path message: expected a message, got {:?}", answer.body));
    for expected in ["/id", "replace", "/name", "/authorizerResultTtlInSeconds"] {
        assert!(
            message.contains(expected),
            "case the unsupported path message: expected {expected:?} in {message:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_every_patch() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-authorizer-notfound").await;
    let missing = "zzzzzzzzzz";
    let id = baseline(
        &gateway,
        &api,
        "the authorizer to patch",
        token_request("present"),
    )
    .await;

    // Rows span each identifier this operation resolves, one unknown at a time.
    assert_failure(
        &update(&gateway, missing, &id, vec![replace("/name", "x")]).await,
        404,
        "NotFoundException",
        "UpdateAuthorizer on an unknown API",
    );
    assert_failure(
        &update(&gateway, &api, missing, vec![replace("/name", "x")]).await,
        404,
        "NotFoundException",
        "UpdateAuthorizer for an unknown authorizer",
    );

    // An authorizer belongs to its own API: another API cannot patch it.
    let (other, _other_root) = create_api(&gateway, "update-authorizer-notfound-other").await;
    assert_failure(
        &update(&gateway, &other, &id, vec![replace("/name", "x")]).await,
        404,
        "NotFoundException",
        "UpdateAuthorizer through another API",
    );
    // And the refusal through the other API left the record where it is.
    let read = get_authorizer(&gateway, &api, &id).await;
    assert_eq!(
        read.body["name"].as_str(),
        Some("present"),
        "case UpdateAuthorizer through another API: the record must stand, got {:?}",
        read.body
    );

    // An unknown API is a not-found before an unknown authorizer of it is: the API is
    // what the authorizer hangs off.
    assert_failure(
        &create_authorizer(&gateway, missing, token_request("orphan")).await,
        404,
        "NotFoundException",
        "CreateAuthorizer on an unknown API",
    );

    // The authorizers of a deleted API go with it, so a patch of one is a not-found too.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &update(&gateway, &api, &id, vec![replace("/name", "x")]).await,
        404,
        "NotFoundException",
        "UpdateAuthorizer after DeleteRestApi",
    );
}
