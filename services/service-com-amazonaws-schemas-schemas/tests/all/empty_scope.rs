// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `ListRegistries` does with a `Scope` the request
//! sent EMPTY.
//!
//! RULE under test: `Scope` names which registries a listing is LIMITED to, so a value
//! that names no scope — the member sent with no characters in it, exactly as an
//! omitted one carries none — limits nothing and the listing reports every registry,
//! while a value that does name something and names no scope of this service is still
//! refused 400 `BadRequestException`.
//!
//! The rule is about what a selecting value SPECIFIES, not about one request, so it is
//! asserted over a table: the empty value crossed with the sibling selecting member
//! (absent, empty, a prefix that matches some, one that matches none) and with the page
//! sizes a listing can be asked for in, and against the listing the request that omits
//! `Scope` entirely is served. The other side of the sentence gets a table of its own.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The
//! registries are created through `CreateRegistry`, so nothing here reads the state
//! struct directly. The harness is the crate's one harness, from
//! `registry_schema_lifecycle`.

use serde_json::Value;
use service_com_amazonaws_schemas_schemas::state::AWS_MANAGED_REGISTRIES;

use crate::registry_schema_lifecycle::{create_registry, registry_names, Answer, Emulator};

/// The registries this suite creates, in no particular order: a listing reports them in
/// key order, which the cases compute rather than assume.
const LOCAL_REGISTRIES: [&str; 3] = ["mine.beta", "mine-alpha", "other"];

/// `/v1/registries` with the query members this case sends, dropping the ones it does
/// not send at all.
fn listing_uri(members: &[&str]) -> String {
    let query: Vec<&str> = members
        .iter()
        .copied()
        .filter(|member| !member.is_empty())
        .collect();
    if query.is_empty() {
        return "/v1/registries".to_string();
    }
    format!("/v1/registries?{}", query.join("&"))
}

/// The `NextToken` an answer carries, or `None` for a page that ends its listing.
fn token_of(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every registry name the store holds once this suite's locals are created, in the key
/// order a listing serves them: the service-owned ones
/// (`crate::state::AWS_MANAGED_REGISTRIES`, the catalogue every account sees) and the
/// locals together.
async fn seeded(emulator: &Emulator) -> Vec<String> {
    for name in LOCAL_REGISTRIES {
        create_registry(emulator, name).await;
    }
    let mut all: Vec<String> = AWS_MANAGED_REGISTRIES
        .iter()
        .map(|name| (*name).to_string())
        .chain(LOCAL_REGISTRIES.iter().map(|name| (*name).to_string()))
        .collect();
    all.sort();
    all
}

/// RULE: an empty `Scope` names no scope, so it limits nothing — the listing is exactly
/// the one the request that omits the member is served, at every page size and
/// alongside every spelling of the sibling selecting member.
#[tokio::test]
async fn an_empty_scope_limits_nothing() {
    let emulator = Emulator::new();
    let all = seeded(&emulator).await;

    // Rows: the sibling selecting member absent, sent empty, a prefix that selects
    // some of the locals, a prefix that selects one of the service-owned registries,
    // and a prefix nothing starts with.
    for prefix in [
        "",
        "registryNamePrefix=",
        "registryNamePrefix=mine",
        "registryNamePrefix=aws.",
        "registryNamePrefix=zz",
    ] {
        // Crossed with the page sizes a first page can be asked for in: unpaged, one
        // item, one short of the whole store, exactly it, and more than it.
        for limit in [
            String::new(),
            "limit=1".to_string(),
            format!("limit={}", all.len() - 1),
            format!("limit={}", all.len()),
            format!("limit={}", all.len() + 2),
        ] {
            let case = format!("scope= with {prefix:?} and {limit:?}");

            let omitted = emulator
                .call("GET", &listing_uri(&[prefix, &limit]), None)
                .await;
            assert_eq!(
                omitted.status, 200,
                "case {case:?}: the request without Scope must be served, got {} {:?}",
                omitted.status, omitted.body
            );

            let empty = emulator
                .call("GET", &listing_uri(&["scope=", prefix, &limit]), None)
                .await;
            assert_eq!(
                empty.status, 200,
                "case {case:?}: an empty Scope limits nothing, so expected 200, got {} {:?}",
                empty.status, empty.body
            );
            assert_eq!(
                registry_names(&empty),
                registry_names(&omitted),
                "case {case:?}: expected the page an absent Scope serves {:?}, got {:?}",
                registry_names(&omitted),
                registry_names(&empty)
            );

            // And that page is the first page of the WHOLE store, both scopes in it:
            // the selection is unlimited, not merely equal to another refusal.
            let page_size = limit
                .strip_prefix("limit=")
                .map(|size| size.parse::<usize>().expect("a numeric row"))
                .unwrap_or(all.len());
            let expected: Vec<String> = all
                .iter()
                .filter(|name| {
                    prefix
                        .strip_prefix("registryNamePrefix=")
                        .is_none_or(|wanted| name.starts_with(wanted))
                })
                .take(page_size)
                .cloned()
                .collect();
            assert_eq!(
                registry_names(&empty),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                registry_names(&empty)
            );
        }
    }
}

/// RULE (the pagination side of the same reading): because an empty `Scope` selects the
/// same listing an absent one does, a token minted under one resumes under the other,
/// and following the tokens walks every registry of both scopes exactly once.
#[tokio::test]
async fn an_empty_scope_pages_the_same_listing_as_an_absent_one() {
    let emulator = Emulator::new();
    let all = seeded(&emulator).await;

    // Rows: page sizes that make the walk take a different number of pages.
    for page_size in [1_usize, 2, all.len() - 1] {
        let case = format!("limit={page_size}");
        let limit = format!("limit={page_size}");

        // Mint the token with `scope=` present and empty, then resume it with the
        // member omitted — and the other way round.
        for (mint, resume) in [("scope=", ""), ("", "scope=")] {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut selector = mint;
            for _ in 0..all.len() + 2 {
                let paging = token
                    .as_deref()
                    .map(|token| format!("nextToken={token}"))
                    .unwrap_or_default();
                let answer = emulator
                    .call("GET", &listing_uri(&[selector, &limit, &paging]), None)
                    .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} minted with {mint:?} resumed with {resume:?}: expected 200, \
                     got {} {:?}",
                    answer.status, answer.body
                );
                let page = registry_names(&answer);
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: a page holds at most {page_size} items, got {page:?}"
                );
                seen.extend(page);
                match token_of(&answer) {
                    Some(next) => token = Some(next),
                    None => break,
                }
                // Every page after the first is asked for the other way round, so the
                // token crosses between the two spellings of "no scope".
                selector = resume;
            }
            assert_eq!(
                seen, all,
                "case {case:?} minted with {mint:?} resumed with {resume:?}: the walk must \
                 report every registry of both scopes exactly once, expected {all:?}, got {seen:?}"
            );
        }
    }
}

/// RULE (the other side of the sentence): only a `Scope` that names NOTHING limits
/// nothing — a value that names something which is not a scope of this service is
/// refused, and the two documented values still limit.
#[tokio::test]
async fn a_scope_that_names_something_else_is_still_refused() {
    let emulator = Emulator::new();
    let all = seeded(&emulator).await;
    let mut locals: Vec<String> = LOCAL_REGISTRIES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    locals.sort();
    let managed: Vec<String> = all
        .iter()
        .filter(|name| !locals.contains(name))
        .cloned()
        .collect();

    // Rows: the documented values still select (in both casings), and values that do
    // name something — a word, a scope with surrounding space, one of the values with
    // a character appended, a single space — name no scope of this service.
    for (scope, expected) in [
        ("scope=Local", Some(locals.clone())),
        ("scope=local", Some(locals.clone())),
        ("scope=AWS", Some(managed.clone())),
        ("scope=aws", Some(managed.clone())),
        ("scope=Everything", None),
        ("scope=%20", None),
        ("scope=%20Local", None),
        ("scope=Locals", None),
        ("scope=0", None),
    ] {
        let answer = emulator.call("GET", &listing_uri(&[scope]), None).await;
        match expected {
            Some(expected) => {
                assert_eq!(
                    answer.status, 200,
                    "case {scope:?}: expected 200, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    registry_names(&answer),
                    expected,
                    "case {scope:?}: expected {expected:?}, got {:?}",
                    registry_names(&answer)
                );
            }
            None => {
                assert_eq!(
                    answer.status, 400,
                    "case {scope:?}: a value that is not a scope is refused, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "BadRequestException",
                    "case {scope:?}: expected BadRequestException, got {:?}",
                    answer.body
                );
            }
        }
    }
}
