// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteAuthorizer` while a `Method` of the API still names the
//! authorizer.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], `create_api`, `create_resource`,
//! `string_member` and `assert_failure` are this crate's one wire-test harness, and the
//! `CreateAuthorizer`/`GetAuthorizer` and `PutMethod` callers are reused from the
//! `authorizer_lifecycle`, `execute_api_data_plane` and `patch_updates` suites.
//!
//! The RULE under test, over a SET of inputs rather than one recorded call: a delete of a
//! record another stored record of the same API DEPENDS on is refused with the modelled
//! `ConflictException` (HTTP 409) while the dependency stands, and the addressed record is
//! left exactly as it was; once the dependency is cleared — by deleting the dependent
//! record or by re-pointing it — the same delete succeeds. The rows differ along what the
//! rule generalises over: the authorizer's own type, which resource and HTTP verb the
//! referencing method sits on, that method's `authorizationType`, how many methods
//! reference the authorizer, and which API the referencing method belongs to.

use serde_json::{json, Value};

use crate::authorizer_lifecycle::{created_authorizer, get_authorizer, lambda_uri, user_pool_arn};
use crate::execute_api_data_plane::put_method;
use crate::patch_updates::{patch, replace};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, string_member, Answer, Gateway,
};

/// The modelled error code a delete the API's own records block carries.
const CONFLICT: &str = "ConflictException";

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

/// `DeleteMethod` for `verb` on `resource`, asserting the modelled 204.
async fn delete_method(gateway: &Gateway, api: &str, resource: &str, verb: &str, case: &str) {
    let answer = gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            None,
        )
        .await;
    assert!(
        (200..300).contains(&answer.status),
        "case {case}: expected DeleteMethod to succeed, got {} with {:?}",
        answer.status,
        answer.body
    );
}

/// One authorizer, and the method that references it.
struct Referenced {
    case: &'static str,
    /// The `CreateAuthorizer` request for the authorizer under test.
    authorizer: Value,
    /// The `authorizationType` the referencing method carries.
    authorization_type: &'static str,
    /// Whether the method sits on the API's root resource or on a child of it.
    on_root: bool,
    /// The verbs the referencing methods are put on — one method per verb, so a row may
    /// hold the reference once or several times over.
    verbs: &'static [&'static str],
}

/// The table every "blocked while referenced" case runs over. Rows differ along the
/// authorizer `type` (the modelled `AuthorizerType` catalog, from
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#AuthorizerType")`), the
/// method's `authorizationType`, the verb and resource the method sits on, and how many
/// methods hold the reference.
fn referenced_table() -> Vec<Referenced> {
    vec![
        Referenced {
            case: "a TOKEN authorizer behind a CUSTOM GET on the root resource",
            authorizer: json!({
                "name": "tokenroot",
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
                "authorizerUri": lambda_uri("authorize"),
            }),
            authorization_type: "CUSTOM",
            on_root: true,
            verbs: &["GET"],
        },
        Referenced {
            case: "a REQUEST authorizer behind a CUSTOM POST on a child resource",
            authorizer: json!({
                "name": "requestchild",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("authorize"),
                "identitySource": "method.request.header.Auth",
            }),
            authorization_type: "CUSTOM",
            on_root: false,
            verbs: &["POST"],
        },
        Referenced {
            case: "a COGNITO_USER_POOLS authorizer behind a COGNITO_USER_POOLS method",
            authorizer: json!({
                "name": "cognito",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [user_pool_arn("us-east-1_pool")],
            }),
            authorization_type: "COGNITO_USER_POOLS",
            on_root: false,
            verbs: &["GET"],
        },
        Referenced {
            case: "a TOKEN authorizer referenced by three methods at once",
            authorizer: json!({
                "name": "tokenmany",
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
                "authorizerUri": lambda_uri("authorize"),
            }),
            authorization_type: "CUSTOM",
            on_root: false,
            verbs: &["GET", "POST", "DELETE"],
        },
    ]
}

#[tokio::test]
async fn an_authorizer_a_method_still_names_cannot_be_deleted() {
    for row in referenced_table() {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "authorizer-in-use").await;
        let resource = if row.on_root {
            root.clone()
        } else {
            let created = create_resource(&gateway, &api, &root, "pets").await;
            string_member(&created, "id", row.case)
        };

        let authorizer = created_authorizer(&gateway, &api, row.case, row.authorizer.clone()).await;
        let id = string_member(&authorizer, "id", row.case);

        for verb in row.verbs {
            put_method(
                &gateway,
                &api,
                &resource,
                verb,
                json!({
                    "authorizationType": row.authorization_type,
                    "authorizerId": id.clone(),
                }),
            )
            .await;
        }

        // While the reference stands the delete is refused, and repeating it is refused
        // the same way: the refusal takes nothing away.
        for round in 0..2 {
            assert_failure(
                &delete_authorizer(&gateway, &api, &id).await,
                409,
                CONFLICT,
                &format!("{} (round {round})", row.case),
            );
        }
        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.body, authorizer,
            "case {}: expected the authorizer untouched by the refusal, got {:?}",
            row.case, read.body
        );

        // Clearing every reference is what frees the delete: the last method taken away
        // is the one that unblocks it, so a row with several references stays blocked
        // until all of them are gone.
        for (index, verb) in row.verbs.iter().enumerate() {
            if index + 1 < row.verbs.len() {
                delete_method(&gateway, &api, &resource, verb, row.case).await;
                assert_failure(
                    &delete_authorizer(&gateway, &api, &id).await,
                    409,
                    CONFLICT,
                    &format!("{} with {} reference(s) left", row.case, row.verbs.len() - index - 1),
                );
            } else {
                delete_method(&gateway, &api, &resource, verb, row.case).await;
            }
        }
        let freed = delete_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            freed.status, 202,
            "case {}: expected 202 once nothing references the authorizer, got {} with {:?}",
            row.case, freed.status, freed.body
        );
        assert_failure(
            &get_authorizer(&gateway, &api, &id).await,
            404,
            "NotFoundException",
            &format!("GetAuthorizer after deleting {}", row.case),
        );
    }
}

#[tokio::test]
async fn re_pointing_the_method_frees_the_authorizer_it_named() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "authorizer-repointed").await;

    let named = created_authorizer(
        &gateway,
        &api,
        "the authorizer the method names",
        json!({
            "name": "named",
            "type": "TOKEN",
            "identitySource": "method.request.header.Auth",
            "authorizerUri": lambda_uri("named"),
        }),
    )
    .await;
    let named_id = string_member(&named, "id", "the authorizer the method names");
    let replacement = created_authorizer(
        &gateway,
        &api,
        "the authorizer the method is re-pointed at",
        json!({
            "name": "replacement",
            "type": "TOKEN",
            "identitySource": "method.request.header.Auth",
            "authorizerUri": lambda_uri("replacement"),
        }),
    )
    .await;
    let replacement_id = string_member(&replacement, "id", "the replacement authorizer");

    put_method(
        &gateway,
        &api,
        &root,
        "GET",
        json!({ "authorizationType": "CUSTOM", "authorizerId": named_id.clone() }),
    )
    .await;
    assert_failure(
        &delete_authorizer(&gateway, &api, &named_id).await,
        409,
        CONFLICT,
        "the named authorizer while the method points at it",
    );

    // An authorizer NO method names is deleted straight away, even while a sibling
    // authorizer of the same API is referenced: the refusal is about the record addressed,
    // not about the API.
    let unreferenced = created_authorizer(
        &gateway,
        &api,
        "an authorizer no method names",
        json!({
            "name": "unreferenced",
            "type": "TOKEN",
            "identitySource": "method.request.header.Auth",
            "authorizerUri": lambda_uri("unreferenced"),
        }),
    )
    .await;
    let unreferenced_id = string_member(&unreferenced, "id", "an authorizer no method names");
    let answer = delete_authorizer(&gateway, &api, &unreferenced_id).await;
    assert_eq!(
        answer.status, 202,
        "case an authorizer no method names: expected 202, got {} with {:?}",
        answer.status, answer.body
    );

    // Re-pointing the method at the replacement clears the reference, so the authorizer
    // it used to name can go — and the replacement is now the one that is blocked.
    let repointed = patch(
        &gateway,
        &format!("/restapis/{api}/resources/{root}/methods/GET"),
        vec![replace("/authorizerId", &replacement_id)],
    )
    .await;
    assert_eq!(
        repointed.status, 200,
        "case UpdateMethod(/authorizerId): expected 200, got {} with {:?}",
        repointed.status, repointed.body
    );
    let answer = delete_authorizer(&gateway, &api, &named_id).await;
    assert_eq!(
        answer.status, 202,
        "case the named authorizer once the method was re-pointed: expected 202, got {} \
         with {:?}",
        answer.status, answer.body
    );
    assert_failure(
        &delete_authorizer(&gateway, &api, &replacement_id).await,
        409,
        CONFLICT,
        "the replacement authorizer the method now points at",
    );
}

#[tokio::test]
async fn a_method_of_another_api_does_not_block_the_delete() {
    // A method references the authorizer of ITS OWN API, so a method of another API that
    // happens to carry the same identifier is no dependency of this one.
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-scope").await;
    let (other, other_root) = create_api(&gateway, "authorizer-scope-other").await;

    let authorizer = created_authorizer(
        &gateway,
        &api,
        "the authorizer of the first API",
        json!({
            "name": "scoped",
            "type": "TOKEN",
            "identitySource": "method.request.header.Auth",
            "authorizerUri": lambda_uri("scoped"),
        }),
    )
    .await;
    let id = string_member(&authorizer, "id", "the authorizer of the first API");

    // The method on the OTHER API names that very identifier.
    put_method(
        &gateway,
        &other,
        &other_root,
        "GET",
        json!({ "authorizationType": "CUSTOM", "authorizerId": id.clone() }),
    )
    .await;

    let answer = delete_authorizer(&gateway, &api, &id).await;
    assert_eq!(
        answer.status, 202,
        "case a method of another API: expected 202, got {} with {:?}",
        answer.status, answer.body
    );
}
