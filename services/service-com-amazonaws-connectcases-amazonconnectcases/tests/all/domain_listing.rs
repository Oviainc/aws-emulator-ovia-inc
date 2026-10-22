// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `ListDomains`, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: a paginated list operation reports EVERY resource the
//! store holds — each entry built from the stored record — in one stable order
//! (creation order, as every listing in this service orders its answer), split
//! into pages of at most the page size the caller asked for (the model's own
//! ceiling for the operation when it asks for none), with `nextToken` present
//! exactly when a further page remains and absent on the last one; passing an
//! issued token back resumes immediately after the entry the previous page
//! stopped at, a page size outside the modelled range and a token the service
//! did not issue for THIS listing are refused with HTTP 400
//! `ValidationException`, an empty token means the start of the listing, and the
//! whole walk writes nothing, so a repeat of any call answers the same thing.
//!
//! The harness (`Emulator`, `string_at`, `is_minted_id`, `domain`) is the one
//! `case_lifecycle` already drives; this suite reuses it rather than standing up
//! a second copy.

use std::collections::HashSet;

use serde_json::{json, Value};

use super::case_lifecycle::{domain, is_minted_id, string_at, Emulator};

/// The ceiling the model puts on `ListDomainsRequest$maxResults`
/// (`@range(min: 1, max: 10)`, from
/// `smithy_query(query="shape", shape="ListDomainsRequest")`), which is also the
/// page size the service answers with when the request names none: the member
/// declares no `@default`.
const PAGE_SIZE: i32 = 10;

/// One domain as `ListDomains` must report it: the three required members of
/// `DomainSummary` (`domainId`, `domainArn`, `name`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    domain_id: String,
    domain_arn: String,
    name: String,
}

impl Summary {
    /// The summary a response entry carries.
    fn from_entry(entry: &Value, case: &str) -> Self {
        Self {
            domain_id: string_at(entry, "domainId", case),
            domain_arn: string_at(entry, "domainArn", case),
            name: string_at(entry, "name", case),
        }
    }
}

/// Create `count` domains and return the summaries a complete listing must
/// report, in the order it must report them (creation order).
///
/// Each expectation is read back through `GetDomain`, so it is what the store
/// holds rather than what this test typed.
async fn domains_in_creation_order(
    emulator: &Emulator,
    count: usize,
    prefix: &str,
) -> Vec<Summary> {
    let mut created: Vec<Summary> = Vec::new();
    for index in 0..count {
        let name = format!("{prefix}-{index}");
        let domain_id = domain(emulator, &name).await;
        let case = format!("GetDomain {name}");
        let fetched = emulator
            .ok(&case, "POST", &format!("/domains/{domain_id}"), json!({}))
            .await;
        created.push(Summary {
            domain_id: string_at(&fetched, "domainId", &case),
            domain_arn: string_at(&fetched, "domainArn", &case),
            name: string_at(&fetched, "name", &case),
        });
    }
    created
}

/// The `ListDomains` path for a page: `maxResults`/`nextToken` are `httpQuery`
/// members, so they ride in the query string.
fn list_path(max_results: Option<i32>, next_token: Option<&str>) -> String {
    let mut params: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        params.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        params.push(format!("nextToken={token}"));
    }
    if params.is_empty() {
        "/domains-list".to_string()
    } else {
        format!("/domains-list?{}", params.join("&"))
    }
}

/// The summaries one `ListDomains` answer carries.
fn listed(case: &str, response: &Value) -> Vec<Summary> {
    response
        .get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: ListDomains returned no domains list: {response:?}"))
        .iter()
        .map(|entry| Summary::from_entry(entry, case))
        .collect()
}

/// Walk every page of `ListDomains` with `max_results`, checking the page-shape
/// invariants on the way, and return the summaries in the order they arrived.
async fn walk_pages(emulator: &Emulator, case: &str, max_results: Option<i32>) -> Vec<Summary> {
    let page_size = max_results.unwrap_or(PAGE_SIZE) as usize;
    let mut seen: Vec<Summary> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let response = emulator
            .ok(case, "POST", &list_path(max_results, token.as_deref()), json!({}))
            .await;
        let page = listed(case, &response);
        pages += 1;
        assert!(
            page.len() <= page_size,
            "case {case:?} page {pages}: a page holds at most {page_size} domains, got {}",
            page.len()
        );
        token = response
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_some() {
            // A token means more results follow, which only happens once a page
            // has been filled.
            assert_eq!(
                page.len(),
                page_size,
                "case {case:?} page {pages}: a page that carries a nextToken must be full, \
                 got {page:?}"
            );
        }
        seen.extend(page);
        if token.is_none() {
            break;
        }
        assert!(
            pages <= seen.len() + 1,
            "case {case:?}: following nextToken must terminate, but page {pages} still carried \
             one after {} entries",
            seen.len()
        );
    }
    seen
}

#[tokio::test]
async fn list_domains_pages_every_stored_domain_in_one_stable_order() {
    let emulator = Emulator::new();
    // More domains than the largest page, so every page size below is exercised
    // on a truncated listing as well as on the final short page.
    let expected = domains_in_creation_order(&emulator, 23, "listing").await;

    // The rows differ along the page size: each published bound of the model's
    // `maxResults` range for this operation (1 and 10), sizes inside it that do
    // and do not divide the listing, and an absent one (the page size the
    // service answers with on its own).
    for max_results in [Some(1), Some(2), Some(7), Some(PAGE_SIZE), None] {
        let case = format!("ListDomains maxResults={max_results:?}");
        let listed = walk_pages(&emulator, &case, max_results).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the pages must report every stored domain exactly once, in \
             creation order; got {listed:?}"
        );
        let distinct: HashSet<&String> = listed.iter().map(|summary| &summary.domain_id).collect();
        assert_eq!(
            distinct.len(),
            listed.len(),
            "case {case:?}: no domain may be reported twice across pages, got {listed:?}"
        );
        for summary in &listed {
            assert!(
                is_minted_id(&summary.domain_id),
                "case {case:?}: domainId must be the server-minted id, got {summary:?}"
            );
            assert!(
                summary.domain_arn.ends_with(&format!("domain/{}", summary.domain_id)),
                "case {case:?}: domainArn must address the stored domain, got {summary:?}"
            );
        }
    }

    // A page size outside the modelled range is refused rather than clamped
    // silently: one past each published bound.
    for bad in [0, -1, PAGE_SIZE + 1] {
        let case = format!("maxResults={bad}");
        let (status, code, message) = emulator
            .failure_detail(&case, "POST", &list_path(Some(bad), None), Some(json!({})))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a maxResults outside 1..{PAGE_SIZE} must be refused with a 400 \
             ValidationException, got {status} {code} ({message})"
        );
    }
}

#[tokio::test]
async fn list_domains_reports_what_state_holds_and_changes_nothing() {
    let emulator = Emulator::new();

    // An empty store lists nothing and ends the iteration — not a 501, and not a
    // page that claims more results exist.
    let empty = emulator
        .ok("empty store", "POST", &list_path(None, None), json!({}))
        .await;
    assert_eq!(
        listed("empty store", &empty),
        Vec::new(),
        "case \"empty store\": an empty store lists no domains, got {empty:?}"
    );
    assert!(
        empty.get("nextToken").is_none_or(Value::is_null),
        "case \"empty store\": there is no further page, got {empty:?}"
    );

    // Each create is visible in the next listing, and only the domains created so
    // far are reported: the response is computed from state, not from the request.
    let mut expected: Vec<Summary> = Vec::new();
    for index in 0..4 {
        let case = format!("after {} creates", index + 1);
        let mut grown = domains_in_creation_order(&emulator, 1, &format!("grow-{index}")).await;
        expected.append(&mut grown);
        let walked = walk_pages(&emulator, &case, Some(2)).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: ListDomains must report exactly the domains the store holds, \
             got {walked:?}"
        );
    }

    // A read changes nothing a later caller sees: the same call twice, and a full
    // walk in between, answer identically.
    let first = emulator
        .ok("repeat", "POST", &list_path(Some(2), None), json!({}))
        .await;
    let walked = walk_pages(&emulator, "repeat walk", Some(1)).await;
    let second = emulator
        .ok("repeat", "POST", &list_path(Some(2), None), json!({}))
        .await;
    assert_eq!(
        first, second,
        "case \"repeat\": ListDomains is a read, so repeating it answers the same page; \
         got {first:?} then {second:?}"
    );
    assert_eq!(
        walked, expected,
        "case \"repeat walk\": paging through the list must not consume it, got {walked:?}"
    );
}

#[tokio::test]
async fn a_list_domains_token_is_this_listing_s_or_it_is_refused() {
    let emulator = Emulator::new();

    // A domain holding more templates than one page, so `ListTemplates` — ANOTHER
    // listing of this same service — issues a token of its own. It is created
    // first so the domains this test expects to see listed are known in full.
    let other = domains_in_creation_order(&emulator, 1, "another-listing").await;
    let other_domain_id = other[0].domain_id.clone();
    for index in 0..2 {
        emulator
            .ok(
                "template",
                "POST",
                &format!("/domains/{other_domain_id}/templates"),
                json!({ "name": format!("template-{index}") }),
            )
            .await;
    }
    let other_listing = {
        let page = emulator
            .ok(
                "ListTemplates first page",
                "POST",
                &format!("/domains/{other_domain_id}/templates-list?maxResults=1"),
                json!({}),
            )
            .await;
        string_at(&page, "nextToken", "ListTemplates first page")
    };

    let mut expected = other;
    expected.extend(domains_in_creation_order(&emulator, 3, "token").await);

    let issued_page = emulator
        .ok("issue a token", "POST", &list_path(Some(1), None), json!({}))
        .await;
    let issued = string_at(&issued_page, "nextToken", "issue a token");

    // Rows differing in FORM: not a byte string at all, valid hex that nothing
    // minted, an issued token with its version marker shortened away, an issued
    // token one character short (so not even a byte string), and a token this
    // service issued for a different listing.
    for (row, token) in [
        ("not a token", "not-a-token".to_string()),
        ("hex nobody minted", ::hex::encode("hello")),
        ("marker dropped", issued[2..].to_string()),
        ("odd length", issued[..issued.len() - 1].to_string()),
        ("another listing's token", other_listing.clone()),
    ] {
        let (status, code, message) = emulator
            .failure_detail(row, "POST", &list_path(None, Some(&token)), Some(json!({})))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {row:?}: a nextToken this service did not issue for this listing must be \
             refused with a 400 ValidationException, got {status} {code} ({message})"
        );
        assert!(
            message.contains("nextToken"),
            "case {row:?}: the refusal must name the member at fault, got {message:?}"
        );
    }

    // An empty `nextToken` is a value the model's `@length(0..9000)` admits and
    // means the start of the listing.
    let from_empty = emulator
        .ok("empty token", "POST", &list_path(None, Some("")), json!({}))
        .await;
    assert_eq!(
        listed("empty token", &from_empty),
        expected,
        "case \"empty token\": an empty token means the start of the listing, got {from_empty:?}"
    );

    // The issued token resumes exactly after the entry its page stopped at.
    let resumed = emulator
        .ok(
            "resume",
            "POST",
            &list_path(Some(1), Some(&issued)),
            json!({}),
        )
        .await;
    assert_eq!(
        listed("resume", &resumed),
        expected[1..2].to_vec(),
        "case \"resume\": the token must resume immediately after the first page, got {resumed:?}"
    );
}
