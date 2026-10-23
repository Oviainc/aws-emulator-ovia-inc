// SPDX-License-Identifier: CC0-1.0

//! `CreateWorkspaceServiceAccountToken` and `ListWorkspaceServiceAccountTokens`, end to
//! end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: a create mints exactly one token on the service
//! account the request names, stores it there with that name, an id unique within the
//! account and an expiry `secondsToLive` after the instant it was created, and hands the
//! caller a key once that nothing else reports; the paginated read serves those stored
//! records in mint order; a request naming no such workspace or service account, or a
//! workspace below the documented Grafana floor, stores nothing and is refused with the
//! modelled error.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::{
    service_state, ServiceAccountToken, SERVICE_ACCOUNT_TOKEN_PAGE_LIMIT,
    SERVICE_ACCOUNT_TOKEN_QUOTA,
};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `ServiceAccountTokenName`'s published bounds. Source: `smithy_query(query="shape",
/// shape="ServiceAccountTokenName")` — `length: {min: 1, max: 128}`.
const NAME_MIN_LEN: usize = 1;
const NAME_MAX_LEN: usize = 128;

/// `CreateWorkspaceServiceAccountTokenRequest$secondsToLive`'s published bounds.
/// Source: `smithy_query(query="operation",
/// operation="CreateWorkspaceServiceAccountToken")` — `secondsToLive` carries
/// `range: {min: 1, max: 2592000}` (30 days).
const SECONDS_TO_LIVE_MIN: i64 = 1;
const SECONDS_TO_LIVE_MAX: i64 = 2_592_000;

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

    /// The created token's `id`, `name` and `key`, as reported.
    fn token(&self) -> (String, String, String) {
        let token = &self.body["serviceAccountToken"];
        (
            token["id"].as_str().unwrap_or_default().to_string(),
            token["name"].as_str().unwrap_or_default().to_string(),
            token["key"].as_str().unwrap_or_default().to_string(),
        )
    }

    /// The reported token list, as `(id, name)` pairs in the order served.
    fn listed(&self) -> Vec<(String, String)> {
        self.body["serviceAccountTokens"]
            .as_array()
            .map(|tokens| {
                tokens
                    .iter()
                    .map(|token| {
                        (
                            token["id"].as_str().unwrap_or_default().to_string(),
                            token["name"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
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

/// A workspace to hang service accounts off, on `version` when one is named.
async fn workspace_on(emulator: &Emulator, version: Option<&str>) -> String {
    let mut request = json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "CUSTOMER_MANAGED",
        "authenticationProviders": ["AWS_SSO"],
    });
    if let Some(version) = version {
        request["grafanaVersion"] = json!(version);
    }
    let created = call(&emulator.router, "POST", "/workspaces", Some(request)).await;
    assert_eq!(
        created.status, 202,
        "the workspace this case needs must be created, got {} with {:?}",
        created.status, created.body
    );
    created.body["workspace"]["id"]
        .as_str()
        .expect("CreateWorkspace answers an id")
        .to_string()
}

/// A workspace and a service account on it, both ready to hold tokens.
async fn workspace_with_account(emulator: &Emulator) -> (String, String) {
    let workspace_id = workspace_on(emulator, None).await;
    let created = call(
        &emulator.router,
        "POST",
        &format!("/workspaces/{workspace_id}/serviceaccounts"),
        Some(json!({"name": "automation", "grafanaRole": "ADMIN"})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the service account this case needs must be created, got {} with {:?}",
        created.status, created.body
    );
    let account_id = created
        .body["id"]
        .as_str()
        .expect("CreateWorkspaceServiceAccount answers an id")
        .to_string();
    (workspace_id, account_id)
}

async fn create_token(
    emulator: &Emulator,
    workspace_id: &str,
    account_id: &str,
    name: &str,
    seconds_to_live: i64,
) -> Answer {
    call(
        &emulator.router,
        "POST",
        &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens"),
        Some(json!({"name": name, "secondsToLive": seconds_to_live})),
    )
    .await
}

async fn list_tokens(
    emulator: &Emulator,
    workspace_id: &str,
    account_id: &str,
    query: &str,
) -> Answer {
    call(
        &emulator.router,
        "GET",
        &format!("/workspaces/{workspace_id}/serviceaccounts/{account_id}/tokens{query}"),
        None,
    )
    .await
}

/// The tokens the shared store holds for one service account, as persisted.
fn stored_tokens(emulator: &Emulator, workspace_id: &str, account_id: &str) -> Vec<ServiceAccountToken> {
    let state = service_state(&*emulator.engine).expect("the grafana store is reachable");
    let store = state.lock();
    store
        .workspaces
        .get(workspace_id)
        .and_then(|workspace| workspace.service_account(account_id))
        .map(|account| account.tokens.clone())
        .unwrap_or_default()
}

/// A create stores the token it reports, and the read serves it back.
///
/// The rows differ along what the rule generalises over: the form and size of the name
/// (one character, the maximum length, punctuation, spaces, non-ASCII) and the lifetime
/// (both published bounds of `secondsToLive` and values between them).
#[tokio::test]
async fn a_create_stores_the_token_it_reports() {
    let max_length_name = "t".repeat(NAME_MAX_LEN);
    let cases: [(&str, &str, i64); 6] = [
        ("single-character name, the shortest lifetime", "a", SECONDS_TO_LIVE_MIN),
        ("dotted name, one minute", "ci.runner", 60),
        ("name at the maximum length, one hour", &max_length_name, 3_600),
        ("name carrying spaces, one day", "nightly reporting job", 86_400),
        ("non-ASCII name, the longest lifetime", "обзор-токен", SECONDS_TO_LIVE_MAX),
        ("name carrying digits, a week", "dashboards-2026", 604_800),
    ];

    let emulator = emulator();
    let (workspace_id, account_id) = workspace_with_account(&emulator).await;
    let mut minted_ids: Vec<String> = Vec::new();
    let mut minted_keys: Vec<String> = Vec::new();

    for (case, name, seconds_to_live) in cases {
        let created = create_token(&emulator, &workspace_id, &account_id, name, seconds_to_live).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.string("workspaceId"),
            workspace_id,
            "case {case:?}: the answer must report the workspace it stored into, got {:?}",
            created.body
        );
        assert_eq!(
            created.string("serviceAccountId"),
            account_id,
            "case {case:?}: the answer must report the account it stored into, got {:?}",
            created.body
        );

        let (id, reported_name, key) = created.token();
        assert_eq!(
            reported_name, name,
            "case {case:?}: the answer must report the stored name, got {:?}",
            created.body
        );
        assert!(
            !id.is_empty(),
            "case {case:?}: a create must mint a token id, got {:?}",
            created.body
        );
        assert!(
            !minted_ids.contains(&id),
            "case {case:?}: id {id:?} was already minted on this account: {minted_ids:?}"
        );
        assert!(
            !key.is_empty(),
            "case {case:?}: a create must hand the caller a key, got {:?}",
            created.body
        );
        assert!(
            !minted_keys.contains(&key),
            "case {case:?}: the key handed out repeats an earlier one"
        );
        minted_ids.push(id.clone());
        minted_keys.push(key.clone());

        // Read back through the store: exactly the record the answer described, with
        // the lifetime the request asked for and no key kept anywhere.
        let tokens = stored_tokens(&emulator, &workspace_id, &account_id);
        let stored = tokens
            .iter()
            .find(|token| token.id == id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the created token {id:?} is not in the store: {tokens:?}")
            });
        assert_eq!(
            stored.name, name,
            "case {case:?}: the stored record must carry the created name, got {stored:?}"
        );
        assert_eq!(
            stored.expires.duration_since(stored.created).ok(),
            Some(Duration::from_secs(
                u64::try_from(seconds_to_live).expect("a positive lifetime fits in u64")
            )),
            "case {case:?}: the token must expire secondsToLive after it was created, \
             got {stored:?}"
        );
        assert!(
            stored.last_used.is_none(),
            "case {case:?}: a token nothing authorized with has no lastUsedAt, got {stored:?}"
        );

        // Read back through the API: the paginated read serves the same record, and
        // never the key.
        let listed = list_tokens(&emulator, &workspace_id, &account_id, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the read must answer HTTP 200, got {} with {:?}",
            listed.status, listed.body
        );
        let reported = listed.listed();
        assert!(
            reported.contains(&(id.clone(), name.to_string())),
            "case {case:?}: the read must report the created token, got {reported:?}"
        );
        let body = listed.body.to_string();
        assert!(
            !body.contains(&key),
            "case {case:?}: no later call may report the key; the read answered {body:?}"
        );
    }

    let tokens = stored_tokens(&emulator, &workspace_id, &account_id);
    assert_eq!(
        tokens.len(),
        minted_ids.len(),
        "each create stores exactly one token, got {tokens:?}"
    );
    let listed = list_tokens(&emulator, &workspace_id, &account_id, "").await;
    assert_eq!(
        listed.listed().iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
        minted_ids,
        "the read serves the stored tokens in the order they were minted, got {:?}",
        listed.body
    );
}

/// A reported token carries the instants it was created and expires, `secondsToLive`
/// apart, and no `lastUsedAt` until something authorizes with it.
#[tokio::test]
async fn a_reported_token_carries_its_lifetime() {
    // The rows differ along the lifetime, over the whole published range.
    let cases: [(&str, i64); 4] = [
        ("the shortest lifetime the model admits", SECONDS_TO_LIVE_MIN),
        ("ten minutes", 600),
        ("thirty days less a second", SECONDS_TO_LIVE_MAX - 1),
        ("the longest lifetime the model admits", SECONDS_TO_LIVE_MAX),
    ];

    let emulator = emulator();
    let (workspace_id, account_id) = workspace_with_account(&emulator).await;

    for (index, (case, seconds_to_live)) in cases.into_iter().enumerate() {
        let name = format!("token-{index}");
        let created = create_token(&emulator, &workspace_id, &account_id, &name, seconds_to_live).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            created.status, created.body
        );
        let (id, _, _) = created.token();

        let listed = list_tokens(&emulator, &workspace_id, &account_id, "").await;
        let reported = listed.body["serviceAccountTokens"]
            .as_array()
            .and_then(|tokens| {
                tokens
                    .iter()
                    .find(|token| token["id"].as_str() == Some(id.as_str()))
                    .cloned()
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: the read must report token {id:?}, got {:?}", listed.body)
            });

        let created_at = reported["createdAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: createdAt must be reported, got {reported:?}"));
        let expires_at = reported["expiresAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expiresAt must be reported, got {reported:?}"));
        let lifetime = expires_at - created_at;
        assert!(
            (lifetime - seconds_to_live as f64).abs() < 1.0,
            "case {case:?}: expected a lifetime of {seconds_to_live} seconds, got {lifetime} \
             from {reported:?}"
        );
        assert!(
            reported.get("lastUsedAt").is_none_or(Value::is_null),
            "case {case:?}: a token nothing authorized with reports no lastUsedAt, got \
             {reported:?}"
        );
    }
}

/// The read is paginated over the mint order: every page size serves the same sequence,
/// and a `nextToken` this service never issued is refused.
#[tokio::test]
async fn the_read_pages_over_the_stored_tokens() {
    let emulator = emulator();
    let (workspace_id, account_id) = workspace_with_account(&emulator).await;

    let mut minted = Vec::new();
    for index in 0..5 {
        let created =
            create_token(&emulator, &workspace_id, &account_id, &format!("token-{index}"), 3_600)
                .await;
        assert_eq!(
            created.status, 200,
            "create {index} must be accepted, got {} with {:?}",
            created.status, created.body
        );
        minted.push(created.token().0);
    }

    // The rows differ along the page size: one at a time, a page that divides the set,
    // one that does not, the whole set, and more than the set holds.
    for (case, page_size) in [
        ("one token per page", 1),
        ("a page smaller than the set", 2),
        ("a page that does not divide the set", 3),
        ("exactly the set", 5),
        ("a page larger than the set", 100),
    ] {
        let mut served: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("?maxResults={page_size}");
            if let Some(token) = &next_token {
                query.push_str(&format!("&nextToken={token}"));
            }
            let page = list_tokens(&emulator, &workspace_id, &account_id, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: a page must answer HTTP 200, got {} with {:?}",
                page.status, page.body
            );
            let ids: Vec<String> = page.listed().into_iter().map(|(id, _)| id).collect();
            assert!(
                ids.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} tokens, got {ids:?}"
            );
            served.extend(ids);
            next_token = page.body["nextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= minted.len() + 1,
                "case {case:?}: pagination must terminate, served {served:?}"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            served, minted,
            "case {case:?}: paging must serve every stored token exactly once, in mint order"
        );
    }

    // A `nextToken` no page of this operation issued is a value it cannot honour.
    for (case, token) in [
        ("a token that is not a number", "not-a-token"),
        ("an empty token", ""),
        ("a negative number", "-1"),
        ("a workspace id", workspace_id.as_str()),
    ] {
        let refused =
            list_tokens(&emulator, &workspace_id, &account_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for nextToken {token:?}, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    assert_eq!(
        SERVICE_ACCOUNT_TOKEN_PAGE_LIMIT, 100,
        "the model bounds maxResults at 100, which is the page this read serves by default"
    );
}

/// A request naming no workspace, no service account of it, or a workspace below the
/// documented Grafana floor stores nothing and is refused with the modelled error.
#[tokio::test]
async fn a_create_against_no_such_resource_is_refused() {
    let emulator = emulator();
    let (workspace_id, account_id) = workspace_with_account(&emulator).await;
    let (other_workspace, other_account) = workspace_with_account(&emulator).await;
    // Account ids are minted per workspace, so a SECOND account on the other workspace
    // carries an id the first workspace holds no account under: that is what makes this
    // a cross-workspace case rather than a repeat of the same id.
    let elsewhere = call(
        &emulator.router,
        "POST",
        &format!("/workspaces/{other_workspace}/serviceaccounts"),
        Some(json!({"name": "second", "grafanaRole": "VIEWER"})),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "the second account this case needs must be created, got {} with {:?}",
        elsewhere.status, elsewhere.body
    );
    let account_elsewhere = elsewhere.string("id");
    assert_ne!(
        account_elsewhere, account_id,
        "this case needs an account id the first workspace does not hold"
    );
    // A workspace below the floor holds no service account at all, so an account id
    // against it names nothing.
    let old_workspace = workspace_on(&emulator, Some("8.4")).await;

    // (case, workspaceId, serviceAccountId, expected create status, expected wire error
    // code, expected read status) — the read differs only where the refusal is about the
    // CREATE: a sub-floor workspace holds no service account at all, so a read of one is
    // a read of a resource that does not exist.
    let cases: [(&str, &str, &str, u16, &str, u16); 7] = [
        (
            "an id no workspace carries",
            "g-0123456789",
            account_id.as_str(),
            404,
            "ResourceNotFoundException",
            404,
        ),
        (
            "an account id this workspace does not hold",
            workspace_id.as_str(),
            "999999",
            404,
            "ResourceNotFoundException",
            404,
        ),
        (
            "an account of another workspace",
            workspace_id.as_str(),
            account_elsewhere.as_str(),
            404,
            "ResourceNotFoundException",
            404,
        ),
        (
            "an account id that is not an id this service mints",
            workspace_id.as_str(),
            "not-an-account",
            404,
            "ResourceNotFoundException",
            404,
        ),
        (
            "a workspace below the documented Grafana floor",
            old_workspace.as_str(),
            "1",
            400,
            "ValidationException",
            404,
        ),
        // `WorkspaceId` is `@pattern("^g-[0-9a-f]{10}$")`, so this never names a
        // workspace at all and is refused at the boundary.
        (
            "a workspace id outside the modelled pattern",
            "0123456789ab",
            account_id.as_str(),
            400,
            "ValidationException",
            400,
        ),
        (
            "a workspace id that is too short",
            "g-0123456",
            account_id.as_str(),
            400,
            "ValidationException",
            400,
        ),
    ];

    for (case, workspace, account, status, code, read_status) in cases {
        let refused = create_token(&emulator, workspace, account, "refused", 3_600).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            refused.body
        );
        assert!(
            stored_tokens(&emulator, &workspace_id, &account_id).is_empty(),
            "case {case:?}: a refused create must store nothing, got {:?}",
            stored_tokens(&emulator, &workspace_id, &account_id)
        );

        // The read refuses a request against a resource that does not exist too.
        let read = list_tokens(&emulator, workspace, account, "").await;
        assert_eq!(
            read.status, read_status,
            "case {case:?}: the read must answer HTTP {read_status}, got {} with {:?}",
            read.status, read.body
        );
    }

    assert!(
        stored_tokens(&emulator, &other_workspace, &other_account).is_empty(),
        "no refused create may land on another account, got {:?}",
        stored_tokens(&emulator, &other_workspace, &other_account)
    );
}

/// The modelled constraints on the request: `name`'s length bounds and
/// `secondsToLive`'s range, each bound and one past it.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let emulator = emulator();
    let (workspace_id, account_id) = workspace_with_account(&emulator).await;

    // (case, name, secondsToLive, expected status)
    let cases: [(&str, String, i64, u16); 6] = [
        (
            "one character below the minimum name length",
            "n".repeat(NAME_MIN_LEN - 1),
            3_600,
            400,
        ),
        ("the minimum name length", "n".repeat(NAME_MIN_LEN), 3_600, 200),
        ("the maximum name length", "m".repeat(NAME_MAX_LEN), 3_600, 200),
        (
            "one character past the maximum name length",
            "o".repeat(NAME_MAX_LEN + 1),
            3_600,
            400,
        ),
        ("a lifetime below the minimum", "ttl-low".to_string(), SECONDS_TO_LIVE_MIN - 1, 400),
        ("a lifetime past the maximum", "ttl-high".to_string(), SECONDS_TO_LIVE_MAX + 1, 400),
    ];

    for (case, name, seconds_to_live, status) in cases {
        let answer = create_token(&emulator, &workspace_id, &account_id, &name, seconds_to_live).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert!(
                stored_tokens(&emulator, &workspace_id, &account_id)
                    .iter()
                    .all(|token| token.name != name),
                "case {case:?}: a refused create must store nothing under {name:?}"
            );
        }
    }
}

/// The store is bounded: past the quota a create is refused rather than adding another
/// record, and the bound is per service account.
#[tokio::test]
async fn a_service_account_holds_a_bounded_number_of_tokens() {
    let emulator = emulator();
    let (workspace_id, account_id) = workspace_with_account(&emulator).await;

    for index in 0..SERVICE_ACCOUNT_TOKEN_QUOTA {
        let answer =
            create_token(&emulator, &workspace_id, &account_id, &format!("t-{index}"), 3_600).await;
        assert_eq!(
            answer.status, 200,
            "create {index} of {SERVICE_ACCOUNT_TOKEN_QUOTA} must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let refused =
        create_token(&emulator, &workspace_id, &account_id, "one-too-many", 3_600).await;
    assert_eq!(
        refused.status, 402,
        "a create past the quota must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {:?}",
        refused.body
    );
    assert_eq!(
        stored_tokens(&emulator, &workspace_id, &account_id).len(),
        SERVICE_ACCOUNT_TOKEN_QUOTA,
        "the refused create must store nothing"
    );

    // Another service account is bounded on its own, not by this one.
    let (other_workspace, other_account) = workspace_with_account(&emulator).await;
    assert_eq!(
        create_token(&emulator, &other_workspace, &other_account, "one-too-many", 3_600)
            .await
            .status,
        200,
        "the quota is per service account, so another account still accepts a create"
    );
}
