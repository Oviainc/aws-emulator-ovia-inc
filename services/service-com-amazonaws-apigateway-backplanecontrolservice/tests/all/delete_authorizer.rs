// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteAuthorizer`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the `CreateRestApi` helper and the
//! `CreateAuthorizer`/`GetAuthorizer` helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle` and `authorizer_lifecycle`.
//!
//! The tests assert the RULE over sets of inputs rather than one recorded call: a delete
//! removes exactly the one record its `restApiId`/`authorizerId` pair names and leaves
//! every sibling record standing, over a table spanning the modelled `AuthorizerType`
//! catalog and which optional members an authorizer carries; an identifier that names no
//! stored record is a `404 NotFoundException` on every row (an unknown API, an unknown
//! authorizer, an authorizer of another API, a second delete of the same id, and a delete
//! after the API itself is gone); and the per-API authorizer quota counts only the
//! authorizers that still exist, so a delete really takes a record away rather than
//! marking it.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_AUTHORIZERS_PER_API, MAX_AUTHORIZER_RESULT_TTL_SECONDS,
};

use crate::authorizer_lifecycle::{
    created_authorizer, get_authorizer, lambda_uri, user_pool_arn,
};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};

/// `DeleteAuthorizer` for `authorizer` of `api`.
async fn delete_authorizer(gateway: &Gateway, api: &str, authorizer: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/authorizers/{authorizer}"),
            None,
        )
        .await
}

/// `DeleteAuthorizer`, asserting the modelled 202 and an empty body.
async fn deleted_authorizer(gateway: &Gateway, api: &str, authorizer: &str, case: &str) {
    let answer = delete_authorizer(gateway, api, authorizer).await;
    assert_eq!(
        answer.status, 202,
        "case {case}: expected 202 from DeleteAuthorizer, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "case {case}: DeleteAuthorizer answers no body, got {:?}",
        answer.body
    );
}

/// A TOKEN authorizer request carrying every member that type requires, so a case that
/// varies one member varies only that one.
///
/// The sibling `authorizer_lifecycle` suite builds the same request for its own cases
/// from a private helper of its own: this suite keeps a local one rather than widening
/// that helper's visibility, because every apigateway fix in flight edits that file and a
/// one-line change there costs this fix a serialized squash merge. The shared pieces this
/// suite does reuse — the emulator harness, `CreateAuthorizer`/`GetAuthorizer` callers and
/// the ARN builders — are imported above.
fn token_request(name: &str) -> Value {
    json!({
        "name": name,
        "type": "TOKEN",
        "identitySource": "method.request.header.Auth",
        "authorizerUri": lambda_uri("authorize"),
    })
}

/// The table every "delete one of these" case runs over. Rows differ along what the rule
/// generalises over: the authorizer `type` (the whole modelled `AuthorizerType` catalog,
/// from `smithy_query(query="shape", shape="com.amazonaws.apigateway#AuthorizerType")`),
/// which optional members the request carries, whether caching is on, and how many
/// provider ARNs it sets.
fn authorizer_table() -> Vec<(&'static str, Value)> {
    vec![
        ("a TOKEN authorizer with only its required members", token_request("token-bare")),
        (
            "a TOKEN authorizer with every optional member set",
            json!({
                "name": "token-full",
                "type": "TOKEN",
                "identitySource": "method.request.header.Authorization",
                "authorizerUri": lambda_uri("full"),
                "authorizerCredentials": "arn:aws:iam::123456789012:role/invoke",
                "identityValidationExpression": "^Bearer [-0-9a-zA-Z._]+$",
                "authType": "custom",
                "authorizerResultTtlInSeconds": 60,
            }),
        ),
        (
            "a REQUEST authorizer with caching disabled",
            json!({
                "name": "request-uncached",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "authorizerResultTtlInSeconds": 0,
            }),
        ),
        (
            "a REQUEST authorizer caching on two identity sources",
            json!({
                "name": "request-cached",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "identitySource": "method.request.header.Auth,method.request.querystring.Name",
                "authorizerResultTtlInSeconds": 120,
            }),
        ),
        (
            "a COGNITO_USER_POOLS authorizer over one pool",
            json!({
                "name": "cognito-one",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [user_pool_arn("us-east-1_one")],
            }),
        ),
        (
            "a COGNITO_USER_POOLS authorizer over three pools",
            json!({
                "name": "cognito-three",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [
                    user_pool_arn("us-east-1_a"),
                    user_pool_arn("us-east-1_b"),
                    user_pool_arn("us-east-1_c"),
                ],
                "authorizerResultTtlInSeconds": MAX_AUTHORIZER_RESULT_TTL_SECONDS,
            }),
        ),
    ]
}

#[tokio::test]
async fn a_delete_removes_the_named_authorizer_and_leaves_every_sibling_standing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-delete").await;

    // Every row of the table is created on ONE API, so each delete runs against a store
    // that holds authorizers of every other shape beside the one it names.
    let mut records: Vec<(&str, String, Value)> = Vec::new();
    for (case, request) in authorizer_table() {
        let created = created_authorizer(&gateway, &api, case, request).await;
        let id = string_member(&created, "id", case);
        records.push((case, id, created));
    }

    // Delete them one at a time. After each delete the named authorizer is gone and every
    // record not yet deleted still reads back member for member.
    for index in 0..records.len() {
        let (case, id, _) = &records[index];
        deleted_authorizer(&gateway, &api, id, case).await;
        assert_failure(
            &get_authorizer(&gateway, &api, id).await,
            404,
            "NotFoundException",
            &format!("GetAuthorizer after deleting {case}"),
        );
        for (other_case, other_id, created) in &records[index + 1..] {
            let read = get_authorizer(&gateway, &api, other_id).await;
            assert_eq!(
                read.status, 200,
                "case GetAuthorizer({other_case}) after deleting {case}: expected 200, \
                 got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                &read.body, created,
                "case GetAuthorizer({other_case}) after deleting {case}: expected the \
                 stored authorizer {created:?}, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_that_names_no_record_is_a_not_found_on_delete() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-delete-notfound").await;
    let (other, _other_root) = create_api(&gateway, "authorizer-delete-notfound-other").await;
    let missing = "zzzzzzzzzz";

    let kept = created_authorizer(&gateway, &api, "the authorizer kept", token_request("kept")).await;
    let kept_id = string_member(&kept, "id", "the authorizer kept");
    let elsewhere = created_authorizer(
        &gateway,
        &other,
        "the authorizer of the other API",
        token_request("elsewhere"),
    )
    .await;
    let elsewhere_id = string_member(&elsewhere, "id", "the authorizer of the other API");

    // Rows span each identifier this operation resolves, one unknown at a time.
    for (case, target_api, target_authorizer) in [
        ("DeleteAuthorizer on an unknown API", missing, kept_id.as_str()),
        ("DeleteAuthorizer for an unknown authorizer", api.as_str(), missing),
        ("DeleteAuthorizer on an unknown API and authorizer", missing, missing),
        (
            "DeleteAuthorizer through another API",
            api.as_str(),
            elsewhere_id.as_str(),
        ),
    ] {
        assert_failure(
            &delete_authorizer(&gateway, target_api, target_authorizer).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // None of those refusals took anything away: both authorizers still read back.
    for (case, target_api, target_authorizer, created) in [
        ("the authorizer kept", api.as_str(), kept_id.as_str(), &kept),
        (
            "the authorizer of the other API",
            other.as_str(),
            elsewhere_id.as_str(),
            &elsewhere,
        ),
    ] {
        let read = get_authorizer(&gateway, target_api, target_authorizer).await;
        assert_eq!(
            &read.body, created,
            "case GetAuthorizer({case}) after the refusals: expected {created:?}, got {:?}",
            read.body
        );
    }

    // A delete is not idempotent: the second one names no authorizer, because the first
    // took the record away.
    let doomed =
        created_authorizer(&gateway, &api, "the authorizer deleted twice", token_request("twice"))
            .await;
    let doomed_id = string_member(&doomed, "id", "the authorizer deleted twice");
    deleted_authorizer(&gateway, &api, &doomed_id, "the first delete").await;
    for round in 0..2 {
        assert_failure(
            &delete_authorizer(&gateway, &api, &doomed_id).await,
            404,
            "NotFoundException",
            &format!("DeleteAuthorizer again (round {round})"),
        );
    }

    // The authorizers of a deleted API go with it, so a delete addressed through that API
    // resolves nothing either.
    let dropped = gateway.call("DELETE", &format!("/restapis/{api}"), None).await;
    assert_eq!(
        dropped.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        dropped.status, dropped.body
    );
    assert_failure(
        &delete_authorizer(&gateway, &api, &kept_id).await,
        404,
        "NotFoundException",
        "DeleteAuthorizer after DeleteRestApi",
    );
    // The other API is untouched by either delete.
    let read = get_authorizer(&gateway, &other, &elsewhere_id).await;
    assert_eq!(
        read.body, elsewhere,
        "case GetAuthorizer(the other API) after DeleteRestApi: expected {elsewhere:?}, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn the_per_api_quota_counts_only_the_authorizers_that_still_exist() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-delete-quota").await;

    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_AUTHORIZERS_PER_API {
        let case = format!("CreateAuthorizer(#{index})");
        let created =
            created_authorizer(&gateway, &api, &case, token_request(&format!("auth-{index}")))
                .await;
        ids.push(string_member(&created, "id", &case));
    }
    assert_failure(
        &gateway
            .call(
                "POST",
                &format!("/restapis/{api}/authorizers"),
                Some(token_request("one-too-many")),
            )
            .await,
        429,
        "LimitExceededException",
        "CreateAuthorizer past the quota",
    );

    // Deleting authorizers frees exactly that many slots: the record is gone from the
    // store, not merely hidden from a read.
    let freed = 3;
    for (index, id) in ids.iter().take(freed).enumerate() {
        deleted_authorizer(&gateway, &api, id, &format!("DeleteAuthorizer(#{index})")).await;
    }
    let mut replacements: Vec<String> = Vec::new();
    for index in 0..freed {
        let case = format!("CreateAuthorizer(replacement #{index})");
        let created = created_authorizer(
            &gateway,
            &api,
            &case,
            token_request(&format!("replacement-{index}")),
        )
        .await;
        replacements.push(string_member(&created, "id", &case));
    }
    assert_failure(
        &gateway
            .call(
                "POST",
                &format!("/restapis/{api}/authorizers"),
                Some(token_request("one-too-many-again")),
            )
            .await,
        429,
        "LimitExceededException",
        "CreateAuthorizer past the quota again",
    );

    // Each replacement is its own record under its own minted id, and every authorizer
    // that was not deleted is still there.
    for (index, id) in replacements.iter().enumerate() {
        assert!(
            !ids[..freed].contains(id),
            "case CreateAuthorizer(replacement #{index}): expected a freshly minted id, \
             got the deleted {id:?}"
        );
        let read = get_authorizer(&gateway, &api, id).await;
        assert_eq!(
            read.status, 200,
            "case GetAuthorizer(replacement #{index}): expected 200, got {} with {:?}",
            read.status, read.body
        );
    }
    for (index, id) in ids.iter().enumerate().skip(freed) {
        let read = get_authorizer(&gateway, &api, id).await;
        assert_eq!(
            read.status, 200,
            "case GetAuthorizer(#{index}) after the deletes: expected 200, got {} with {:?}",
            read.status, read.body
        );
    }
}
