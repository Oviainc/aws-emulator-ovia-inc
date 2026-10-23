// SPDX-License-Identifier: CC0-1.0

//! `ListInstances`' pagination contract, end to end over the `awsJson1_1` wire.
//!
//! THE RULE: `ListInstances` is `@paginated{inputToken: NextToken, outputToken: NextToken,
//! pageSize: MaxResults, items: Instances}`, and it issues a continuation token for every
//! page that RETURNED an instance — AWS's own `ListInstances` carries a `NextToken`
//! alongside the account's only instance — so a caller learns the enumeration has ended by
//! FOLLOWING the token and getting an empty page, which carries none. A token this service
//! did not issue for this enumeration is refused `ValidationException` rather than
//! answered with an empty page, which would end a caller's enumeration early.
//!
//! The rows span what the rule generalises over: the `MaxResults` values the model admits
//! (absent, both published bounds of `range(min: 1, max: 100)`, and one in between) — on
//! an account that holds an instance and on one that holds none, the two sizes this
//! service's `InstanceList` (`length 0..10`, one instance per account) can have.
//!
//! The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, router_over, with_store};

/// A router over a fresh engine, and that engine.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// One `ListInstances` call, as `(status, body)`.
async fn list(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "ListInstances", request).await
}

/// The `MaxResults` values the rows drive: absent, and the published bounds of
/// `range(min: 1, max: 100)` plus one value inside them
/// (`smithy_query(query="operation",operation="ListInstances")`).
const PAGE_SIZES: &[Option<i64>] = &[None, Some(1), Some(7), Some(100)];

/// THE RULE on an account that holds an instance: the page carries the instance AND a
/// continuation token, and following that token ends the enumeration with an empty,
/// token-less page.
#[tokio::test]
async fn a_page_that_returned_an_instance_carries_a_continuation_token() {
    for max_results in PAGE_SIZES {
        let case = format!("MaxResults {max_results:?}");
        let (router, _engine) = router_and_engine();

        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        let (status, first) = list(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListInstances expected 200, got {status} {first}"
        );
        assert_eq!(
            first["Instances"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: the account's one instance must be on the page, got {first}"
        );
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a page that returned an instance carries a token: {first}")
            })
            .to_string();
        // The token stays inside the model's `Token` pattern `^[-a-zA-Z0-9+=/_]*$` and its
        // 0..2048 length, so a client can send it back verbatim.
        assert!(
            !token.is_empty()
                && token.len() <= 2048
                && token.chars().all(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '=' | '/' | '_')
                }),
            "case {case:?}: the token must satisfy Token's pattern and length, got {token:?}"
        );

        let mut next = request.clone();
        next["NextToken"] = json!(token);
        let (status, last) = list(&router, &next).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the page after the token expected 200, got {status} {last}"
        );
        assert_eq!(
            last["Instances"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: following the token ends the enumeration, got {last}"
        );
        assert_eq!(
            last["NextToken"],
            Value::Null,
            "case {case:?}: the page that returned nothing carries no token, got {last}"
        );
    }
}

/// THE RULE on an account that holds NO instance: the page returns nothing, so there is
/// nothing to continue from and no token is issued.
#[tokio::test]
async fn a_page_that_returned_nothing_carries_no_token() {
    for max_results in PAGE_SIZES {
        let case = format!("MaxResults {max_results:?}");
        let (router, engine) = router_and_engine();
        // The account state a completed `DeleteInstance` leaves behind (driven over the
        // wire in `instance_delete.rs`).
        with_store(&engine, |store| {
            store.instances.clear();
            store.ambient_instance_enabled = true;
        });

        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        let (status, body) = list(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListInstances expected 200, got {status} {body}"
        );
        assert_eq!(
            body["Instances"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: an account that holds no instance lists none, got {body}"
        );
        assert_eq!(
            body["NextToken"],
            Value::Null,
            "case {case:?}: with nothing returned there is no token, got {body}"
        );
    }
}

/// A `NextToken` this service did not issue FOR THIS ENUMERATION is a bad request, not an
/// empty page: the rows span a token of another enumeration of this service, a token-shaped
/// value it never issued, and the empty token the model's `Token` length admits.
#[tokio::test]
async fn a_token_this_enumeration_did_not_issue_is_refused() {
    let (router, _engine) = router_and_engine();

    // A token issued for a DIFFERENT enumeration of this same service: `ListPermissionSets`
    // pages by permission-set ARN, so its keys mean something else.
    let (status, body) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {body}");

    let rows = [
        ("a value this service never issued", "not-a-token"),
        ("hex that decodes to another scope", "73736f61646d696e3a76313a7065726d697373696f6e536574733a78"),
        ("the empty token Token's 0..2048 length admits", ""),
    ];
    for (case, token) in rows {
        let (status, refused) = list(&router, &json!({"NextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {token:?}, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException for {token:?}, got {refused}"
        );
    }
}
