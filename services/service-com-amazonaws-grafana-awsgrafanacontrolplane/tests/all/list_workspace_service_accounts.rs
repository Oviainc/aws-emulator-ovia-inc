// SPDX-License-Identifier: CC0-1.0

//! `ListWorkspaceServiceAccounts`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: a list reports exactly the service accounts the store holds for
//! the workspace the request names, in the order they were minted, projected as
//! `ServiceAccountSummary`, cut into pages of at most `maxResults` whose `nextToken`
//! names the account the following page starts at; a `workspaceId` naming no workspace
//! is a `ResourceNotFoundException` and a `nextToken` this operation never issued is a
//! `ValidationException`, and neither a list nor a page changes what a later caller
//! sees.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::{
    service_state, LIFECYCLE_TRANSITION, SERVICE_ACCOUNT_PAGE_LIMIT,
};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Every value of `Role`, the shape `CreateWorkspaceServiceAccountRequest$grafanaRole`
/// targets. Source: `smithy_query(query="shape", shape="Role")` — `values: ["ADMIN",
/// "EDITOR", "VIEWER"]`. Written out here so a role a create stores but a list drops
/// fails this test.
const EVERY_ROLE: [&str; 3] = ["ADMIN", "EDITOR", "VIEWER"];

/// The published bounds of `ListWorkspaceServiceAccountsRequest$maxResults`. Source:
/// `smithy_query(query="operation", operation="ListWorkspaceServiceAccounts")` —
/// `maxResults: {range: {min: 1, max: 100}}`.
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 100;

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

    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }

    /// The listed accounts as `(id, name, role, isDisabled)` rows.
    fn accounts(&self) -> Vec<(String, String, String, String)> {
        self.body["serviceAccounts"]
            .as_array()
            .unwrap_or_else(|| panic!("serviceAccounts is a required member: {:?}", self.body))
            .iter()
            .map(|account| {
                let field = |name: &str| account[name].as_str().unwrap_or_default().to_string();
                (
                    field("id"),
                    field("name"),
                    field("grafanaRole"),
                    field("isDisabled"),
                )
            })
            .collect()
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

async fn workspace(emulator: &Emulator) -> String {
    workspace_on(emulator, None).await
}

/// Create `name` as `role` on `workspace_id` and answer the minted id.
async fn create_account(emulator: &Emulator, workspace_id: &str, name: &str, role: &str) -> String {
    let created = call(
        &emulator.router,
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

/// `ListWorkspaceServiceAccounts` over HTTP, with the page parameters the case names.
async fn list(
    emulator: &Emulator,
    workspace_id: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    call(
        &emulator.router,
        "GET",
        &format!("/workspaces/{workspace_id}/serviceaccounts{suffix}"),
        None,
    )
    .await
}

/// The service accounts the shared store holds for `workspace_id`, as persisted:
/// `(id, name, role, isDisabled)` rows in the order they were minted.
fn stored_rows(emulator: &Emulator, workspace_id: &str) -> Vec<(String, String, String, String)> {
    let state = service_state(&*emulator.engine).expect("the grafana store is reachable");
    let store = state.lock();
    store
        .workspaces
        .get(workspace_id)
        .map(|workspace| {
            workspace
                .service_accounts
                .iter()
                .map(|account| {
                    (
                        account.id.clone(),
                        account.name.clone(),
                        account.grafana_role.clone(),
                        account.disabled.to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A list reports exactly the stored accounts of the named workspace, in mint order.
///
/// The rows differ along what the rule generalises over: how many accounts the
/// workspace holds (none, one, several), the roles they carry (every value `Role`
/// admits) and the form of their names.
#[tokio::test]
async fn a_list_reports_every_stored_account_in_mint_order() {
    // (case, the account names to create, in order)
    let cases: [(&str, &[&str]); 5] = [
        ("a workspace holding no account", &[]),
        ("one account", &["automation"]),
        (
            "several accounts, mixed name forms",
            &["a", "ops.automation", "nightly reporting", "обзор-сервис"],
        ),
        ("names that sort against their mint order", &["zz", "mm", "aa"]),
        (
            "more accounts than names sort distinctly",
            &["sa-1", "sa-2", "sa-3", "sa-4", "sa-5", "sa-6", "sa-7"],
        ),
    ];

    let emulator = emulator();
    for (case, names) in cases {
        let workspace_id = workspace(&emulator).await;
        let mut expected = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let role = EVERY_ROLE[index % EVERY_ROLE.len()];
            let id = create_account(&emulator, &workspace_id, name, role).await;
            expected.push((id, (*name).to_string(), role.to_string(), "false".to_string()));
        }

        let listed = list(&emulator, &workspace_id, None, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.string("workspaceId"),
            workspace_id,
            "case {case:?}: the list reports the workspace it read, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.accounts(),
            expected,
            "case {case:?}: the list must report the stored accounts in mint order, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.accounts(),
            stored_rows(&emulator, &workspace_id),
            "case {case:?}: the list must agree with the persisted records, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.next_token(),
            None,
            "case {case:?}: a page holding everything carries no nextToken, got {:?}",
            listed.body
        );

        // A read may not change what a later caller sees.
        let again = list(&emulator, &workspace_id, None, None).await;
        assert_eq!(
            (again.status, again.accounts(), again.next_token()),
            (listed.status, listed.accounts(), listed.next_token()),
            "case {case:?}: listing twice must answer the same, got {:?}",
            again.body
        );
    }
}

/// `maxResults` bounds a page and its `nextToken` resumes the ordering: walking the
/// pages yields every stored account exactly once, in mint order.
///
/// The rows differ along the account count and the page size, including the published
/// bounds of `maxResults` and a page size equal to, below and above the count.
#[tokio::test]
async fn pages_bounded_by_max_results_cover_every_account_once() {
    // (case, accounts to create, maxResults)
    let cases: [(&str, usize, Option<i64>); 8] = [
        ("one account at a time", 5, Some(1)),
        ("the published minimum page size, nothing to page", 0, Some(1)),
        ("a page size dividing the count", 6, Some(2)),
        ("a page size not dividing the count", 7, Some(3)),
        ("a page size equal to the count", 4, Some(4)),
        ("a page size above the count", 3, Some(10)),
        ("the published maximum page size", 12, Some(MAX_MAX_RESULTS)),
        ("no page size at all", 9, None),
    ];

    let emulator = emulator();
    for (case, count, max_results) in cases {
        let workspace_id = workspace(&emulator).await;
        for index in 0..count {
            let role = EVERY_ROLE[index % EVERY_ROLE.len()];
            create_account(&emulator, &workspace_id, &format!("sa-{index}"), role).await;
        }
        let stored = stored_rows(&emulator, &workspace_id);

        let mut walked = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let page = list(&emulator, &workspace_id, max_results, token.as_deref()).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: page {pages} expected HTTP 200, got {} with {:?}",
                page.status, page.body
            );
            let rows = page.accounts();
            let page_size = max_results.unwrap_or(SERVICE_ACCOUNT_PAGE_LIMIT.into());
            assert!(
                i64::try_from(rows.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: page {pages} holds {} rows, above maxResults {page_size}",
                rows.len()
            );
            walked.extend(rows);
            pages += 1;
            assert!(
                pages <= count + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match page.next_token() {
                None => break,
                Some(next) => {
                    assert!(
                        stored.iter().any(|(id, ..)| *id == next),
                        "case {case:?}: nextToken {next:?} must name a stored account, got {stored:?}"
                    );
                    token = Some(next);
                }
            }
        }

        assert_eq!(
            walked, stored,
            "case {case:?}: walking the pages must yield every stored account once, in \
             mint order"
        );
    }
}

/// A `maxResults` outside the published range is refused at the boundary: both bounds
/// are admitted and one past each is a `ValidationException`.
#[tokio::test]
async fn a_max_results_outside_the_published_range_is_refused() {
    let emulator = emulator();
    let workspace_id = workspace(&emulator).await;
    create_account(&emulator, &workspace_id, "automation", "ADMIN").await;

    // (case, maxResults, expected status)
    let cases: [(&str, i64, u16); 5] = [
        ("one below the published minimum", MIN_MAX_RESULTS - 1, 400),
        ("the published minimum", MIN_MAX_RESULTS, 200),
        ("inside the range", 7, 200),
        ("the published maximum", MAX_MAX_RESULTS, 200),
        ("one past the published maximum", MAX_MAX_RESULTS + 1, 400),
    ];

    for (case, max_results, status) in cases {
        let answer = list(&emulator, &workspace_id, Some(max_results), None).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} for maxResults {max_results}, got {} \
             with {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
        }
    }
}

/// A `nextToken` this operation never issued is a `ValidationException`; a token whose
/// account is gone still serves the accounts after it.
#[tokio::test]
async fn a_next_token_this_operation_never_issued_is_refused() {
    let emulator = emulator();
    let workspace_id = workspace(&emulator).await;
    let mut ids = Vec::new();
    for index in 0..4 {
        ids.push(create_account(&emulator, &workspace_id, &format!("sa-{index}"), "VIEWER").await);
    }

    // (case, the nextToken, expected status)
    let cases: [(&str, &str, u16); 7] = [
        ("a word", "not-a-token", 400),
        ("an id with a non-digit", "12a", 400),
        ("a workspace id", &workspace_id, 400),
        ("a signed number", "-1", 400),
        ("a decimal point", "1.0", 400),
        ("an id this workspace holds", &ids[1], 200),
        // An id beyond every account: a token the caller may still hold after the
        // accounts it named were removed. The page is empty, not refused.
        ("an id past every account", "999999", 200),
    ];

    for (case, token, status) in cases {
        let answer = list(&emulator, &workspace_id, None, Some(token)).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} for nextToken {token:?}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
        }
    }

    // Resuming at a token whose account was removed meanwhile still serves the rest.
    {
        let state = service_state(&*emulator.engine).expect("the grafana store is reachable");
        let mut store = state.lock();
        let held = store
            .workspaces
            .get_mut(&workspace_id)
            .expect("the workspace this case created is stored");
        held.service_accounts.retain(|account| account.id != ids[1]);
    }
    let resumed = list(&emulator, &workspace_id, None, Some(&ids[1])).await;
    assert_eq!(
        resumed.status, 200,
        "a token whose account is gone must still serve a page, got {} with {:?}",
        resumed.status, resumed.body
    );
    assert_eq!(
        resumed
            .accounts()
            .into_iter()
            .map(|(id, ..)| id)
            .collect::<Vec<_>>(),
        ids[2..].to_vec(),
        "the resumed page must hold the accounts after the token, got {:?}",
        resumed.body
    );
}

/// A list is scoped to the workspace the request names, and a `workspaceId` naming no
/// workspace is refused rather than answering an empty list.
#[tokio::test]
async fn a_list_is_scoped_to_the_workspace_it_names() {
    let emulator = emulator();
    let first = workspace(&emulator).await;
    let second = workspace(&emulator).await;
    let deleted = workspace(&emulator).await;
    create_account(&emulator, &first, "shared-name", "ADMIN").await;
    create_account(&emulator, &first, "only-in-first", "EDITOR").await;
    create_account(&emulator, &second, "shared-name", "VIEWER").await;
    create_account(&emulator, &deleted, "doomed", "ADMIN").await;
    assert_eq!(
        call(
            &emulator.router,
            "DELETE",
            &format!("/workspaces/{deleted}"),
            None
        )
        .await
        .status,
        202,
        "DeleteWorkspace must accept the delete this case needs"
    );
    // The delete is ACCEPTED, not finished: the workspace is removed from the store once
    // the removal the clock owes it falls due, and only then does its id name nothing.
    tokio::time::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200)).await;

    for (case, workspace_id, names) in [
        ("the workspace holding two accounts", &first, vec!["shared-name", "only-in-first"]),
        ("the workspace holding one", &second, vec!["shared-name"]),
    ] {
        let listed = list(&emulator, workspace_id, None, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed
                .accounts()
                .into_iter()
                .map(|(_, name, ..)| name)
                .collect::<Vec<_>>(),
            names,
            "case {case:?}: only this workspace's accounts are reported, got {:?}",
            listed.body
        );
    }

    // (case, the workspaceId, expected status, expected wire error code)
    let refused: [(&str, &str, u16, &str); 5] = [
        ("an id no workspace carries", "g-0123456789", 404, "ResourceNotFoundException"),
        ("the id of a deleted workspace", &deleted, 404, "ResourceNotFoundException"),
        // `WorkspaceId` is `@pattern("^g-[0-9a-f]{10}$")`, so these name no workspace
        // at all and are refused at the boundary.
        ("an id with no g- prefix", "0123456789ab", 400, "ValidationException"),
        ("an id that is too short", "g-0123456", 400, "ValidationException"),
        ("an id carrying non-hex digits", "g-0123456xyz", 400, "ValidationException"),
    ];

    for (case, workspace_id, status, code) in refused {
        let answer = list(&emulator, workspace_id, None, None).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} for {workspace_id:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answer.body
        );
    }
}

/// A workspace below the documented Grafana floor can hold no service account, so its
/// list is empty rather than refused — and one at or above the floor reports what it
/// holds.
#[tokio::test]
async fn a_workspace_below_the_grafana_floor_lists_nothing() {
    // (case, grafanaVersion, whether the workspace may hold accounts)
    let cases: [(&str, Option<&str>, bool); 5] = [
        ("one major below the floor", Some("8.4"), false),
        ("far below the floor", Some("6.7"), false),
        ("the floor itself", Some("9"), true),
        ("above the floor", Some("10.4"), true),
        ("whatever the service defaults to", None, true),
    ];

    let emulator = emulator();
    for (case, version, holds_accounts) in cases {
        let workspace_id = workspace_on(&emulator, version).await;
        if holds_accounts {
            create_account(&emulator, &workspace_id, "automation", "ADMIN").await;
        }

        let listed = list(&emulator, &workspace_id, None, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?} (version {version:?}): expected HTTP 200, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.accounts().len(),
            usize::from(holds_accounts),
            "case {case:?} (version {version:?}): the list must report what is stored, \
             got {:?}",
            listed.body
        );
    }
}
