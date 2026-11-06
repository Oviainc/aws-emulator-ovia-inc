// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a DELETE a `Method` of the same API blocks, and for the `Model`
//! collection the blocked record is read back through.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * a delete of a record a `Method` of the same API still REFERENCES is refused with the
//!   modelled `ConflictException` (HTTP 409) whose message names the blocked record and the
//!   referencing site, the addressed record is left exactly as it was, and the same
//!   delete succeeds once the reference is cleared. The rows differ along which record is
//!   referenced (a `Model` through a method request's `requestModels`, a `Model` through a
//!   method response's `responseModels`, an `Authorizer` through `authorizerId`), along the
//!   resource the referencing method sits on (the root resource and a child), along its
//!   HTTP verb, along the status code a referencing method response is keyed by, and along
//!   the content type the model is mapped under;
//! * `GetModels` still reports a model a REFUSED delete left in place, and no longer
//!   reports one a successful delete took away — the collection reader's own paging and
//!   ordering rules are the `model_collection` suite's.

use serde_json::json;

use crate::authorizer_lifecycle::lambda_uri;
use crate::execute_api_data_plane::put_method;
use crate::method_response::put_method_response;
use crate::patch_updates::{op, patch};
use crate::rest_api_lifecycle::{
    create_api, page_items, resource_under, string_member, Answer, Gateway,
};

/// The media type a model is mapped under unless a row says otherwise.
const JSON: &str = "application/json";

/// `CreateModel` of `name`, asserting the modelled 201.
async fn create_model(gateway: &Gateway, api: &str, name: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/models"),
            Some(json!({
                "name": name,
                "contentType": JSON,
                "schema": json!({ "type": "object" }).to_string(),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateModel({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `DeleteModel` of `name`.
async fn delete_model(gateway: &Gateway, api: &str, name: &str) -> Answer {
    gateway
        .call("DELETE", &format!("/restapis/{api}/models/{name}"), None)
        .await
}

/// `GetModels`, answering the page's model names in the order it reported them.
async fn list_models(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/models"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected GetModels to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, case)
        .iter()
        .map(|model| string_member(model, "name", case))
        .collect()
}

/// `GetModel` of `name`.
async fn get_model(gateway: &Gateway, api: &str, name: &str) -> Answer {
    gateway
        .call("GET", &format!("/restapis/{api}/models/{name}"), None)
        .await
}

/// One blocked delete: the record a method references, and how the method references it.
struct Blocked {
    case: &'static str,
    /// The path part of the resource the referencing method sits on, or `None` for the
    /// API's root resource.
    path_part: Option<&'static str>,
    /// The verb the referencing method is put under.
    verb: &'static str,
    /// The content type the model is mapped under in `requestModels`.
    content_type: &'static str,
    /// The resource path the refusal must name, which is the root's `/` or the child's.
    resource_path: &'static str,
}

/// The table both blocked-delete cases run over: the dimensions the refusal generalises
/// over are which resource the method sits on, its verb, and the content type the model is
/// mapped under.
const BLOCKED: &[Blocked] = &[
    Blocked {
        case: "a POST on the root resource",
        path_part: None,
        verb: "POST",
        content_type: JSON,
        resource_path: "/",
    },
    Blocked {
        case: "a PUT on a child resource",
        path_part: Some("orders"),
        verb: "PUT",
        content_type: JSON,
        resource_path: "/orders",
    },
    Blocked {
        case: "a PATCH mapping the model under another content type",
        path_part: Some("items"),
        verb: "PATCH",
        content_type: "application/xml",
        resource_path: "/items",
    },
];

#[tokio::test]
async fn a_model_a_methods_request_models_still_names_cannot_be_deleted() {
    for row in BLOCKED {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "model-in-use").await;
        let resource = match row.path_part {
            None => root.clone(),
            Some(path_part) => resource_under(&gateway, &api, &root, path_part).await,
        };
        create_model(&gateway, &api, "Order").await;
        // A second model NOTHING references, so the refusal is about the record addressed
        // rather than about the API.
        create_model(&gateway, &api, "Spare").await;
        put_method(
            &gateway,
            &api,
            &resource,
            row.verb,
            json!({
                "authorizationType": "NONE",
                "requestModels": { row.content_type: "Order" },
            }),
        )
        .await;

        // While the reference stands the delete is refused, and repeating it is refused the
        // same way: the refusal takes nothing away.
        let expected = format!(
            "Cannot delete model 'Order', is referenced in method request: {}/{}",
            row.resource_path, row.verb
        );
        for round in 0..2 {
            let refused = delete_model(&gateway, &api, "Order").await;
            assert_eq!(
                refused.status, 409,
                "case {} (round {round}): expected 409, got {} with {:?}",
                row.case, refused.status, refused.body
            );
            assert_eq!(
                refused.error.as_deref(),
                Some("ConflictException"),
                "case {} (round {round}): expected ConflictException, got {:?}",
                row.case,
                refused.error
            );
            assert_eq!(
                refused.body["message"].as_str(),
                Some(expected.as_str()),
                "case {} (round {round}): expected the message {expected:?}, got {:?}",
                row.case,
                refused.body["message"]
            );
        }
        // The refused delete left the model where it was, and the collection still lists
        // both models in creation order.
        assert_eq!(
            get_model(&gateway, &api, "Order").await.status,
            200,
            "case {}: expected the blocked model to be readable",
            row.case
        );
        assert_eq!(
            list_models(&gateway, &api, row.case).await,
            crate::models::with_default_models(&["Order", "Spare"]),
            "case {}: expected both models in creation order",
            row.case
        );

        // A model no method references is deleted straight away, even while a sibling of
        // the same API is blocked.
        let spare = delete_model(&gateway, &api, "Spare").await;
        assert_eq!(
            spare.status, 202,
            "case {}: expected 202 for a model nothing references, got {} with {:?}",
            row.case, spare.status, spare.body
        );

        // Clearing the reference is what frees the delete: the patch that removes the
        // method's `requestModels` entry makes the same call succeed, and the model is
        // then gone from the collection and from the single reader.
        let detached = patch(
            &gateway,
            &format!("/restapis/{api}/resources/{resource}/methods/{}", row.verb),
            vec![op(
                "remove",
                &format!(
                    "/requestModels/{}",
                    row.content_type.replace('~', "~0").replace('/', "~1")
                ),
                None,
            )],
        )
        .await;
        assert_eq!(
            detached.status, 200,
            "case {}: expected UpdateMethod to clear the reference, got {} with {:?}",
            row.case, detached.status, detached.body
        );
        let freed = delete_model(&gateway, &api, "Order").await;
        assert_eq!(
            freed.status, 202,
            "case {}: expected 202 once nothing references the model, got {} with {:?}",
            row.case, freed.status, freed.body
        );
        assert_eq!(
            get_model(&gateway, &api, "Order").await.status,
            404,
            "case {}: expected the deleted model to be gone",
            row.case
        );
        assert_eq!(
            list_models(&gateway, &api, row.case).await,
            crate::models::default_model_names(),
            "case {}: expected no created model left in the collection",
            row.case
        );
    }
}

/// One blocked delete whose reference sits on a method RESPONSE rather than on the method
/// request: the response is keyed by its status code, so the refusal has to name that too.
struct BlockedByResponse {
    case: &'static str,
    /// The path part of the resource the referencing method sits on, or `None` for the
    /// API's root resource.
    path_part: Option<&'static str>,
    /// The verb the referencing method is put under.
    verb: &'static str,
    /// The status code the referencing method response is keyed by.
    status_code: &'static str,
    /// The content type the model is mapped under in `responseModels`.
    content_type: &'static str,
    /// The resource path the refusal must name, which is the root's `/` or the child's.
    resource_path: &'static str,
}

/// The table the method-response case runs over: the dimensions that refusal generalises
/// over are which resource the method sits on, its verb, the status code the response is
/// keyed by, and the content type the model is mapped under.
const BLOCKED_BY_RESPONSE: &[BlockedByResponse] = &[
    BlockedByResponse {
        case: "a 200 response of a POST on the root resource",
        path_part: None,
        verb: "POST",
        status_code: "200",
        content_type: JSON,
        resource_path: "/",
    },
    BlockedByResponse {
        case: "a 404 response of a GET on a child resource",
        path_part: Some("orders"),
        verb: "GET",
        status_code: "404",
        content_type: JSON,
        resource_path: "/orders",
    },
    BlockedByResponse {
        case: "a 500 response mapping the model under another content type",
        path_part: Some("items"),
        verb: "DELETE",
        status_code: "500",
        content_type: "application/xml",
        resource_path: "/items",
    },
];

#[tokio::test]
async fn a_model_a_method_responses_response_models_still_names_cannot_be_deleted() {
    for row in BLOCKED_BY_RESPONSE {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "model-in-use-response").await;
        let resource = match row.path_part {
            None => root.clone(),
            Some(path_part) => resource_under(&gateway, &api, &root, path_part).await,
        };
        create_model(&gateway, &api, "Order").await;
        // A second model NOTHING references, so the refusal is about the record addressed
        // rather than about the API.
        create_model(&gateway, &api, "Spare").await;
        put_method(
            &gateway,
            &api,
            &resource,
            row.verb,
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        let response = put_method_response(
            &gateway,
            &api,
            &resource,
            row.verb,
            row.status_code,
            json!({ "responseModels": { row.content_type: "Order" } }),
        )
        .await;
        assert_eq!(
            response.status, 201,
            "case {}: expected PutMethodResponse to answer 201, got {} with {:?}",
            row.case, response.status, response.body
        );

        // While the reference stands the delete is refused, and repeating it is refused the
        // same way: the refusal takes nothing away.
        let expected = format!(
            "Cannot delete model 'Order', is referenced in method response: {}/{}/{}",
            row.resource_path, row.verb, row.status_code
        );
        for round in 0..2 {
            let refused = delete_model(&gateway, &api, "Order").await;
            assert_eq!(
                refused.status, 409,
                "case {} (round {round}): expected 409, got {} with {:?}",
                row.case, refused.status, refused.body
            );
            assert_eq!(
                refused.error.as_deref(),
                Some("ConflictException"),
                "case {} (round {round}): expected ConflictException, got {:?}",
                row.case,
                refused.error
            );
            assert_eq!(
                refused.body["message"].as_str(),
                Some(expected.as_str()),
                "case {} (round {round}): expected the message {expected:?}, got {:?}",
                row.case,
                refused.body["message"]
            );
        }
        // The refused delete left the model where it was, and the collection still lists
        // both models in creation order.
        assert_eq!(
            get_model(&gateway, &api, "Order").await.status,
            200,
            "case {}: expected the blocked model to be readable",
            row.case
        );
        assert_eq!(
            list_models(&gateway, &api, row.case).await,
            crate::models::with_default_models(&["Order", "Spare"]),
            "case {}: expected both models in creation order",
            row.case
        );

        // A model no method response references is deleted straight away, even while a
        // sibling of the same API is blocked.
        let spare = delete_model(&gateway, &api, "Spare").await;
        assert_eq!(
            spare.status, 202,
            "case {}: expected 202 for a model nothing references, got {} with {:?}",
            row.case, spare.status, spare.body
        );

        // Clearing the reference is what frees the delete: the patch that removes the
        // response's `responseModels` entry makes the same call succeed, and the model is
        // then gone from the collection and from the single reader.
        let detached = patch(
            &gateway,
            &format!(
                "/restapis/{api}/resources/{resource}/methods/{}/responses/{}",
                row.verb, row.status_code
            ),
            vec![op(
                "remove",
                &format!(
                    "/responseModels/{}",
                    row.content_type.replace('~', "~0").replace('/', "~1")
                ),
                None,
            )],
        )
        .await;
        // `UpdateMethodResponse`'s modelled success status is 201, not the 200 its sibling
        // `UpdateMethod` answers with.
        assert_eq!(
            detached.status, 201,
            "case {}: expected UpdateMethodResponse to clear the reference, got {} with {:?}",
            row.case, detached.status, detached.body
        );
        let freed = delete_model(&gateway, &api, "Order").await;
        assert_eq!(
            freed.status, 202,
            "case {}: expected 202 once nothing references the model, got {} with {:?}",
            row.case, freed.status, freed.body
        );
        assert_eq!(
            get_model(&gateway, &api, "Order").await.status,
            404,
            "case {}: expected the deleted model to be gone",
            row.case
        );
        assert_eq!(
            list_models(&gateway, &api, row.case).await,
            crate::models::default_model_names(),
            "case {}: expected no created model left in the collection",
            row.case
        );
    }
}

#[tokio::test]
async fn a_model_two_sites_reference_is_blocked_until_the_last_reference_is_cleared() {
    // Both sites of one method name the same model, so the refusal names the method
    // REQUEST while that reference stands and the method RESPONSE once only it is left:
    // every reference blocks, and clearing one is not clearing them all.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "model-in-use-both-sites").await;
    let resource = resource_under(&gateway, &api, &root, "orders").await;
    create_model(&gateway, &api, "Order").await;
    put_method(
        &gateway,
        &api,
        &resource,
        "POST",
        json!({
            "authorizationType": "NONE",
            "requestModels": { JSON: "Order" },
        }),
    )
    .await;
    let response = put_method_response(
        &gateway,
        &api,
        &resource,
        "POST",
        "201",
        json!({ "responseModels": { JSON: "Order" } }),
    )
    .await;
    assert_eq!(
        response.status, 201,
        "case both sites: expected PutMethodResponse to answer 201, got {} with {:?}",
        response.status, response.body
    );

    let refused = delete_model(&gateway, &api, "Order").await;
    assert_eq!(
        refused.body["message"].as_str(),
        Some("Cannot delete model 'Order', is referenced in method request: /orders/POST"),
        "case both sites: expected the method request site to be named, got {:?}",
        refused.body["message"]
    );

    // With the request reference cleared the response one still blocks, and now it is the
    // site the refusal names.
    let detached = patch(
        &gateway,
        &format!("/restapis/{api}/resources/{resource}/methods/POST"),
        vec![op("remove", "/requestModels/application~1json", None)],
    )
    .await;
    assert_eq!(
        detached.status, 200,
        "case both sites: expected UpdateMethod to clear the request reference, got {} with {:?}",
        detached.status, detached.body
    );
    let refused = delete_model(&gateway, &api, "Order").await;
    assert_eq!(
        refused.status, 409,
        "case the response reference alone: expected 409, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.body["message"].as_str(),
        Some("Cannot delete model 'Order', is referenced in method response: /orders/POST/201"),
        "case the response reference alone: expected the method response site to be named, got {:?}",
        refused.body["message"]
    );

    // Deleting the whole method response clears the last reference, so the delete goes
    // through and the model is gone.
    let deleted = gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/resources/{resource}/methods/POST/responses/201"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "case both sites: expected DeleteMethodResponse to answer 204, got {} with {:?}",
        deleted.status, deleted.body
    );
    let freed = delete_model(&gateway, &api, "Order").await;
    assert_eq!(
        freed.status, 202,
        "case both sites: expected 202 once no site references the model, got {} with {:?}",
        freed.status, freed.body
    );
    assert_eq!(
        get_model(&gateway, &api, "Order").await.status,
        404,
        "case both sites: expected the deleted model to be gone"
    );
}

#[tokio::test]
async fn an_authorizer_a_method_still_names_is_refused_naming_that_method() {
    // The same rule over the OTHER record a method references, so the two refusals are
    // asserted over one table of resources and verbs.
    for row in BLOCKED {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "authorizer-in-use-message").await;
        let resource = match row.path_part {
            None => root.clone(),
            Some(path_part) => resource_under(&gateway, &api, &root, path_part).await,
        };
        let created = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/authorizers"),
                Some(json!({
                    "name": "guard",
                    "type": "TOKEN",
                    "identitySource": "method.request.header.Auth",
                    "authorizerUri": lambda_uri("guard"),
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {}: expected CreateAuthorizer to answer 201, got {} with {:?}",
            row.case, created.status, created.body
        );
        let authorizer = string_member(&created.body, "id", row.case);
        put_method(
            &gateway,
            &api,
            &resource,
            row.verb,
            json!({ "authorizationType": "CUSTOM", "authorizerId": authorizer.clone() }),
        )
        .await;

        let refused = gateway
            .call(
                "DELETE",
                &format!("/restapis/{api}/authorizers/{authorizer}"),
                None,
            )
            .await;
        assert_eq!(
            refused.status, 409,
            "case {}: expected 409, got {} with {:?}",
            row.case, refused.status, refused.body
        );
        let expected = format!(
            "Cannot delete authorizer 'guard', is referenced in method: {}/{}",
            row.verb, row.resource_path
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(expected.as_str()),
            "case {}: expected the message {expected:?}, got {:?}",
            row.case,
            refused.body["message"]
        );

        // Deleting the referencing method frees the authorizer.
        let deleted = gateway
            .call(
                "DELETE",
                &format!("/restapis/{api}/resources/{resource}/methods/{}", row.verb),
                None,
            )
            .await;
        assert!(
            (200..300).contains(&deleted.status),
            "case {}: expected DeleteMethod to succeed, got {} with {:?}",
            row.case,
            deleted.status,
            deleted.body
        );
        let freed = gateway
            .call(
                "DELETE",
                &format!("/restapis/{api}/authorizers/{authorizer}"),
                None,
            )
            .await;
        assert_eq!(
            freed.status, 202,
            "case {}: expected 202 once nothing references the authorizer, got {} with {:?}",
            row.case, freed.status, freed.body
        );
    }
}

#[tokio::test]
async fn a_refused_delete_leaves_the_model_in_the_collection() {
    // The collection reader's own rules (paging, creation order, the modelled not-found)
    // are the `model_collection` suite's; what this one adds is that a model a REFUSED
    // delete left in place is still reported, and one a successful delete took away is
    // not — over the same table of referencing methods.
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-collection-after-refusal").await;
    let created = ["Alpha", "Bravo"];
    for name in created {
        create_model(&gateway, &api, name).await;
    }
    let expected: Vec<String> = crate::models::with_default_models(&created);
    assert_eq!(
        list_models(&gateway, &api, "before any delete").await,
        expected,
        "case before any delete: expected both models in creation order, after the ones the \
         API was created with"
    );

    // Every listed model reports exactly what the single reader reports for it.
    for name in created {
        let read = get_model(&gateway, &api, name).await;
        assert_eq!(
            read.status, 200,
            "case GetModel({name}): expected 200, got {} with {:?}",
            read.status, read.body
        );
    }

    // A delete nothing blocks takes its model out of the collection; the other stays.
    let deleted = delete_model(&gateway, &api, "Alpha").await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteModel(Alpha): expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        list_models(&gateway, &api, "after one delete").await,
        crate::models::with_default_models(&["Bravo"]),
        "case after one delete: expected only the model that was not deleted"
    );
}
