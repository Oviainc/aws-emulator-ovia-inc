// SPDX-License-Identifier: CC0-1.0

//! `DeleteWorkspaceServiceAccount`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: a delete releases exactly the one stored service account the two
//! ids on the request path name, together with every token that account holds, reports the
//! ids of the record it removed, and leaves every other account of that workspace — and
//! every other workspace — as it was; an id naming no such workspace or service account
//! (never created, or already deleted) removes nothing and is refused with the modelled
//! 404 `ResourceNotFoundException`.

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

/// A `workspaceId` satisfying `WorkspaceId`'s published `^g-[0-9a-f]{10}$` pattern that no
/// create ever minted. Source: `smithy_query(query="operation",
/// operation="DeleteWorkspaceServiceAccount")` — `workspaceId` carries that pattern.
const UNMINTED_WORKSPACE_ID: &str = "g-ffffffffff";

/// The emulator under test: the router a client talks to, beside the one shared engine its
/// handlers write through, so a test can read the state back as stored.
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

    /// The reported service account ids, in the order served.
    fn listed_account_ids(&self) -> Vec<String> {
        self.body["serviceAccounts"]
            .as_array()
            .map(|accounts| {
                accounts
                    .iter()
                    .map(|account| account["id"].as_str().unwrap_or_default().to_string())
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

/// A service account named `name` on `workspace_id`, as `role`.
async fn create_account(emulator: &Emulator, workspace_id: &str, name: &str, role: &str) -> String {
    let created = control(
        emulator,
        "POST",
        &format!("/workspaces/{workspace_id}/serviceaccounts"),
        Some(json!({"name": name, "grafanaRole": role})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the service account {name:?} this case needs must be created, got {} with {:?}",
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
) -> (String, String) {
    let created = control(
        emulator,
        "POST",
        &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
        Some(json!({"name": name, "secondsToLive": 3_600})),
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

async fn delete_account(emulator: &Emulator, workspace_id: &str, account_id: &str) -> Answer {
    control(
        emulator,
        "DELETE",
        &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}"),
        None,
    )
    .await
}

async fn list_accounts(emulator: &Emulator, workspace_id: &str) -> Answer {
    control(
        emulator,
        "GET",
        &format!("/workspaces/{workspace_id}/serviceaccounts"),
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

/// The ids of the service accounts the shared store holds for one workspace, as persisted.
fn stored_account_ids(emulator: &Emulator, workspace_id: &str) -> Vec<String> {
    let state = service_state(&*emulator.engine).expect("the grafana store is reachable");
    let store = state.lock();
    store
        .workspaces
        .get(workspace_id)
        .map(|workspace| {
            workspace
                .service_accounts
                .iter()
                .map(|account| account.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The ids of the tokens the shared store holds for one service account, as persisted.
fn stored_token_ids(emulator: &Emulator, workspace_id: &str, account_id: &str) -> Vec<String> {
    let state = service_state(&*emulator.engine).expect("the grafana store is reachable");
    let store = state.lock();
    store
        .workspaces
        .get(workspace_id)
        .and_then(|workspace| workspace.service_account(account_id))
        .map(|account| account.tokens.iter().map(|t| t.id.clone()).collect())
        .unwrap_or_default()
}

/// A delete releases exactly the account it names, with every token that account holds,
/// and reports the record it removed.
///
/// The rows differ along what the rule generalises over: how many accounts the workspace
/// holds and which of them is released (the only one, the first of several, one in the
/// middle, the last), along the released account's name and role, and along how many
/// tokens it held when it went (none, one, several).
#[tokio::test]
async fn a_delete_releases_exactly_the_account_it_names() {
    // (case, accounts held, index deleted, name, grafanaRole, tokens on that account)
    let cases: [(&str, usize, usize, &str, &str, usize); 5] = [
        ("the only account the workspace holds", 1, 0, "a", "ADMIN", 0),
        ("the first of three, holding one token", 3, 0, "ci.runner", "EDITOR", 1),
        (
            "one in the middle of five, holding three tokens",
            5,
            2,
            "nightly reporting job",
            "VIEWER",
            3,
        ),
        ("the last of four", 4, 3, "обзор-аккаунт", "ADMIN", 2),
        ("one of two", 2, 1, "dashboards-2026", "EDITOR", 0),
    ];

    for (case, held, deleted_index, name, role, tokens) in cases {
        let emulator = emulator();
        let (workspace_id, _) = workspace(&emulator).await;
        // A second workspace holding an account of its own: the delete must not reach it.
        let (other_workspace, _) = workspace(&emulator).await;
        let bystander_workspace_account =
            create_account(&emulator, &other_workspace, "elsewhere", "ADMIN").await;

        let mut minted = Vec::new();
        for index in 0..held {
            let (account_name, account_role) = if index == deleted_index {
                (name.to_string(), role)
            } else {
                (format!("kept-{index}"), "VIEWER")
            };
            minted
                .push(create_account(&emulator, &workspace_id, &account_name, account_role).await);
        }
        let target = minted[deleted_index].clone();
        let kept: Vec<String> = minted.iter().filter(|id| **id != target).cloned().collect();

        for token_index in 0..tokens {
            create_token(
                &emulator,
                &workspace_id,
                &target,
                &format!("token-{token_index}"),
            )
            .await;
        }
        // A token on an account beside it: deleting the target must not release it.
        let neighbour = kept.first().cloned();
        let neighbour_token = if let Some(neighbour) = neighbour.as_ref() {
            let (id, _) = create_token(&emulator, &workspace_id, neighbour, "neighbour").await;
            Some(id)
        } else {
            None
        };

        let deleted = delete_account(&emulator, &workspace_id, &target).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            (
                deleted.string("workspaceId"),
                deleted.string("serviceAccountId"),
            ),
            (workspace_id.clone(), target.clone()),
            "case {case:?}: the answer must report the record it removed, got {:?}",
            deleted.body
        );

        // Read back through the store: the named account is gone and the rest are
        // untouched, in the order they were minted.
        let remaining = stored_account_ids(&emulator, &workspace_id);
        assert_eq!(
            remaining, kept,
            "case {case:?}: the store must hold every account but {target:?}, got {remaining:?}"
        );

        // Read back through the API: the paginated read serves the same records.
        let listed = list_accounts(&emulator, &workspace_id).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the read must answer HTTP 200, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.listed_account_ids(),
            kept,
            "case {case:?}: the read must report every account but {target:?}, got {:?}",
            listed.body
        );

        // The released account's tokens went with it: nothing can be read for it any more.
        let orphaned = list_tokens(&emulator, &workspace_id, &target).await;
        assert_eq!(
            (orphaned.status, orphaned.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the released account's tokens must be gone with it, got {} with \
             {:?}",
            orphaned.status,
            orphaned.body
        );
        assert!(
            stored_token_ids(&emulator, &workspace_id, &target).is_empty(),
            "case {case:?}: the store must hold no token of the released account"
        );

        // The account beside it kept its own token, and the other workspace its account.
        if let (Some(neighbour), Some(token)) = (neighbour.as_ref(), neighbour_token.as_ref()) {
            assert_eq!(
                stored_token_ids(&emulator, &workspace_id, neighbour),
                vec![token.clone()],
                "case {case:?}: the delete must not reach another account's tokens"
            );
        }
        assert_eq!(
            stored_account_ids(&emulator, &other_workspace),
            vec![bystander_workspace_account.clone()],
            "case {case:?}: the delete must not reach another workspace's accounts"
        );

        // A second delete of the released account has nothing to remove.
        let again = delete_account(&emulator, &workspace_id, &target).await;
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
            "case {case:?}: the refusal must name the account id it could not find, got {:?}",
            again.body
        );
        assert_eq!(
            stored_account_ids(&emulator, &workspace_id),
            kept,
            "case {case:?}: the refused second delete must store nothing"
        );
    }
}

/// An id naming no workspace or service account is refused and removes nothing.
///
/// The rows differ along which of the two ids names nothing and along the form of that id
/// (a workspace id matching the published pattern that was never minted, a decimal id no
/// account carries, a non-decimal id, and the id of an account of ANOTHER workspace).
#[tokio::test]
async fn an_id_naming_nothing_is_refused_and_removes_nothing() {
    let emulator = emulator();
    let (workspace_id, _) = workspace(&emulator).await;
    let mine = create_account(&emulator, &workspace_id, "automation", "ADMIN").await;

    // A second workspace holding more accounts than the first, so its last account id
    // names no account of the workspace under test.
    let (other_workspace, _) = workspace(&emulator).await;
    for name in ["first", "second"] {
        create_account(&emulator, &other_workspace, name, "VIEWER").await;
    }
    let elsewhere = create_account(&emulator, &other_workspace, "elsewhere", "EDITOR").await;
    let others = stored_account_ids(&emulator, &other_workspace);

    // (case, workspaceId, serviceAccountId, the resourceType the refusal names)
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "a workspace id of the published form that was never minted",
            UNMINTED_WORKSPACE_ID.to_string(),
            mine.clone(),
            WORKSPACE_TYPE,
        ),
        (
            "a decimal service account id this workspace never minted",
            workspace_id.clone(),
            "999".to_string(),
            SERVICE_ACCOUNT_TYPE,
        ),
        (
            "a non-decimal service account id",
            workspace_id.clone(),
            "not-an-account".to_string(),
            SERVICE_ACCOUNT_TYPE,
        ),
        (
            "an account id another workspace carries",
            workspace_id.clone(),
            elsewhere.clone(),
            SERVICE_ACCOUNT_TYPE,
        ),
    ];

    for (case, workspace_arg, account_arg, resource_type) in cases {
        let refused = delete_account(&emulator, &workspace_arg, &account_arg).await;
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

        // Nothing was released: every account every workspace held is still there.
        assert_eq!(
            stored_account_ids(&emulator, &workspace_id),
            vec![mine.clone()],
            "case {case:?}: a refused delete must remove nothing"
        );
        assert_eq!(
            stored_account_ids(&emulator, &other_workspace),
            others,
            "case {case:?}: a refused delete must remove nothing"
        );
    }
}

/// Every key the released account's tokens handed out authorizes nothing afterwards, while
/// the keys of another account of the same workspace keep working: the delete "will delete
/// any tokens created for the service account, as well".
///
/// The rows differ along how many tokens the released account held.
#[tokio::test]
async fn the_released_accounts_token_keys_stop_authorizing() {
    for (case, tokens) in [("one token", 1usize), ("three tokens", 3usize)] {
        let emulator = emulator();
        let (workspace_id, endpoint) = workspace(&emulator).await;
        let target = create_account(&emulator, &workspace_id, "automation", "ADMIN").await;
        let bystander = create_account(&emulator, &workspace_id, "bystander", "VIEWER").await;

        let mut keys = Vec::new();
        for index in 0..tokens {
            keys.push(
                create_token(
                    &emulator,
                    &workspace_id,
                    &target,
                    &format!("token-{index}"),
                )
                .await,
            );
        }
        let (bystander_token, bystander_key) =
            create_token(&emulator, &workspace_id, &bystander, "bystander").await;

        // The workspace's own Grafana HTTP API answers once the clock has moved the
        // workspace out of CREATING.
        tokio::time::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(100)).await;
        let (host, path) = split_endpoint(&endpoint);

        for (index, (id, key)) in keys.iter().enumerate() {
            let answered = authorize(&emulator, &host, &path, key).await;
            assert_eq!(
                answered.status, 200,
                "case {case:?}: token {index} ({id}) must authorize before the delete, got {} \
                 with {:?}",
                answered.status, answered.body
            );
        }

        let deleted = delete_account(&emulator, &workspace_id, &target).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must answer HTTP 200, got {} with {:?}",
            deleted.status, deleted.body
        );

        for (index, (id, key)) in keys.iter().enumerate() {
            let revoked = authorize(&emulator, &host, &path, key).await;
            assert_eq!(
                revoked.status, 401,
                "case {case:?}: the released account's token {index} ({id}) must authorize \
                 nothing, got {} with {:?}",
                revoked.status, revoked.body
            );
        }

        let answered = authorize(&emulator, &host, &path, &bystander_key).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: another account's token ({bystander_token}) must keep authorizing, \
             got {} with {:?}",
            answered.status, answered.body
        );
    }
}

/// Present `key` as a bearer token to the workspace's own Grafana HTTP API.
async fn authorize(emulator: &Emulator, host: &str, path: &str, key: &str) -> Answer {
    request(
        emulator,
        "GET",
        &format!("{path}/api/org"),
        host,
        Some(&format!("Bearer {key}")),
        None,
    )
    .await
}

/// The `host:port` and the path of an announced endpoint: what a client splits a URL into
/// when it issues the request.
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
