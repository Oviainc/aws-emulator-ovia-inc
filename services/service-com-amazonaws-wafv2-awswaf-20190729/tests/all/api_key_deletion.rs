// SPDX-License-Identifier: CC0-1.0

//! `DeleteAPIKey`, end to end over the wire.
//!
//! The operation "deletes the specified API key", so the RULE asserted here is: the
//! key a request names is removed from the scope it names, and every later read —
//! its decrypted form and the scope's listing — no longer reports it, while every
//! other stored key is untouched. A value that does not decode to a key this
//! service minted is refused as the invalid parameter naming the submitted value,
//! and a well-formed key the scope does not hold is the nonexistent item — the same
//! two failures, told apart the same way, as on the read path
//! (`GetDecryptedAPIKey`). A refused delete removes nothing.
//!
//! Each suite asserts that over a TABLE spanning the class rather than over one
//! request: both scopes, one to the documented maximum of token domains, several
//! stored keys with only one of them deleted, every key of a scope deleted in turn,
//! a re-mint after a delete, and each refusal — a malformed token, a well-formed
//! token no mint answered, a key addressed in the other scope, and a key deleted
//! twice.
//!
//! Every call goes through the router's front door, the same door an SDK uses; the
//! only direct call into the crate is [`is_api_key_shaped`], used to assert that a
//! case meant to be a WELL-FORMED unknown key really is one.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    is_api_key_shaped, MAX_TOKEN_DOMAINS_PER_API_KEY, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::web_acl_lifecycle::Waf;

/// The operation under test, named once.
const DELETE: &str = "DeleteAPIKey";
/// The mint whose keys are deleted.
const CREATE: &str = "CreateAPIKey";
/// The reads a deleted key must disappear from.
const DECRYPT: &str = "GetDecryptedAPIKey";
/// See [`DECRYPT`].
const LIST: &str = "ListAPIKeys";

/// The longest value `APIKey` allows
/// (`smithy_query(query="shape",shape="APIKey")` → `length 1..2048`).
const MAX_API_KEY_LENGTH: usize = 2048;

/// The `ErrorReason` WAF carries for a value it cannot decode as an API key.
/// Written out here rather than imported, so the table does not restate the code
/// under test.
const KEY_FORMAT_REASON: &str = "The parameter contains formatting that is not valid.";

/// The error code of a refusal, as its `__type` carries it.
fn error_code(body: &Value) -> &str {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The other scope: a scope is a namespace, so each case has exactly one.
fn other_scope(scope: &str) -> &'static str {
    if scope == SCOPE_REGIONAL {
        SCOPE_CLOUDFRONT
    } else {
        SCOPE_REGIONAL
    }
}

/// Mint one key for `scope` over `token_domains` and answer its value.
async fn mint(waf: &Waf, scope: &str, token_domains: &[String], case: &str) -> String {
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

/// The key values the scope's listing reports, in creation order.
async fn listed_keys(waf: &Waf, scope: &str) -> Vec<String> {
    let page = waf.ok(LIST, json!({ "Scope": scope })).await;
    page["APIKeySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("scope {scope}: a listing must carry APIKeySummaries, got {page}"))
        .iter()
        .map(|summary| summary["APIKey"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Delete `api_key` from `scope`; the call must succeed and answer a body with no
/// members of its own (`DeleteAPIKeyResponse` models none).
async fn delete(waf: &Waf, scope: &str, api_key: &str, case: &str) {
    let answered = waf
        .ok(DELETE, json!({ "Scope": scope, "APIKey": api_key }))
        .await;
    assert!(
        answered.as_object().is_some_and(serde_json::Map::is_empty) || answered.is_null(),
        "case {case:?}: a delete must answer no members, got {answered}"
    );
}

/// A token list spanning what a definition can be: one domain, the documented
/// maximum of them, and the punctuation `TokenDomain`'s pattern allows.
fn domain_sets() -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("one domain", vec!["example.com".to_string()]),
        (
            "the punctuation the pattern allows",
            vec![
                "sub_domain.example.com".to_string(),
                "hyphen-ated.example.com".to_string(),
                "path/segment".to_string(),
            ],
        ),
        (
            "the most domains one key can serve",
            (0..MAX_TOKEN_DOMAINS_PER_API_KEY)
                .map(|index| format!("app{index}.example.com"))
                .collect(),
        ),
    ]
}

/// `domains` as the SET they name: sorted lexicographically, the order WAF stores a
/// key's domains in and therefore the order every read reports.
fn as_set(domains: &[String]) -> Vec<String> {
    let mut set = domains.to_vec();
    set.sort_unstable();
    set.dedup();
    set
}

/// `domains` with `suffix` appended to each, so one row yields as many DISTINCT
/// definitions as it needs keys: a scope holds one key per domain set.
fn variant(domains: &[String], suffix: usize) -> Vec<String> {
    domains
        .iter()
        .map(|domain| format!("{domain}.v{suffix}"))
        .collect()
}

/// A deleted key is gone from the scope that held it: neither the listing nor the
/// decrypted read reports it, and every other key of that scope is untouched.
#[tokio::test]
async fn a_deleted_key_is_gone_from_every_later_read() {
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        for (domains_case, domains) in domain_sets() {
            let waf = Waf::new();
            let case = format!("{scope}, {domains_case}");

            // Three keys, so a delete has to remove exactly one of them, and one key
            // of the OTHER scope, which a delete here must never touch. A scope holds
            // one key per domain set, so the three keys are three definitions.
            let definitions: Vec<Vec<String>> =
                (0..3).map(|index| variant(&domains, index)).collect();
            let mut minted: Vec<String> = Vec::new();
            for definition in &definitions {
                minted.push(mint(&waf, scope, definition, &case).await);
            }
            let elsewhere = mint(
                &waf,
                other_scope(scope),
                &["kept.example.com".to_string()],
                &case,
            )
            .await;

            let deleted = minted.remove(1);
            delete(&waf, scope, &deleted, &case).await;

            assert_eq!(
                listed_keys(&waf, scope).await,
                minted,
                "case {case:?}: the scope must hold exactly the keys that were not \
                 deleted, in creation order"
            );
            let (status, body) = waf
                .call(DECRYPT, json!({ "Scope": scope, "APIKey": &deleted }))
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "WAFNonexistentItemException"),
                "case {case:?}: a deleted key must name nothing stored, got status \
                 {status} and body {body}"
            );
            // Every key left still decrypts to its own definition.
            for (api_key, definition) in minted.iter().zip([&definitions[0], &definitions[2]]) {
                let decrypted = waf
                    .ok(DECRYPT, json!({ "Scope": scope, "APIKey": api_key }))
                    .await;
                assert_eq!(
                    decrypted["TokenDomains"],
                    json!(as_set(definition)),
                    "case {case:?}: a delete must leave the other keys alone, got \
                     {decrypted}"
                );
            }
            assert_eq!(
                listed_keys(&waf, other_scope(scope)).await,
                vec![elsewhere],
                "case {case:?}: a delete in one scope must not touch the other"
            );
        }
    }
}

/// Every key of a scope can be deleted, in any order, and a mint after a delete is
/// a key of its own that sorts after the keys that remain.
#[tokio::test]
async fn deleting_every_key_empties_the_scope_and_a_later_mint_still_works() {
    let waf = Waf::new();
    let case = "five keys deleted in turn";
    // One definition per key: a scope holds one key per domain set.
    let definitions: Vec<Vec<String>> = (0..5)
        .map(|index| vec![format!("drained{index}.example.com")])
        .collect();

    let mut remaining: Vec<String> = Vec::new();
    for definition in &definitions {
        remaining.push(mint(&waf, SCOPE_REGIONAL, definition, case).await);
    }

    // Deleted from the middle outwards, so the order of the remaining keys is
    // asserted after each removal rather than only at the end.
    for index in [2_usize, 0, 2, 0, 0] {
        let going = remaining.remove(index);
        delete(&waf, SCOPE_REGIONAL, &going, case).await;
        assert_eq!(
            listed_keys(&waf, SCOPE_REGIONAL).await,
            remaining,
            "case {case:?}: after deleting one key the scope must report the rest in \
             creation order"
        );
    }
    assert!(
        listed_keys(&waf, SCOPE_REGIONAL).await.is_empty(),
        "case {case:?}: a scope whose every key was deleted must report none"
    );

    // A mint after the deletes is a key of its own, and the scope reports it alone.
    let fresh = mint(&waf, SCOPE_REGIONAL, &definitions[0], case).await;
    assert_eq!(
        listed_keys(&waf, SCOPE_REGIONAL).await,
        vec![fresh.clone()],
        "case {case:?}: a mint after a delete must be the scope's only key"
    );
    let decrypted = waf
        .ok(DECRYPT, json!({ "Scope": SCOPE_REGIONAL, "APIKey": &fresh }))
        .await;
    assert_eq!(
        decrypted["TokenDomains"],
        json!(as_set(&definitions[0])),
        "case {case:?}: the fresh key must decrypt to its own definition, got \
         {decrypted}"
    );
}

/// A value this service did not mint names no key to delete: one that does not
/// decode to a key is the invalid parameter, echoing the submitted token, and a
/// well-formed one the scope does not hold is the nonexistent item. Neither removes
/// anything.
#[tokio::test]
async fn deleting_a_key_the_service_did_not_mint_is_refused() {
    let waf = Waf::new();
    let stored = mint(
        &waf,
        SCOPE_REGIONAL,
        &["stored.example.com".to_string()],
        "a stored key",
    )
    .await;
    let before = listed_keys(&waf, SCOPE_REGIONAL).await;

    // Not the form a minted key takes — the wrong alphabet, a length no envelope is
    // rendered at, an envelope too small to carry a key, or such a value as long as
    // the model allows a key to be — so no key WAF could decode.
    for value in [
        "sim2real-never-issued-api-key-token-000000000000",
        "not-a-key",
        "k",
        "ABCDEF0123456789",
        "key_with_underscores",
        &"0".repeat(63),
        &"0".repeat(65),
        &"k".repeat(MAX_API_KEY_LENGTH - 3),
    ] {
        let case = format!("APIKey={value:.32?}");
        assert!(
            !is_api_key_shaped(value),
            "case {case:?}: this case must be a MALFORMED key, got one with the \
             minted form"
        );
        for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
            let body = waf
                .refused(
                    DELETE,
                    json!({ "Scope": scope, "APIKey": value }),
                    "WAFInvalidParameterException",
                    &case,
                )
                .await;
            assert_eq!(
                (
                    body["Parameter"].as_str(),
                    body["Field"].as_str(),
                    body["Reason"].as_str()
                ),
                (Some(value), None, Some(KEY_FORMAT_REASON)),
                "case {case:?}: the refusal must name the value it could not decode \
                 and no field, got {body}"
            );
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                format!(
                    "Error reason: {KEY_FORMAT_REASON}, field: null, parameter: {value}"
                ),
                "case {case:?}: the refusal must read the way WAF words it, got {body}"
            );
        }
    }

    // Well formed, and no key: the store holds nothing under it, in either scope.
    let unknown = crate::api_key_minting::well_formed_unknown_key();
    assert!(
        is_api_key_shaped(&unknown),
        "the unknown-key case must be WELL FORMED, got {unknown:?}"
    );
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let case = format!("a well-formed unknown key in {scope}");
        waf.refused(
            DELETE,
            json!({ "Scope": scope, "APIKey": &unknown }),
            "WAFNonexistentItemException",
            &case,
        )
        .await;
    }

    // A key of the other scope, which is a namespace of its own, and the same key
    // deleted twice: both name nothing stored where they are addressed.
    waf.refused(
        DELETE,
        json!({ "Scope": SCOPE_CLOUDFRONT, "APIKey": &stored }),
        "WAFNonexistentItemException",
        "a stored key addressed in the other scope",
    )
    .await;
    assert_eq!(
        listed_keys(&waf, SCOPE_REGIONAL).await,
        before,
        "case {:?}: a refused delete must remove nothing",
        "every refusal above"
    );

    delete(&waf, SCOPE_REGIONAL, &stored, "the stored key").await;
    waf.refused(
        DELETE,
        json!({ "Scope": SCOPE_REGIONAL, "APIKey": &stored }),
        "WAFNonexistentItemException",
        "the same key deleted twice",
    )
    .await;
    assert!(
        listed_keys(&waf, SCOPE_REGIONAL).await.is_empty(),
        "the second delete must leave the scope as the first one did"
    );
}

/// A `Scope` that is not one of the two the model declares is refused by the request
/// validator, and no key is removed for it.
#[tokio::test]
async fn an_unmodelled_scope_is_refused_and_removes_nothing() {
    let waf = Waf::new();
    let stored = mint(
        &waf,
        SCOPE_REGIONAL,
        &["scoped.example.com".to_string()],
        "a stored key",
    )
    .await;

    // `Scope` is `CLOUDFRONT | REGIONAL`
    // (`smithy_query(query="shape",shape="Scope")`); everything else names no
    // namespace, including the right value in the wrong case.
    for scope in ["", "regional", "GLOBAL", "CLOUDFRONT REGIONAL"] {
        let case = format!("{DELETE} with Scope={scope:?}");
        let (status, body) = waf
            .call(DELETE, json!({ "Scope": scope, "APIKey": &stored }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unmodelled scope must be refused, got status {status} \
             and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a member-constraint failure is the request validator's \
             own refusal, got {body}"
        );
    }
    assert_eq!(
        listed_keys(&waf, SCOPE_REGIONAL).await,
        vec![stored],
        "a refused delete must leave the stored key in place"
    );
}
