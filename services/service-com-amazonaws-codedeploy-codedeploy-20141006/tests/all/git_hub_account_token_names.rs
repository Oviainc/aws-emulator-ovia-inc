// SPDX-License-Identifier: CC0-1.0

//! `ListGitHubAccountTokenNames`, driven over the wire.
//!
//! RULE under test: the operation answers the names of the GitHub account
//! connections THIS STORE holds, in link order, one page at a time — so an account
//! that has linked none answers an empty list, a `nextToken` continues the listing
//! rather than restarting it, and a token this service did not issue is refused with
//! `InvalidNextTokenException` (HTTP 400).
//!
//! CodeDeploy's API models no operation that CREATES a GitHub account connection
//! (the link is made by the GitHub OAuth handshake the CodeDeploy console drives),
//! so these cases seed the connection names straight into the shared store — the
//! same store `with_state` hands every handler — and then assert the response is
//! computed from it. That is also why this suite keeps its own harness instead of
//! the lifecycle suite's: it needs the engine handle, not only the router.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codedeploy_codedeploy_20141006::state::{
    ComAmazonawsCodedeployCodedeploy20141006State as CodeDeployState, PAGE_SIZE,
};

/// One emulator with one CodeDeploy service mounted on one shared engine, keeping
/// the engine handle so a case can seed the store a console handshake would fill.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codedeploy_codedeploy_20141006::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("CodeDeploy_20141006.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` and require success, answering the response body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {parsed:?}");
        parsed
    }

    /// Put `names` in the store as this account's linked GitHub connections.
    fn link_connections(&self, names: &[String]) {
        self.engine
            .state::<CodeDeployState>()
            .lock()
            .git_hub_account_token_names = names.to_vec();
    }

    /// The connection names the store holds right now.
    fn stored_connections(&self) -> Vec<String> {
        self.engine
            .state::<CodeDeployState>()
            .lock()
            .git_hub_account_token_names
            .clone()
    }

    /// Walk every page of the listing, answering the names collected in order and
    /// how many pages it took.
    async fn list_all(&self, case: &str) -> (Vec<String>, usize) {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let request = match &token {
                Some(token) => json!({"nextToken": token}),
                None => json!({}),
            };
            let page = self.ok("ListGitHubAccountTokenNames", request).await;
            let names = page["tokenNameList"].as_array().cloned();
            assert!(
                names.is_some(),
                "case {case}: expected a tokenNameList on page {pages}, got {page:?}"
            );
            let names = names.unwrap_or_default();
            assert!(
                names.len() <= PAGE_SIZE,
                "case {case}: expected at most {PAGE_SIZE} names on page {pages}, got {}",
                names.len()
            );
            collected.extend(names.iter().filter_map(|name| name.as_str().map(str::to_string)));
            pages += 1;
            token = page["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(pages < 16, "case {case}: paging did not terminate");
        }
        (collected, pages)
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// `count` generated connection names, in link order.
fn generated(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("gh-conn-{index:04}")).collect()
}

/// RULE: the listing reports exactly the connection names the store holds, in link
/// order, split into pages of at most [`PAGE_SIZE`], with a `nextToken` only while
/// names remain.
#[tokio::test]
async fn the_listing_reports_the_stored_connection_names() {
    // Rows differ along HOW MANY connections the store holds — none, one, several,
    // exactly one page, one past a page, and several pages — because that is what
    // the paging rule generalises over.
    let counts = [0usize, 1, 7, PAGE_SIZE - 1, PAGE_SIZE, PAGE_SIZE + 1, 2 * PAGE_SIZE + 3];

    for count in counts {
        let case = format!("{count} linked connections");
        let emulator = Emulator::new();
        let seeded = generated(count);
        emulator.link_connections(&seeded);

        let (listed, pages) = emulator.list_all(&case).await;
        assert_eq!(
            listed, seeded,
            "case {case}: expected every stored name once, in link order, got {listed:?}"
        );
        let expected_pages = count.div_ceil(PAGE_SIZE).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} page(s), got {pages}"
        );
        // A listing is a READ: the store a later caller sees is untouched.
        assert_eq!(
            emulator.stored_connections(),
            seeded,
            "case {case}: listing must not change the stored connections"
        );
    }
}

/// RULE: the names come back verbatim, whatever their form — the listing stores and
/// reports the name the connection was linked under, it does not normalise it.
#[tokio::test]
async fn connection_names_come_back_verbatim() {
    // Rows differ along the FORM of the name. `GitHubAccountTokenName`
    // (`smithy_query(query="shape",shape="GitHubAccountTokenName")`) is a plain
    // string with no length, pattern or enum constraint, so every one of these is
    // a name the model admits.
    let seeded: Vec<String> = vec![
        "a".to_string(),
        "MyGitHubConnection".to_string(),
        "my-git_hub.connection".to_string(),
        "UPPER lower 0123".to_string(),
        "連携".to_string(),
        "z".repeat(256),
    ];
    let emulator = Emulator::new();
    emulator.link_connections(&seeded);

    let (listed, pages) = emulator.list_all("mixed name forms").await;
    assert_eq!(pages, 1, "case mixed name forms: expected one page, got {pages}");
    assert_eq!(
        listed, seeded,
        "case mixed name forms: expected the names verbatim, got {listed:?}"
    );
}

/// RULE: a `nextToken` the service did not issue — or one that no longer names a
/// stored connection — is refused with `InvalidNextTokenException` (HTTP 400),
/// never answered with a restarted listing.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    emulator.link_connections(&generated(PAGE_SIZE + 5));

    // A token this service DID issue, captured so the stale-token row below can
    // replay it after the name it continues from is gone.
    let first_page = emulator.ok("ListGitHubAccountTokenNames", json!({})).await;
    let issued = first_page["nextToken"]
        .as_str()
        .expect("a full first page carries a nextToken")
        .to_string();

    // Rows differ along the FORM of the bad token: not this service's shape, the
    // tag alone, an undecodable payload, empty, and a token whose item is gone.
    let bogus = ["not-a-token", "p", "pzzz-", "", "p6e6f742d7374", issued.as_str()];
    // Only the last row needs the store emptied; do it after the issued token was
    // captured so every row runs against the same handler.
    for (index, token) in bogus.iter().enumerate() {
        if index + 1 == bogus.len() {
            emulator.link_connections(&[]);
        }
        let (status, body) = emulator
            .call("ListGitHubAccountTokenNames", json!({"nextToken": token}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNextTokenException"),
            "case token {token:?}: expected 400 InvalidNextTokenException, got {status} {body:?}"
        );
    }
}

/// RULE: an account that has linked no GitHub connection answers an empty list and
/// no `nextToken` — the recorded behaviour of a fresh account — and says so again on
/// a second call.
#[tokio::test]
async fn an_account_with_no_linked_connection_answers_an_empty_list() {
    let emulator = Emulator::new();
    for attempt in 1..=2 {
        let body = emulator.ok("ListGitHubAccountTokenNames", json!({})).await;
        assert_eq!(
            body["tokenNameList"],
            json!([]),
            "case attempt {attempt}: expected an empty tokenNameList, got {body:?}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case attempt {attempt}: expected no nextToken, got {body:?}"
        );
    }
    assert!(
        emulator.stored_connections().is_empty(),
        "case empty store: listing must not invent a connection, got {:?}",
        emulator.stored_connections()
    );
}
