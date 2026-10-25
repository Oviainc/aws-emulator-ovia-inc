// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `execute-api` data plane's API-KEY GATE: which requests a
//! deployed method whose `apiKeyRequired` is true admits, and that the refusal happens
//! BEFORE the method's integration is dispatched.
//!
//! The rule asserted here is that a method requiring an API key admits a request only when
//! the request presents the value of an ENABLED stored key that this api's records
//! associate with the INVOKED STAGE, and refuses every other request `403` without
//! dispatching the integration. Two associations say so, and both are driven:
//!
//! * a `UsagePlan` ("used to specify who can access associated API stages") whose
//!   `apiStages` name this api and stage, with a `UsagePlanKey` adding the key to it — the
//!   way a client associates a key today;
//! * the key's own `stageKeys`, which `CreateApiKey$stageKeys` documents as "DEPRECATED FOR
//!   USAGE PLANS - Specifies stages associated with the API key".
//!
//! The rows differ along what the rule generalises over: which of the two associations
//! covers the key, which api and which stage the association names, whether the key is
//! enabled, whether the presented value is a stored key's at all, which stage the call is
//! addressed to, and the integration type behind the method — a `MOCK`, which the stage
//! serves in full, and an `AWS_PROXY`, whose backend this emulator cannot call, so an
//! admitted call is answered `500` and a refused one `403`. That pair is what pins the
//! ORDER: the `403` cannot have come from the integration.
//!
//! Every case drives the real wire path through the multi-service router on the api's own
//! invoke host, with the control plane driven through the same emulator ([`Gateway`]).

use bytes::Bytes;
use serde_json::{json, Value};

use crate::api_key_lifecycle::create_key;
use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};
use crate::usage_plan_key_lifecycle::create_usage_plan_key;
use crate::usage_plan_lifecycle::create_usage_plan;

/// The region the invoke host is addressed in; any region is served, so one is enough.
const REGION: &str = "us-east-1";

/// The one key type a `UsagePlanKey` takes: "Currently, the valid key type is `API_KEY`"
/// (`smithy_query(query="doc", shape="UsagePlanKey", member="type")`).
const API_KEY_TYPE: &str = "API_KEY";

/// One answered invoke: its status line and its body verbatim.
struct Invoked {
    status: u16,
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
}

/// Issue one unsigned GET to `api`'s own invoke endpoint at `path`, carrying `headers`.
async fn invoke(gateway: &Gateway, api: &str, path: &str, headers: &[(&str, &str)]) -> Invoked {
    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}{path}"))
        .header("host", &host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::new())
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked { status, body }
}

/// `PutMethod` on `resource` for GET with `request` as its body, asserting the modelled
/// 201.
async fn put_method(gateway: &Gateway, api: &str, resource: &str, request: Value) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(request),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutMethod({resource}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutIntegration` on a GET method with `request` as its body, asserting the modelled 201.
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
        "case PutIntegration({resource}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutMethodResponse` then `PutIntegrationResponse` for `status` on a GET method,
/// asserting the modelled 201 for each.
async fn put_responses(gateway: &Gateway, api: &str, resource: &str, status: &str, body: &str) {
    for (what, uri, request) in [
        (
            "PutMethodResponse",
            format!("/restapis/{api}/resources/{resource}/methods/GET/responses/{status}"),
            json!({}),
        ),
        (
            "PutIntegrationResponse",
            format!(
                "/restapis/{api}/resources/{resource}/methods/GET/integration/responses/{status}"
            ),
            json!({ "responseTemplates": { "application/json": body } }),
        ),
    ] {
        let answer = gateway.call("PUT", &uri, Some(request)).await;
        assert_eq!(
            answer.status, 201,
            "case {what}({resource}/{status}): expected 201, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// `CreateDeployment` of `api` into `stage`, asserting the modelled 201.
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
        "case CreateDeployment({api} -> {stage}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// A child resource of `parent` named `path_part`, answering its resource id.
async fn resource_id(gateway: &Gateway, api: &str, parent: &str, path_part: &str) -> String {
    let created = create_resource(gateway, api, parent, path_part).await;
    string_member(&created, "id", &format!("CreateResource({path_part})"))
}

/// A GET method requiring an API key, behind a `MOCK` integration that answers `body`.
async fn keyed_mock(gateway: &Gateway, api: &str, parent: &str, path_part: &str, body: &str) {
    let resource = resource_id(gateway, api, parent, path_part).await;
    put_method(
        gateway,
        api,
        &resource,
        json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
    )
    .await;
    put_integration(gateway, api, &resource, json!({ "type": "MOCK" })).await;
    put_responses(gateway, api, &resource, "200", body).await;
}

/// A GET method requiring an API key, behind an `AWS_PROXY` integration naming a Lambda
/// function — a backend this emulator cannot call, so a call that REACHES it is answered
/// as an integration that could not be performed.
async fn keyed_proxy(gateway: &Gateway, api: &str, parent: &str, path_part: &str) {
    let resource = resource_id(gateway, api, parent, path_part).await;
    put_method(
        gateway,
        api,
        &resource,
        json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
    )
    .await;
    put_integration(
        gateway,
        api,
        &resource,
        json!({
            "type": "AWS_PROXY",
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 arn:aws:lambda:{REGION}:000000000000:function:keyed/invocations"
            ),
        }),
    )
    .await;
}

/// `CreateApiKey` for `value`, answering the key's id. `enabled` and `stage_keys` are the
/// two members a row varies.
async fn api_key(
    gateway: &Gateway,
    case: &str,
    value: &str,
    enabled: bool,
    stages: Value,
) -> String {
    let created = create_key(
        gateway,
        case,
        json!({ "name": case, "value": value, "enabled": enabled, "stageKeys": stages }),
    )
    .await;
    string_member(&created, "id", case)
}

/// `CreateUsagePlan` over `api_stages` with `key_id` added to it, which is how a client
/// associates an API key with an API stage.
async fn meter(gateway: &Gateway, case: &str, api_stages: Value, key_id: &str) {
    let plan = create_usage_plan(
        gateway,
        case,
        json!({ "name": format!("plan-{case}"), "apiStages": api_stages }),
    )
    .await;
    let plan = string_member(&plan, "id", case);
    let answer = create_usage_plan_key(gateway, &plan, key_id, API_KEY_TYPE).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected CreateUsagePlanKey to answer 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// A distinct API key value of the 40 characters AWS mints: a key's value is unique per
/// account, so no two rows may supply the same one.
fn key_value(n: u8) -> String {
    format!("{}{n:02}", "k".repeat(38))
}

#[tokio::test]
async fn a_key_required_method_admits_only_a_key_the_records_associate_with_that_stage() {
    let gateway = Gateway::new();

    // One api with both integration types behind a key-required method, deployed to TWO
    // stages, so a row can associate a key with one stage and call the other.
    let (api, root) = create_api(&gateway, "api-key-gate").await;
    keyed_mock(&gateway, &api, &root, "mocked", r#"{"served":true}"#).await;
    keyed_proxy(&gateway, &api, &root, "proxied").await;
    deploy(&gateway, &api, "prod").await;
    deploy(&gateway, &api, "beta").await;

    // And a SECOND api, so a row can associate a key with another api's stage of the same
    // name.
    let (other_api, other_root) = create_api(&gateway, "api-key-gate-other").await;
    keyed_mock(&gateway, &other_api, &other_root, "mocked", r#"{"other":true}"#).await;
    deploy(&gateway, &other_api, "prod").await;

    // Each key differs from the admitting one in exactly one property: the association's
    // mechanism, the api or the stage it names, or the key's own `enabled` flag.
    let metered = key_value(1);
    let deprecated = key_value(2);
    let other_stage = key_value(3);
    let other_apis = key_value(4);
    let plan_without_stages = key_value(5);
    let disabled = key_value(6);
    let unassociated = key_value(7);

    let metered_id = api_key(&gateway, "metered", &metered, true, json!([])).await;
    meter(
        &gateway,
        "metered",
        json!([{ "apiId": api, "stage": "prod" }]),
        &metered_id,
    )
    .await;

    api_key(
        &gateway,
        "deprecated",
        &deprecated,
        true,
        json!([{ "restApiId": api, "stageName": "prod" }]),
    )
    .await;

    let other_stage_id = api_key(&gateway, "other-stage", &other_stage, true, json!([])).await;
    meter(
        &gateway,
        "other-stage",
        json!([{ "apiId": api, "stage": "beta" }]),
        &other_stage_id,
    )
    .await;

    let other_apis_id = api_key(&gateway, "other-api", &other_apis, true, json!([])).await;
    meter(
        &gateway,
        "other-api",
        json!([{ "apiId": other_api, "stage": "prod" }]),
        &other_apis_id,
    )
    .await;

    let stageless_id = api_key(&gateway, "no-stages", &plan_without_stages, true, json!([])).await;
    meter(&gateway, "no-stages", json!([]), &stageless_id).await;

    let disabled_id = api_key(&gateway, "disabled", &disabled, false, json!([])).await;
    meter(
        &gateway,
        "disabled",
        json!([{ "apiId": api, "stage": "prod" }]),
        &disabled_id,
    )
    .await;

    api_key(&gateway, "unassociated", &unassociated, true, json!([])).await;

    /// One row: the path invoked, the `x-api-key` value presented (none at all for
    /// `None`), and the answer the records earn it.
    struct Case<'c> {
        name: &'c str,
        path: &'c str,
        presented: Option<&'c str>,
        status: u16,
        /// The body for a served call, the gateway response's `message` for a refusal.
        expected: &'c str,
    }

    let cases: &[Case] = &[
        Case {
            name: "a key a usage plan meters for this api stage",
            path: "/prod/mocked",
            presented: Some(&metered),
            status: 200,
            expected: r#"{"served":true}"#,
        },
        Case {
            name: "a key whose own stageKeys name this api stage",
            path: "/prod/mocked",
            presented: Some(&deprecated),
            status: 200,
            expected: r#"{"served":true}"#,
        },
        Case {
            name: "a key metered for another stage of this api, calling that stage",
            path: "/beta/mocked",
            presented: Some(&other_stage),
            status: 200,
            expected: r#"{"served":true}"#,
        },
        Case {
            name: "a key metered for another stage of this api",
            path: "/prod/mocked",
            presented: Some(&other_stage),
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "a key metered for this stage, calling the other stage",
            path: "/beta/mocked",
            presented: Some(&metered),
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "a key metered for another api's stage of the same name",
            path: "/prod/mocked",
            presented: Some(&other_apis),
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "a key metered by a plan that names no api stage at all",
            path: "/prod/mocked",
            presented: Some(&plan_without_stages),
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "a disabled key metered for this api stage",
            path: "/prod/mocked",
            presented: Some(&disabled),
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "an enabled key with no association at all",
            path: "/prod/mocked",
            presented: Some(&unassociated),
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "a value no stored key carries",
            path: "/prod/mocked",
            presented: Some("not-a-stored-key-value-at-all-0000000000"),
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "an empty key header",
            path: "/prod/mocked",
            presented: Some(""),
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "no key header at all",
            path: "/prod/mocked",
            presented: None,
            status: 403,
            expected: "Forbidden",
        },
        // The same rule in front of an integration whose backend cannot be called: a
        // refused call never reaches it (403, not the integration's 500), and an admitted
        // one does.
        Case {
            name: "no key header at all, in front of a proxy integration",
            path: "/prod/proxied",
            presented: None,
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "a value no stored key carries, in front of a proxy integration",
            path: "/prod/proxied",
            presented: Some("not-a-stored-key-value-at-all-0000000000"),
            status: 403,
            expected: "Forbidden",
        },
        Case {
            name: "a key a usage plan meters, in front of a proxy integration",
            path: "/prod/proxied",
            presented: Some(&metered),
            status: 500,
            expected: "Internal server error",
        },
    ];

    for case in cases {
        let headers: Vec<(&str, &str)> = case
            .presented
            .map(|value| vec![("x-api-key", value)])
            .unwrap_or_default();
        let answered = invoke(&gateway, &api, case.path, &headers).await;
        assert_eq!(
            answered.status, case.status,
            "case {:?} (GET {}): expected {}, got {} with {:?}",
            case.name, case.path, case.status, answered.status, answered.body
        );
        let observed = if case.status == 200 {
            answered.body.clone()
        } else {
            answered.message()
        };
        assert_eq!(
            observed, case.expected,
            "case {:?} (GET {}): expected {:?}, got {:?}",
            case.name, case.path, case.expected, answered.body
        );
    }
}
