// SPDX-License-Identifier: CC0-1.0

//! Workspace Grafana API keys, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: an API key is a named sub-resource of ONE workspace, so
//! `CreateWorkspaceApiKey` persists it on that workspace under a name unique there and
//! answers with a freshly minted token, and `DeleteWorkspaceApiKey` removes exactly the
//! named key from that workspace — a name the workspace does not hold (never created,
//! already deleted, or past its `secondsToLive`) is a `ResourceNotFoundException`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::{
    API_KEY_QUOTA, LIFECYCLE_TRANSITION,
};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The permission levels a key role may carry.
///
/// Source: the `com.amazonaws.grafana#Role` shape's values, which are also the set
/// `CreateWorkspaceApiKeyRequest$keyRole` documents ("Valid values: ADMIN|EDITOR|VIEWER").
/// Spelled out here, independently of the crate's own constant, so a wrong set in
/// `src/state.rs` fails this test.
const KEY_ROLES: [&str; 3] = ["ADMIN", "EDITOR", "VIEWER"];

/// The published bounds of `CreateWorkspaceApiKeyRequest$secondsToLive`
/// (`@range(min: 1, max: 2592000)` — "Keys can be valid for up to 30 days").
const MIN_SECONDS_TO_LIVE: i64 = 1;
const MAX_SECONDS_TO_LIVE: i64 = 2_592_000;

/// The bounds of `ApiKeyName` (`@length(min: 1, max: 100)`).
const MIN_KEY_NAME_LENGTH: usize = 1;
const MAX_KEY_NAME_LENGTH: usize = 100;

/// A SigV4 credential scope naming this service; it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

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

    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
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

/// A workspace to hang keys on, created through `CreateWorkspace`.
async fn workspace(router: &MultiServiceRouter) -> String {
    let answer = call(
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
        answer.status, 202,
        "CreateWorkspace must answer HTTP 202, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["workspace"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkspace must report an id: {:?}", answer.body))
        .to_string()
}

async fn create_key(
    router: &MultiServiceRouter,
    workspace_id: &str,
    name: &str,
    role: &str,
    seconds_to_live: i64,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/workspaces/{workspace_id}/apikeys"),
        Some(json!({
            "keyName": name,
            "keyRole": role,
            "secondsToLive": seconds_to_live,
        })),
    )
    .await
}

async fn delete_key(router: &MultiServiceRouter, workspace_id: &str, name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/workspaces/{workspace_id}/apikeys/{name}"),
        None,
    )
    .await
}

/// The rows of the main table: a key name, a role and a lifetime, differing along every
/// dimension the rule generalises over — name length (both `ApiKeyName` bounds), name
/// shape, every role the `Role` shape declares (VIEWER included, which no recorded case
/// cites), and both published `secondsToLive` bounds plus a value between them.
fn key_cases() -> Vec<(&'static str, String, &'static str, i64)> {
    vec![
        ("a plain name, admin, one hour", "dashboards".to_string(), "ADMIN", 3_600),
        (
            "the shortest name ApiKeyName allows",
            "k".repeat(MIN_KEY_NAME_LENGTH),
            "EDITOR",
            60,
        ),
        (
            "the longest name ApiKeyName allows",
            "k".repeat(MAX_KEY_NAME_LENGTH),
            "VIEWER",
            MAX_SECONDS_TO_LIVE,
        ),
        (
            "a punctuated, mixed-case name at the shortest lifetime",
            "Team.Key_1-ci".to_string(),
            "VIEWER",
            MIN_SECONDS_TO_LIVE,
        ),
        (
            "the longest lifetime the model admits, as an editor",
            "thirty-days".to_string(),
            "EDITOR",
            MAX_SECONDS_TO_LIVE,
        ),
    ]
}

/// A created key is persisted on its workspace and removed from it by the delete: the
/// create echoes the key it stored and mints a token, the first delete reports that key,
/// and a second delete of the same name answers `ResourceNotFoundException` — after which
/// the name is free again.
#[tokio::test]
async fn a_created_api_key_is_deleted_from_the_workspace_that_holds_it() {
    let router = emulator();
    let workspace_id = workspace(&router).await;
    let mut tokens: Vec<String> = Vec::new();

    for (case, name, role, seconds_to_live) in key_cases() {
        let created = create_key(&router, &workspace_id, &name, role, seconds_to_live).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateWorkspaceApiKey, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.member("keyName"),
            name,
            "case {case:?}: the created key must report the name it was given, got {:?}",
            created.body["keyName"]
        );
        assert_eq!(
            created.member("workspaceId"),
            workspace_id,
            "case {case:?}: the created key must report its workspace, got {:?}",
            created.body["workspaceId"]
        );

        let token = created.member("key").to_string();
        assert!(
            !token.is_empty() && token != name && token != workspace_id,
            "case {case:?}: the key token must be a minted secret, not echoed input, got \
             {token:?}"
        );
        assert!(
            !tokens.contains(&token),
            "case {case:?}: every key must get its own token, but {token:?} was minted twice"
        );
        tokens.push(token);

        let deleted = delete_key(&router, &workspace_id, &name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteWorkspaceApiKey, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            (deleted.member("keyName"), deleted.member("workspaceId")),
            (name.as_str(), workspace_id.as_str()),
            "case {case:?}: the delete must report the key and workspace it removed, got {:?}",
            deleted.body
        );

        let again = delete_key(&router, &workspace_id, &name).await;
        assert_eq!(
            (again.status, again.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: deleting a key the workspace no longer holds must answer a 404 \
             ResourceNotFoundException, got {} with {:?}",
            again.status,
            again.body
        );
        assert_eq!(
            again.member("resourceId"),
            name,
            "case {case:?}: the refusal must name the key it did not find, got {:?}",
            again.body
        );

        // The name was released by the delete, so it can be taken again.
        let remade = create_key(&router, &workspace_id, &name, role, seconds_to_live).await;
        assert_eq!(
            remade.status, 200,
            "case {case:?}: a deleted name is free again, got {} with {:?}",
            remade.status, remade.body
        );
        assert_eq!(
            delete_key(&router, &workspace_id, &name).await.status,
            200,
            "case {case:?}: the re-made key must be deletable too"
        );
    }
}

/// "Keynames must be unique to the workspace": a name a workspace already holds is
/// refused with `ConflictException`, and the same name on another workspace is not.
#[tokio::test]
async fn a_key_name_is_unique_within_one_workspace_only() {
    let router = emulator();
    let first = workspace(&router).await;
    let second = workspace(&router).await;

    // The rows differ in name shape and in how many times the name is re-taken.
    let cases: Vec<(&str, String, usize)> = vec![
        ("a plain name, taken twice", "metrics".to_string(), 1),
        ("the same name, retried three times", "retried".to_string(), 3),
        (
            "the longest name ApiKeyName allows",
            "n".repeat(MAX_KEY_NAME_LENGTH),
            2,
        ),
        (
            "the shortest name ApiKeyName allows",
            "n".repeat(MIN_KEY_NAME_LENGTH),
            1,
        ),
    ];

    for (case, name, retries) in cases {
        let created = create_key(&router, &first, &name, "ADMIN", 600).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first create must succeed, got {} with {:?}",
            created.status, created.body
        );
        let token = created.member("key").to_string();

        for retry in 0..retries {
            let refused = create_key(&router, &first, &name, "EDITOR", 900).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (409, "ConflictException"),
                "case {case:?}: retry {retry} on a taken name must answer a 409 \
                 ConflictException, got {} with {:?}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.member("resourceId"),
                name,
                "case {case:?}: the conflict must name the key, got {:?}",
                refused.body
            );
        }

        // Uniqueness is per workspace: the other workspace may hold the same name, and
        // its key is a different key with its own token.
        let elsewhere = create_key(&router, &second, &name, "VIEWER", 600).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {case:?}: another workspace may hold the same key name, got {} with {:?}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.member("key"),
            token.as_str(),
            "case {case:?}: two workspaces' keys of one name are two keys"
        );

        // And deleting one workspace's key leaves the other workspace's in place.
        assert_eq!(
            delete_key(&router, &first, &name).await.status,
            200,
            "case {case:?}: the first workspace's key must be deletable"
        );
        let survivor = delete_key(&router, &second, &name).await;
        assert_eq!(
            survivor.status, 200,
            "case {case:?}: deleting one workspace's key must not touch the other's, got {} \
             with {:?}",
            survivor.status, survivor.body
        );
    }
}

/// `keyRole` admits exactly the values the `Role` shape declares; anything else is a 400
/// `ValidationException` that persists nothing.
#[tokio::test]
async fn a_key_role_outside_the_documented_set_is_refused() {
    let router = emulator();
    let workspace_id = workspace(&router).await;

    for role in KEY_ROLES {
        let accepted = create_key(&router, &workspace_id, role, role, 600).await;
        assert_eq!(
            accepted.status, 200,
            "role {role:?} is one the model declares and must be accepted, got {} with {:?}",
            accepted.status, accepted.body
        );
    }

    // Rows differ along case, spacing, emptiness and plausible-but-absent roles.
    let refused_roles = [
        "admin",
        "Editor",
        "VIEWER ",
        "",
        "SUPERADMIN",
        "ADMIN,EDITOR",
        "Grafana Admin",
    ];
    for role in refused_roles {
        let refused = create_key(&router, &workspace_id, "refused-role", role, 600).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "ValidationException"),
            "role {role:?} is outside the documented set and must answer a 400 \
             ValidationException, got {} with {:?}",
            refused.status,
            refused.body
        );
        // Nothing was stored, so there is no key of that name to delete.
        let absent = delete_key(&router, &workspace_id, "refused-role").await;
        assert_eq!(
            absent.status, 404,
            "role {role:?}: a refused create must persist nothing, got {} with {:?}",
            absent.status, absent.body
        );
    }
}

/// A key belongs to one workspace: an id naming no workspace holds no keys, and deleting
/// the workspace forgets the keys it held.
#[tokio::test]
async fn keys_exist_only_on_a_workspace_that_exists() {
    let router = emulator();
    let workspace_id = workspace(&router).await;
    assert_eq!(
        create_key(&router, &workspace_id, "doomed", "ADMIN", 600)
            .await
            .status,
        200,
        "the key must be created while the workspace exists"
    );

    // An id that satisfies `WorkspaceId`'s ^g-[0-9a-f]{10}$ but names no workspace, and
    // the id of a workspace that has been deleted: both hold no keys at all.
    let deleted = workspace(&router).await;
    assert_eq!(
        create_key(&router, &deleted, "short-lived", "EDITOR", 600)
            .await
            .status,
        200,
        "the second workspace's key must be created while it exists"
    );
    assert_eq!(
        call(&router, "DELETE", &format!("/workspaces/{deleted}"), None)
            .await
            .status,
        202,
        "DeleteWorkspace must accept the deletion"
    );
    // The delete is ACCEPTED, not finished: the workspace is removed from the store once
    // the removal the clock owes it falls due, and only then does its id name nothing.
    tokio::time::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200)).await;

    let cases = [
        ("an id naming no workspace", "g-0123456789", "absent"),
        ("a workspace that was deleted", deleted.as_str(), "short-lived"),
    ];
    for (case, id, key_name) in cases {
        let created = create_key(&router, id, key_name, "ADMIN", 600).await;
        assert_eq!(
            (created.status, created.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: creating a key on a workspace that does not exist must answer a \
             404 ResourceNotFoundException, got {} with {:?}",
            created.status,
            created.body
        );
        assert_eq!(
            created.member("resourceId"),
            id,
            "case {case:?}: the refusal must name the workspace, got {:?}",
            created.body
        );

        let removed = delete_key(&router, id, key_name).await;
        assert_eq!(
            (removed.status, removed.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: deleting a key on a workspace that does not exist must answer a \
             404 ResourceNotFoundException, got {} with {:?}",
            removed.status,
            removed.body
        );
    }

    // The surviving workspace still holds its own key.
    assert_eq!(
        delete_key(&router, &workspace_id, "doomed").await.status,
        200,
        "the surviving workspace's key must be untouched by the other workspace's deletion"
    );
}

/// `secondsToLive` is "the time in seconds until the key expires": once it has run out the
/// workspace no longer holds the key — without any call in between — while a key with a
/// longer lifetime is still there.
#[tokio::test]
async fn a_key_is_gone_once_its_seconds_to_live_have_run_out() {
    let router = emulator();
    let workspace_id = workspace(&router).await;

    assert_eq!(
        create_key(&router, &workspace_id, "expiring", "ADMIN", MIN_SECONDS_TO_LIVE)
            .await
            .status,
        200,
        "the expiring key must be created"
    );
    assert_eq!(
        create_key(&router, &workspace_id, "long-lived", "ADMIN", MAX_SECONDS_TO_LIVE)
            .await
            .status,
        200,
        "the long-lived key must be created"
    );

    // The expiry is owed by the clock, not by a call: nothing is done here but wait.
    std::thread::sleep(Duration::from_secs(MIN_SECONDS_TO_LIVE as u64) + Duration::from_millis(200));

    let expired = delete_key(&router, &workspace_id, "expiring").await;
    assert_eq!(
        (expired.status, expired.error_code()),
        (404, "ResourceNotFoundException"),
        "a key past its secondsToLive is no longer held by the workspace, got {} with {:?}",
        expired.status,
        expired.body
    );

    // Its name is free, and the key whose lifetime has not run out is still there.
    assert_eq!(
        create_key(&router, &workspace_id, "expiring", "ADMIN", MAX_SECONDS_TO_LIVE)
            .await
            .status,
        200,
        "an expired key releases its name"
    );
    assert_eq!(
        delete_key(&router, &workspace_id, "long-lived").await.status,
        200,
        "a key whose secondsToLive has not run out must still be held"
    );
}

/// The per-workspace key store is bounded: at the bound a create answers the modelled
/// `ServiceQuotaExceededException`, and deleting one key makes room for exactly one more.
#[tokio::test]
async fn a_create_past_the_key_quota_is_refused() {
    let router = emulator();
    let workspace_id = workspace(&router).await;

    for index in 0..API_KEY_QUOTA {
        let role = KEY_ROLES[index % KEY_ROLES.len()];
        let answer = create_key(&router, &workspace_id, &format!("key-{index}"), role, 600).await;
        assert_eq!(
            answer.status, 200,
            "key {index} is inside the quota and must be created, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let refused = create_key(&router, &workspace_id, "one-too-many", "ADMIN", 600).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (402, "ServiceQuotaExceededException"),
        "a create past the quota must answer a 402 ServiceQuotaExceededException, got {} \
         with {:?}",
        refused.status,
        refused.body
    );

    assert_eq!(
        delete_key(&router, &workspace_id, "key-0").await.status,
        200,
        "deleting a key must succeed"
    );
    let readmitted = create_key(&router, &workspace_id, "one-too-many", "ADMIN", 600).await;
    assert_eq!(
        readmitted.status, 200,
        "deleting one key frees room for exactly one more, got {} with {:?}",
        readmitted.status, readmitted.body
    );
    let refused_again = create_key(&router, &workspace_id, "two-too-many", "ADMIN", 600).await;
    assert_eq!(
        refused_again.status, 402,
        "and only one: the next create is refused again, got {} with {:?}",
        refused_again.status, refused_again.body
    );
}
