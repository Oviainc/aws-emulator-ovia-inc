// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` data plane's STATUS TAXONOMY: that a client driving a deployed REST
//! API's invoke URL can tell the situations apart.
//!
//! This is the invariant the issue this suite was written for recorded as the worst of the
//! damage: every request to an invoke URL was answered `404` with
//! `{"__type":"UnknownService", …}`, the emulator's own front-door refusal, so "every
//! negative path is indistinguishable from every other — a missing Lambda permission, a
//! wrong integration URI, an undeployed stage and a denied authorizer all collapse to the
//! same 404 instead of 403/500. Any test that asserts on API Gateway's status taxonomy
//! silently passes or fails for the wrong reason."
//!
//! The per-rule behaviour of the data plane is pinned in
//! `tests/all/execute_api_data_plane.rs`, one rule at a time. What this suite adds is the
//! CROSS-CUTTING assertion that no single rule can make: over the recorded situations as
//! ONE table, each answer is API Gateway's own (never the router's `UnknownService`), and
//! the classes are pairwise DISTINCT — so a client, and a check, can branch on them.
//!
//! Every case drives the real wire path through the multi-service router, on the api's own
//! invoke host, with the control plane driven through the same emulator ([`Gateway`]).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};

/// The region the invoke host is addressed in; see `execute_api_data_plane`'s note on why
/// one is enough.
const REGION: &str = "us-east-1";

/// The error code the emulator's front door answers a request it can resolve no target
/// service for. It is the ONE answer an invoke URL must never get: it is the emulator
/// refusing a URL this very service published, and it is not in API Gateway's taxonomy at
/// all.
const ROUTER_REFUSAL: &str = "UnknownService";

/// One answered invoke: the status, the `x-amzn-errortype` header the router's own
/// refusals carry, and the body verbatim.
struct Invoked {
    status: u16,
    error_type: Option<String>,
    body: String,
}

impl Invoked {
    /// The `message` member of a JSON body — what every API Gateway gateway response
    /// carries — or the body itself when it carries none.
    fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|body| body.get("message")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| self.body.clone())
    }

    /// Whether this answer is the router's own refusal rather than the service's: the
    /// `__type` of its awsJson envelope, or the header it sets beside it.
    fn is_router_refusal(&self) -> bool {
        self.error_type.as_deref() == Some(ROUTER_REFUSAL) || self.body.contains(ROUTER_REFUSAL)
    }
}

/// Issue one unsigned GET to `host` at `path` — what a client of a published invoke URL
/// sends: the api's own endpoint host, and no AWS credential at all.
async fn invoke_on_host(gateway: &Gateway, host: &str, path: &str) -> Invoked {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("the test request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked {
        status,
        error_type,
        body,
    }
}

/// [`invoke_on_host`] against `api`'s own invoke endpoint.
async fn invoke(gateway: &Gateway, api: &str, path: &str) -> Invoked {
    invoke_on_host(gateway, &format!("{api}.execute-api.{REGION}.amazonaws.com"), path).await
}

/// `PutMethod` with `authorizationType: NONE` on `resource`, asserting it was accepted.
async fn put_open_method(gateway: &Gateway, api: &str, resource: &str) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutMethod: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutIntegration` on a method, asserting it was accepted. The integration's own verb
/// travels as `httpMethod`, the `jsonName` of the input's `integrationHttpMethod` member.
async fn put_integration(gateway: &Gateway, api: &str, resource: &str, request: Value) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            Some(request),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegration: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutIntegrationResponse` for `status` on a method's integration, asserting it was
/// accepted.
async fn put_integration_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    status: &str,
    body: &str,
) {
    let answer = gateway
        .call(
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/GET/integration/responses/{status}"
            ),
            Some(json!({ "responseTemplates": { "application/json": body } })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegrationResponse({status}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `CreateDeployment` into `stage`, asserting it was accepted.
async fn deploy(gateway: &Gateway, api: &str, stage: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": stage })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateDeployment({stage}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// A child resource under `parent`, answering its resource id.
async fn resource_under(gateway: &Gateway, api: &str, parent: &str, path_part: &str) -> String {
    let created = create_resource(gateway, api, parent, path_part).await;
    string_member(&created, "id", &format!("CreateResource({path_part})"))
}

#[tokio::test]
async fn the_recorded_invoke_situations_are_each_answered_distinguishably() {
    let gateway = Gateway::new();

    // An API that is deployed, with two methods: one a `MOCK` that the stage can serve in
    // full, and one whose integration names a Lambda function — the `AWS_PROXY` shape the
    // recorded "missing permission" and "wrong integration URI" checks drive, which no
    // deployment package runs for here.
    let (api, root) = create_api(&gateway, "taxonomy").await;
    let mocked = resource_under(&gateway, &api, &root, "mocked").await;
    put_open_method(&gateway, &api, &mocked).await;
    put_integration(&gateway, &api, &mocked, json!({ "type": "MOCK" })).await;
    put_integration_response(&gateway, &api, &mocked, "200", r#"{"at":"mocked"}"#).await;
    let proxied = resource_under(&gateway, &api, &root, "proxied").await;
    put_open_method(&gateway, &api, &proxied).await;
    put_integration(
        &gateway,
        &api,
        &proxied,
        json!({
            "type": "AWS_PROXY",
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 arn:aws:lambda:{REGION}:000000000000:function:absent/invocations"
            ),
        }),
    )
    .await;
    deploy(&gateway, &api, "prod").await;

    // And an API that was never deployed: the recorded "before any deployment" check.
    let (undeployed, undeployed_root) = create_api(&gateway, "never-deployed").await;
    let unreachable = resource_under(&gateway, &undeployed, &undeployed_root, "items").await;
    put_open_method(&gateway, &undeployed, &unreachable).await;
    put_integration(&gateway, &undeployed, &unreachable, json!({ "type": "MOCK" })).await;
    put_integration_response(&gateway, &undeployed, &unreachable, "200", r#"{"at":"items"}"#).await;

    /// One recorded situation: which API and path it drives, and the answer API Gateway
    /// gives it.
    struct Situation {
        case: &'static str,
        api: String,
        path: &'static str,
        status: u16,
        message: &'static str,
    }

    let situations = [
        Situation {
            case: "an integration the stage can serve in full",
            api: api.clone(),
            path: "/prod/mocked",
            status: 200,
            message: r#"{"at":"mocked"}"#,
        },
        Situation {
            case: "an integration whose backend cannot be called",
            api: api.clone(),
            path: "/prod/proxied",
            status: 500,
            message: "Internal server error",
        },
        Situation {
            case: "a path the deployment holds no resource for",
            api: api.clone(),
            path: "/prod/absent",
            status: 403,
            message: "Missing Authentication Token",
        },
        Situation {
            case: "an API with no deployment at all",
            api: undeployed.clone(),
            path: "/prod/items",
            status: 403,
            message: "Forbidden",
        },
    ];

    let mut classes: BTreeSet<(u16, String)> = BTreeSet::new();
    for situation in &situations {
        let answered = invoke(&gateway, &situation.api, situation.path).await;
        assert!(
            !answered.is_router_refusal(),
            "case {:?}: an invoke URL must never be answered the front door's {ROUTER_REFUSAL} \
             refusal, got {} with {:?}",
            situation.case,
            answered.status,
            answered.body
        );
        assert_eq!(
            answered.status, situation.status,
            "case {:?}: expected {}, got {} with {:?}",
            situation.case, situation.status, answered.status, answered.body
        );
        assert_eq!(
            answered.message(),
            situation.message,
            "case {:?}: wrong body for {}",
            situation.case,
            situation.path
        );
        classes.insert((answered.status, answered.message()));
    }

    // The point of the table: each situation is its OWN answer. A client branching on the
    // status and the message can tell all four apart, which is exactly what one collapsed
    // `404` made impossible.
    assert_eq!(
        classes.len(),
        situations.len(),
        "case \"the taxonomy as a whole\": expected {} distinguishable answers, got {classes:?}",
        situations.len()
    );
}
