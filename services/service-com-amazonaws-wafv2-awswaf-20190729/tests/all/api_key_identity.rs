// SPDX-License-Identifier: CC0-1.0

//! An API key's identity — the token-domain SET it is defined over — end to end over
//! the wire.
//!
//! `CreateAPIKey` "creates an API key that contains a set of token domains" and the
//! key it answers is the key a client copies into its JavaScript CAPTCHA
//! integration. The RULE asserted here is therefore: a scope holds ONE key per
//! token-domain set. Repeating the call for a set the scope already holds a key for
//! answers the byte-identical key and leaves the listing with one summary for it,
//! whatever order the domains are given in and however many times a domain is
//! repeated; a different set is a different key. And because the domains are a SET,
//! every read of the key — its decrypted form and its listing summary — reports them
//! in one order, lexicographic, rather than in whichever order the mint was given.
//!
//! Each suite asserts that over a TABLE spanning the class: one domain to the
//! documented maximum of five, already-sorted and reverse-sorted and shuffled
//! orders, a set naming a domain twice, both scopes, and the sets that differ from
//! one another by one domain.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    MAX_TOKEN_DOMAINS_PER_API_KEY, MINTED_API_KEY_VERSION, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::web_acl_lifecycle::Waf;

/// The operations under test, named once.
const CREATE: &str = "CreateAPIKey";
/// See [`CREATE`].
const DECRYPT: &str = "GetDecryptedAPIKey";
/// See [`CREATE`].
const DELETE: &str = "DeleteAPIKey";
/// See [`CREATE`].
const LIST: &str = "ListAPIKeys";

/// Mint one key for `scope` over `token_domains`; the call must succeed and answer a
/// key value.
async fn mint(waf: &Waf, scope: &str, token_domains: &[&str], case: &str) -> String {
    let answered = waf
        .ok(
            CREATE,
            json!({ "Scope": scope, "TokenDomains": token_domains }),
        )
        .await;
    answered["APIKey"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a mint must answer an APIKey, got {answered}"))
        .to_string()
}

/// The `TokenDomains` of a decrypted key or a listing summary.
fn token_domains(body: &Value, case: &str) -> Vec<String> {
    body["TokenDomains"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected TokenDomains, got {body}"))
        .iter()
        .map(|domain| domain.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Read `api_key` back in decrypted form; the call must succeed.
async fn decrypt(waf: &Waf, scope: &str, api_key: &str) -> Value {
    waf.ok(DECRYPT, json!({ "Scope": scope, "APIKey": api_key }))
        .await
}

/// Every summary the scope's listing reports, in creation order.
async fn summaries(waf: &Waf, scope: &str) -> Vec<Value> {
    let page = waf.ok(LIST, json!({ "Scope": scope })).await;
    page["APIKeySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("scope {scope}: a listing must carry APIKeySummaries, got {page}"))
        .clone()
}

/// The key values the scope's listing reports, in creation order.
async fn listed_keys(waf: &Waf, scope: &str) -> Vec<String> {
    summaries(waf, scope)
        .await
        .iter()
        .map(|summary| summary["APIKey"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `domains`, sorted lexicographically with duplicates collapsed: the SET the
/// definition names, which is what every read of the key must report.
fn as_set(domains: &[&str]) -> Vec<String> {
    let mut set: Vec<String> = domains.iter().map(|domain| (*domain).to_string()).collect();
    set.sort_unstable();
    set.dedup();
    set
}

/// One domain set, and the orders the same set can be submitted in.
struct DomainSet {
    /// What the row is called in an assertion.
    case: &'static str,
    /// The orders the same set is submitted in: each must mint or answer one key.
    orders: Vec<Vec<&'static str>>,
}

/// The table the suites span: one domain to the documented maximum of five, in
/// several orders each, and a set that names a domain twice.
fn domain_sets() -> Vec<DomainSet> {
    vec![
        DomainSet {
            case: "one domain",
            orders: vec![vec!["example.com"], vec!["example.com"]],
        },
        DomainSet {
            case: "two domains, submitted in both orders",
            orders: vec![
                vec!["alpha.example.com", "zeta.example.com"],
                vec!["zeta.example.com", "alpha.example.com"],
            ],
        },
        DomainSet {
            case: "three domains, submitted in three orders",
            orders: vec![
                vec!["b.example.com", "a.example.com", "c.example.com"],
                vec!["a.example.com", "b.example.com", "c.example.com"],
                vec!["c.example.com", "b.example.com", "a.example.com"],
            ],
        },
        DomainSet {
            case: "a set naming one of its domains twice",
            orders: vec![
                vec!["dup.example.com", "other.example.com", "dup.example.com"],
                vec!["other.example.com", "dup.example.com"],
            ],
        },
        DomainSet {
            case: "the punctuation TokenDomain's pattern allows",
            orders: vec![
                vec!["sub_domain.example.com", "path/segment", "hyphen-ated.example.com"],
                vec!["path/segment", "hyphen-ated.example.com", "sub_domain.example.com"],
            ],
        },
        DomainSet {
            case: "the most domains one key can be used for",
            orders: vec![
                vec![
                    "app4.example.com",
                    "app3.example.com",
                    "app2.example.com",
                    "app1.example.com",
                    "app0.example.com",
                ],
                vec![
                    "app0.example.com",
                    "app1.example.com",
                    "app2.example.com",
                    "app3.example.com",
                    "app4.example.com",
                ],
            ],
        },
    ]
}

/// Every call for one domain set answers the one key that set already has, in either
/// scope — and the scope keeps exactly one summary for it.
#[tokio::test]
async fn every_mint_for_one_domain_set_answers_the_same_key() {
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        for row in domain_sets() {
            let waf = Waf::new();
            let case = format!("{scope}: {}", row.case);
            assert!(
                row.orders.iter().all(|order| order.len() <= MAX_TOKEN_DOMAINS_PER_API_KEY),
                "case {case:?}: every row must stay within the documented maximum of \
                 {MAX_TOKEN_DOMAINS_PER_API_KEY} domains"
            );

            let first = mint(&waf, scope, &row.orders[0], &case).await;
            for order in &row.orders {
                let again = mint(&waf, scope, order, &case).await;
                assert_eq!(
                    again, first,
                    "case {case:?}: a mint for a set the scope already holds a key for \
                     must answer that key, got {again} for order {order:?}"
                );
            }

            let listed = listed_keys(&waf, scope).await;
            assert_eq!(
                listed,
                vec![first.clone()],
                "case {case:?}: the scope must hold exactly one summary for the set, got \
                 {listed:?}"
            );
            // Repeating the mint does not re-version the key either: `Version` is
            // WAF's own bookkeeping for the key it already holds.
            let versions: Vec<i64> = summaries(&waf, scope)
                .await
                .iter()
                .map(|summary| summary["Version"].as_i64().unwrap_or_default())
                .collect();
            assert_eq!(
                versions,
                vec![i64::from(MINTED_API_KEY_VERSION)],
                "case {case:?}: the key must keep the version it was minted at, got \
                 {versions:?}"
            );
            // The one key still decrypts to the SET it was defined over.
            let decrypted = decrypt(&waf, scope, &first).await;
            assert_eq!(
                token_domains(&decrypted, &case),
                as_set(&row.orders[0]),
                "case {case:?}: the key must decrypt to its domain set, got {decrypted}"
            );
        }
    }
}

/// Every read of a key reports its domains in lexicographic order, whatever order
/// the mint was given: the decrypted key and the listing summary report one record.
#[tokio::test]
async fn a_key_reports_its_token_domains_in_lexicographic_order() {
    let waf = Waf::new();
    for (index, row) in domain_sets().into_iter().enumerate() {
        for (order_index, order) in row.orders.iter().enumerate() {
            let case = format!("{}, order {order_index}", row.case);
            // A domain suffix per row, so each order of one row is the same set and
            // the rows are different sets.
            let suffixed: Vec<String> = order
                .iter()
                .map(|domain| format!("{domain}.r{index}"))
                .collect();
            let submitted: Vec<&str> = suffixed.iter().map(String::as_str).collect();
            let expected = as_set(&submitted);
            let api_key = mint(&waf, SCOPE_REGIONAL, &submitted, &case).await;

            let decrypted = decrypt(&waf, SCOPE_REGIONAL, &api_key).await;
            let reported = token_domains(&decrypted, &case);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the domains sorted lexicographically, got \
                 {reported:?}"
            );
            let summary = summaries(&waf, SCOPE_REGIONAL)
                .await
                .into_iter()
                .find(|summary| summary["APIKey"].as_str() == Some(api_key.as_str()))
                .unwrap_or_else(|| panic!("case {case:?}: the key must appear in the listing"));
            assert_eq!(
                token_domains(&summary, &case),
                expected,
                "case {case:?}: the summary must report the same order the decrypted key \
                 does, got {summary}"
            );
        }
    }
}

/// A DIFFERENT domain set is a different key: adding, removing or replacing one
/// domain mints a key of its own, each scope keeps one summary per set, and deleting
/// one set's key removes only that summary.
#[tokio::test]
async fn a_different_domain_set_is_a_different_key() {
    let waf = Waf::new();
    // Sets that differ from one another by exactly one domain, plus one that differs
    // in every domain.
    let sets: Vec<(&str, Vec<&str>)> = vec![
        ("one domain", vec!["one.example.com"]),
        (
            "that domain plus another",
            vec!["one.example.com", "two.example.com"],
        ),
        (
            "the same count with one domain replaced",
            vec!["one.example.com", "three.example.com"],
        ),
        ("a disjoint set", vec!["far.example.com", "away.example.com"]),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, domains) in &sets {
        let api_key = mint(&waf, SCOPE_REGIONAL, domains, case).await;
        assert!(
            !minted.contains(&api_key),
            "case {case:?}: a different domain set must be a different key, got {api_key}"
        );
        minted.push(api_key);
    }
    assert_eq!(
        listed_keys(&waf, SCOPE_REGIONAL).await,
        minted,
        "case {:?}: the scope must hold one summary per domain set, in creation order",
        "every set minted"
    );

    // A key of the same set in the OTHER scope is a key of its own: a scope is a
    // namespace.
    let elsewhere = mint(&waf, SCOPE_CLOUDFRONT, &sets[0].1, "the other scope").await;
    assert!(
        !minted.contains(&elsewhere),
        "case {:?}: a scope is a namespace, so the same set is a key of its own there, \
         got {elsewhere}",
        "the other scope"
    );

    // Deleting one set's key removes that summary and nothing else, and the set can
    // then be minted again — as a key of its own.
    let deleted = minted.remove(1);
    waf.ok(
        DELETE,
        json!({ "Scope": SCOPE_REGIONAL, "APIKey": &deleted }),
    )
    .await;
    assert_eq!(
        listed_keys(&waf, SCOPE_REGIONAL).await,
        minted,
        "case {:?}: a delete must remove exactly the deleted set's summary",
        "deleting one set's key"
    );
    let reminted = mint(&waf, SCOPE_REGIONAL, &sets[1].1, "after the delete").await;
    assert_ne!(
        reminted, deleted,
        "case {:?}: a set whose key was deleted must mint a key of its own, got \
         {reminted}",
        "after the delete"
    );
}
