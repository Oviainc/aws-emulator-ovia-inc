// SPDX-License-Identifier: CC0-1.0

//! The `ResourceNotFoundException` message a workspace sub-resource is refused with,
//! end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: when a request names a sub-resource of a workspace that the
//! workspace does not hold, the 404 `ResourceNotFoundException` message is one finished
//! sentence — it ends in a period — whose only variable part is the sub-resource
//! identifier taken verbatim from the request; the workspace id, which the request path
//! already carries, is never repeated in it. For an API key the sentence is
//! `API key <keyName> not found.`, and for a service account `Service account
//! <serviceAccountId> not found.`

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The bounds of `ApiKeyName`. Source: `smithy_query(query="operation",
/// operation="DeleteWorkspaceApiKey")` — `keyName` carries `@length(min: 1, max: 100)`.
const MIN_KEY_NAME_LENGTH: usize = 1;
const MAX_KEY_NAME_LENGTH: usize = 100;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn string(&self, member: &str) -> &str {
        self.body[member].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "grafana.eu-west-1.amazonaws.com")
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

/// A workspace to hang sub-resources on, created through `CreateWorkspace`.
async fn workspace(router: &MultiServiceRouter) -> String {
    let created = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "accountAccessType": "CURRENT_ACCOUNT",
            "permissionType": "CUSTOMER_MANAGED",
            "authenticationProviders": ["AWS_SSO"],
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the workspace this case needs must be created, got {} with {:?}",
        created.status, created.body
    );
    created.body["workspace"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkspace must report an id: {:?}", created.body))
        .to_string()
}

/// Assert one refusal: the modelled status and code, the exact sentence, and that the
/// workspace id the path carried is not repeated inside the message.
fn assert_refusal(case: &str, answer: &Answer, expected_message: &str, workspace_id: &str) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: expected a 404 ResourceNotFoundException, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.string("message"),
        expected_message,
        "case {case:?}: expected message {expected_message:?}, got {:?}",
        answer.string("message")
    );
    assert!(
        !answer.string("message").contains(workspace_id),
        "case {case:?}: the message must not repeat the workspace id {workspace_id:?} the \
         request path already carries, got {:?}",
        answer.string("message")
    );
}

/// The rows of the API-key table: key names differing along every dimension the rule
/// generalises over — both published `ApiKeyName` bounds, name shape, case, digits and
/// punctuation — none of them the name the recorded run happened to generate.
fn key_names() -> Vec<(&'static str, String)> {
    vec![
        ("a plain lowercase name", "dashboards".to_string()),
        (
            "the shortest name ApiKeyName allows",
            "k".repeat(MIN_KEY_NAME_LENGTH),
        ),
        (
            "the longest name ApiKeyName allows",
            "k".repeat(MAX_KEY_NAME_LENGTH),
        ),
        (
            "a punctuated, mixed-case name",
            "Team.Key_1-ci".to_string(),
        ),
        ("a name of digits only", "20260101".to_string()),
        ("a name carrying the workspace prefix", "g-key".to_string()),
    ]
}

/// A `DeleteWorkspaceApiKey` for a name the workspace does not hold reports AWS's one
/// sentence, whether the name was never created or has just been deleted.
#[tokio::test]
async fn an_unknown_api_key_is_refused_with_the_key_name_alone() {
    let router = emulator();
    let workspace_id = workspace(&router).await;

    for (case, name) in key_names() {
        let expected = format!("API key {name} not found.");

        // A name no create ever took on this workspace.
        let never_created = call(
            &router,
            "DELETE",
            &format!("/workspaces/{workspace_id}/apikeys/{name}"),
            None,
        )
        .await;
        assert_refusal(
            &format!("{case}, never created"),
            &never_created,
            &expected,
            &workspace_id,
        );

        // The same name after a create and a delete: the second delete of a released
        // name reports the same sentence.
        let created = call(
            &router,
            "POST",
            &format!("/workspaces/{workspace_id}/apikeys"),
            Some(json!({"keyName": name, "keyRole": "ADMIN", "secondsToLive": 3_600})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the key must be created first, got {} with {:?}",
            created.status, created.body
        );
        let deleted = call(
            &router,
            "DELETE",
            &format!("/workspaces/{workspace_id}/apikeys/{name}"),
            None,
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the first delete must succeed, got {} with {:?}",
            deleted.status, deleted.body
        );
        let again = call(
            &router,
            "DELETE",
            &format!("/workspaces/{workspace_id}/apikeys/{name}"),
            None,
        )
        .await;
        assert_refusal(
            &format!("{case}, deleted twice"),
            &again,
            &expected,
            &workspace_id,
        );
        assert_eq!(
            again.string("resourceId"),
            name,
            "case {case:?}: the refusal must still report the key as its resourceId, got {:?}",
            again.body
        );
    }
}

/// Every operation that reaches a service account of a workspace refuses an id the
/// workspace does not hold with the same one sentence, naming the id alone.
#[tokio::test]
async fn an_unknown_service_account_is_refused_with_the_account_id_alone() {
    let router = emulator();
    let workspace_id = workspace(&router).await;
    // A workspace beside it, so one row can use the id of an account that exists
    // elsewhere: an id is a service account only on the workspace that holds it.
    let other_workspace = workspace(&router).await;
    let elsewhere = call(
        &router,
        "POST",
        &format!("/workspaces/{other_workspace}/serviceaccounts"),
        Some(json!({"name": "bystander", "grafanaRole": "ADMIN"})),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "the other workspace's service account must be created, got {} with {:?}",
        elsewhere.status, elsewhere.body
    );
    let elsewhere_id = elsewhere.string("id").to_string();

    // The rows differ along the form of the id that names nothing: a decimal id no
    // account carries, one well past any minted id, a non-decimal id, and the id of an
    // account that exists on ANOTHER workspace.
    let account_ids = vec![
        ("a decimal id no account of this workspace carries", "7".to_string()),
        ("an id well past any minted one", "987654321".to_string()),
        ("a non-decimal id", "not-an-id".to_string()),
        ("an id minted on another workspace", elsewhere_id),
    ];

    for (case, account_id) in account_ids {
        let expected = format!("Service account {account_id} not found.");

        // The three operations whose path reaches a service account of a workspace.
        let listed = call(
            &router,
            "GET",
            &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
            None,
        )
        .await;
        assert_refusal(
            &format!("{case}, ListWorkspaceServiceAccountTokens"),
            &listed,
            &expected,
            &workspace_id,
        );

        let minted = call(
            &router,
            "POST",
            &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
            Some(json!({"name": "a-token", "secondsToLive": 600})),
        )
        .await;
        assert_refusal(
            &format!("{case}, CreateWorkspaceServiceAccountToken"),
            &minted,
            &expected,
            &workspace_id,
        );

        let released = call(
            &router,
            "DELETE",
            &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens/1"),
            None,
        )
        .await;
        assert_refusal(
            &format!("{case}, DeleteWorkspaceServiceAccountToken"),
            &released,
            &expected,
            &workspace_id,
        );
    }
}
