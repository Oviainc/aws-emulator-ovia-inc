// SPDX-License-Identifier: CC0-1.0

//! `DeleteWorkspaceServiceAccountToken`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: a delete releases exactly the one stored token the three ids on
//! the request path name, leaving every other token of that account — and every other
//! account and workspace — as it was, reports the ids of the record it removed, and
//! disables the key that token handed out; an id naming no such workspace, service
//! account or token (never created, or already released) removes nothing and is refused
//! with the modelled 404 `ResourceNotFoundException`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::{
    service_state, LIFECYCLE_TRANSITION,
};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The `resourceType` each refusal names, as the error payloads report it.
const WORKSPACE_TYPE: &str = "Workspace";
const SERVICE_ACCOUNT_TYPE: &str = "ServiceAccount";
const SERVICE_ACCOUNT_TOKEN_TYPE: &str = "ServiceAccountToken";

/// A `workspaceId` satisfying `WorkspaceId`'s published `^g-[0-9a-f]{10}$` pattern that
/// no create ever minted. Source: `smithy_query(query="operation",
/// operation="DeleteWorkspaceServiceAccountToken")` — `workspaceId` carries that pattern.
const UNMINTED_WORKSPACE_ID: &str = "g-ffffffffff";

/// The emulator under test: the router a client talks to, beside the one shared engine
/// its handlers write through, so a test can read the state back as stored.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
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

    fn string(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The reported token ids, in the order served.
    fn listed_ids(&self) -> Vec<String> {
        self.body["serviceAccountTokens"]
            .as_array()
            .map(|tokens| {
                tokens
                    .iter()
                    .map(|token| token["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn request(
    emulator: &Emulator,
    method: &str,
    uri: &str,
    host: &str,
    authorization: Option<&str>,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder().method(method).uri(uri);
    if !host.is_empty() {
        builder = builder.header("host", host);
    }
    if let Some(authorization) = authorization {
        builder = builder.header("authorization", authorization);
    }
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = emulator.router.dispatch(request).await;
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

/// A control-plane call, signed for this service.
async fn control(emulator: &Emulator, method: &str, uri: &str, body: Option<Value>) -> Answer {
    request(
        emulator,
        method,
        uri,
        "grafana.eu-west-1.amazonaws.com",
        Some(SIGNED_FOR_EU_WEST_1),
        body,
    )
    .await
}

/// A workspace, plus the endpoint its own Grafana HTTP API answers on.
async fn workspace(emulator: &Emulator) -> (String, String) {
    let created = control(
        emulator,
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
    let workspace = &created.body["workspace"];
    (
        workspace["id"]
            .as_str()
            .expect("CreateWorkspace answers an id")
            .to_string(),
        workspace["endpoint"]
            .as_str()
            .expect("CreateWorkspace answers an endpoint")
            .to_string(),
    )
}

/// A service account named `name` on `workspace_id`.
async fn service_account(emulator: &Emulator, workspace_id: &str, name: &str) -> String {
    let created = control(
        emulator,
        "POST",
        &format!("/workspaces/{workspace_id}/serviceaccounts"),
        Some(json!({"name": name, "grafanaRole": "ADMIN"})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the service account this case needs must be created, got {} with {:?}",
        created.status, created.body
    );
    created.string("id")
}

/// A token on `account_id`: its id and the key the create handed out once.
async fn create_token(
    emulator: &Emulator,
    workspace_id: &str,
    account_id: &str,
    name: &str,
    seconds_to_live: i64,
) -> (String, String) {
    let created = control(
        emulator,
        "POST",
        &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
        Some(json!({"name": name, "secondsToLive": seconds_to_live})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the token this case needs must be created, got {} with {:?}",
        created.status, created.body
    );
    let token = &created.body["serviceAccountToken"];
    (
        token["id"]
            .as_str()
            .expect("the create answers a token id")
            .to_string(),
        token["key"]
            .as_str()
            .expect("the create answers a key")
            .to_string(),
    )
}

async fn delete_token(
    emulator: &Emulator,
    workspace_id: &str,
    account_id: &str,
    token_id: &str,
) -> Answer {
    control(
        emulator,
        "DELETE",
        &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens/{token_id}"),
        None,
    )
    .await
}

async fn list_tokens(emulator: &Emulator, workspace_id: &str, account_id: &str) -> Answer {
    control(
        emulator,
        "GET",
        &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
        None,
    )
    .await
}

/// The ids of the tokens the shared store holds for one service account, as persisted.
fn stored_ids(emulator: &Emulator, workspace_id: &str, account_id: &str) -> Vec<String> {
    let state = service_state(&*emulator.engine).expect("the grafana store is reachable");
    let store = state.lock();
    store
        .workspaces
        .get(workspace_id)
        .and_then(|workspace| workspace.service_account(account_id))
        .map(|account| {
            account
                .tokens
                .iter()
                .map(|token| token.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// A delete releases exactly the token it names and reports the record it removed.
///
/// The rows differ along what the rule generalises over: how many tokens the account
/// holds and which of them is released (the only one, the first of several, one in the
/// middle, the last), and along the released token's name and lifetime.
#[tokio::test]
async fn a_delete_releases_exactly_the_token_it_names() {
    // (case, how many tokens the account holds, the index deleted, name, secondsToLive)
    let cases: [(&str, usize, usize, &str, i64); 5] = [
        ("the only token the account holds", 1, 0, "a", 1),
        ("the first of three", 3, 0, "ci.runner", 60),
        ("one in the middle of five", 5, 2, "nightly reporting job", 3_600),
        ("the last of four", 4, 3, "обзор-токен", 2_592_000),
        ("one of two", 2, 1, "dashboards-2026", 604_800),
    ];

    for (case, held, deleted_index, name, seconds_to_live) in cases {
        let emulator = emulator();
        let (workspace_id, _) = workspace(&emulator).await;
        let account_id = service_account(&emulator, &workspace_id, "automation").await;
        // A second account on the same workspace: the delete must not reach it.
        let other_account = service_account(&emulator, &workspace_id, "bystander").await;
        let (bystander_token, _) =
            create_token(&emulator, &workspace_id, &other_account, "bystander", 600).await;

        let mut minted = Vec::new();
        for index in 0..held {
            let (token_name, lifetime) = if index == deleted_index {
                (name.to_string(), seconds_to_live)
            } else {
                (format!("kept-{index}"), 600)
            };
            let (id, _) =
                create_token(&emulator, &workspace_id, &account_id, &token_name, lifetime).await;
            minted.push(id);
        }
        let target = minted[deleted_index].clone();
        let kept: Vec<String> = minted
            .iter()
            .filter(|id| **id != target)
            .cloned()
            .collect();

        let deleted = delete_token(&emulator, &workspace_id, &account_id, &target).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            (
                deleted.string("workspaceId"),
                deleted.string("serviceAccountId"),
                deleted.string("tokenId"),
            ),
            (workspace_id.clone(), account_id.clone(), target.clone()),
            "case {case:?}: the answer must report the record it removed, got {:?}",
            deleted.body
        );

        // Read back through the store: the named token is gone and the rest are
        // untouched, in the order they were minted.
        let remaining = stored_ids(&emulator, &workspace_id, &account_id);
        assert_eq!(
            remaining, kept,
            "case {case:?}: the store must hold every token but {target:?}, got {remaining:?}"
        );

        // Read back through the API: the paginated read serves the same records.
        let listed = list_tokens(&emulator, &workspace_id, &account_id).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the read must answer HTTP 200, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.listed_ids(),
            kept,
            "case {case:?}: the read must report every token but {target:?}, got {:?}",
            listed.body
        );

        // The account beside it kept its own token.
        assert_eq!(
            stored_ids(&emulator, &workspace_id, &other_account),
            vec![bystander_token.clone()],
            "case {case:?}: the delete must not reach another service account's tokens"
        );

        // A second delete of the released token has nothing to remove.
        let again = delete_token(&emulator, &workspace_id, &account_id, &target).await;
        assert_eq!(
            (again.status, again.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: releasing {target:?} twice must answer a 404 \
             ResourceNotFoundException, got {} with {:?}",
            again.status,
            again.body
        );
        assert!(
            again.string("message").contains(&target),
            "case {case:?}: the refusal must name the token id it could not find, got {:?}",
            again.body
        );
        assert_eq!(
            stored_ids(&emulator, &workspace_id, &account_id),
            kept,
            "case {case:?}: the refused second delete must store nothing"
        );
    }
}

/// An id naming no workspace, service account or token is refused and removes nothing.
///
/// The rows differ along which of the three ids names nothing and along the form of that
/// id (a workspace id matching the published pattern that was never minted, a decimal id
/// no account or token carries, a non-decimal id, and the id of a token of ANOTHER
/// account or workspace).
#[tokio::test]
async fn an_id_naming_nothing_is_refused_and_removes_nothing() {
    let emulator = emulator();
    let (workspace_id, _) = workspace(&emulator).await;
    let account_id = service_account(&emulator, &workspace_id, "automation").await;
    let other_account = service_account(&emulator, &workspace_id, "bystander").await;
    let (mine, _) = create_token(&emulator, &workspace_id, &account_id, "mine", 600).await;
    // Token ids are minted per service account, so the SECOND token of the account
    // beside it carries an id the account under test does not: it is the id that shows a
    // delete reaches only the account the request names.
    let (theirs, _) = create_token(&emulator, &workspace_id, &other_account, "theirs", 600).await;
    let (theirs_second, _) =
        create_token(&emulator, &workspace_id, &other_account, "theirs-second", 600).await;

    // A second workspace, holding more service accounts than the first, so its last
    // account id names no account of the workspace under test.
    let (other_workspace, _) = workspace(&emulator).await;
    for name in ["first", "second"] {
        service_account(&emulator, &other_workspace, name).await;
    }
    let other_workspace_account = service_account(&emulator, &other_workspace, "elsewhere").await;
    let (elsewhere, _) =
        create_token(&emulator, &other_workspace, &other_workspace_account, "elsewhere", 600)
            .await;

    // (case, workspaceId, serviceAccountId, tokenId, the resourceType the refusal names)
    let cases: Vec<(&str, String, String, String, &str)> = vec![
        (
            "a workspace id of the published form that was never minted",
            UNMINTED_WORKSPACE_ID.to_string(),
            account_id.clone(),
            mine.clone(),
            WORKSPACE_TYPE,
        ),
        (
            "a service account id no workspace account carries",
            workspace_id.clone(),
            "999".to_string(),
            mine.clone(),
            SERVICE_ACCOUNT_TYPE,
        ),
        (
            "a non-decimal service account id",
            workspace_id.clone(),
            "not-an-account".to_string(),
            mine.clone(),
            SERVICE_ACCOUNT_TYPE,
        ),
        (
            "a decimal token id this account never minted",
            workspace_id.clone(),
            account_id.clone(),
            "4242".to_string(),
            SERVICE_ACCOUNT_TOKEN_TYPE,
        ),
        (
            "a non-decimal token id",
            workspace_id.clone(),
            account_id.clone(),
            "not-a-token".to_string(),
            SERVICE_ACCOUNT_TOKEN_TYPE,
        ),
        (
            "a token id another service account of the same workspace carries",
            workspace_id.clone(),
            account_id.clone(),
            theirs_second.clone(),
            SERVICE_ACCOUNT_TOKEN_TYPE,
        ),
        (
            "a service account of another workspace",
            workspace_id.clone(),
            other_workspace_account.clone(),
            elsewhere.clone(),
            SERVICE_ACCOUNT_TYPE,
        ),
    ];

    for (case, workspace_arg, account_arg, token_arg, resource_type) in cases {
        let refused = delete_token(&emulator, &workspace_arg, &account_arg, &token_arg).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {} with {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.string("resourceType"),
            resource_type,
            "case {case:?}: the refusal must name the resource that was not found, got {:?}",
            refused.body
        );

        // Nothing was released: every token every account held is still there.
        assert_eq!(
            stored_ids(&emulator, &workspace_id, &account_id),
            vec![mine.clone()],
            "case {case:?}: a refused delete must remove nothing"
        );
        assert_eq!(
            stored_ids(&emulator, &workspace_id, &other_account),
            vec![theirs.clone(), theirs_second.clone()],
            "case {case:?}: a refused delete must remove nothing"
        );
        assert_eq!(
            stored_ids(&emulator, &other_workspace, &other_workspace_account),
            vec![elsewhere.clone()],
            "case {case:?}: a refused delete must remove nothing"
        );
    }
}

/// The released token's key authorizes nothing afterwards, while the keys of the tokens
/// beside it keep working: deleting a token "disable[s] the key associated with" it.
///
/// The rows differ along which of the account's tokens is released.
#[tokio::test]
async fn the_released_tokens_key_stops_authorizing() {
    for (case, deleted_index) in [("the first token", 0usize), ("the last token", 2usize)] {
        let emulator = emulator();
        let (workspace_id, endpoint) = workspace(&emulator).await;
        let account_id = service_account(&emulator, &workspace_id, "automation").await;

        let mut tokens = Vec::new();
        for index in 0..3 {
            tokens.push(
                create_token(
                    &emulator,
                    &workspace_id,
                    &account_id,
                    &format!("token-{index}"),
                    3_600,
                )
                .await,
            );
        }

        // The workspace's own Grafana HTTP API answers once the clock has moved the
        // workspace out of CREATING.
        tokio::time::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(100)).await;
        let (host, path) = split_endpoint(&endpoint);

        for (index, (id, key)) in tokens.iter().enumerate() {
            let answered = request(
                &emulator,
                "GET",
                &format!("{path}/api/org"),
                &host,
                Some(&format!("Bearer {key}")),
                None,
            )
            .await;
            assert_eq!(
                answered.status, 200,
                "case {case:?}: token {index} ({id}) must authorize before any delete, got {} \
                 with {:?}",
                answered.status, answered.body
            );
        }

        let (target_id, target_key) = tokens[deleted_index].clone();
        let deleted = delete_token(&emulator, &workspace_id, &account_id, &target_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must answer HTTP 200, got {} with {:?}",
            deleted.status, deleted.body
        );

        let revoked = request(
            &emulator,
            "GET",
            &format!("{path}/api/org"),
            &host,
            Some(&format!("Bearer {target_key}")),
            None,
        )
        .await;
        assert_eq!(
            revoked.status, 401,
            "case {case:?}: the released token's key must authorize nothing, got {} with {:?}",
            revoked.status, revoked.body
        );

        for (index, (id, key)) in tokens.iter().enumerate() {
            if index == deleted_index {
                continue;
            }
            let answered = request(
                &emulator,
                "GET",
                &format!("{path}/api/org"),
                &host,
                Some(&format!("Bearer {key}")),
                None,
            )
            .await;
            assert_eq!(
                answered.status, 200,
                "case {case:?}: token {index} ({id}) must keep authorizing after another is \
                 released, got {} with {:?}",
                answered.status, answered.body
            );
        }
    }
}

/// The `host:port` and the path of an announced endpoint: what a client splits a URL
/// into when it issues the request.
fn split_endpoint(endpoint: &str) -> (String, String) {
    let without_scheme = endpoint
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("an announced endpoint must be https, got {endpoint:?}"));
    match without_scheme.find('/') {
        Some(cut) => (
            without_scheme[..cut].to_string(),
            without_scheme[cut..].to_string(),
        ),
        None => (without_scheme.to_string(), String::new()),
    }
}
