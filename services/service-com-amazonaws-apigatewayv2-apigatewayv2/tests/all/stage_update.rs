// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateStage`.
//!
//! The rules these cases assert:
//!
//! * `PATCH /v2/apis/{ApiId}/stages/{StageName}` is a patch of the STORED stage: a
//!   member the request carries replaces the stored value, a member it omits leaves the
//!   stored value as it was, and what the call reports is what a later `GetStage`
//!   reports. The members no request supplies — the name the stage is keyed by, its
//!   `CreatedDate`, its `Tags`, and whether API Gateway manages it — are carried over,
//!   and a patch that rewrote the record moves `Stage$LastUpdatedDate` ("The timestamp
//!   when the stage was last updated") to the instant it resolved at.
//! * A request the service cannot satisfy is REFUSED with the modelled error and
//!   rewrites nothing: an api or a stage that does not exist is a 404, a `DeploymentId`
//!   naming no deployment of the api a 404, and a member breaking a documented rule —
//!   a `Description` past its shape's bound, a stage variable outside the documented
//!   alphabets, an `AccessLogSettings$Format` selecting no request id, a
//!   WebSocket-only member on an HTTP api, or a `DeploymentId` on a stage whose
//!   deployments the service releases ("Can't be updated if autoDeploy is enabled") —
//!   a 400.
//! * An auto-deploying stage has its deployments released BY THE SERVICE ("Specifies
//!   whether a deployment was automatically released", `Deployment$AutoDeployed`), so a
//!   patch that leaves the stage auto-deploying with nothing to serve releases one and
//!   reports it; a stage that already serves a deployment keeps it.
//!
//! The rows vary what the rules generalise over: the member's type (string, boolean,
//! nested structure, map of strings, map of structures), the number of members patched
//! (none, one, several), map size and keys (different keys, empty, the published value
//! bound), the protocol of the api holding the stage, and where the stage's deployment
//! came from. The recorded case that filed the issue — a patch of `StageVariables` and
//! `RouteSettings` read back through `GetStage` — is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code, instant, strip_nulls};

/// The upper bound of `StringWithLengthBetween0And1024`, the shape
/// `UpdateStage$Description` targets.
const MAX_DESCRIPTION_LEN: usize = 1024;

/// The upper bound of `StringWithLengthBetween0And2048`, the value shape of
/// `StageVariablesMap` (`smithy_query(query="shape", shape="StageVariablesMap")`).
const MAX_STAGE_VARIABLE_VALUE_LEN: usize = 2048;

/// The `DeploymentStatus` a released deployment carries, from the values
/// `DeploymentStatus` declares — `PENDING | FAILED | DEPLOYED`.
const DEPLOYED: &str = "DEPLOYED";

/// A `DeploymentId` of the service's own shape (ten lowercase alphanumerics) that no api
/// ever minted, for the row that names a deployment which is not there.
const UNMINTED_DEPLOYMENT_ID: &str = "zzzzzzzzzz";

/// The placeholder a row writes where a real, service-minted `DeploymentId` has to go:
/// a test never hardcodes an id the service mints.
const SUBSTITUTED: &str = "substituted";

fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

fn api_request(protocol: &str, name: &str) -> Value {
    if protocol == "WEBSOCKET" {
        websocket_api(name)
    } else {
        http_api(name)
    }
}

/// Create one api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one route under `api_id`, so a deployment has something to capture.
async fn create_route(emulator: &MultiServiceRouter, case: &str, api_id: &str, route_key: &str) {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({ "routeKey": route_key })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateRoute, got {status} {created}"
    );
}

/// Create one deployment of `api_id` and answer its `DeploymentId`.
async fn create_deployment(emulator: &MultiServiceRouter, case: &str, api_id: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateDeployment, got {status} {created}"
    );
    as_str(&created, "deploymentId")
}

/// Create one stage under `api_id` and answer its `StageName`.
async fn create_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: &Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateStage, got {status} {created}"
    );
    as_str(&created, "stageName")
}

/// Read one stage back and answer `(status, body)`.
async fn get_stage(
    emulator: &MultiServiceRouter,
    api_id: &str,
    stage_name: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage_name}"),
        None,
    )
    .await
}

/// Patch one stage and answer `(status, body)`.
async fn update_stage(
    emulator: &MultiServiceRouter,
    api_id: &str,
    stage_name: &str,
    patch: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}/stages/{stage_name}"),
        Some(patch.clone()),
    )
    .await
}

/// The record `created` becomes once `patch` is applied: every member the patch names
/// takes the patch's value, and every other member keeps the stored one. That is the
/// patch rule itself, written once, so each row only has to supply the patch.
fn patched_record(created: &Value, patch: &Value) -> Value {
    let mut expected = created.clone();
    for (member, value) in patch.as_object().expect("the patch is a JSON object") {
        expected[member] = value.clone();
    }
    expected
}

/// `document` without `member`, so two records can be compared on everything the patch
/// rule fixes while the instant the stage was last updated is asserted on its own.
fn without(document: &Value, member: &str) -> Value {
    let mut stripped = strip_nulls(document);
    if let Some(entries) = stripped.as_object_mut() {
        entries.remove(member);
    }
    stripped
}

/// The stage every patch row starts from: it carries every member the service stores,
/// so a row can replace any one of them, and it serves a deployment a caller asked for,
/// so `AutoDeploy` can be switched on without the service releasing another.
fn base_stage(deployment_id: &str) -> Value {
    json!({
        "stageName": "prod",
        "description": "the stage every row starts from",
        "autoDeploy": false,
        "clientCertificateId": "cert1234ab",
        "deploymentId": deployment_id,
        "accessLogSettings": {
            "destinationArn":
                "arn:aws:logs:eu-west-1:123456789012:log-group:/aws/apigateway/prod",
            "format": "$context.requestId $context.status"
        },
        "defaultRouteSettings": {
            "dataTraceEnabled": true,
            "detailedMetricsEnabled": true,
            "loggingLevel": "INFO",
            "throttlingBurstLimit": 100,
            "throttlingRateLimit": 50.5
        },
        "routeSettings": {
            "$connect": { "throttlingBurstLimit": 10, "throttlingRateLimit": 5.5 }
        },
        "stageVariables": { "table": "orders", "stack": "prod" },
        "tags": { "team": "platform" }
    })
}

/// One patch row: the request body sent to `UpdateStage`.
struct PatchRow {
    case: &'static str,
    patch: Value,
}

fn patch_rows() -> Vec<PatchRow> {
    let longest_variable_value = "v".repeat(MAX_STAGE_VARIABLE_VALUE_LEN);
    vec![
        PatchRow {
            case: "one string member",
            patch: json!({ "description": "the patched stage" }),
        },
        PatchRow {
            case: "one boolean member, flipped",
            patch: json!({ "autoDeploy": true }),
        },
        PatchRow {
            case: "the stage variables and the route settings, the recorded case",
            patch: json!({
                "stageVariables": { "table": "orders-v2", "region": "eu-west-1" },
                "routeSettings": {
                    "$default": { "detailedMetricsEnabled": true, "throttlingBurstLimit": 42 }
                }
            }),
        },
        PatchRow {
            case: "a stage variable map with different keys replaces the stored map",
            patch: json!({ "stageVariables": { "only": "one" } }),
        },
        PatchRow {
            case: "an empty stage variable map clears the stored map",
            patch: json!({ "stageVariables": {} }),
        },
        PatchRow {
            case: "a stage variable value at the published bound",
            patch: json!({ "stageVariables": { "bounded": longest_variable_value } }),
        },
        PatchRow {
            case: "an empty route settings map clears the stored map",
            patch: json!({ "routeSettings": {} }),
        },
        PatchRow {
            case: "a nested structure member, replaced wholesale",
            patch: json!({
                "accessLogSettings": { "format": "$context.requestId" }
            }),
        },
        PatchRow {
            case: "the default route settings, every member of the nested shape",
            patch: json!({
                "defaultRouteSettings": {
                    "dataTraceEnabled": false,
                    "detailedMetricsEnabled": false,
                    "loggingLevel": "ERROR",
                    "throttlingBurstLimit": 7,
                    "throttlingRateLimit": 1.5
                }
            }),
        },
        PatchRow {
            case: "the client certificate, a WebSocket-only string member",
            patch: json!({ "clientCertificateId": "cert9999zz" }),
        },
        PatchRow {
            case: "the stage moves onto another deployment of its own api",
            // The id is substituted below: a stage can only serve a deployment its own
            // api holds.
            patch: json!({ "deploymentId": SUBSTITUTED }),
        },
        PatchRow {
            case: "several members of different types at once",
            patch: json!({
                "description": "several members at once",
                "autoDeploy": true,
                "stageVariables": { "stack": "canary" },
                "defaultRouteSettings": { "throttlingRateLimit": 99.5 }
            }),
        },
        PatchRow {
            case: "a patch naming no member leaves the record as it is",
            patch: json!({}),
        },
    ]
}

#[tokio::test]
async fn update_stage_replaces_the_members_it_names_and_keeps_the_rest() {
    let emulator = emulator();

    for (index, row) in patch_rows().into_iter().enumerate() {
        // One api per row, so a row that releases or moves a deployment cannot disturb
        // another row's stage. A WebSocket api, so every member the stage stores —
        // including the ones documented "Supported only for WebSocket APIs" — is one
        // this stage may carry.
        let api_id = create_api(
            &emulator,
            row.case,
            &websocket_api(&format!("stage-patch-{index}")),
        )
        .await;
        create_route(&emulator, row.case, &api_id, "$connect").await;
        let deployment_id = create_deployment(&emulator, row.case, &api_id).await;
        let other_deployment_id = create_deployment(&emulator, row.case, &api_id).await;
        let stage_name = create_stage(
            &emulator,
            row.case,
            &api_id,
            &base_stage(&deployment_id),
        )
        .await;
        let (status, created) = get_stage(&emulator, &api_id, &stage_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetStage on the fresh stage, got {status} {created}",
            row.case
        );
        let created_at = instant(row.case, &created, "createdDate");
        let updated_before = instant(row.case, &created, "lastUpdatedDate");

        let mut patch = row.patch.clone();
        if patch.get("deploymentId").is_some() {
            patch["deploymentId"] = json!(other_deployment_id);
        }
        let (status, updated) = update_stage(&emulator, &api_id, &stage_name, &patch).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from UpdateStage, got {status} {updated}",
            row.case
        );
        let expected = patched_record(&created, &patch);
        assert_eq!(
            without(&updated, "lastUpdatedDate"),
            without(&expected, "lastUpdatedDate"),
            "case {:?}: UpdateStage must report the patched record; expected {expected}, got {updated}",
            row.case
        );

        // The write is what a later caller sees, not just what this call answered.
        let (status, fetched) = get_stage(&emulator, &api_id, &stage_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetStage after the patch, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            strip_nulls(&fetched),
            strip_nulls(&updated),
            "case {:?}: GetStage must report what UpdateStage stored; expected {updated}, got {fetched}",
            row.case
        );

        // The identity of the stage and the instant it was created do not move; the
        // instant it was last updated moves exactly when the patch rewrote the record.
        assert_eq!(
            as_str(&fetched, "stageName"),
            stage_name,
            "case {:?}: the patch must not change the StageName, got {fetched}",
            row.case
        );
        assert_eq!(
            instant(row.case, &fetched, "createdDate"),
            created_at,
            "case {:?}: the patch must not move the CreatedDate, got {fetched}",
            row.case
        );
        let updated_after = instant(row.case, &fetched, "lastUpdatedDate");
        let patched_a_member = !patch
            .as_object()
            .expect("the patch is a JSON object")
            .is_empty();
        if patched_a_member {
            assert!(
                updated_after >= updated_before,
                "case {:?}: the patched stage's LastUpdatedDate must not go backwards; \
                 expected at least {updated_before}, got {updated_after}",
                row.case
            );
        } else {
            assert_eq!(
                updated_after, updated_before,
                "case {:?}: a patch that configured nothing must leave LastUpdatedDate \
                 where it was; expected {updated_before}, got {updated_after}",
                row.case
            );
        }
    }
}

/// How a refusal row names the deployment it sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeploymentRef {
    /// The patch sends no `DeploymentId`.
    None,
    /// A well-formed id no api ever minted.
    Unminted,
    /// An id of the row's own api, substituted at run time.
    Minted,
}

/// One refusal row: the stage it patches, the patch, and the modelled answer.
struct RefusalRow {
    case: &'static str,
    /// The `ProtocolType` of the api holding the stage, so a member documented for one
    /// protocol can be aimed at an api of the other.
    protocol: &'static str,
    /// An `ApiId` to send instead of the row's own api, for the "no such api" row.
    api_id: Option<&'static str>,
    /// A `StageName` to send instead of the row's own stage, for the "no such stage"
    /// row.
    stage_name: Option<&'static str>,
    patch: Value,
    deployment: DeploymentRef,
    status: u16,
    code: &'static str,
}

fn refusal_rows() -> Vec<RefusalRow> {
    let too_long_description = "d".repeat(MAX_DESCRIPTION_LEN + 1);
    let too_long_variable_value = "v".repeat(MAX_STAGE_VARIABLE_VALUE_LEN + 1);
    vec![
        RefusalRow {
            case: "an ApiId no api holds",
            protocol: "WEBSOCKET",
            api_id: Some("nosuchapi00"),
            stage_name: None,
            patch: json!({ "description": "unreachable" }),
            deployment: DeploymentRef::None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalRow {
            case: "a StageName the api does not hold",
            protocol: "WEBSOCKET",
            api_id: None,
            stage_name: Some("nosuchstage"),
            patch: json!({ "description": "unreachable" }),
            deployment: DeploymentRef::None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalRow {
            case: "a DeploymentId naming no deployment of the api",
            protocol: "WEBSOCKET",
            api_id: None,
            stage_name: None,
            patch: json!({}),
            deployment: DeploymentRef::Unminted,
            status: 404,
            code: "NotFoundException",
        },
        RefusalRow {
            case: "a DeploymentId on a stage the patch leaves auto-deploying",
            protocol: "WEBSOCKET",
            api_id: None,
            stage_name: None,
            patch: json!({ "autoDeploy": true }),
            deployment: DeploymentRef::Minted,
            status: 400,
            code: "BadRequestException",
        },
        RefusalRow {
            case: "a Description one character past its shape's bound",
            protocol: "WEBSOCKET",
            api_id: None,
            stage_name: None,
            patch: json!({ "description": too_long_description }),
            deployment: DeploymentRef::None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalRow {
            case: "a stage variable name outside the documented alphabet",
            protocol: "WEBSOCKET",
            api_id: None,
            stage_name: None,
            patch: json!({ "stageVariables": { "not-a-name": "ok" } }),
            deployment: DeploymentRef::None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalRow {
            case: "a stage variable value outside the documented pattern",
            protocol: "WEBSOCKET",
            api_id: None,
            stage_name: None,
            patch: json!({ "stageVariables": { "table": "two words" } }),
            deployment: DeploymentRef::None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalRow {
            case: "a stage variable value one character past its shape's bound",
            protocol: "WEBSOCKET",
            api_id: None,
            stage_name: None,
            patch: json!({ "stageVariables": { "table": too_long_variable_value } }),
            deployment: DeploymentRef::None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalRow {
            case: "an access log format selecting no request id",
            protocol: "WEBSOCKET",
            api_id: None,
            stage_name: None,
            patch: json!({ "accessLogSettings": { "format": "$context.status" } }),
            deployment: DeploymentRef::None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalRow {
            case: "a client certificate on an HTTP api",
            protocol: "HTTP",
            api_id: None,
            stage_name: None,
            patch: json!({ "clientCertificateId": "cert4321ba" }),
            deployment: DeploymentRef::None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalRow {
            case: "route settings data tracing on an HTTP api",
            protocol: "HTTP",
            api_id: None,
            stage_name: None,
            patch: json!({ "routeSettings": { "GET /x": { "dataTraceEnabled": true } } }),
            deployment: DeploymentRef::None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalRow {
            case: "a default route settings logging level on an HTTP api",
            protocol: "HTTP",
            api_id: None,
            stage_name: None,
            patch: json!({ "defaultRouteSettings": { "loggingLevel": "INFO" } }),
            deployment: DeploymentRef::None,
            status: 400,
            code: "BadRequestException",
        },
    ]
}

#[tokio::test]
async fn a_patch_the_service_cannot_satisfy_is_refused_and_rewrites_nothing() {
    let emulator = emulator();

    for (index, row) in refusal_rows().into_iter().enumerate() {
        let api_id = create_api(
            &emulator,
            row.case,
            &api_request(row.protocol, &format!("stage-patch-refused-{index}")),
        )
        .await;
        create_route(
            &emulator,
            row.case,
            &api_id,
            if row.protocol == "WEBSOCKET" {
                "$connect"
            } else {
                "GET /x"
            },
        )
        .await;
        let deployment_id = create_deployment(&emulator, row.case, &api_id).await;
        // The stage every refusal row patches: a plain one for an HTTP api, so the
        // WebSocket-only rows are refused for the protocol and not for the stage.
        let mut request = json!({ "stageName": "prod", "description": "the stored stage",
                                  "deploymentId": deployment_id });
        if row.protocol == "WEBSOCKET" {
            request["stageVariables"] = json!({ "table": "orders" });
        }
        let stage_name = create_stage(&emulator, row.case, &api_id, &request).await;
        let (status, stored) = get_stage(&emulator, &api_id, &stage_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetStage on the fresh stage, got {status} {stored}",
            row.case
        );

        let mut patch = row.patch.clone();
        match row.deployment {
            DeploymentRef::None => {}
            DeploymentRef::Unminted => patch["deploymentId"] = json!(UNMINTED_DEPLOYMENT_ID),
            DeploymentRef::Minted => patch["deploymentId"] = json!(deployment_id),
        }
        let (status, answer) = update_stage(
            &emulator,
            row.api_id.unwrap_or(&api_id),
            row.stage_name.unwrap_or(&stage_name),
            &patch,
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected {} from UpdateStage, got {status} {answer}",
            row.case, row.status
        );
        assert_eq!(
            error_code(&answer),
            row.code,
            "case {:?}: expected a {} , got {answer}",
            row.case,
            row.code
        );
        assert!(
            answer
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {:?}: the refusal must carry a message, got {answer}",
            row.case
        );

        // The refused patch wrote nothing: a later caller reads the stage as it was.
        let (status, fetched) = get_stage(&emulator, &api_id, &stage_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetStage after the refusal, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            strip_nulls(&fetched),
            strip_nulls(&stored),
            "case {:?}: the refused patch must leave the stage as it was; expected \
             {stored}, got {fetched}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_patch_that_leaves_a_stage_auto_deploying_releases_a_deployment_the_service_owns() {
    let emulator = emulator();
    let case = "auto-deploy switched on by a patch";

    let api_id = create_api(&emulator, case, &http_api("stage-patch-autodeploy")).await;
    create_route(&emulator, case, &api_id, "GET /items").await;
    // A stage that did not ask for auto-deploy and named no deployment serves none, so
    // there is nothing for it to keep when auto-deploy goes on.
    let stage_name = create_stage(
        &emulator,
        case,
        &api_id,
        &json!({ "stageName": "prod", "autoDeploy": false }),
    )
    .await;
    let (status, created) = get_stage(&emulator, &api_id, &stage_name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetStage, got {status} {created}"
    );
    assert!(
        created.get("deploymentId").is_none_or(Value::is_null),
        "case {case:?}: a stage that asked for neither must serve no deployment, got {created}"
    );

    let (status, updated) =
        update_stage(&emulator, &api_id, &stage_name, &json!({ "autoDeploy": true })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdateStage, got {status} {updated}"
    );
    let released = as_str(&updated, "deploymentId");
    assert_minted_id(case, "deploymentId", &released);
    assert!(
        !as_str(&updated, "lastDeploymentStatusMessage").is_empty(),
        "case {case:?}: an auto-deployed stage must report the status of its last \
         deployment, got {updated}"
    );

    // The deployment is a real resource of this api, released by the SERVICE.
    let (status, deployment) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/deployments/{released}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetDeployment on the released deployment, got \
         {status} {deployment}"
    );
    assert_eq!(
        deployment.get("autoDeployed"),
        Some(&json!(true)),
        "case {case:?}: the released deployment must report that the service released \
         it, got {deployment}"
    );
    assert_eq!(
        as_str(&deployment, "deploymentStatus"),
        DEPLOYED,
        "case {case:?}: the released deployment must be {DEPLOYED}, got {deployment}"
    );

    // A stage that already serves a deployment keeps it: a later patch releases no
    // second one.
    let (status, patched) = update_stage(
        &emulator,
        &api_id,
        &stage_name,
        &json!({ "description": "still the same deployment" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the second UpdateStage, got {status} {patched}"
    );
    assert_eq!(
        as_str(&patched, "deploymentId"),
        released,
        "case {case:?}: a stage that already serves a deployment must keep it, got {patched}"
    );
    let (status, deployments) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/deployments"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetDeployments, got {status} {deployments}"
    );
    let count = deployments
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        count, 1,
        "case {case:?}: exactly one deployment must have been released, got {deployments}"
    );
}
