// SPDX-License-Identifier: CC0-1.0

//! The address a workspace's native Grafana HTTP API is announced at, across EVERY
//! projection of the workspace that carries it.
//!
//! The rule under test: `WorkspaceDescription$endpoint` and `WorkspaceSummary$endpoint`
//! are "The URL that users can use to access the Grafana console in the workspace", so
//! every operation that reports a workspace reports the SAME address for it — the one
//! this emulator serves that workspace's Grafana HTTP API on (`workspace_grafana_api`
//! covers what answers there, and which endpoint a request is announced). A client that
//! read the endpoint from any one of them therefore holds the same reachable URL, and
//! two workspaces never share one.
//!
//! The rows differ along the projection the endpoint was read from: `CreateWorkspace`,
//! `DescribeWorkspace`, `UpdateWorkspace`, the `ListWorkspaces` summary and
//! `DeleteWorkspace` — and along the workspace, of which there are two, so a projection
//! that reported a constant or another record's address fails here.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::LIFECYCLE_TRANSITION;
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a control-plane
/// request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The endpoint every request below reaches this emulator on, and so the authority the
/// announced workspace endpoints have to name.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The path prefix a workspace endpoint carries under that host, spelled out here
/// independently of the crate's own `WORKSPACE_API_PATH_PREFIX`.
const ENDPOINT_PREFIX: &str = "/grafana-workspaces/";

/// Long enough for the `CREATING` -> `ACTIVE` transition the clock owes a new workspace
/// to come due.
const PAST_THE_TRANSITION: Duration = LIFECYCLE_TRANSITION.saturating_mul(3);

/// What a call answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One control-plane call, reaching the emulator at [`EMULATOR_HOST`].
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", EMULATOR_HOST)
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
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// Create one workspace named `name`: its id, and the endpoint the create announced.
async fn create_workspace(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "accountAccessType": "CURRENT_ACCOUNT",
            "permissionType": "SERVICE_MANAGED",
            "authenticationProviders": ["SAML"],
            "workspaceName": name,
            "workspaceRoleArn": "arn:aws:iam::000000000000:role/grafana-workspace",
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: the workspace must be created, got {} with {:?}",
        created.status, created.body
    );
    let id = created.body["workspace"]["id"]
        .as_str()
        .expect("CreateWorkspace answers an id")
        .to_string();
    let endpoint = created.body["workspace"]["endpoint"]
        .as_str()
        .expect("CreateWorkspace answers an endpoint")
        .to_string();
    (id, endpoint)
}

#[tokio::test]
async fn every_projection_reports_the_one_endpoint_the_workspace_is_served_at() {
    let router = emulator();

    // Two workspaces, so a projection reporting a constant or the other record's
    // address cannot pass.
    let (first_id, first_created_endpoint) = create_workspace(&router, "monitoring").await;
    let (second_id, second_created_endpoint) = create_workspace(&router, "reporting").await;
    assert_ne!(
        first_created_endpoint, second_created_endpoint,
        "each workspace must announce its own endpoint, both got {first_created_endpoint:?}"
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for (case, id, created_endpoint) in [
        ("the first workspace", &first_id, &first_created_endpoint),
        ("the second workspace", &second_id, &second_created_endpoint),
    ] {
        let want = format!("https://{EMULATOR_HOST}{ENDPOINT_PREFIX}{id}");
        assert_eq!(
            created_endpoint, &want,
            "case {case:?}: CreateWorkspace must announce the endpoint this emulator serves \
             the workspace at, expected {want:?}, got {created_endpoint:?}"
        );

        let described = call(&router, "GET", &format!("/workspaces/{id}"), None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeWorkspace must answer 200, got {} with {:?}",
            described.status, described.body
        );

        let updated = call(
            &router,
            "PUT",
            &format!("/workspaces/{id}"),
            Some(json!({"workspaceDescription": format!("{case} updated")})),
        )
        .await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: UpdateWorkspace must answer 202, got {} with {:?}",
            updated.status, updated.body
        );

        let listed = call(&router, "GET", "/workspaces", None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListWorkspaces must answer 200, got {} with {:?}",
            listed.status, listed.body
        );
        let summary = listed.body["workspaces"]
            .as_array()
            .and_then(|workspaces| {
                workspaces
                    .iter()
                    .find(|workspace| workspace["id"] == json!(id))
                    .cloned()
            })
            .unwrap_or(Value::Null);

        let deleted = call(&router, "DELETE", &format!("/workspaces/{id}"), None).await;
        assert_eq!(
            deleted.status, 202,
            "case {case:?}: DeleteWorkspace must answer 202, got {} with {:?}",
            deleted.status, deleted.body
        );

        // One address, reported by every projection that carries it.
        for (projection, reported) in [
            ("DescribeWorkspace", &described.body["workspace"]["endpoint"]),
            ("UpdateWorkspace", &updated.body["workspace"]["endpoint"]),
            ("the ListWorkspaces summary", &summary["endpoint"]),
            ("DeleteWorkspace", &deleted.body["workspace"]["endpoint"]),
        ] {
            assert_eq!(
                *reported,
                json!(want),
                "case {case:?} read from {projection}: expected the endpoint {want:?}, got \
                 {reported:?}"
            );
        }
    }
}
