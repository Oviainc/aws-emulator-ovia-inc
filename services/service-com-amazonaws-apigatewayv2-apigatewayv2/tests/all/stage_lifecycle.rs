// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `Stage` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreateStage` persists a stage under its api, keyed by the `StageName` the
//!   request named, and answers 201 with the stored stage document** — the members the
//!   request supplied, the documented defaults for the ones it omitted (`AutoDeploy`
//!   false, `ApiGatewayManaged` false), and the instants it was created and last
//!   updated at. `GetStage` reports that same record, member for member: nothing is
//!   echoed from the request without going through the store.
//! * A stage of an api whose `AutoDeploy` is on has a deployment **released by the
//!   service** ("Specifies whether a deployment was automatically released",
//!   `Deployment$AutoDeployed`), so it reports a `DeploymentId` of its own api whose
//!   deployment is `DEPLOYED` and auto-released; a stage that did not ask for
//!   auto-deploy, and named no deployment, reports none.
//! * A request the service cannot satisfy is REFUSED with the modelled error rather
//!   than creating a stage: an api or a `DeploymentId` that does not exist is a 404, a
//!   `StageName` that already names a stage of the api a 409, a request member that
//!   breaks a documented rule a 400, and the api's stage quota a 429.
//! * An `AccessLogSettings$Format` must select the request id ("The format must include
//!   at least $context.requestId"), so a format that selects none is refused 400 and
//!   stores nothing, while a settings block that names no format at all configures no
//!   access logging and is stored as it came.
//! * Deploying to a stage that already exists moves the stage onto that deployment,
//!   which `GetStage` reports.
//!
//! The rows vary what the rules generalise over: the parent api's protocol type, the
//! form and length of the stage name, which optional members the request carries, the
//! `AutoDeploy` flag, where the stage's deployment came from, what an access log format
//! selects, and the bounds of the shapes the members target. The recorded case that
//! filed the issue — a `$default` stage created with `AutoDeploy` on under an HTTP api —
//! is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, assert_created_now, assert_minted_id, call, emulator, error_code,
};

/// The upper bound of `StringWithLengthBetween1And128`, the shape
/// `CreateStage$StageName` targets.
const MAX_STAGE_NAME_LEN: usize = 128;

/// The upper bound of `StringWithLengthBetween0And1024`, the shape
/// `CreateStage$Description` targets.
const MAX_DESCRIPTION_LEN: usize = 1024;

/// AWS's published API Gateway quota of stages per API.
const MAX_STAGES_PER_API: usize = 10;

/// The `DeploymentStatus` a released deployment carries, from the values
/// `DeploymentStatus` declares — `PENDING | FAILED | DEPLOYED`
/// (`smithy_query(query="shape", shape="DeploymentStatus")`).
const DEPLOYED: &str = "DEPLOYED";

/// Where the stage's `DeploymentId` is expected to come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deployment {
    /// The stage reports none: it did not ask for auto-deploy and named no deployment.
    None,
    /// The service released one for an auto-deploying stage.
    AutoReleased,
    /// The one the request named.
    Named,
}

/// One row: the api to create the stage under, the `CreateStage` body, and what the
/// stored stage must report.
struct Row {
    case: &'static str,
    /// The `CreateApi` request the stage is created under.
    api: Value,
    /// Route keys to create under the api before the stage.
    routes: &'static [&'static str],
    /// The `CreateStage` body. When `deployment` is [`Deployment::Named`] a
    /// `deploymentId` member is added to it at run time, naming a deployment created
    /// first through `CreateDeployment`.
    request: Value,
    /// The `StageName` the stage is keyed by and reports.
    stage_name: String,
    /// The `AutoDeploy` the stored stage must report.
    auto_deploy: bool,
    /// The `Description` the stored stage must report.
    description: Option<String>,
    deployment: Deployment,
}

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the $default stage of an HTTP api, created with AutoDeploy on",
            api: http_api("metrics-api"),
            routes: &["GET /x"],
            request: json!({ "stageName": "$default", "autoDeploy": true }),
            stage_name: "$default".to_string(),
            auto_deploy: true,
            description: None,
            deployment: Deployment::AutoReleased,
        },
        Row {
            case: "a named stage carrying no optional member at all",
            api: http_api("plain-api"),
            routes: &["GET /items"],
            request: json!({ "stageName": "prod" }),
            stage_name: "prod".to_string(),
            auto_deploy: false,
            description: None,
            deployment: Deployment::None,
        },
        Row {
            case: "a stage name carrying hyphens and underscores",
            api: http_api("dashed-api"),
            routes: &[],
            request: json!({ "stageName": "pre-prod_v2", "autoDeploy": false }),
            stage_name: "pre-prod_v2".to_string(),
            auto_deploy: false,
            description: None,
            deployment: Deployment::None,
        },
        Row {
            case: "a stage name at the upper bound of its shape: 128 characters",
            api: http_api("long-name-api"),
            routes: &[],
            request: json!({ "stageName": "s".repeat(MAX_STAGE_NAME_LEN) }),
            stage_name: "s".repeat(MAX_STAGE_NAME_LEN),
            auto_deploy: false,
            description: None,
            deployment: Deployment::None,
        },
        Row {
            case: "an HTTP stage with every optional member the service stores",
            api: http_api("fully-specified-api"),
            routes: &["GET /x", "POST /y"],
            request: json!({
                "stageName": "observed",
                "autoDeploy": true,
                "description": "detailed metrics and a throttle",
                "defaultRouteSettings": {
                    "detailedMetricsEnabled": true,
                    "throttlingBurstLimit": 500,
                    "throttlingRateLimit": 12.5
                },
                "routeSettings": {
                    "GET /x": {
                        "detailedMetricsEnabled": true,
                        "throttlingBurstLimit": 0,
                        "throttlingRateLimit": 0.0
                    }
                },
                "stageVariables": { "table_name": "items-v1", "endpoint": "https://example.com/path" },
                "accessLogSettings": {
                    "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:api-logs",
                    "format": "$context.requestId"
                },
                "tags": { "team": "platform", "env": "test" }
            }),
            stage_name: "observed".to_string(),
            auto_deploy: true,
            description: Some("detailed metrics and a throttle".to_string()),
            deployment: Deployment::AutoReleased,
        },
        Row {
            case: "a WebSocket stage with the client certificate and logging members \
                   that protocol supports",
            api: websocket_api("socket-api"),
            routes: &["$connect", "$disconnect"],
            request: json!({
                "stageName": "sockets",
                "clientCertificateId": "abcd1234ef",
                "defaultRouteSettings": {
                    "dataTraceEnabled": true,
                    "loggingLevel": "INFO",
                    "throttlingRateLimit": 7.5
                }
            }),
            stage_name: "sockets".to_string(),
            auto_deploy: false,
            description: None,
            deployment: Deployment::None,
        },
        Row {
            case: "a description at the lower bound of its shape: the empty string",
            api: http_api("empty-description-api"),
            routes: &[],
            request: json!({ "stageName": "blank", "description": "" }),
            stage_name: "blank".to_string(),
            auto_deploy: false,
            description: Some(String::new()),
            deployment: Deployment::None,
        },
        Row {
            case: "a description at the upper bound of its shape: 1024 characters",
            api: http_api("long-description-api"),
            routes: &[],
            request: json!({
                "stageName": "wordy",
                "description": "d".repeat(MAX_DESCRIPTION_LEN)
            }),
            stage_name: "wordy".to_string(),
            auto_deploy: false,
            description: Some("d".repeat(MAX_DESCRIPTION_LEN)),
            deployment: Deployment::None,
        },
        Row {
            case: "a stage deployed to a deployment the caller created first",
            api: http_api("explicit-deployment-api"),
            routes: &["ANY /{proxy+}"],
            request: json!({ "stageName": "canary" }),
            stage_name: "canary".to_string(),
            auto_deploy: false,
            description: None,
            deployment: Deployment::Named,
        },
        Row {
            case: "an auto-deploying stage of an api that has no routes yet",
            api: http_api("routeless-api"),
            routes: &[],
            request: json!({ "stageName": "early", "autoDeploy": true }),
            stage_name: "early".to_string(),
            auto_deploy: true,
            description: None,
            deployment: Deployment::AutoReleased,
        },
    ]
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

/// Give `api_id` one route per key.
async fn create_routes(emulator: &MultiServiceRouter, case: &str, api_id: &str, keys: &[&str]) {
    for route_key in keys {
        let (status, created) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": route_key })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateRoute for {route_key:?}, got {status} {created}"
        );
    }
}

/// Deploy `api_id` and answer the `DeploymentId` the service minted.
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

/// Create one stage and answer `(status, body)`.
async fn create_stage(
    emulator: &MultiServiceRouter,
    api_id: &str,
    request: Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(request),
    )
    .await
}

/// Every member the stored stage reports, whatever the request looked like.
fn assert_stage(case: &str, row: &Row, body: &Value, named_deployment: Option<&String>) {
    assert_eq!(
        body.get("stageName"),
        Some(&json!(row.stage_name)),
        "case {case:?}: expected StageName {:?}, got {body}",
        row.stage_name
    );
    assert_eq!(
        body.get("autoDeploy"),
        Some(&json!(row.auto_deploy)),
        "case {case:?}: expected AutoDeploy {}, got {body}",
        row.auto_deploy
    );
    assert_eq!(
        body.get("apiGatewayManaged"),
        Some(&json!(false)),
        "case {case:?}: a stage the caller asked for is not API-Gateway-managed, got {body}"
    );
    assert_eq!(
        body.get("description"),
        row.description.as_ref().map(|text| json!(text)).as_ref(),
        "case {case:?}: expected Description {:?}, got {body}",
        row.description
    );
    assert_created_now(case, body);
    // Nothing has rewritten the record, so it was last updated when it was created.
    assert_eq!(
        body.get("lastUpdatedDate"),
        body.get("createdDate"),
        "case {case:?}: expected LastUpdatedDate to be the creation instant, got {body}"
    );

    // Every member the request supplied is reported back from the store, unchanged.
    for member in [
        "defaultRouteSettings",
        "routeSettings",
        "stageVariables",
        "accessLogSettings",
        "clientCertificateId",
        "tags",
    ] {
        if let Some(sent) = row.request.get(member) {
            assert_eq!(
                body.get(member),
                Some(sent),
                "case {case:?}: expected {member} {sent}, got {body}"
            );
        }
    }
    // A member the request left out is reported as absent, not as an invented value.
    for member in ["accessLogSettings", "clientCertificateId"] {
        if row.request.get(member).is_none() {
            assert_eq!(
                body.get(member),
                None,
                "case {case:?}: {member} was not requested, got {body}"
            );
        }
    }

    match row.deployment {
        Deployment::None => {
            assert_eq!(
                body.get("deploymentId"),
                None,
                "case {case:?}: a stage that asked for no deployment reports none, got {body}"
            );
            assert_eq!(
                body.get("lastDeploymentStatusMessage"),
                None,
                "case {case:?}: a stage that released no deployment has no status message, got {body}"
            );
        }
        Deployment::AutoReleased => {
            let deployment_id = as_str(body, "deploymentId");
            assert_minted_id(case, "DeploymentId", &deployment_id);
            let message = as_str(body, "lastDeploymentStatusMessage");
            assert!(
                message.contains(&deployment_id),
                "case {case:?}: LastDeploymentStatusMessage must describe the released \
                 deployment {deployment_id:?}, got {message:?}"
            );
        }
        Deployment::Named => {
            let expected = named_deployment.expect("the row names a deployment");
            assert_eq!(
                body.get("deploymentId"),
                Some(&json!(expected)),
                "case {case:?}: expected the DeploymentId the request named, got {body}"
            );
            assert_eq!(
                body.get("lastDeploymentStatusMessage"),
                None,
                "case {case:?}: the service released nothing by itself, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn create_stage_persists_the_record_get_stage_reports_back() {
    let emulator = emulator();

    for mut row in rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        create_routes(&emulator, row.case, &api_id, row.routes).await;

        let named_deployment = if row.deployment == Deployment::Named {
            let deployment_id = create_deployment(&emulator, row.case, &api_id).await;
            row.request
                .as_object_mut()
                .expect("the CreateStage body is an object")
                .insert("deploymentId".to_string(), json!(deployment_id));
            Some(deployment_id)
        } else {
            None
        };

        let (status, created) = create_stage(&emulator, &api_id, row.request.clone()).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateStage, got {status} {created}",
            row.case
        );
        assert_stage(row.case, &row, &created, named_deployment.as_ref());

        // The same record, read back through the api that owns it: the create answered
        // with what the store holds, so the two documents are identical.
        // Every stage name in the table is path-safe, `$default` included.
        let stage_path = format!("/v2/apis/{api_id}/stages/{}", row.stage_name);
        let (status, fetched) = call(&emulator, "GET", &stage_path, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetStage, got {status} {fetched}",
            row.case
        );
        assert_stage(row.case, &row, &fetched, named_deployment.as_ref());
        assert_eq!(
            fetched, created,
            "case {:?}: GetStage must report the document CreateStage answered with",
            row.case
        );

        // A read must not change what a later caller sees.
        let (_, refetched) = call(&emulator, "GET", &stage_path, None).await;
        assert_eq!(
            refetched, fetched,
            "case {:?}: GetStage changed the record it reported",
            row.case
        );

        // The deployment an auto-deploying stage released is one of this api's own,
        // and the service released it rather than the caller.
        if row.deployment == Deployment::AutoReleased {
            let deployment_id = as_str(&created, "deploymentId");
            let (status, deployment) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected 200 from GetDeployment for the released deployment, \
                 got {status} {deployment}",
                row.case
            );
            assert_eq!(
                deployment.get("autoDeployed"),
                Some(&json!(true)),
                "case {:?}: a deployment the service released is auto-deployed, got {deployment}",
                row.case
            );
            assert_eq!(
                deployment.get("deploymentStatus"),
                Some(&json!(DEPLOYED)),
                "case {:?}: expected DeploymentStatus {DEPLOYED}, got {deployment}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_second_stage_with_the_same_name_is_refused_and_the_first_is_untouched() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "the owning api", &http_api("conflict-api")).await;
    let (status, created) = create_stage(
        &emulator,
        &api_id,
        json!({ "stageName": "prod", "description": "the first one" }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from the first CreateStage, got {status} {created}"
    );

    for (case, request) in [
        ("the same name again", json!({ "stageName": "prod" })),
        (
            "the same name with different members",
            json!({ "stageName": "prod", "autoDeploy": true, "description": "the second one" }),
        ),
    ] {
        let (status, body) = create_stage(&emulator, &api_id, request).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 from CreateStage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    }

    // The refused creates rewrote nothing: the stage still reports what the first
    // create stored.
    let (status, fetched) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/stages/prod"), None)
        .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetStage, got {status} {fetched}"
    );
    assert_eq!(
        fetched, created,
        "a refused CreateStage must not change the stage that is there"
    );

    // The name belongs to ONE api: another api may carry a stage of the same name.
    let other = create_api(&emulator, "another api", &http_api("other-api")).await;
    let (status, body) = create_stage(&emulator, &other, json!({ "stageName": "prod" })).await;
    assert_eq!(
        status, 201,
        "a stage name is unique within its api only, got {status} {body}"
    );
}

#[tokio::test]
async fn a_stage_the_service_cannot_create_is_refused_rather_than_created() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "an HTTP api", &http_api("refusal-api")).await;
    create_routes(&emulator, "an HTTP api", &api_id, &["GET /items"]).await;

    // Each case names the stage it tried to create, so the absence check below can ask
    // for it by name.
    let cases: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "an api that does not exist",
            "nosuchapi00".to_string(),
            json!({ "stageName": "prod" }),
            404,
            "NotFoundException",
        ),
        (
            "a DeploymentId naming no deployment of this api",
            api_id.clone(),
            json!({ "stageName": "detached", "deploymentId": "nosuchdep0" }),
            404,
            "NotFoundException",
        ),
        (
            "a StageName one character past the upper bound of its shape",
            api_id.clone(),
            json!({ "stageName": "s".repeat(MAX_STAGE_NAME_LEN + 1) }),
            400,
            "BadRequestException",
        ),
        (
            "a StageName carrying a character stage names do not admit",
            api_id.clone(),
            json!({ "stageName": "pro d" }),
            400,
            "BadRequestException",
        ),
        (
            "a StageName that is a reserved-looking name the service does not reserve",
            api_id.clone(),
            json!({ "stageName": "$latest" }),
            400,
            "BadRequestException",
        ),
        (
            "a Description one character past the upper bound of its shape",
            api_id.clone(),
            json!({
                "stageName": "wordy",
                "description": "d".repeat(MAX_DESCRIPTION_LEN + 1)
            }),
            400,
            "BadRequestException",
        ),
        (
            "a stage variable name carrying a character the documented rule excludes",
            api_id.clone(),
            json!({ "stageName": "badvarname", "stageVariables": { "table-name": "items" } }),
            400,
            "BadRequestException",
        ),
        (
            "a stage variable value carrying a character the documented pattern excludes",
            api_id.clone(),
            json!({ "stageName": "badvarvalue", "stageVariables": { "table": "items v1" } }),
            400,
            "BadRequestException",
        ),
        (
            "a stage variable value that is empty, which the documented pattern excludes",
            api_id.clone(),
            json!({ "stageName": "emptyvar", "stageVariables": { "table": "" } }),
            400,
            "BadRequestException",
        ),
        (
            "a ClientCertificateId, which only a WebSocket api supports",
            api_id.clone(),
            json!({ "stageName": "certified", "clientCertificateId": "abcd1234ef" }),
            400,
            "BadRequestException",
        ),
    ];

    for (case, target_api, request, status_expected, code) in cases {
        let stage_name = as_str(&request, "stageName");
        let (status, body) = create_stage(&emulator, &target_api, request).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected {status_expected} from CreateStage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );

        // A refused create stored nothing: the name it tried to take is still free.
        if target_api == api_id && !stage_name.contains(' ') {
            let (status, fetched) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/stages/{stage_name}"),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused CreateStage must store nothing, got {status} {fetched}"
            );
            assert_eq!(
                error_code(&fetched),
                "NotFoundException",
                "case {case:?}: expected NotFoundException from GetStage, got {fetched}"
            );
        }
    }
}

#[tokio::test]
async fn a_stage_of_an_api_that_is_at_its_stage_quota_is_refused() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "a busy api", &http_api("quota-api")).await;
    for index in 0..MAX_STAGES_PER_API {
        let (status, body) =
            create_stage(&emulator, &api_id, json!({ "stageName": format!("stage{index}") })).await;
        assert_eq!(
            status, 201,
            "stage {index}: expected 201 from CreateStage, got {status} {body}"
        );
    }

    let (status, body) = create_stage(&emulator, &api_id, json!({ "stageName": "onetoomany" })).await;
    assert_eq!(
        status, 429,
        "expected 429 once the api holds {MAX_STAGES_PER_API} stages, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException at the stage quota, got {body}"
    );

    // Another api is not at the quota: the bound is per api.
    let other = create_api(&emulator, "a fresh api", &http_api("quota-free-api")).await;
    let (status, body) = create_stage(&emulator, &other, json!({ "stageName": "onetoomany" })).await;
    assert_eq!(
        status, 201,
        "the stage quota is per api, got {status} {body}"
    );
}

#[tokio::test]
async fn a_stage_is_reported_only_through_the_api_that_owns_it() {
    let emulator = emulator();

    let first = create_api(&emulator, "the owning api", &http_api("owner-api")).await;
    let second = create_api(&emulator, "another api", &http_api("bystander-api")).await;
    let (status, body) = create_stage(&emulator, &first, json!({ "stageName": "prod" })).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateStage, got {status} {body}"
    );

    for (case, uri) in [
        (
            "the stage of another api",
            format!("/v2/apis/{second}/stages/prod"),
        ),
        (
            "a stage name nothing created",
            format!("/v2/apis/{first}/stages/nosuchstage"),
        ),
        (
            "an api that does not exist",
            "/v2/apis/nosuchapi00/stages/prod".to_string(),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetStage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn an_access_log_format_must_select_the_request_id() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the owning api", &http_api("access-log-api")).await;

    // "The format must include at least $context.requestId"
    // (`AccessLogSettings$Format`), so a format that selects none could never tie an
    // entry to the request that produced it. The rows vary what the format DOES select
    // and whether the settings name a destination.
    for (case, stage_name, settings) in [
        (
            "a format selecting other context variables",
            "other-variables",
            json!({ "format": "$context.status $context.protocol" }),
        ),
        (
            "a format at the lower bound of its shape: the empty string",
            "empty-format",
            json!({ "format": "" }),
        ),
        (
            "a format naming a destination but selecting no request id",
            "with-destination",
            json!({
                "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:api-logs",
                "format": "$context.identity.sourceIp - $context.requestTime"
            }),
        ),
    ] {
        let (status, body) = create_stage(
            &emulator,
            &api_id,
            json!({ "stageName": stage_name, "accessLogSettings": settings }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateStage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );

        // A refused create stored nothing.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused CreateStage must store nothing, got {status} {fetched}"
        );
    }

    // A format that DOES select the request id is stored as it came, and so is a
    // settings block that names no format at all: that one configures no access logging,
    // so there is no format to require anything of.
    for (case, stage_name, settings) in [
        (
            "a format selecting the request id among other variables",
            "logged",
            json!({
                "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:api-logs",
                "format": "$context.requestId $context.status"
            }),
        ),
        (
            "a format selecting nothing but the request id",
            "minimal-format",
            json!({ "format": "$context.requestId" }),
        ),
        (
            "no format at all, which configures no access logging",
            "unformatted",
            json!({ "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:api-logs" }),
        ),
    ] {
        let (status, created) = create_stage(
            &emulator,
            &api_id,
            json!({ "stageName": stage_name, "accessLogSettings": settings }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateStage, got {status} {created}"
        );
        assert_eq!(
            created.get("accessLogSettings"),
            Some(&settings),
            "case {case:?}: expected the settings the request carried, got {created}"
        );
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} {fetched}"
        );
        assert_eq!(
            fetched, created,
            "case {case:?}: GetStage must report the record CreateStage stored"
        );
    }
}

#[tokio::test]
async fn deploying_to_a_stage_moves_it_onto_that_deployment() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the owning api", &http_api("redeployed-api")).await;
    create_routes(&emulator, "the owning api", &api_id, &["GET /items"]).await;

    // A stage that asked for no deployment serves none.
    let (status, created) = create_stage(&emulator, &api_id, json!({ "stageName": "prod" })).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateStage, got {status} {created}"
    );
    assert_eq!(
        created.get("deploymentId"),
        None,
        "a stage created with no deployment must serve none, got {created}"
    );

    // Deploying to it, and then deploying again, moves the stage onto each deployment in
    // turn: a `Deployment` "must be associated with a Stage for it to be callable over
    // the internet" (the `Deployment` shape's own doc).
    let mut released = Vec::new();
    for case in ["the first deployment", "a second deployment"] {
        let (status, deployment) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/deployments"),
            Some(json!({ "stageName": "prod" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateDeployment, got {status} {deployment}"
        );
        let deployment_id = as_str(&deployment, "deploymentId");
        assert!(
            !released.contains(&deployment_id),
            "case {case:?}: DeploymentId {deployment_id:?} was minted twice"
        );
        released.push(deployment_id.clone());

        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/prod"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "deploymentId"),
            deployment_id,
            "case {case:?}: expected the stage to serve the deployment released to it, got {fetched}"
        );
    }
}
