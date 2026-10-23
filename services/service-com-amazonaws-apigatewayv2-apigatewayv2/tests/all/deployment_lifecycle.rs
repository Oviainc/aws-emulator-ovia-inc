// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `Deployment` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreateDeployment` persists a deployment under its api and answers 201 with the
//!   `DeploymentId` it minted**, reporting the `Description` the request carried (and
//!   none when it carried none), `DeploymentStatus` `DEPLOYED`, `AutoDeployed` false
//!   (the caller asked for this deployment, the service did not release it by itself)
//!   and the creation instant. `GetDeployment` reports that same record, member for
//!   member — nothing is echoed from the request without going through the store.
//! * A deployment belongs to ONE api: its id is not found through another api's path,
//!   and an api — or a `StageName` — that does not exist is a 404 rather than a
//!   deployment created nowhere.
//!
//! The rows vary what the rule generalises over: the api's protocol type, how the api
//! got its routes (quick create's `$default`, explicit `CreateRoute` calls, none at
//! all), whether the request carries a description, and the length of that description
//! at each bound of the shape it targets. The recorded case that filed the issue — a
//! `CreateDeployment` call carrying only the api id — is one row.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, assert_created_now, assert_minted_id, call, emulator, error_code,
};

/// The `DeploymentStatus` a released deployment carries, from the values
/// `DeploymentStatus` declares — `PENDING | FAILED | DEPLOYED`
/// (`smithy_query(query="shape", shape="DeploymentStatus")`).
const DEPLOYED: &str = "DEPLOYED";

/// The upper bound of `StringWithLengthBetween0And1024`, the shape
/// `CreateDeployment$Description` targets.
const MAX_DESCRIPTION_LEN: usize = 1024;

/// One row: the api to deploy, the routes to give it first, the `CreateDeployment`
/// body, and the `Description` the stored deployment must report.
struct Row {
    case: &'static str,
    /// The `CreateApi` request the deployment is created under.
    api: Value,
    /// Route keys to create under the api before deploying it.
    routes: &'static [&'static str],
    /// The `CreateDeployment` body; `None` sends no payload at all.
    request: Option<Value>,
    /// The `Description` the answer — and the later `GetDeployment` — must carry.
    description: Option<String>,
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
            case: "an HTTP api deployed by a request carrying only the api id",
            api: http_api("recorder-api"),
            routes: &["GET /items"],
            request: Some(json!({})),
            description: None,
        },
        Row {
            case: "an HTTP api with no routes, deployed with a description",
            api: http_api("empty-api"),
            routes: &[],
            request: Some(json!({ "description": "the first cut" })),
            description: Some("the first cut".to_string()),
        },
        Row {
            case: "a quick-create HTTP api whose $default route API Gateway manages",
            api: json!({
                "name": "quick-create-api",
                "protocolType": "HTTP",
                "target": "arn:aws:lambda:eu-west-1:000000000000:function:handler"
            }),
            routes: &[],
            request: Some(json!({ "description": "quick create, then deploy" })),
            description: Some("quick create, then deploy".to_string()),
        },
        Row {
            case: "a WebSocket api with the three route keys a socket api needs",
            api: websocket_api("socket-api"),
            routes: &["$connect", "$disconnect", "$default"],
            request: Some(json!({ "description": "sockets" })),
            description: Some("sockets".to_string()),
        },
        Row {
            case: "a description at the lower bound of its shape: the empty string",
            api: http_api("empty-description-api"),
            routes: &["POST /submit"],
            request: Some(json!({ "description": "" })),
            description: Some(String::new()),
        },
        Row {
            case: "a description at the upper bound of its shape: 1024 characters",
            api: http_api("long-description-api"),
            routes: &["ANY /{proxy+}"],
            request: Some(json!({ "description": "d".repeat(MAX_DESCRIPTION_LEN) })),
            description: Some("d".repeat(MAX_DESCRIPTION_LEN)),
        },
        Row {
            case: "a WebSocket api deployed by a request with no payload at all",
            api: websocket_api("no-payload-socket-api"),
            routes: &["$connect"],
            request: None,
            description: None,
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
async fn create_routes(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_keys: &[&str],
) {
    for route_key in route_keys {
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

/// Every member a stored deployment reports, whatever the request looked like.
fn assert_deployment(case: &str, body: &Value, description: Option<&String>) {
    assert_eq!(
        body.get("deploymentStatus"),
        Some(&json!(DEPLOYED)),
        "case {case:?}: expected DeploymentStatus {DEPLOYED}, got {body}"
    );
    assert_eq!(
        body.get("autoDeployed"),
        Some(&json!(false)),
        "case {case:?}: a deployment the caller asked for is not auto-released, got {body}"
    );
    assert_eq!(
        body.get("description"),
        description.map(|text| json!(text)).as_ref(),
        "case {case:?}: expected Description {description:?}, got {body}"
    );
    assert_created_now(case, body);
}

#[tokio::test]
async fn create_deployment_persists_the_record_get_deployment_reports_back() {
    let emulator = emulator();

    for row in rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        create_routes(&emulator, row.case, &api_id, row.routes).await;

        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/deployments"),
            row.request.clone(),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateDeployment, got {status} {created}",
            row.case
        );
        let deployment_id = as_str(&created, "deploymentId");
        assert_minted_id(row.case, "DeploymentId", &deployment_id);
        assert_deployment(row.case, &created, row.description.as_ref());

        // The same record, read back through the api that owns it.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetDeployment, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "deploymentId"),
            deployment_id,
            "case {:?}: expected the stored deployment's own id, got {fetched}",
            row.case
        );
        assert_deployment(row.case, &fetched, row.description.as_ref());

        // A read must not change what a later caller sees.
        let (_, refetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
            None,
        )
        .await;
        assert_eq!(
            refetched, fetched,
            "case {:?}: GetDeployment changed the record it reported",
            row.case
        );
    }
}

#[tokio::test]
async fn every_deployment_is_distinct_and_belongs_to_the_one_api_that_owns_it() {
    let emulator = emulator();

    let first = create_api(&emulator, "the owning api", &http_api("owner-api")).await;
    let second = create_api(&emulator, "another api", &http_api("other-api")).await;
    create_routes(&emulator, "the owning api", &first, &["GET /items"]).await;

    // One api carries MANY deployments: each call mints a new one rather than
    // rewriting the last.
    let mut minted: Vec<(String, String)> = Vec::new();
    for index in 0..5 {
        let description = format!("deployment #{index}");
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{first}/deployments"),
            Some(json!({ "description": description })),
        )
        .await;
        assert_eq!(
            status, 201,
            "deployment {index}: expected 201 from CreateDeployment, got {status} {created}"
        );
        minted.push((as_str(&created, "deploymentId"), description));
    }
    let ids: BTreeSet<&String> = minted.iter().map(|(id, _)| id).collect();
    assert_eq!(
        ids.len(),
        minted.len(),
        "expected one distinct DeploymentId per create, got {minted:?}"
    );

    // Each one still reports its OWN description: a later create did not overwrite it.
    for (index, (id, description)) in minted.iter().enumerate() {
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{first}/deployments/{id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "deployment {index}: expected 200 from GetDeployment, got {status} {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "description"),
            *description,
            "deployment {index}: expected its own Description, got {fetched}"
        );
    }

    for (case, uri) in [
        (
            "a deployment of another api",
            format!("/v2/apis/{second}/deployments/{}", minted[0].0),
        ),
        (
            "a deployment id nothing minted",
            format!("/v2/apis/{first}/deployments/nosuchdep0"),
        ),
        (
            "an api that does not exist",
            format!("/v2/apis/nosuchapi00/deployments/{}", minted[0].0),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetDeployment, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_deployment_naming_an_api_or_stage_that_does_not_exist_is_refused() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "an api with no stages", &http_api("stageless-api")).await;
    create_routes(&emulator, "an api with no stages", &api_id, &["GET /items"]).await;

    for (case, uri, request) in [
        (
            "an api that does not exist",
            "/v2/apis/nosuchapi00/deployments".to_string(),
            json!({}),
        ),
        (
            "a StageName naming no stage of this api",
            format!("/v2/apis/{api_id}/deployments"),
            json!({ "stageName": "prod" }),
        ),
    ] {
        let (status, body) = call(&emulator, "POST", &uri, Some(request)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from CreateDeployment, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // One past the upper bound of `StringWithLengthBetween0And1024` is refused, and
    // the deployment is not created: the id a later create mints is the api's first.
    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({ "description": "d".repeat(MAX_DESCRIPTION_LEN + 1) })),
    )
    .await;
    assert_eq!(
        status, 400,
        "a Description one character past its bound must be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "expected BadRequestException for a Description past its bound, got {body}"
    );

    // A refused create stored nothing: the api that only saw refusals has no
    // deployment to read.
    let (status, body) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/deployments/nosuchdep0"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "expected the api to hold no deployment, got {status} {body}"
    );
}
