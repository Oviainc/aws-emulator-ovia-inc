// SPDX-License-Identifier: CC0-1.0

//! WHICH record a call addressed by a record NESTED under a REST API reports when the
//! address resolves to nothing, asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the create
//! helpers are this crate's existing harness.
//!
//! The RULE under test: a call addressed by a record nested under a REST API resolves that
//! record by its WHOLE key in ONE lookup, so the refusal names the record whose existence
//! the call needs — the stage a stage-addressed call acts on or renders a document from,
//! the resource a resource-addressed call acts on or hangs a new child off — with that
//! record's own fixed sentence, whether the child label, the `restApiId` or both resolve to
//! nothing. The account-qualified API-identifier sentence belongs to a call whose own
//! target IS the API (`rest_api_identifier_refusals` asserts that side of the boundary).
//!
//! The rows span what the rule generalises over rather than repeating one recorded call:
//! both record families (the stage and the resource), every operation of this service
//! addressed by one of them — reads, patches, deletes, the two cache flushes, the export a
//! stage renders, the two creates that hang a child off a resource, and the three tag
//! operations, which address a record by its ARN — each of the three ways the address can
//! fail (an unresolvable API with a stored child label, a stored API with an unresolvable
//! child label, and both unresolvable), and unknown identifiers differing in length,
//! alphabet and case — the sentences are fixed text, so no row may quote an identifier or
//! the account back.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{
    assert_failure, create_api, resource_under, Answer, Gateway,
};
use crate::stage_lifecycle::create_deployed_stage;
use crate::tag_reads_by_arn::{apigateway_arn, encode_label, get_tags};

/// The sentence this service states for a stage address that resolves to no stage
/// (`state::NO_SUCH_STAGE`).
const NO_SUCH_STAGE: &str = "Invalid stage identifier specified";

/// The sentence this service states for a resource address that resolves to no resource
/// (`state::NO_SUCH_RESOURCE`).
const NO_SUCH_RESOURCE: &str = "Invalid Resource identifier specified";

/// Identifiers no stored API has, differing in length (one character, the ten characters
/// API Gateway mints, longer), alphabet (letters, digits, a mix, a hyphen) and case.
/// `restApiId` is a plain `String` with no `@pattern`
/// (`smithy_query(query="operation", operation="GetStage")`), so every row reaches the
/// lookup.
const UNKNOWN_APIS: &[&str] = &["q", "zzzzzzzzzz", "0123456789", "Ab3Xy9Zq1W", "no-such-api-01"];

/// Child labels no stored API has a record under, in the same spread: a stage name and a
/// resource id are both plain `String` labels.
const UNKNOWN_CHILDREN: &[&str] = &["x", "zzzzzzzzzz", "0123456789", "Ab3Xy9Zq1W", "no-such-01"];

/// A patch body that is valid in itself, so a row reaches the address lookup instead of
/// being refused for its body.
fn patch(path: &str, value: &str) -> Value {
    json!({ "patchOperations": [{ "op": "replace", "path": path, "value": value }] })
}

/// Every call of this service addressed by a STAGE of the API, with `api` and `stage` the
/// address under test.
fn stage_requests(api: &str, stage: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        (
            "GetStage",
            "GET",
            format!("/restapis/{api}/stages/{stage}"),
            None,
        ),
        (
            "UpdateStage",
            "PATCH",
            format!("/restapis/{api}/stages/{stage}"),
            Some(patch("/description", "d")),
        ),
        (
            "DeleteStage",
            "DELETE",
            format!("/restapis/{api}/stages/{stage}"),
            None,
        ),
        (
            "FlushStageCache",
            "DELETE",
            format!("/restapis/{api}/stages/{stage}/cache/data"),
            None,
        ),
        (
            "FlushStageAuthorizersCache",
            "DELETE",
            format!("/restapis/{api}/stages/{stage}/cache/authorizers"),
            None,
        ),
        // The export is a document RENDERED from the stage, so the stage is the record
        // whose existence the call needs.
        (
            "GetExport",
            "GET",
            format!("/restapis/{api}/stages/{stage}/exports/oas30"),
            None,
        ),
    ]
}

/// Every call of this service addressed by a RESOURCE of the API, with `api` and
/// `resource` the address under test.
fn resource_requests(
    api: &str,
    resource: &str,
) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        (
            "GetResource",
            "GET",
            format!("/restapis/{api}/resources/{resource}"),
            None,
        ),
        (
            "UpdateResource",
            "PATCH",
            format!("/restapis/{api}/resources/{resource}"),
            Some(patch("/pathPart", "renamed")),
        ),
        (
            "DeleteResource",
            "DELETE",
            format!("/restapis/{api}/resources/{resource}"),
            None,
        ),
        // The two calls that hang a NEW child off the resource: the child is being
        // created, so the resource is the record whose existence the call needs.
        (
            "CreateResource",
            "POST",
            format!("/restapis/{api}/resources/{resource}"),
            Some(json!({ "pathPart": "kittens" })),
        ),
        (
            "PutMethod",
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        ),
    ]
}

/// Every TAG call of this service, which addresses the record by its ARN rather than by a
/// URI label: the `/restapis/{restApiId}/stages/{stageName}` resource portion of a stage's
/// ARN names the STAGE, so these report the stage too.
fn stage_arn_requests(
    api: &str,
    stage: &str,
) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let arn = encode_label(&apigateway_arn(&format!("/restapis/{api}/stages/{stage}")));
    vec![
        ("GetTags", "GET", format!("/tags/{arn}"), None),
        (
            "TagResource",
            "PUT",
            format!("/tags/{arn}"),
            Some(json!({ "tags": { "owner": "team" } })),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{arn}?tagKeys=owner"),
            None,
        ),
    ]
}

/// Assert `answer` is a 404 `NotFoundException` whose message is exactly `message`, and
/// that it quotes back neither an identifier nor the account.
fn assert_reports(answer: &Answer, case: &str, message: &str) {
    assert_failure(answer, 404, "NotFoundException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

#[tokio::test]
async fn a_stage_addressed_call_reports_the_stage_it_cannot_resolve() {
    let gateway = Gateway::new();
    // One stored API with one stored stage supplies the halves a row keeps valid, so each
    // refusal is about the half under test alone.
    let (stored, _) = create_api(&gateway, "nested-stage-refusals").await;
    create_deployed_stage(&gateway, &stored, "prod").await;

    for (index, unknown) in UNKNOWN_APIS.iter().enumerate() {
        let unknown_stage = UNKNOWN_CHILDREN[index];
        // The three ways a stage address fails: the API, the stage, and both.
        let addresses = [
            (*unknown, "prod"),
            (stored.as_str(), unknown_stage),
            (*unknown, unknown_stage),
        ];
        for (api, stage) in addresses {
            for (operation, method, uri, body) in stage_requests(api, stage) {
                let case = format!("{operation} [{method} {uri}]");
                let answer = gateway.call(method, &uri, body).await;
                assert_reports(&answer, &case, NO_SUCH_STAGE);
            }
            // The tag calls address the same stage by its ARN.
            for (operation, method, uri, body) in stage_arn_requests(api, stage) {
                let case = format!("{operation} [{method} {uri}]");
                let answer = gateway.call(method, &uri, body).await;
                assert_reports(&answer, &case, NO_SUCH_STAGE);
            }
        }
    }

    // The same calls against the STORED address are served, so the refusals above are
    // about the address and not about the requests. The delete of the stage goes last: it
    // is the one row that removes the record the others resolve.
    for (operation, method, uri, body) in stage_requests(&stored, "prod")
        .into_iter()
        .filter(|(operation, ..)| *operation != "DeleteStage")
    {
        let answer = gateway.call(method, &uri, body).await;
        assert!(
            (200..300).contains(&answer.status),
            "case {operation}(stored prod): expected a success, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{stored}/stages/prod"), None)
        .await;
    assert!(
        (200..300).contains(&deleted.status),
        "case DeleteStage(stored prod): expected a success, got {} with {:?}",
        deleted.status,
        deleted.body
    );
}

#[tokio::test]
async fn a_stage_arn_is_reported_as_the_stage_and_an_api_arn_as_the_api() {
    let gateway = Gateway::new();
    let (stored, _) = create_api(&gateway, "stage-arn-refusals").await;
    create_deployed_stage(&gateway, &stored, "prod").await;

    // The tag calls against the STORED stage ARN are served, so the refusals the stage
    // suite above asserts are about the address and not about the ARN form.
    for (operation, method, uri, body) in stage_arn_requests(&stored, "prod") {
        let answer = gateway.call(method, &uri, body).await;
        assert!(
            (200..300).contains(&answer.status),
            "case {operation}(the stored stage's ARN): expected a success, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // Where the rule STOPS, on the same operation: an ARN whose own record IS the API
    // keeps reporting the account-qualified API identifier.
    for unknown in UNKNOWN_APIS {
        let answer = get_tags(&gateway, &apigateway_arn(&format!("/restapis/{unknown}"))).await;
        assert_reports(
            &answer,
            &format!("GetTags(the ARN of the unknown api {unknown})"),
            &crate::rest_api_identifier_refusals::no_such_api(unknown),
        );
    }
}

#[tokio::test]
async fn a_resource_addressed_call_reports_the_resource_it_cannot_resolve() {
    let gateway = Gateway::new();
    let (stored, root) = create_api(&gateway, "nested-resource-refusals").await;
    let resource = resource_under(&gateway, &stored, &root, "pets").await;

    for (index, unknown) in UNKNOWN_APIS.iter().enumerate() {
        let unknown_resource = UNKNOWN_CHILDREN[index];
        let addresses = [
            (*unknown, resource.as_str()),
            (stored.as_str(), unknown_resource),
            (*unknown, unknown_resource),
        ];
        for (api, addressed) in addresses {
            for (operation, method, uri, body) in resource_requests(api, addressed) {
                let case = format!("{operation} [{method} {uri}]");
                let answer = gateway.call(method, &uri, body).await;
                assert_reports(&answer, &case, NO_SUCH_RESOURCE);
            }
        }
    }

    // The same calls against the STORED address are served. The delete of the resource
    // goes last: it is the one row that removes the record the others resolve.
    for (operation, method, uri, body) in resource_requests(&stored, &resource)
        .into_iter()
        .filter(|(operation, ..)| *operation != "DeleteResource")
    {
        let answer = gateway.call(method, &uri, body).await;
        assert!(
            (200..300).contains(&answer.status),
            "case {operation}(stored pets): expected a success, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
    let deleted = gateway
        .call(
            "DELETE",
            &format!("/restapis/{stored}/resources/{resource}"),
            None,
        )
        .await;
    assert!(
        (200..300).contains(&deleted.status),
        "case DeleteResource(stored pets): expected a success, got {} with {:?}",
        deleted.status,
        deleted.body
    );
}
