// SPDX-License-Identifier: CC0-1.0

//! `ListAPIKeys`, end to end over the wire.
//!
//! The operation "retrieves a list of the API keys that you've defined for the
//! specified scope", so the RULE these suites assert is: the answer is the set of
//! API keys STORED under the scope the request names — each reported exactly as it
//! was stored (its key value, its token domains in their stored order, when it was
//! created and its version), in creation order, cut into pages of at most `Limit`
//! summaries that a `NextMarker` resumes — and the call REPORTS: it changes nothing
//! a later caller sees.
//!
//! Each suite asserts that over a TABLE spanning the class rather than over one
//! request: both scopes, key values from one character to the longest the model
//! allows, one, two and three token domains including the longest one allowed,
//! every page size, a scope holding no keys at all, and each refusal.
//!
//! `CreateAPIKey` is the one operation that mints a key, and these suites are about
//! the LISTING rather than the mint, so they seed the keys through the state API
//! ([`seed_api_key`], the same call that handler makes) — which is also how a case
//! pins a key value, a creation instant and a listing position the mint chooses for
//! itself — and then drive every assertion through the router's front door, the same
//! door an SDK uses. The mint and its round trip have their own suite
//! (`crate::api_key_minting`).

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    ApiKeyEntry, ComAmazonawsWafv2Awswaf20190729State, MAX_API_KEYS_PER_SCOPE, SCOPE_CLOUDFRONT,
    SCOPE_REGIONAL,
};

use crate::web_acl_lifecycle::Waf;

/// The operation under test, named once.
const OPERATION: &str = "ListAPIKeys";

/// The instant the seeded creation timestamps below are expressed relative to
/// (epoch seconds for 2026-01-01T00:00:00Z), so the suites never race a wall clock.
const BASE_SECS: i64 = 1_767_225_600;

/// The longest value `APIKey` allows
/// (`smithy_query(query="shape",shape="APIKey")` → `length 1..2048`).
const MAX_API_KEY_LENGTH: usize = 2048;

/// The longest value `TokenDomain` allows
/// (`smithy_query(query="shape",shape="TokenDomain")` → `length 1..253`, pattern
/// `^[\w\.\-/]+$`).
const MAX_TOKEN_DOMAIN_LENGTH: usize = 253;

/// The bounds of `PaginationLimit`, which `ListAPIKeys$Limit` targets
/// (`smithy_query(query="shape",shape="PaginationLimit")` → `range 1..100`).
const MIN_PAGE_LIMIT: i64 = 1;
/// See [`MIN_PAGE_LIMIT`].
const MAX_PAGE_LIMIT: i64 = 100;

/// Store an API key under `scope`, as the minting operation will, and answer the
/// entry as it was stored. The store refuses a key past
/// [`MAX_API_KEYS_PER_SCOPE`], which no case here means to reach.
fn seed_api_key(
    engine: &engine::InMemoryEngine,
    scope: &str,
    api_key: &str,
    token_domains: &[&str],
    created_at_millis: i64,
) -> ApiKeyEntry {
    engine
        .state::<ComAmazonawsWafv2Awswaf20190729State>()
        .lock()
        .insert_api_key(
            scope,
            api_key,
            token_domains.iter().map(|domain| domain.to_string()).collect(),
            created_at_millis,
        )
        .unwrap_or_else(|| {
            panic!("the store must hold {api_key:?} in {scope}, within the per-scope bound")
        })
}

/// The awsJson error code of a refusal, as its `__type` carries it.
fn error_code(body: &Value) -> &str {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The message of a refusal.
fn error_message(body: &Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// `millis` since the epoch as the epoch seconds an awsJson1_1 body carries a
/// timestamp as.
#[allow(clippy::cast_precision_loss)]
fn epoch_seconds(millis: i64) -> f64 {
    millis as f64 / 1_000.0
}

/// How many keys the store holds, over every scope: what the reporting cases check
/// a listing left alone.
fn stored_key_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<ComAmazonawsWafv2Awswaf20190729State>()
        .lock()
        .api_keys
        .len()
}

/// One `ListAPIKeys` call for `scope`, with the `Limit` and `NextMarker` the page
/// asks for.
async fn list_page(waf: &Waf, scope: &str, limit: Option<i64>, marker: Option<&str>) -> Value {
    let mut request = json!({ "Scope": scope });
    if let Some(limit) = limit {
        request["Limit"] = json!(limit);
    }
    if let Some(marker) = marker {
        request["NextMarker"] = json!(marker);
    }
    waf.ok(OPERATION, request).await
}

/// The `APIKeySummaries` of a page, as an array.
fn summaries(page: &Value, case: &str) -> Vec<Value> {
    page["APIKeySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a page must carry APIKeySummaries, got {page}"))
        .clone()
}

/// One seeded key: what it is called in an assertion, the scope it lives in, its
/// key value and the token domains it was defined with.
struct KeyCase {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The scope the key is stored in.
    scope: &'static str,
    /// The key value, which is the key's identity within its scope.
    api_key: String,
    /// The token domains the key carries, in their stored order.
    token_domains: Vec<String>,
}

/// The seeded fixture the reporting cases read: six keys over both scopes, their
/// key values spanning one character to the longest the model allows and their
/// token-domain lists spanning one, two and three domains — including the longest
/// domain allowed and the punctuation `TokenDomain`'s pattern permits.
fn key_cases() -> Vec<KeyCase> {
    let longest_key = "k".repeat(MAX_API_KEY_LENGTH);
    let longest_domain = format!(
        "{}.example.com",
        "d".repeat(MAX_TOKEN_DOMAIN_LENGTH - ".example.com".len())
    );
    vec![
        KeyCase {
            case: "shortest key value, one domain",
            scope: SCOPE_REGIONAL,
            api_key: "k".to_string(),
            token_domains: vec!["example.com".to_string()],
        },
        KeyCase {
            case: "two domains, in the order they were given",
            scope: SCOPE_REGIONAL,
            api_key: "regional-two-domains".to_string(),
            token_domains: vec!["zeta.example.com".to_string(), "alpha.example.com".to_string()],
        },
        KeyCase {
            case: "three domains, with the punctuation the pattern allows",
            scope: SCOPE_REGIONAL,
            api_key: "regional+three/domains=".to_string(),
            token_domains: vec![
                "sub_domain.example.com".to_string(),
                "hyphen-ated.example.com".to_string(),
                "path/segment".to_string(),
            ],
        },
        KeyCase {
            case: "longest key value the model allows",
            scope: SCOPE_REGIONAL,
            api_key: longest_key,
            token_domains: vec![longest_domain],
        },
        KeyCase {
            case: "cloudfront key, one domain",
            scope: SCOPE_CLOUDFRONT,
            api_key: "cloudfront-one-domain".to_string(),
            token_domains: vec!["cdn.example.com".to_string()],
        },
        KeyCase {
            case: "cloudfront key sharing the regional key's value",
            scope: SCOPE_CLOUDFRONT,
            api_key: "k".to_string(),
            token_domains: vec!["shared.example.com".to_string()],
        },
    ]
}

/// Seed [`key_cases`] in order, one second apart, and answer the entries as stored.
fn seed_key_cases(engine: &engine::InMemoryEngine, cases: &[KeyCase]) -> Vec<ApiKeyEntry> {
    cases
        .iter()
        .enumerate()
        .map(|(index, case)| {
            let domains: Vec<&str> = case.token_domains.iter().map(String::as_str).collect();
            seed_api_key(
                engine,
                case.scope,
                &case.api_key,
                &domains,
                (BASE_SECS + index as i64) * 1_000,
            )
        })
        .collect()
}

/// A listing reports every stored key of the scope it names, exactly as stored, in
/// creation order — and reports nothing of the other scope: a scope is a namespace.
#[tokio::test]
async fn list_api_keys_reports_every_stored_key_of_the_scope_as_stored() {
    let (waf, engine) = Waf::sharing_engine();
    let cases = key_cases();
    let stored = seed_key_cases(&engine, &cases);

    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let page = list_page(&waf, scope, None, None).await;
        let reported = summaries(&page, scope);
        let expected: Vec<(&KeyCase, &ApiKeyEntry)> = cases
            .iter()
            .zip(stored.iter())
            .filter(|(case, _)| case.scope == scope)
            .collect();

        assert_eq!(
            reported.len(),
            expected.len(),
            "scope {scope}: a listing must report every stored key of the scope and \
             nothing of the other, got {page}"
        );

        for (summary, (case, entry)) in reported.iter().zip(expected.iter()) {
            assert_eq!(
                summary["APIKey"].as_str(),
                Some(case.api_key.as_str()),
                "case {:?}: the summary must carry the stored key value, got {summary}",
                case.case
            );
            let domains: Vec<&str> = summary["TokenDomains"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the summary must carry TokenDomains, got {summary}",
                        case.case
                    )
                })
                .iter()
                .map(|domain| domain.as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                domains, case.token_domains,
                "case {:?}: the summary must carry the stored token domains in their \
                 stored order, got {summary}",
                case.case
            );
            assert_eq!(
                summary["CreationTimestamp"].as_f64(),
                Some(epoch_seconds(entry.created_at_millis)),
                "case {:?}: the summary must carry the stored creation timestamp, got \
                 {summary}",
                case.case
            );
            assert_eq!(
                summary["Version"].as_i64(),
                Some(i64::from(entry.version)),
                "case {:?}: the summary must carry the stored version, got {summary}",
                case.case
            );
        }
    }
}

/// A listing pages by `Limit` and `NextMarker`: every page holds
/// `min(Limit, remaining)` summaries in creation order, every non-empty page
/// carries a marker to resume from, and the empty page that follows the last one
/// carries none.
#[tokio::test]
async fn list_api_keys_pages_by_limit_and_next_marker() {
    let (waf, engine) = Waf::sharing_engine();
    let all_keys: Vec<String> = (1..=5).map(|index| format!("key-{index}")).collect();
    for (index, api_key) in all_keys.iter().enumerate() {
        seed_api_key(
            &engine,
            SCOPE_REGIONAL,
            api_key,
            &["example.com"],
            (BASE_SECS + index as i64) * 1_000,
        );
    }

    for limit in [1_usize, 2, 3, 5, 8] {
        let case = format!("Limit={limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            pages += 1;
            assert!(
                pages <= all_keys.len() + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let page = list_page(
                &waf,
                SCOPE_REGIONAL,
                Some(limit as i64),
                marker.as_deref(),
            )
            .await;
            let reported = summaries(&page, &case);
            let remaining = all_keys.len() - seen.len();
            assert_eq!(
                reported.len(),
                remaining.min(limit),
                "case {case:?}: page {pages} must hold min(limit, remaining) summaries, \
                 got {page}"
            );
            for summary in &reported {
                seen.push(summary["APIKey"].as_str().unwrap_or_default().to_string());
            }
            marker = page["NextMarker"].as_str().map(str::to_string);
            assert_eq!(
                marker.is_some(),
                !reported.is_empty(),
                "case {case:?}: page {pages} must carry a marker exactly when it holds \
                 a summary, got {page}"
            );
            if marker.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, all_keys,
            "case {case:?}: paging must enumerate every stored key exactly once, in \
             creation order"
        );
        assert_eq!(
            pages,
            all_keys.len().div_ceil(limit) + 1,
            "case {case:?}: expected ceil(5 / limit) full pages plus the empty page \
             that ends the loop, got {pages}"
        );
    }

    // No `Limit` at all: one page with everything on it — and still a marker.
    let page = list_page(&waf, SCOPE_REGIONAL, None, None).await;
    let reported: Vec<String> = summaries(&page, "no Limit")
        .iter()
        .map(|summary| summary["APIKey"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, all_keys,
        "an unlimited listing must report every stored key, got {page}"
    );
    assert!(
        page["NextMarker"].as_str().is_some(),
        "an unlimited non-empty listing must still carry a marker, got {page}"
    );
}

/// A scope holding no keys answers an empty page and no marker, whether the other
/// scope holds keys or not.
#[tokio::test]
async fn a_scope_with_no_keys_lists_empty_and_carries_no_marker() {
    let (waf, engine) = Waf::sharing_engine();

    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let page = list_page(&waf, scope, None, None).await;
        assert!(
            summaries(&page, scope).is_empty(),
            "scope {scope}: an empty store must list no summaries, got {page}"
        );
        assert!(
            page["NextMarker"].is_null(),
            "scope {scope}: an empty page must carry no marker, got {page}"
        );
    }

    seed_api_key(
        &engine,
        SCOPE_REGIONAL,
        "regional-only",
        &["example.com"],
        BASE_SECS * 1_000,
    );

    let page = list_page(&waf, SCOPE_CLOUDFRONT, None, None).await;
    assert!(
        summaries(&page, "cloudfront while regional holds a key").is_empty(),
        "a scope holding no keys must list none even when the other scope holds \
         some, got {page}"
    );
    assert!(
        page["NextMarker"].is_null(),
        "a scope holding no keys must carry no marker, got {page}"
    );
}

/// A `Limit` on either published bound is honoured; one outside the modelled range
/// is refused — by the request validator, which words the member's own constraint —
/// with no page at all.
#[tokio::test]
async fn list_api_keys_refuses_a_limit_outside_the_modelled_range() {
    let (waf, engine) = Waf::sharing_engine();
    seed_api_key(
        &engine,
        SCOPE_REGIONAL,
        "bounded-limit",
        &["example.com"],
        BASE_SECS * 1_000,
    );

    for limit in [MIN_PAGE_LIMIT, MAX_PAGE_LIMIT] {
        let case = format!("Limit={limit}");
        let page = list_page(&waf, SCOPE_REGIONAL, Some(limit), None).await;
        assert_eq!(
            summaries(&page, &case).len(),
            1,
            "case {case:?}: a limit on the bound must be honoured, got {page}"
        );
    }

    for limit in [MIN_PAGE_LIMIT - 1, -1, MAX_PAGE_LIMIT + 1] {
        let case = format!("Limit={limit}");
        let (status, body) = waf
            .call(
                OPERATION,
                json!({ "Scope": SCOPE_REGIONAL, "Limit": limit }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a limit outside {MIN_PAGE_LIMIT}..{MAX_PAGE_LIMIT} must be \
             refused, got status {status} and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a member-constraint failure is the request validator's own \
             refusal, got {body}"
        );
        assert!(
            error_message(&body).contains("at 'limit' failed to satisfy constraint:"),
            "case {case:?}: the clause must name limit, got {body}"
        );
        assert!(
            body["APIKeySummaries"].is_null(),
            "case {case:?}: a refusal must carry no page, got {body}"
        );
    }
}

/// A `Scope` that is not one of the two the model declares is refused by the
/// request validator, and no page comes back with it.
#[tokio::test]
async fn list_api_keys_refuses_an_unmodelled_scope() {
    let (waf, engine) = Waf::sharing_engine();
    seed_api_key(
        &engine,
        SCOPE_REGIONAL,
        "scoped-key",
        &["example.com"],
        BASE_SECS * 1_000,
    );

    // `Scope` is `CLOUDFRONT | REGIONAL`
    // (`smithy_query(query="shape",shape="Scope")`); everything else names no
    // namespace, including the right value in the wrong case.
    for scope in ["", "regional", "GLOBAL", "CLOUDFRONT REGIONAL"] {
        let case = format!("Scope={scope:?}");
        let (status, body) = waf.call(OPERATION, json!({ "Scope": scope })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unmodelled scope must be refused, got status {status} \
             and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a member-constraint failure is the request validator's own \
             refusal, got {body}"
        );
        assert!(
            error_message(&body).contains("at 'scope' failed to satisfy constraint:"),
            "case {case:?}: the clause must name scope, got {body}"
        );
        assert!(
            body["APIKeySummaries"].is_null(),
            "case {case:?}: a refusal must carry no page, got {body}"
        );
    }
}

/// A listing reports a scope filled to its bound in one page, and the store
/// refuses a key past that bound rather than growing without limit.
#[tokio::test]
async fn a_full_scope_lists_every_key_it_holds() {
    let (waf, engine) = Waf::sharing_engine();
    let all_keys: Vec<String> = (0..MAX_API_KEYS_PER_SCOPE)
        .map(|index| format!("full-{index:02}"))
        .collect();
    for (index, api_key) in all_keys.iter().enumerate() {
        seed_api_key(
            &engine,
            SCOPE_REGIONAL,
            api_key,
            &["example.com"],
            (BASE_SECS + index as i64) * 1_000,
        );
    }

    let page = list_page(&waf, SCOPE_REGIONAL, None, None).await;
    let reported: Vec<String> = summaries(&page, "a full scope")
        .iter()
        .map(|summary| summary["APIKey"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, all_keys,
        "case {:?}: a listing must report every key the scope holds, in creation \
         order, got {page}",
        "a full scope"
    );

    let past_the_bound = engine
        .state::<ComAmazonawsWafv2Awswaf20190729State>()
        .lock()
        .insert_api_key(
            SCOPE_REGIONAL,
            "one-too-many",
            vec!["example.com".to_string()],
            BASE_SECS * 1_000,
        );
    assert!(
        past_the_bound.is_none(),
        "case {:?}: the store must refuse a key past {MAX_API_KEYS_PER_SCOPE} per \
         scope, got {past_the_bound:?}",
        "one past the bound"
    );
    // The other scope is a namespace of its own, so the bound has not closed it.
    assert!(
        engine
            .state::<ComAmazonawsWafv2Awswaf20190729State>()
            .lock()
            .insert_api_key(
                SCOPE_CLOUDFRONT,
                "other-scope",
                vec!["example.com".to_string()],
                BASE_SECS * 1_000,
            )
            .is_some(),
        "case {:?}: a scope filled to its bound must not bound the other scope",
        "the other scope"
    );
}

/// A key value is the key's identity within its scope, so storing one the scope
/// already holds leaves the listing alone: one summary, in its original place,
/// with the creation instant and the token domains it was first stored with.
#[tokio::test]
async fn re_storing_a_key_the_scope_already_holds_changes_no_listing() {
    let (waf, engine) = Waf::sharing_engine();
    let first = seed_api_key(
        &engine,
        SCOPE_REGIONAL,
        "shared-value",
        &["example.com"],
        BASE_SECS * 1_000,
    );
    seed_api_key(
        &engine,
        SCOPE_REGIONAL,
        "later-value",
        &["later.example.com"],
        (BASE_SECS + 1) * 1_000,
    );
    let again = seed_api_key(
        &engine,
        SCOPE_REGIONAL,
        "shared-value",
        &["moved.example.com"],
        (BASE_SECS + 2) * 1_000,
    );

    assert_eq!(
        (
            again.cursor.as_str(),
            again.created_at_millis,
            again.token_domains.as_slice()
        ),
        (
            first.cursor.as_str(),
            first.created_at_millis,
            first.token_domains.as_slice()
        ),
        "case {:?}: re-storing a held key must answer the stored entry unchanged, got \
         {again:?}",
        "a key the scope already holds"
    );

    let page = list_page(&waf, SCOPE_REGIONAL, None, None).await;
    let reported = summaries(&page, "after re-storing a held key");
    let keys: Vec<&str> = reported
        .iter()
        .map(|summary| summary["APIKey"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        keys,
        ["shared-value", "later-value"],
        "case {:?}: the listing must hold one summary per key value, in its original \
         creation order, got {page}",
        "a key the scope already holds"
    );
    assert_eq!(
        reported
            .first()
            .map(|summary| summary["CreationTimestamp"].as_f64()),
        Some(Some(epoch_seconds(first.created_at_millis))),
        "case {:?}: the summary must keep the creation instant it was first stored \
         with, got {page}",
        "a key the scope already holds"
    );
}

/// A listing REPORTS: two identical calls answer the same page, a page taken with
/// a marker does not consume anything, and the store holds exactly what it held
/// before.
#[tokio::test]
async fn listing_api_keys_changes_nothing_a_later_caller_sees() {
    let (waf, engine) = Waf::sharing_engine();
    let cases = key_cases();
    seed_key_cases(&engine, &cases);
    let before = stored_key_count(&engine);

    let first = list_page(&waf, SCOPE_REGIONAL, None, None).await;
    let marker = first["NextMarker"]
        .as_str()
        .expect("a non-empty listing carries a marker")
        .to_string();
    // Walk a page past the end, then ask for the whole listing again.
    let _ = list_page(&waf, SCOPE_REGIONAL, Some(2), Some(&marker)).await;
    let second = list_page(&waf, SCOPE_REGIONAL, None, None).await;

    assert_eq!(
        first, second,
        "case {:?}: a listing must answer the same page every time",
        "repeated unlimited listing"
    );
    assert_eq!(
        stored_key_count(&engine),
        before,
        "case {:?}: a listing must not add or remove a stored key, held {before} before",
        "store unchanged"
    );
}
