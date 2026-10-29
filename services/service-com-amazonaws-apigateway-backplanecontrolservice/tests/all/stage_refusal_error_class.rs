// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EXCEPTION CLASS a deployed stage names when it refuses a
//! request, and for the two admission steps that must run before the method's integration
//! is dispatched.
//!
//! The RULE under test, over a SET of refusals rather than one recorded call: a request the
//! `execute-api` endpoint refuses is answered with the gateway response for its type AND
//! names that refusal's exception class in `x-amzn-errortype`, which is the header a client
//! classifies an answer by — the `Forbidden` envelope is a `ForbiddenException`, the
//! missing-token envelope a `MissingAuthenticationTokenException`, the authorizer's 401 an
//! `UnauthorizedException`, the IAM-shaped denial an `AccessDeniedException`, and a
//! validator's 400 a `BadRequestException`. The class is the endpoint's own: the data plane
//! is not in this service's control-plane model at all, so a class that model happens not to
//! declare is still the one the endpoint reports, and a refusal must not be left with the
//! prose of its body as a caller's only handle on it.
//!
//! The rows span what the rule generalises over: WHICH refusal the request earns (the API
//! key gate, the route that matches nothing, an `AWS_IAM` method reached unsigned, a stage
//! with no deployment, an authorizer whose identity source is absent, each half of a request
//! validator) and, in the second test, whether the developer has CUSTOMIZED that gateway
//! response's status code and template — a customization changes what the caller reads, not
//! which class the refusal is.
//!
//! Beside the class, the controls pin the ORDER both admission steps run in: a key-required
//! method and a body-validating method are each put in front of an `AWS_PROXY` integration,
//! whose backend this emulator cannot call, so a request that REACHES the integration is
//! answered visibly differently from one refused before it. Every row is a terminated
//! response: a refused invoke is answered, never left open.
//!
//! Every case drives the real wire path — an unsigned request to the api's own invoke host
//! through the multi-service router, with the control plane driven through the same emulator
//! ([`Gateway`]) — so what is asserted is what a client of the published invoke URL sees.

use serde_json::{json, Value};

use crate::api_key_lifecycle::create_key;
use crate::execute_api_data_plane::{
    deploy, integration_response, json_template, method_response, put_integration, put_method,
};
use crate::gateway_response::put_gateway_response;
use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Gateway};
use crate::stage_request_validation::{
    create_model, create_validator, invoke, order_schema, Invoked, GOOD_BODY, JSON,
};
use crate::test_invoke_method::lambda_uri;
use crate::usage_plan_key_lifecycle::create_usage_plan_key;
use crate::usage_plan_lifecycle::create_usage_plan;

/// The stage every api below is deployed to.
const STAGE: &str = "test";

/// The model the validated route declares for [`JSON`] payloads.
const MODEL: &str = "Order";

/// The one key type a `UsagePlanKey` takes: "Currently, the valid key type is `API_KEY`"
/// (`smithy_query(query="doc", shape="UsagePlanKey", member="type")`).
const API_KEY_TYPE: &str = "API_KEY";

/// The body the `MOCK` route answers with: what proves a request reached an integration, so
/// no refusal may carry it.
const SERVED_BODY: &str = r#"{"served":true}"#;

/// The sentence the uncustomized `API_CONFIGURATION_ERROR` gateway response carries, which
/// is what a request that reached an integration whose backend cannot be called earns.
const CONFIGURATION_ERROR: &str = "Internal server error";

/// The exception class of every refusal whose body is the `Forbidden` envelope.
const FORBIDDEN_EXCEPTION: &str = "ForbiddenException";

/// The class of the refusal whose body is `Missing Authentication Token`.
const MISSING_TOKEN_EXCEPTION: &str = "MissingAuthenticationTokenException";

/// The class of a request validator's refusal, either half.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// A distinct API key value of the 40 characters AWS mints; a value is unique per account,
/// so no two keys below may carry the same one.
fn key_value(n: u8) -> String {
    format!("{}{n:02}", "k".repeat(38))
}

/// The `uri` of the Lambda proxy integration both admission steps are tested in front of.
fn proxy_integration() -> Value {
    json!({ "type": "AWS_PROXY", "httpMethod": "POST", "uri": lambda_uri() })
}

/// A `MOCK` route under `root` that answers [`SERVED_BODY`] for `verb`, carrying
/// `method_request`; answers nothing, because every row addresses it by path.
async fn mock_route(
    gateway: &Gateway,
    api: &str,
    root: &str,
    route: &str,
    verb: &str,
    method_request: Value,
) {
    let resource = resource_under(gateway, api, root, route).await;
    put_method(gateway, api, &resource, verb, method_request).await;
    put_integration(gateway, api, &resource, verb, json!({ "type": "MOCK" })).await;
    method_response(gateway, api, &resource, verb, "200", json!({})).await;
    integration_response(
        gateway,
        api,
        &resource,
        verb,
        "200",
        json!({ "responseTemplates": json_template(SERVED_BODY) }),
    )
    .await;
}

/// A route under `root` whose method carries `method_request` and whose integration is the
/// Lambda proxy of [`proxy_integration`]: the backend this emulator cannot call, so a
/// request that reaches it is answered differently from one refused before it.
async fn proxy_route(
    gateway: &Gateway,
    api: &str,
    root: &str,
    route: &str,
    verb: &str,
    method_request: Value,
) {
    let resource = resource_under(gateway, api, root, route).await;
    put_method(gateway, api, &resource, verb, method_request).await;
    put_integration(gateway, api, &resource, verb, proxy_integration()).await;
}

/// `CreateApiKey` for `value` with a usage plan over `(api, STAGE)` metering it — the way a
/// client associates a key with an api stage today.
async fn metered_key(gateway: &Gateway, case: &str, api: &str, value: &str) {
    let created = create_key(
        gateway,
        case,
        json!({ "name": case, "value": value, "enabled": true }),
    )
    .await;
    let key_id = string_member(&created, "id", case);
    let plan = create_usage_plan(
        gateway,
        case,
        json!({
            "name": format!("plan-{case}"),
            "apiStages": [{ "apiId": api, "stage": STAGE }],
        }),
    )
    .await;
    let plan = string_member(&plan, "id", case);
    let answer = create_usage_plan_key(gateway, &plan, &key_id, API_KEY_TYPE).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected CreateUsagePlanKey to answer 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// A `TOKEN` authorizer of `api` declaring `identitySource`, answering its id.
async fn token_authorizer(gateway: &Gateway, api: &str, identity_source: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(json!({
                "name": "token-authorizer",
                "type": "TOKEN",
                "identitySource": identity_source,
                "authorizerUri": lambda_uri(),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateAuthorizer: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", "CreateAuthorizer")
}

/// One refused invoke: what the request carries, and the answer the records earn it.
struct Refusal<'c> {
    case: &'c str,
    /// Which api the request addresses: every row but the undeployed one uses the first.
    api: &'c str,
    verb: &'c str,
    path: String,
    headers: Vec<(&'c str, &'c str)>,
    body: &'c str,
    status: u16,
    message: &'c str,
    /// The exception class the refusal must name in `x-amzn-errortype`.
    class: &'c str,
}

/// Assert one refused invoke: its status, its `message`, its class, and that nothing of the
/// integration's own answer reached the caller.
async fn assert_refused(gateway: &Gateway, refusal: &Refusal<'_>) {
    let answered = invoke(
        gateway,
        refusal.api,
        refusal.verb,
        &refusal.path,
        &refusal.headers,
        refusal.body,
    )
    .await;
    assert_eq!(
        answered.status, refusal.status,
        "case {:?}: expected {}, got {} with {:?}",
        refusal.case, refusal.status, answered.status, answered.body
    );
    assert_eq!(
        answered.message(),
        refusal.message,
        "case {:?}: expected the message {:?}, got {:?}",
        refusal.case,
        refusal.message,
        answered.body
    );
    assert_eq!(
        answered.error_type.as_deref(),
        Some(refusal.class),
        "case {:?}: expected the refusal to name the class {:?}, got {:?}",
        refusal.case,
        refusal.class,
        answered.error_type
    );
    assert!(
        !answered.body.contains("served"),
        "case {:?}: expected nothing of the integration's answer, got {:?}",
        refusal.case,
        answered.body
    );
}

#[tokio::test]
async fn every_refusal_a_deployed_stage_answers_names_its_exception_class() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "refusal-classes").await;
    create_model(&gateway, &api, MODEL, order_schema()).await;
    let validator = create_validator(&gateway, &api, "bodyonly", true, false).await;
    let parameters = create_validator(&gateway, &api, "paramsonly", false, true).await;
    let authorizer = token_authorizer(&gateway, &api, "method.request.header.Auth").await;

    // A key-required GET and a body-validated POST, each in front of the Lambda proxy whose
    // backend cannot be called: the recorded shape of both causes under test.
    proxy_route(
        &gateway,
        &api,
        &root,
        "items",
        "GET",
        json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
    )
    .await;
    proxy_route(
        &gateway,
        &api,
        &root,
        "orders",
        "POST",
        json!({
            "authorizationType": "NONE",
            "requestModels": { JSON: MODEL },
            "requestValidatorId": validator,
        }),
    )
    .await;
    // The remaining refusal classes, each on its own route, plus one route that is served in
    // full so a refusal is distinguishable from an answer.
    mock_route(
        &gateway,
        &api,
        &root,
        "signed",
        "GET",
        json!({ "authorizationType": "AWS_IAM" }),
    )
    .await;
    mock_route(
        &gateway,
        &api,
        &root,
        "guarded",
        "GET",
        json!({ "authorizationType": "CUSTOM", "authorizerId": authorizer }),
    )
    .await;
    mock_route(
        &gateway,
        &api,
        &root,
        "tenants",
        "POST",
        json!({
            "authorizationType": "NONE",
            "requestParameters": { "method.request.querystring.tenant": true },
            "requestValidatorId": parameters,
        }),
    )
    .await;
    mock_route(
        &gateway,
        &api,
        &root,
        "open",
        "GET",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    deploy(&gateway, &api, STAGE).await;

    // A second api that was never deployed: its stage serves nothing at all.
    let (undeployed, undeployed_root) = create_api(&gateway, "refusal-classes-undeployed").await;
    mock_route(
        &gateway,
        &undeployed,
        &undeployed_root,
        "open",
        "GET",
        json!({ "authorizationType": "NONE" }),
    )
    .await;

    // The key the metered control row presents, and a value no stored key carries.
    let metered = key_value(1);
    metered_key(&gateway, "metered", &api, &metered).await;
    let unknown = key_value(2);

    let refusals = [
        Refusal {
            case: "a key-required method reached with no key at all",
            api: &api,
            verb: "GET",
            path: format!("/{STAGE}/items?id=keyless"),
            headers: Vec::new(),
            body: "",
            status: 403,
            message: "Forbidden",
            class: FORBIDDEN_EXCEPTION,
        },
        Refusal {
            case: "a key-required method reached with a value no stored key carries",
            api: &api,
            verb: "GET",
            path: format!("/{STAGE}/items?id=unknown-key"),
            headers: vec![("x-api-key", unknown.as_str())],
            body: "",
            status: 403,
            message: "Forbidden",
            class: FORBIDDEN_EXCEPTION,
        },
        Refusal {
            case: "a stage with no deployment at all",
            api: &undeployed,
            verb: "GET",
            path: format!("/{STAGE}/open"),
            headers: Vec::new(),
            body: "",
            status: 403,
            message: "Forbidden",
            class: FORBIDDEN_EXCEPTION,
        },
        Refusal {
            case: "a path the deployment holds no resource for",
            api: &api,
            verb: "GET",
            path: format!("/{STAGE}/absent"),
            headers: Vec::new(),
            body: "",
            status: 403,
            message: "Missing Authentication Token",
            class: MISSING_TOKEN_EXCEPTION,
        },
        Refusal {
            case: "an AWS_IAM method reached with no credential",
            api: &api,
            verb: "GET",
            path: format!("/{STAGE}/signed"),
            headers: Vec::new(),
            body: "",
            status: 403,
            message: "Missing Authentication Token",
            class: MISSING_TOKEN_EXCEPTION,
        },
        Refusal {
            case: "an authorizer whose identity source the request omits",
            api: &api,
            verb: "GET",
            path: format!("/{STAGE}/guarded"),
            headers: Vec::new(),
            body: "",
            status: 401,
            message: "Unauthorized",
            class: "UnauthorizedException",
        },
        Refusal {
            case: "a payload the method's model does not admit",
            api: &api,
            verb: "POST",
            path: format!("/{STAGE}/orders"),
            headers: vec![("content-type", JSON)],
            body: r#"{"qty":2}"#,
            status: 400,
            message: "Invalid request body",
            class: BAD_REQUEST_EXCEPTION,
        },
        Refusal {
            case: "a request that omits a required request parameter",
            api: &api,
            verb: "POST",
            path: format!("/{STAGE}/tenants"),
            headers: vec![("content-type", JSON)],
            body: GOOD_BODY,
            status: 400,
            message: "Missing required request parameters: [tenant]",
            class: BAD_REQUEST_EXCEPTION,
        },
    ];

    for refusal in &refusals {
        assert_refused(&gateway, refusal).await;
    }

    // The controls. A request the stage SERVES carries no refusal class at all, so the
    // header above is the refusal's own and not something every response carries.
    let served = invoke(
        &gateway,
        &api,
        "GET",
        &format!("/{STAGE}/open"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case \"a served request\": expected 200, got {} with {:?}",
        served.status, served.body
    );
    assert_eq!(
        served.body, SERVED_BODY,
        "case \"a served request\": expected the integration's own body, got {:?}",
        served.body
    );
    assert_eq!(
        served.error_type, None,
        "case \"a served request\": expected no refusal class, got {:?}",
        served.error_type
    );

    // And the ORDER both admission steps run in: the request each step ADMITS reaches the
    // integration — whose backend cannot be called, which is itself observable — so the
    // refusals above happened before the integration rather than from it.
    for (case, verb, path, headers, body) in [
        (
            "a key the records meter for this api stage",
            "GET",
            format!("/{STAGE}/items?id=withkey"),
            vec![("x-api-key", metered.as_str())],
            "",
        ),
        (
            "a payload the method's model admits",
            "POST",
            format!("/{STAGE}/orders"),
            vec![("content-type", JSON)],
            GOOD_BODY,
        ),
    ] {
        let admitted: Invoked = invoke(&gateway, &api, verb, &path, &headers, body).await;
        assert_eq!(
            admitted.status, 500,
            "case {case:?}: expected the integration's own answer 500, got {} with {:?}",
            admitted.status, admitted.body
        );
        assert_eq!(
            admitted.message(),
            CONFIGURATION_ERROR,
            "case {case:?}: expected the integration's own answer {CONFIGURATION_ERROR:?}, got \
             {:?}",
            admitted.body
        );
        assert_ne!(
            admitted.error_type.as_deref(),
            Some(FORBIDDEN_EXCEPTION),
            "case {case:?}: expected the admitted request to have passed the admission step, \
             got {:?}",
            admitted.error_type
        );
        assert_ne!(
            admitted.error_type.as_deref(),
            Some(BAD_REQUEST_EXCEPTION),
            "case {case:?}: expected the admitted request to have passed the admission step, \
             got {:?}",
            admitted.error_type
        );
    }
}

#[tokio::test]
async fn a_resource_policy_denial_names_the_iam_access_denied_class() {
    // The one refusal whose body is NOT the gateway's `{"message": …}` envelope: a request
    // the api's own `policy` does not let through is answered with the IAM-shaped
    // authorization sentence, so its class is the IAM one a caller reads that sentence under
    // rather than the `Forbidden` envelope's.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "refusal-classes-policy").await;
    mock_route(
        &gateway,
        &api,
        &root,
        "open",
        "GET",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": "*",
            "Action": "execute-api:Invoke",
            "Resource": "execute-api:/*",
        }],
    });
    let patched = gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}"),
            Some(json!({
                "patchOperations": [
                    { "op": "replace", "path": "/policy", "value": document.to_string() }
                ]
            })),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "case UpdateRestApi(/policy): expected 200, got {} with {:?}",
        patched.status, patched.body
    );
    deploy(&gateway, &api, STAGE).await;

    let answered = invoke(&gateway, &api, "GET", &format!("/{STAGE}/open"), &[], "").await;
    assert_eq!(
        answered.status, 403,
        "case \"a request the api's policy denies\": expected 403, got {} with {:?}",
        answered.status, answered.body
    );
    assert!(
        answered.body.contains("is not authorized to perform: execute-api:Invoke"),
        "case \"a request the api's policy denies\": expected the authorization sentence, got \
         {:?}",
        answered.body
    );
    assert_eq!(
        answered.error_type.as_deref(),
        Some("AccessDeniedException"),
        "case \"a request the api's policy denies\": expected the IAM class, got {:?}",
        answered.error_type
    );
}

#[tokio::test]
async fn a_customized_gateway_response_still_names_the_refusals_class() {
    // A developer's `PutGatewayResponse` decides the status code and the body a refusal is
    // answered with; it does not decide which exception the refusal IS, so the class is
    // unchanged by it.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "refusal-classes-customized").await;
    create_model(&gateway, &api, MODEL, order_schema()).await;
    let validator = create_validator(&gateway, &api, "bodyonly", true, false).await;
    mock_route(
        &gateway,
        &api,
        &root,
        "items",
        "GET",
        json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
    )
    .await;
    mock_route(
        &gateway,
        &api,
        &root,
        "orders",
        "POST",
        json!({
            "authorizationType": "NONE",
            "requestModels": { JSON: MODEL },
            "requestValidatorId": validator,
        }),
    )
    .await;

    // Each customization moves the status code to one the response type does not document
    // and replaces the body, so neither the status nor the prose can be what the class is
    // read from. `GatewayResponse$statusCode` admits any three-digit code.
    for (response_type, status, message) in [
        ("INVALID_API_KEY", "451", "no key for you"),
        ("BAD_REQUEST_BODY", "422", "that body will not do"),
    ] {
        let answer = put_gateway_response(
            &gateway,
            &api,
            response_type,
            json!({
                "statusCode": status,
                "responseTemplates": json_template(
                    &json!({ "message": message, "customized": true }).to_string(),
                ),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case PutGatewayResponse({response_type}): expected 201, got {} with {:?}",
            answer.status, answer.body
        );
    }
    deploy(&gateway, &api, STAGE).await;

    for (case, verb, path, headers, body, status, message, class) in [
        (
            "a customized INVALID_API_KEY response",
            "GET",
            format!("/{STAGE}/items"),
            vec![],
            "",
            451,
            "no key for you",
            FORBIDDEN_EXCEPTION,
        ),
        (
            "a customized BAD_REQUEST_BODY response",
            "POST",
            format!("/{STAGE}/orders"),
            vec![("content-type", JSON)],
            r#"{"qty":2}"#,
            422,
            "that body will not do",
            BAD_REQUEST_EXCEPTION,
        ),
    ] {
        let answered = invoke(&gateway, &api, verb, &path, &headers, body).await;
        assert_eq!(
            answered.status, status,
            "case {case:?}: expected the customized {status}, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.message(),
            message,
            "case {case:?}: expected the customized body, got {:?}",
            answered.body
        );
        assert!(
            answered.body.contains("customized"),
            "case {case:?}: expected the customized template, got {:?}",
            answered.body
        );
        assert_eq!(
            answered.error_type.as_deref(),
            Some(class),
            "case {case:?}: expected the refusal to name the class {class:?}, got {:?}",
            answered.error_type
        );
    }
}
