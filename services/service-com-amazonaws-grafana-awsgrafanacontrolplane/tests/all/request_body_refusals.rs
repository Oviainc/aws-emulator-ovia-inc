// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request its own boundary validator rejected, end to
//! end over HTTP.
//!
//! The rule under test: Amazon Managed Grafana validates the request BODY against
//! the operation's schema before any handler runs, and a body that fails it is
//! refused with HTTP 400 and the (unmodelled) error code `BadRequestException`,
//! whose message names the body — while a value bound to the request URI (a
//! `@httpLabel` path segment, a `@httpQuery` parameter) is not part of that body,
//! so a refusal of one keeps the modelled `ValidationException` with the member's
//! own constraint message.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization and the refusal rendering — the same path a client takes. A
//! client branches on the code, so the code is what each row asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service; it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The code and message a body the request validator rejected is refused with.
/// Recorded behaviour: `BadRequestException` with "Invalid request body".
const BAD_REQUEST: &str = "BadRequestException";
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The code the service itself raises for a value it refused — the one the model
/// declares (HTTP 400).
const VALIDATION: &str = "ValidationException";

/// Published bounds, spelled out here independently of the crate's constants.
/// Source: `smithy_query(query="operation", operation="CreateWorkspaceApiKey,
/// CreateWorkspaceServiceAccountToken,CreateWorkspaceServiceAccount,CreateWorkspace")`.
const SECONDS_TO_LIVE_MIN: i64 = 1;
const SECONDS_TO_LIVE_MAX: i64 = 2_592_000;
const API_KEY_NAME_MAX_LEN: usize = 100;
const SERVICE_ACCOUNT_NAME_MAX_LEN: usize = 128;
const TOKEN_NAME_MAX_LEN: usize = 128;
/// `ListWorkspaces$maxResults` / `ListVersions$maxResults`: `@range(min: 1, max: 100)`.
const MAX_RESULTS_CEILING: i64 = 100;

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:9297")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A workspace with a service account on it, so a case can address real resources
/// and reach the validator rather than a not-found refusal.
async fn workspace_with_account(router: &MultiServiceRouter) -> (String, String) {
    let created = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "accountAccessType": "CURRENT_ACCOUNT",
            "permissionType": "SERVICE_MANAGED",
            "authenticationProviders": ["SAML"],
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the workspace these cases need must be created, got {} with {:?}",
        created.status, created.body
    );
    let id = created.body["workspace"]["id"]
        .as_str()
        .expect("CreateWorkspace answers an id")
        .to_string();

    let account = call(
        router,
        "POST",
        &format!("/workspaces/{id}/serviceaccounts"),
        Some(json!({"name": "automation", "grafanaRole": "ADMIN"})),
    )
    .await;
    assert_eq!(
        account.status, 200,
        "the service account these cases need must be created, got {} with {:?}",
        account.status, account.body
    );
    let account_id = account.body["id"]
        .as_str()
        .expect("CreateWorkspaceServiceAccount answers an id")
        .to_string();
    (id, account_id)
}

/// A value in the request BODY outside a bound the model declares is refused with
/// `BadRequestException`.
///
/// The rows span the operations that carry a constrained member in their body and
/// the kinds of bound a body member can fail — a `@range` on either side (the
/// recorded case, `secondsToLive` one past its maximum, is one row of five), a
/// `@length` minimum and maximum on four different members, and a `@pattern` —
/// and the member is a different one in nearly every row.
#[tokio::test]
async fn a_body_value_outside_a_modelled_bound_is_refused_as_a_bad_request() {
    let router = emulator();
    let (workspace_id, account_id) = workspace_with_account(&router).await;

    let too_long_key_name = "k".repeat(API_KEY_NAME_MAX_LEN + 1);
    let too_long_account_name = "a".repeat(SERVICE_ACCOUNT_NAME_MAX_LEN + 1);
    let too_long_token_name = "t".repeat(TOKEN_NAME_MAX_LEN + 1);
    let cases: Vec<(&str, &str, String, Value)> = vec![
        (
            "a token lifetime one second past the maximum",
            "POST",
            format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
            json!({"name": "ci", "secondsToLive": SECONDS_TO_LIVE_MAX + 1}),
        ),
        (
            "a token lifetime one second below the minimum",
            "POST",
            format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
            json!({"name": "ci", "secondsToLive": SECONDS_TO_LIVE_MIN - 1}),
        ),
        (
            "a negative token lifetime",
            "POST",
            format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
            json!({"name": "ci", "secondsToLive": -86_400}),
        ),
        (
            "a token name past its length maximum",
            "POST",
            format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
            json!({"name": too_long_token_name, "secondsToLive": 600}),
        ),
        (
            "an empty token name",
            "POST",
            format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
            json!({"name": "", "secondsToLive": 600}),
        ),
        (
            "an API key lifetime past the maximum",
            "POST",
            format!("/workspaces/{workspace_id}/apikeys"),
            json!({
                "keyName": "too-long-lived",
                "keyRole": "ADMIN",
                "secondsToLive": SECONDS_TO_LIVE_MAX + 1,
            }),
        ),
        (
            "an API key name past its length maximum",
            "POST",
            format!("/workspaces/{workspace_id}/apikeys"),
            json!({
                "keyName": too_long_key_name,
                "keyRole": "ADMIN",
                "secondsToLive": 600,
            }),
        ),
        (
            "an empty service-account name",
            "POST",
            format!("/workspaces/{workspace_id}/serviceaccounts"),
            json!({"name": "", "grafanaRole": "VIEWER"}),
        ),
        (
            "a service-account name past its length maximum",
            "POST",
            format!("/workspaces/{workspace_id}/serviceaccounts"),
            json!({"name": too_long_account_name, "grafanaRole": "VIEWER"}),
        ),
        (
            "a workspace name outside WorkspaceName's pattern",
            "POST",
            "/workspaces".to_string(),
            json!({
                "accountAccessType": "CURRENT_ACCOUNT",
                "permissionType": "SERVICE_MANAGED",
                "authenticationProviders": ["SAML"],
                "workspaceName": "not a legal name",
            }),
        ),
        (
            "a client token outside ClientToken's pattern",
            "POST",
            "/workspaces".to_string(),
            json!({
                "accountAccessType": "CURRENT_ACCOUNT",
                "permissionType": "SERVICE_MANAGED",
                "authenticationProviders": ["SAML"],
                "clientToken": "has a space",
            }),
        ),
        (
            "a configuration document below OverridableConfigurationJson's minimum length",
            "PUT",
            format!("/workspaces/{workspace_id}/configuration"),
            json!({"configuration": "{"}),
        ),
    ];

    for (case, method, uri, body) in cases {
        let refused = call(&router, method, &uri, Some(body)).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, BAD_REQUEST),
            "case {case:?}: a body the request validator rejects must answer a 400 \
             {BAD_REQUEST}, got {} with {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.message(),
            INVALID_REQUEST_BODY,
            "case {case:?}: the refusal must state what it validated, got {:?}",
            refused.body
        );
    }

    // And nothing was stored by any of them: the service account still holds no
    // token, and the workspace still holds no API key.
    let tokens = call(
        &router,
        "GET",
        &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
        None,
    )
    .await;
    assert_eq!(
        tokens.body["serviceAccountTokens"],
        json!([]),
        "a refused create must store nothing, got {:?}",
        tokens.body
    );
    let key_gone = call(
        &router,
        "DELETE",
        &format!("/workspaces/{workspace_id}/apikeys/too-long-lived"),
        None,
    )
    .await;
    assert_eq!(
        (key_gone.status, key_gone.error_code()),
        (404, "ResourceNotFoundException"),
        "a refused API-key create must store nothing, got {} with {:?}",
        key_gone.status,
        key_gone.body
    );
}

/// A value bound to the request URI is not part of the body the validator checks,
/// so its refusal keeps the modelled `ValidationException` and the member's own
/// constraint message.
///
/// The rows span the two URI bindings this service uses — a `@httpLabel` path
/// segment (`workspaceId`, whose `^g-[0-9a-f]{10}$` is failed by a value that is
/// too short, wrongly cased, and not hex at all) and a `@httpQuery` parameter
/// (`maxResults`, on either side of its published `@range`, on two different
/// operations).
#[tokio::test]
async fn a_uri_bound_value_outside_its_bound_keeps_the_modelled_validation_exception() {
    let router = emulator();

    let cases: [(&str, &str, String); 7] = [
        (
            "a workspace id that is not workspace-shaped",
            "GET",
            "/workspaces/not-a-workspace".to_string(),
        ),
        (
            "a workspace id with too few hex digits",
            "GET",
            "/workspaces/g-0123".to_string(),
        ),
        (
            "a workspace id in upper case",
            "GET",
            "/workspaces/g-0123456789A".to_string(),
        ),
        (
            "a malformed workspace id on a sub-resource read",
            "GET",
            "/workspaces/not-a-workspace/serviceaccounts".to_string(),
        ),
        (
            "maxResults below the range minimum on ListWorkspaces",
            "GET",
            "/workspaces?maxResults=0".to_string(),
        ),
        (
            "maxResults past the range maximum on ListWorkspaces",
            "GET",
            format!("/workspaces?maxResults={}", MAX_RESULTS_CEILING + 1),
        ),
        (
            "maxResults past the range maximum on ListVersions",
            "GET",
            format!("/versions?maxResults={}", MAX_RESULTS_CEILING + 1),
        ),
    ];

    for (case, method, uri) in cases {
        let refused = call(&router, method, &uri, None).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, VALIDATION),
            "case {case:?}: a URI-bound value is refused by the service itself, so it must \
             answer a 400 {VALIDATION}, got {} with {:?}",
            refused.status,
            refused.body
        );
        assert_ne!(
            refused.message(),
            INVALID_REQUEST_BODY,
            "case {case:?}: the refusal must state the member's own constraint, not the \
             body validator's message, got {:?}",
            refused.body
        );
    }
}

/// A required member the request leaves out is absence, not a value outside a
/// bound: it keeps the modelled `ValidationException` the service's own validator
/// answers.
#[tokio::test]
async fn an_absent_required_member_keeps_the_modelled_validation_exception() {
    let router = emulator();
    let (workspace_id, account_id) = workspace_with_account(&router).await;

    let cases: Vec<(&str, &str, String, Value)> = vec![
        (
            "a token create with no secondsToLive",
            "POST",
            format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
            json!({"name": "ci"}),
        ),
        (
            "an API key create with no keyRole",
            "POST",
            format!("/workspaces/{workspace_id}/apikeys"),
            json!({"keyName": "no-role", "secondsToLive": 600}),
        ),
        (
            "a workspace create with no permissionType",
            "POST",
            "/workspaces".to_string(),
            json!({
                "accountAccessType": "CURRENT_ACCOUNT",
                "authenticationProviders": ["SAML"],
            }),
        ),
    ];

    for (case, method, uri, body) in cases {
        let refused = call(&router, method, &uri, Some(body)).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, VALIDATION),
            "case {case:?}: an absent required member must answer a 400 {VALIDATION}, got \
             {} with {:?}",
            refused.status,
            refused.body
        );
    }
}
