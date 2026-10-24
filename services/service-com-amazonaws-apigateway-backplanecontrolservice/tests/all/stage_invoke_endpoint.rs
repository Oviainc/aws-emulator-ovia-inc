// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH ADDRESSING of a deployed REST API stage the data plane
//! answers, and for the two deletions whose refusal a stage or an integration decides.
//!
//! A client reaches AWS at `{rest-api-id}.execute-api.{region}.amazonaws.com`, which
//! resolves nowhere on the machine this emulator runs on: it reaches the emulator at the
//! emulator's OWN endpoint — a loopback address or `localhost`, which names no service —
//! and carries the stage and the resource path it would have carried to AWS. So the rule
//! asserted here is that a request to a deployed stage is resolved to its api by the STAGE
//! SEGMENT of its path whenever the host names no api, and that the api's own
//! `execute-api` host keeps resolving by host. The sibling `execute_api_data_plane` suite
//! asserts WHAT a resolved stage serves; this one asserts only that the client reaching it
//! at a reachable endpoint gets the same answer.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service — so what is asserted is what a client sees, not what a
//! handler returns.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the addressing rule
//! over a table of hosts (each loopback spelling, `localhost`, no host at all, the api's
//! own `execute-api` host, another api's, the control plane's) and of paths (the stage
//! root, a nested resource, a path the deployment does not hold, this service's own
//! control-plane path space), the one-claimant rule over a stage name two apis hold, and
//! each deletion's refusal over the states its record can be in.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};

/// The region the api's own invoke host is addressed in. Any region is served — the host's
/// region label is not matched against a list — so one is enough here, and
/// `src/service/invoke.rs`'s own unit test varies the label.
const REGION: &str = "us-east-1";

/// One answered invoke: the status line and the body verbatim.
struct Invoked {
    status: u16,
    body: String,
}

/// Issue one request at `path` with `host` as its `Host` header, or with NO host header at
/// all when `host` is `None` — the shape an HTTP/2 client's request arrives in.
async fn invoke_on_host(gateway: &Gateway, host: Option<&str>, path: &str) -> Invoked {
    let mut builder = http::Request::builder().method("GET");
    builder = match host {
        Some(host) => builder.uri(format!("https://{host}{path}")).header("host", host),
        None => builder.uri(path),
    };
    let request = builder
        .body(Bytes::new())
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked { status, body }
}

/// One request through the data plane carrying `headers` and a body, for the cases that
/// vary what the request DECLARES rather than where it is addressed.
async fn invoke_with_headers(
    gateway: &Gateway,
    path: &str,
    headers: &[(&str, &str)],
) -> Invoked {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://127.0.0.1:9297{path}"))
        .header("host", "127.0.0.1:9297");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from_static(b"{\"sent\":true}"))
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked { status, body }
}

/// A `MOCK` `POST` on the root whose request templates key two content types, with an
/// integration response for each status they name, and `passthrough` as its
/// `passthroughBehavior` (omitted when `None`, which leaves the documented default).
///
/// The `404` integration response carries a `selectionPattern`, so a call that reaches it
/// has been selected BY THE STATUS its request template named rather than by falling back
/// to the one integration response with no pattern.
async fn mock_templated_api(gateway: &Gateway, name: &str, passthrough: Option<&str>) -> String {
    let (api, root) = create_api(gateway, name).await;
    let mut integration = json!({
        "type": "MOCK",
        "requestTemplates": {
            "application/json": "{\"statusCode\": 200}",
            "application/xml": "{\"statusCode\": \"404\"}",
        },
    });
    if let Some(passthrough) = passthrough {
        integration["passthroughBehavior"] = json!(passthrough);
    }
    let answers: &[(&str, String, Value)] = &[
        (
            "PUT",
            format!("/restapis/{api}/resources/{root}/methods/POST"),
            json!({ "authorizationType": "NONE" }),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{root}/methods/POST/integration"),
            integration,
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{root}/methods/POST/responses/200"),
            json!({}),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{root}/methods/POST/responses/404"),
            json!({}),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{root}/methods/POST/integration/responses/200"),
            json!({ "responseTemplates": { "application/json": r#"{"routed":"json"}"# } }),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{root}/methods/POST/integration/responses/404"),
            json!({
                "selectionPattern": "4\\d{2}",
                "responseTemplates": { "application/json": r#"{"routed":"xml"}"# },
            }),
        ),
    ];
    for (verb, uri, request) in answers {
        let answer = gateway.call(verb, uri, Some(request.clone())).await;
        assert!(
            (200..300).contains(&answer.status),
            "case {uri}: expected the control-plane call to be accepted, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
    deploy(gateway, &api, "prod").await;
    api
}

#[tokio::test]
async fn a_mocks_status_comes_from_the_request_template_its_content_type_selects() {
    let gateway = Gateway::new();
    // One api per passthrough behaviour, since the behaviour is the integration's and not
    // the request's. Every row addresses its api PATH-STYLE (`/{api}/prod`), which names
    // the api explicitly, so the three sharing one stage name is immaterial here.
    let default = mock_templated_api(&gateway, "passthrough-default", None).await;
    let when_no_templates =
        mock_templated_api(&gateway, "passthrough-when-no-templates", Some("WHEN_NO_TEMPLATES"))
            .await;
    let never = mock_templated_api(&gateway, "passthrough-never", Some("NEVER")).await;

    // Rows differ along the content type the request declares — one each key, one keyed by
    // neither, one with a parameter on it, one absent — and along the integration's
    // `passthroughBehavior`, which is what decides a call no template maps. The body names
    // WHICH template's status was selected, so a row cannot pass on the status alone.
    let cases: &[(&str, &str, Option<&str>, u16, &str)] = &[
        (
            "the content type the first template keys",
            default.as_str(),
            Some("application/json"),
            200,
            r#"{"routed":"json"}"#,
        ),
        (
            "the content type the second template keys",
            default.as_str(),
            Some("application/xml"),
            404,
            r#"{"routed":"xml"}"#,
        ),
        (
            "the keyed content type carrying a charset parameter",
            default.as_str(),
            Some("application/xml; charset=utf-8"),
            404,
            r#"{"routed":"xml"}"#,
        ),
        (
            "the keyed content type spelled in upper case",
            default.as_str(),
            Some("APPLICATION/XML"),
            404,
            r#"{"routed":"xml"}"#,
        ),
        (
            "no content type at all, which selects the JSON template",
            default.as_str(),
            None,
            200,
            r#"{"routed":"json"}"#,
        ),
        (
            "an unmapped content type under the default behaviour, passed through",
            default.as_str(),
            Some("text/plain"),
            200,
            r#"{"routed":"json"}"#,
        ),
        (
            "an unmapped content type where only a template-less call may pass through",
            when_no_templates.as_str(),
            Some("text/plain"),
            415,
            "Unsupported Media Type",
        ),
        (
            "an unmapped content type where nothing may pass through",
            never.as_str(),
            Some("text/plain"),
            415,
            "Unsupported Media Type",
        ),
        (
            "a mapped content type where nothing may pass through, which still maps",
            never.as_str(),
            Some("application/xml"),
            404,
            r#"{"routed":"xml"}"#,
        ),
    ];

    for (case, api, content_type, status, expected_body) in cases {
        let headers: Vec<(&str, &str)> = content_type
            .map(|value| vec![("content-type", value)])
            .unwrap_or_default();
        let answered =
            invoke_with_headers(&gateway, &format!("/{api}/prod"), &headers).await;
        assert_eq!(
            answered.status, *status,
            "case {case:?} ({content_type:?}): expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        assert!(
            answered.body.contains(expected_body),
            "case {case:?} ({content_type:?}): expected a body carrying {expected_body:?}, \
             got {:?}",
            answered.body
        );
    }
}

/// Put a `MOCK` method on `resource` that answers `status` with `body`, through the control
/// plane, exactly as a developer builds one.
async fn mock_method(gateway: &Gateway, api: &str, resource: &str, status: &str, body: &str) {
    let answers: &[(&str, String, Value)] = &[
        (
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/GET"),
            json!({ "authorizationType": "NONE" }),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/GET/responses/{status}"),
            json!({}),
        ),
        (
            "PUT",
            format!(
                "/restapis/{api}/resources/{resource}/methods/GET/integration/responses/{status}"
            ),
            json!({ "responseTemplates": { "application/json": body } }),
        ),
    ];
    for (verb, uri, request) in answers {
        let answer = gateway.call(verb, uri, Some(request.clone())).await;
        assert!(
            (200..300).contains(&answer.status),
            "case {uri}: expected the control-plane call to be accepted, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

/// `CreateDeployment` of `stage`, answering the deployment id.
async fn deploy(gateway: &Gateway, api: &str, stage: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": stage })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateDeployment({stage:?}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", &format!("CreateDeployment({stage:?})"))
}

/// One api with a `MOCK` `GET` on the root and on `/pets`, deployed to `stage`.
async fn deployed_api(gateway: &Gateway, name: &str, stage: &str) -> String {
    let (api, root) = create_api(gateway, name).await;
    mock_method(gateway, &api, &root, "200", r#"{"at":"root"}"#).await;
    let pets = create_resource(gateway, &api, &root, "pets").await;
    let pets = string_member(&pets, "id", "CreateResource(\"pets\")");
    mock_method(gateway, &api, &pets, "200", r#"{"at":"pets"}"#).await;
    deploy(gateway, &api, stage).await;
    api
}

#[tokio::test]
async fn a_deployed_stage_is_served_at_every_endpoint_a_client_can_reach() {
    let gateway = Gateway::new();
    let api = deployed_api(&gateway, "reachable-endpoint", "prod").await;

    // Rows differ along the HOST — every spelling of the emulator's own endpoint a client
    // points at, the api's own `execute-api` host, and a host that names some other
    // service — and along the PATH, which is where an api the host does not name, and the
    // stage, are read from. The body proves WHICH resource of WHICH api answered, so a row
    // cannot pass by being refused in the same status class.
    let own_host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let cases: &[(&str, Option<String>, String, u16, &str)] = &[
        (
            "the emulator's own endpoint on a loopback address and port",
            Some("127.0.0.1:9297".to_string()),
            "/prod".to_string(),
            200,
            r#"{"at":"root"}"#,
        ),
        (
            "the emulator's own endpoint as localhost",
            Some("localhost".to_string()),
            "/prod/pets".to_string(),
            200,
            r#"{"at":"pets"}"#,
        ),
        (
            "path-style on a host that names THIS service, the control plane's own",
            Some(format!("apigateway.{REGION}.amazonaws.com")),
            format!("/{api}/prod/pets"),
            200,
            r#"{"at":"pets"}"#,
        ),
        (
            "path-style on a host that names ANOTHER service, which keeps it",
            Some("s3.us-east-1.amazonaws.com".to_string()),
            format!("/{api}/prod/pets"),
            404,
            "UnknownService",
        ),
        (
            "path-style on a loopback endpoint: the api id ahead of the stage",
            Some("127.0.0.1:9297".to_string()),
            format!("/{api}/prod/pets"),
            200,
            r#"{"at":"pets"}"#,
        ),
        (
            "path-style on a loopback endpoint with no port",
            Some("127.0.0.1".to_string()),
            format!("/{api}/prod"),
            200,
            r#"{"at":"root"}"#,
        ),
        (
            "path-style on the localhost endpoint carrying a port",
            Some("localhost:9297".to_string()),
            format!("/{api}/prod"),
            200,
            r#"{"at":"root"}"#,
        ),
        (
            "no host header at all, as an HTTP/2 request with no authority arrives",
            None,
            "/prod".to_string(),
            200,
            r#"{"at":"root"}"#,
        ),
        (
            "the api's own execute-api host, which names the api itself",
            Some(own_host.clone()),
            "/prod/pets".to_string(),
            200,
            r#"{"at":"pets"}"#,
        ),
        (
            "a trailing slash on the stage root",
            Some("127.0.0.1:9297".to_string()),
            "/prod/".to_string(),
            200,
            r#"{"at":"root"}"#,
        ),
        // Resolved to the api, and refused BY the api: a path the deployment holds no
        // resource for is the data plane's own `403`, never the front door's `404`.
        (
            "a path the deployment holds no resource for",
            Some("127.0.0.1:9297".to_string()),
            "/prod/pets/1/toys".to_string(),
            403,
            "Missing Authentication Token",
        ),
        (
            "a stage this api does not hold",
            Some(own_host.clone()),
            "/beta/pets".to_string(),
            403,
            "Forbidden",
        ),
        (
            "path-style naming a stage this api does not hold",
            Some("127.0.0.1:9297".to_string()),
            format!("/{api}/beta/pets"),
            403,
            "Forbidden",
        ),
    ];

    for (case, host, path, status, expected_body) in cases {
        let answered = invoke_on_host(&gateway, host.as_deref(), path).await;
        assert_eq!(
            answered.status, *status,
            "case {case:?} ({host:?}{path}): expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        assert!(
            answered.body.contains(expected_body),
            "case {case:?} ({host:?}{path}): expected a body carrying {expected_body:?}, got {:?}",
            answered.body
        );
    }
}

#[tokio::test]
async fn a_request_that_names_no_one_api_is_left_to_the_front_door() {
    let gateway = Gateway::new();
    let first = deployed_api(&gateway, "shared-stage-first", "prod").await;
    let second = deployed_api(&gateway, "shared-stage-second", "prod").await;
    // A stage name only ONE api holds, so the rows below can tell "no api holds it" from
    // "two do".
    let only = deployed_api(&gateway, "sole-stage", "staging").await;

    // Rows differ along what the path's first segment names: a stage two apis hold (so it
    // names no ONE api), a stage no api holds, this service's own control-plane path space,
    // and ApiGatewayManagementApi's `@connections` space — plus the two rows that still
    // resolve, so the table is not passing by refusing everything.
    let cases: &[(&str, String, u16)] = &[
        ("a stage name two apis hold", "/prod/pets".to_string(), 404),
        (
            "a stage name no api holds",
            "/unreleased/pets".to_string(),
            404,
        ),
        (
            "this service's own control-plane path space",
            "/restapis".to_string(),
            404,
        ),
        (
            "the @connections path space of the management api",
            "/staging/@connections/abc123".to_string(),
            404,
        ),
        (
            "a stage name exactly one api holds",
            "/staging/pets".to_string(),
            200,
        ),
        (
            "path-style, which names the api the stage name cannot",
            format!("/{first}/prod/pets"),
            200,
        ),
    ];

    for (case, path, status) in cases {
        let answered = invoke_on_host(&gateway, Some("127.0.0.1:9297"), path).await;
        assert_eq!(
            answered.status, *status,
            "case {case:?} ({path}): expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        if *status == 404 {
            assert!(
                answered.body.contains("UnknownService"),
                "case {case:?} ({path}): expected the front door's own refusal for a request \
                 no service claims, got {:?}",
                answered.body
            );
        }
    }
    // Each api is still reachable on its own host, which names it without the path having
    // to: the ambiguity above is the ADDRESSING's, not the records'.
    for api in [&first, &second, &only] {
        let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
        let stage = if api == &only { "staging" } else { "prod" };
        let answered = invoke_on_host(&gateway, Some(&host), &format!("/{stage}/pets")).await;
        assert_eq!(
            answered.status, 200,
            "case {api}'s own host: expected the stage to be served, got {} with {:?}",
            answered.status, answered.body
        );
    }
}

#[tokio::test]
async fn a_deployment_a_stage_still_serves_is_not_deleted() {
    let gateway = Gateway::new();
    let api = deployed_api(&gateway, "deployment-deletion", "prod").await;
    let served = deploy(&gateway, &api, "beta").await;
    let unreferenced = deploy(&gateway, &api, "doomed").await;
    // The stage that pointed at `unreferenced` is removed, so nothing serves it any more.
    let dropped = gateway
        .call("DELETE", &format!("/restapis/{api}/stages/doomed"), None)
        .await;
    assert_eq!(
        dropped.status, 202,
        "case DeleteStage(\"doomed\"): expected 202, got {} with {:?}",
        dropped.status, dropped.body
    );

    // Rows differ along the state of the record named: served by a stage, served by no
    // stage, and never stored at all — the three outcomes `DeleteDeployment` distinguishes.
    let cases: &[(&str, &str, u16, Option<&str>)] = &[
        (
            "a deployment a stage still serves",
            served.as_str(),
            400,
            Some("BadRequestException"),
        ),
        (
            "a deployment no stage serves",
            unreferenced.as_str(),
            202,
            None,
        ),
        (
            "a deployment that was never stored",
            "zzzzzz",
            404,
            Some("NotFoundException"),
        ),
    ];

    for (case, deployment, status, error) in cases {
        let answered = gateway
            .call(
                "DELETE",
                &format!("/restapis/{api}/deployments/{deployment}"),
                None,
            )
            .await;
        assert_eq!(
            answered.status, *status,
            "case {case:?}: expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.error.as_deref(),
            *error,
            "case {case:?}: expected the error code {error:?}, got {:?} with {:?}",
            answered.error,
            answered.body
        );
    }

    // The refused deletion left its record alone and the accepted one removed only its
    // own: read back through `GetDeployment`, the operation a client checks with.
    let read_back: &[(&str, &str, u16)] = &[
        ("the deployment a stage still serves", served.as_str(), 200),
        ("the deleted deployment", unreferenced.as_str(), 404),
    ];
    for (case, deployment, status) in read_back {
        let answered = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/deployments/{deployment}"),
                None,
            )
            .await;
        assert_eq!(
            answered.status, *status,
            "case {case:?}: expected GetDeployment to answer {status}, got {} with {:?}",
            answered.status, answered.body
        );
    }
    // And the stage whose deployment survived still serves it.
    let answered = invoke_on_host(&gateway, Some("127.0.0.1:9297"), "/beta/pets").await;
    assert_eq!(
        answered.status, 200,
        "case the surviving stage: expected it to keep serving, got {} with {:?}",
        answered.status, answered.body
    );
}

#[tokio::test]
async fn only_an_integration_response_that_exists_is_deleted() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-response-deletion").await;
    mock_method(&gateway, &api, &root, "200", r#"{"ok":true}"#).await;
    // A second integration response on the same integration, so a deletion can be shown to
    // remove ONLY the one the URI names.
    let sibling = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{root}/methods/GET/integration/responses/404"),
            Some(json!({ "selectionPattern": "4\\d{2}" })),
        )
        .await;
    assert!(
        (200..300).contains(&sibling.status),
        "case PutIntegrationResponse(404): expected it to be accepted, got {} with {:?}",
        sibling.status,
        sibling.body
    );

    // Rows differ along what the status code in the URI names: a stored response, the same
    // one once it is gone, and a code no response was ever created for. Every row is a
    // well-formed `StatusCode` (`^[1-5]\d\d$`), so none of them is refused by the modelled
    // constraint instead of by the record's absence.
    let cases: &[(&str, &str, u16, Option<&str>)] = &[
        ("a stored integration response", "200", 204, None),
        (
            "the same response once it is gone",
            "200",
            404,
            Some("NotFoundException"),
        ),
        (
            "a status code no response was created for",
            "500",
            404,
            Some("NotFoundException"),
        ),
    ];

    for (case, status_code, status, error) in cases {
        let answered = gateway
            .call(
                "DELETE",
                &format!(
                    "/restapis/{api}/resources/{root}/methods/GET/integration/responses/{status_code}"
                ),
                None,
            )
            .await;
        assert_eq!(
            answered.status, *status,
            "case {case:?}: expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.error.as_deref(),
            *error,
            "case {case:?}: expected the error code {error:?}, got {:?} with {:?}",
            answered.error,
            answered.body
        );
    }

    // Read back through `GetIntegration`: the deleted response is gone from the
    // integration's own `integrationResponses` and the sibling is untouched.
    let integration = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{root}/methods/GET/integration"),
            None,
        )
        .await;
    assert_eq!(
        integration.status, 200,
        "case GetIntegration: expected 200, got {} with {:?}",
        integration.status, integration.body
    );
    let responses = &integration.body["integrationResponses"];
    assert!(
        responses.get("200").is_none(),
        "case the deleted response: expected it gone from integrationResponses, got {responses:?}"
    );
    assert!(
        responses.get("404").is_some(),
        "case the sibling response: expected it kept in integrationResponses, got {responses:?}"
    );
}
