// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDeployment`, read back through `GetDeployment`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod`/`CreateDeployment` helpers are this
//! crate's existing harness, reused from `rest_api_lifecycle` and the sibling
//! `stage_lifecycle` suite.
//!
//! The RULE under test: an `UpdateDeployment` applies every entry of `patchOperations`
//! to the stored `Deployment` its `(restApiId, deploymentId)` names, in request order,
//! and reports the updated record — so a later read reports exactly what the patch set —
//! while a patch naming a member the record does not let a caller change, an op that
//! member does not support, or a value outside its bound is refused with a modelled
//! error and changes nothing. The one patchable member is `description`: of the four
//! members `Deployment` declares (`smithy_query(query="operation",
//! operation="UpdateDeployment")` reports `id`, `description`, `createdDate` and
//! `apiSummary`) the service mints the `id`, stamps the `createdDate`, and takes the
//! `apiSummary` as the snapshot of the API "at the date and time that the deployment
//! resource was created".
//!
//! The tables span what the rule generalises over: the op (`add`, `replace`, `remove`),
//! whether the member was set or absent beforehand, the value's size and character set
//! (ASCII, multibyte, an empty string, the published `MAX_STRING_CHARS` bound and one
//! past it), how many ops one request carries and in which order, and which member or op
//! the refused rows name.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::MAX_STRING_CHARS;

use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, string_member, Answer, Gateway,
};
use crate::stage_lifecycle::create_deployment;

/// An API with one resource carrying one method, which is what a deployment snapshots.
///
/// Built from this crate's own `CreateRestApi`/`CreateResource`/`PutMethod` harness, so
/// the `apiSummary` a deployment freezes is a real one.
async fn api_with_a_method(gateway: &Gateway, name: &str) -> (String, String) {
    let (api, root) = create_api(gateway, name).await;
    let resource = create_resource(gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", name);
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE", "apiKeyRequired": true })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected 201 from PutMethod, got {} with {:?}",
        answer.status, answer.body
    );
    (api, resource_id)
}

/// One `patchOperations` entry, as a client spells it on the wire.
fn op(kind: &str, path: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "op": kind, "path": path, "value": value }),
        None => json!({ "op": kind, "path": path }),
    }
}

/// A `replace` entry, the op a client issues most.
fn replace(path: &str, value: &str) -> Value {
    op("replace", path, Some(value))
}

/// `UpdateDeployment` for `deployment` of `api`.
async fn update_deployment(
    gateway: &Gateway,
    api: &str,
    deployment: &str,
    operations: Vec<Value>,
) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/deployments/{deployment}"),
            Some(json!({ "patchOperations": Value::Array(operations) })),
        )
        .await
}

/// `GetDeployment` for `deployment` of `api`, asserting the modelled 200.
async fn get_deployment(gateway: &Gateway, api: &str, deployment: &str, case: &str) -> Value {
    let answer = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/deployments/{deployment}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetDeployment, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// A string of `chars` ASCII characters, for the rows that sit on a published bound.
fn filler(chars: usize) -> String {
    "d".repeat(chars)
}

#[tokio::test]
async fn a_patched_deployment_description_is_stored_and_read_back() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "patch-deployment").await;

    // Rows differ along what the rule generalises over: the op, whether the member was
    // set beforehand, the value's size and character set, and how many ops one request
    // carries (the last op of a request on one member is what the record keeps, because
    // the ops are applied in request order).
    let at_bound = filler(MAX_STRING_CHARS);
    let cases: Vec<(&str, Value, Vec<Value>, Value)> = vec![
        (
            "replace a description that was set",
            json!({ "description": "initial description" }),
            vec![replace("/description", "updated description")],
            json!("updated description"),
        ),
        (
            "add a description that was absent",
            json!({}),
            vec![op("add", "/description", Some("added description"))],
            json!("added description"),
        ),
        (
            "add over a description that was set",
            json!({ "description": "initial description" }),
            vec![op("add", "/description", Some("added over"))],
            json!("added over"),
        ),
        (
            "remove a description that was set",
            json!({ "description": "initial description" }),
            vec![op("remove", "/description", None)],
            Value::Null,
        ),
        (
            "remove a description that was absent",
            json!({}),
            vec![op("remove", "/description", None)],
            Value::Null,
        ),
        (
            "replace with a multibyte value",
            json!({ "description": "initial description" }),
            vec![replace("/description", "déploiement « canari » 🚀")],
            json!("déploiement « canari » 🚀"),
        ),
        (
            "replace with an empty value",
            json!({ "description": "initial description" }),
            vec![replace("/description", "")],
            json!(""),
        ),
        (
            // The published bound this emulator holds caller-sized strings to; one past
            // it is refused below.
            "replace with a value at the bound",
            json!({ "description": "initial description" }),
            vec![replace("/description", &at_bound)],
            Value::String(at_bound.clone()),
        ),
        (
            "two ops on one member, applied in request order",
            json!({}),
            vec![
                op("add", "/description", Some("first")),
                replace("/description", "second"),
                replace("/description", "third"),
            ],
            json!("third"),
        ),
        (
            "a remove after an add leaves the member cleared",
            json!({}),
            vec![
                op("add", "/description", Some("transient")),
                op("remove", "/description", None),
            ],
            Value::Null,
        ),
        (
            "an empty list changes nothing",
            json!({ "description": "initial description" }),
            vec![],
            json!("initial description"),
        ),
    ];

    for (case, create_request, operations, expected) in cases {
        let deployment = create_deployment(&gateway, &api, case, create_request).await;
        let created = get_deployment(&gateway, &api, &deployment, case).await;

        let answer = update_deployment(&gateway, &api, &deployment, operations).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the modelled 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["description"], expected,
            "case {case}: UpdateDeployment must report description as {expected:?}, got {:?}",
            answer.body["description"]
        );
        // The identity and the creation instant are the service's, not the caller's: a
        // patch of the description leaves both where the create put them.
        assert_eq!(
            answer.body["id"], created["id"],
            "case {case}: expected the id the create minted ({:?}), got {:?}",
            created["id"], answer.body["id"]
        );
        assert_eq!(
            answer.body["createdDate"], created["createdDate"],
            "case {case}: expected the createdDate the create stamped ({:?}), got {:?}",
            created["createdDate"], answer.body["createdDate"]
        );
        // This operation takes no `embed`, so it reports no `apiSummary` — the same rule
        // `CreateDeployment` follows, and `GetDeployment` is the call that takes one.
        assert!(
            answer.body["apiSummary"].is_null(),
            "case {case}: the summary is embedded only on request, got {:?}",
            answer.body["apiSummary"]
        );

        // The round trip: the record a later reader sees is the one the update reported.
        let read = get_deployment(&gateway, &api, &deployment, case).await;
        assert_eq!(
            read, answer.body,
            "case {case}: GetDeployment must report the record the update reported, got {read:?}"
        );
        // The snapshot is still the snapshot: patching the description does not touch
        // the methods the deployment froze.
        let embedded = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/deployments/{deployment}?embed=apisummary"),
                None,
            )
            .await;
        assert_eq!(
            embedded.body["apiSummary"],
            json!({ "/pets": { "GET": { "authorizationType": "NONE", "apiKeyRequired": true } } }),
            "case {case}: expected the methods at creation time, got {:?}",
            embedded.body["apiSummary"]
        );
    }
}

#[tokio::test]
async fn an_unsupported_patch_path_is_refused_with_the_path_the_op_and_the_supported_set() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "deployment-unsupported-path").await;

    // Every member of `Deployment` other than `description`, under each op a caller can
    // send, plus a member the shape does not declare at all: none of them names something
    // this resource lets a caller change.
    for (kind, path) in [
        ("replace", "/id"),
        ("add", "/id"),
        ("remove", "/id"),
        ("replace", "/createdDate"),
        ("add", "/createdDate"),
        ("replace", "/apiSummary"),
        ("remove", "/apiSummary"),
        ("add", "/apiSummary/~1pets"),
        ("replace", "/nosuchmember"),
    ] {
        let case = format!("UpdateDeployment({kind} {path})");
        let deployment = create_deployment(
            &gateway,
            &api,
            &case,
            json!({ "description": "initial description" }),
        )
        .await;
        let answer =
            update_deployment(&gateway, &api, &deployment, vec![op(kind, path, Some("x"))]).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(
                format!(
                    "Invalid patch path  '{path}' specified for op '{kind}'. \
                     Must be one of: [/description]"
                )
                .as_str()
            ),
            "case {case}: expected the refusal naming the path, the op and the supported set, got {:?}",
            answer.body["message"]
        );
        // Nothing was applied: the stored record is the one the create wrote.
        let read = get_deployment(&gateway, &api, &deployment, &case).await;
        assert_eq!(
            read["description"].as_str(),
            Some("initial description"),
            "case {case}: a refused patch must leave the stored deployment alone, got {:?}",
            read["description"]
        );
    }
}

#[tokio::test]
async fn a_patch_the_description_cannot_take_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "deployment-refused-patch").await;

    // `move`, `copy` and `test` are declared by the model's `Op` enum but name nothing on
    // these resources, so all three earn the one refusal this service gives an op no
    // resource supports.
    let one_past_bound = filler(MAX_STRING_CHARS + 1);
    let refused: Vec<(String, Vec<Value>, u16, &str)> = vec![
        (
            "a move of /description".to_string(),
            vec![json!({ "op": "move", "path": "/description", "from": "/id" })],
            400,
            "BadRequestException",
        ),
        (
            "a copy of /description".to_string(),
            vec![json!({ "op": "copy", "path": "/description", "from": "/id" })],
            400,
            "BadRequestException",
        ),
        (
            "a test of /description".to_string(),
            vec![json!({ "op": "test", "path": "/description", "value": "x" })],
            400,
            "BadRequestException",
        ),
        (
            // `op` is modelled as an enum, so a value outside it is refused by the
            // request walk before the handler runs — a 400 all the same, never a 501.
            "an off-model op".to_string(),
            vec![json!({ "op": "upsert", "path": "/description", "value": "x" })],
            400,
            "ValidationException",
        ),
        (
            "a replace with no value".to_string(),
            vec![op("replace", "/description", None)],
            400,
            "BadRequestException",
        ),
        (
            "a path reaching into the single-valued member".to_string(),
            vec![replace("/description/first", "x")],
            400,
            "BadRequestException",
        ),
        (
            "a path that is not a JSON Pointer".to_string(),
            vec![replace("description", "x")],
            400,
            "BadRequestException",
        ),
        (
            "an entry with no path at all".to_string(),
            vec![json!({ "op": "replace", "value": "x" })],
            400,
            "BadRequestException",
        ),
        (
            format!("a value one past the {MAX_STRING_CHARS}-character bound"),
            vec![replace("/description", &one_past_bound)],
            400,
            "BadRequestException",
        ),
        (
            // The list is applied as a whole: an op the service cannot apply discards
            // the ops before it rather than half-applying the request.
            "a valid op followed by an unsupported one".to_string(),
            vec![
                replace("/description", "would-be-applied"),
                replace("/id", "x"),
            ],
            400,
            "BadRequestException",
        ),
    ];

    for (case, operations, status, code) in &refused {
        let deployment = create_deployment(
            &gateway,
            &api,
            case,
            json!({ "description": "initial description" }),
        )
        .await;
        let answer = update_deployment(&gateway, &api, &deployment, operations.clone()).await;
        assert_failure(&answer, *status, code, case);
        let read = get_deployment(&gateway, &api, &deployment, case).await;
        assert_eq!(
            read["description"].as_str(),
            Some("initial description"),
            "case {case}: a refused patch must leave the stored deployment alone, got {:?}",
            read["description"]
        );
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_update_deployment() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "deployment-unknown-id").await;
    let deployment = create_deployment(
        &gateway,
        &api,
        "UpdateDeployment(unknown identifiers)",
        json!({ "description": "initial description" }),
    )
    .await;
    // An API with no deployment of its own, so the id above names nothing in it: a
    // deployment is resolved WITHIN the API the request names, not globally.
    let (other, _) = create_api(&gateway, "deployment-unknown-id-other").await;

    for (case, rest_api_id, deployment_id, operations) in [
        (
            "an unknown api",
            "zzzzzzzzzz".to_string(),
            deployment.clone(),
            vec![replace("/description", "x")],
        ),
        (
            "an unknown deployment",
            api.clone(),
            "zzzzzzzzzz".to_string(),
            vec![replace("/description", "x")],
        ),
        (
            "a deployment of another api",
            other.clone(),
            deployment.clone(),
            vec![replace("/description", "x")],
        ),
        (
            // The identifier is resolved whatever the list carries, so an empty list is
            // refused for the same reason.
            "an unknown deployment with an empty list",
            api.clone(),
            "zzzzzzzzzz".to_string(),
            vec![],
        ),
    ] {
        let answer = update_deployment(&gateway, &rest_api_id, &deployment_id, operations).await;
        assert_failure(&answer, 404, "NotFoundException", case);
    }

    // The deployment that does exist is untouched by any of those refusals.
    let read = get_deployment(&gateway, &api, &deployment, "after the refusals").await;
    assert_eq!(
        read["description"].as_str(),
        Some("initial description"),
        "case after the refusals: expected the stored description, got {:?}",
        read["description"]
    );
    assert_eq!(
        string_member(&read, "id", "after the refusals"),
        deployment,
        "case after the refusals: expected the deployment to keep its id, got {read:?}"
    );
}
