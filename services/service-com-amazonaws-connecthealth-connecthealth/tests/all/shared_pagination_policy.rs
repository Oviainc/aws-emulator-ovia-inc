// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage that the two paginated listings of this service serve ONE
//! pagination rule.
//!
//! The RULE under test, asserted over a table that drives BOTH listings with the same
//! rows: a `@paginated` listing of this service
//! (`smithy_query(query="operation", operation="ListDomains,ListSubscriptions", model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//! reports both as `@paginated(inputToken=nextToken, outputToken=nextToken,
//! pageSize=maxResults)` with `maxResults` an `Integer` carrying `@range{min:1,max:100}`)
//!
//! * serves a FULL page when the caller asks for no `maxResults`, and at most
//!   `maxResults` records when it does;
//! * REFUSES a page size outside that range, with AWS's standard constraint sentence,
//!   rather than clamping it;
//! * treats a `nextToken` as a resume point only when this service issued it for the very
//!   collection being listed, and refuses every other value;
//! * reports a `nextToken` only when a record is left after the page it just served, so
//!   the last page — and an empty one — carries none.
//!
//! Both listings answer that rule from the same code, so each case asserts the two
//! answers AGREE as well as that each is right: the status, the error code and the detail
//! text of a refusal are compared listing-against-listing, which is what a rule
//! implemented once, and reported on each operation's own error channel, has to produce.
//!
//! The rows differ along what the rule generalises over: the page size (absent, each
//! published bound, inside the bound, exactly the collection's size, one past each bound
//! and far past the maximum), the collection's size (full and empty), and the token's
//! origin (none, one this listing issued, one the OTHER listing issued, one nothing
//! issued).
//!
//! The harness (`Emulator`, `error_code`, `string_at`) is the sibling suites': one way of
//! driving this service through signed restJson1 requests, not two.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The Region every request of this suite is signed for.
const REGION: &str = "us-east-2";

/// The `@range` of `maxResults` on BOTH listings, as the model reports it (see the module
/// doc): the bound the table walks, at each end and past each end.
const PAGE_SIZE_RANGE: (i64, i64) = (1, 100);

/// How many records each listing's collection holds in the cases below — small enough to
/// page through at every size from one to one past it, and larger than one so a first
/// page can be a partial one.
const COLLECTION_SIZE: usize = 3;

/// The body member a refused ConnectHealth call reports its detail text in.
const DETAIL_MEMBER: &str = "Message";

/// One paginated listing, as a case drives it.
struct Listing {
    operation: &'static str,
    /// The listing's URI with no query string.
    base: String,
    /// The output member carrying the page.
    member: &'static str,
    /// The member of one entry that is its cursor key.
    key: &'static str,
    /// A request to the SAME listing whose collection holds nothing, so the page is
    /// empty.
    empty: String,
}

impl Listing {
    /// `path` with one more query parameter, whichever separator it needs.
    fn with_param(path: &str, name: &str, value: &str) -> String {
        let separator = if path.contains('?') { '&' } else { '?' };
        format!("{path}{separator}{name}={value}")
    }

    /// This listing's URI with an optional page size and an optional token.
    fn request(&self, base: &str, max_results: Option<i64>, token: Option<&str>) -> String {
        let mut path = base.to_string();
        if let Some(max_results) = max_results {
            path = Self::with_param(&path, "maxResults", &max_results.to_string());
        }
        if let Some(token) = token {
            path = Self::with_param(&path, "nextToken", token);
        }
        path
    }

    /// The cursor keys of one page, in the order the listing reported them.
    fn keys(&self, case: &str, body: &Value) -> Vec<String> {
        body.get(self.member)
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {} must carry a {} list, got {body}",
                    self.operation, self.member
                )
            })
            .iter()
            .map(|entry| string_at(entry, self.key))
            .collect()
    }
}

/// The detail text of a refusal, failing with the whole body when it carries none.
fn detail_text(case: &str, body: &Value) -> String {
    body.get(DETAIL_MEMBER)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a refusal must carry a {DETAIL_MEMBER} member, got {body}")
        })
        .to_string()
}

/// AWS's standard constraint-violation sentence for one violation, written here
/// independently of the emulator's own renderer.
///
/// Source: the envelope AWS documents for a modelled constraint violation — `N validation
/// error(s) detected: Value '<value>' at '<member>' failed to satisfy constraint:
/// <requirement>`.
fn range_sentence(value: i64, member_path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// Both listings of this service, each over a collection of exactly [`COLLECTION_SIZE`]
/// persisted records, plus a request to the same listing whose collection is empty.
///
/// The domains collection is the [`COLLECTION_SIZE`] domains this Region holds; the
/// subscriptions collection is the [`COLLECTION_SIZE`] subscriptions of the first of
/// them. The empty page of the subscriptions listing is another of those domains, which
/// holds none; the empty page of the domains listing is this Region's `DELETED` domains,
/// of which there are none — so neither empty collection changes the size of the other
/// listing's.
async fn listings(emulator: &Emulator) -> Vec<Listing> {
    let mut domains: Vec<String> = Vec::new();
    for index in 0..COLLECTION_SIZE {
        let created = emulator
            .create_domain(&json!({ "name": format!("clinic-{index}") }))
            .await;
        domains.push(string_at(&created, "domainId"));
    }
    // The two domains the collections are built on: the ids are sorted so the parent is
    // the same record whatever order the ids were minted in.
    domains.sort();
    let parent = domains.first().expect("a created domain").clone();
    let childless = domains.last().expect("a second created domain").clone();
    assert_ne!(
        parent, childless,
        "the suite needs two distinct domains, got {domains:?}"
    );
    for _ in 0..COLLECTION_SIZE {
        emulator.create_subscription(&parent).await;
    }

    vec![
        Listing {
            operation: "ListDomains",
            base: "/domain".to_string(),
            member: "domains",
            key: "domainId",
            // `DomainStatus` is `ACTIVE | DELETED | DELETING`; no domain of this Region
            // has been deleted, so this filter selects nothing.
            empty: "/domain?status=DELETED".to_string(),
        },
        Listing {
            operation: "ListSubscriptions",
            base: format!("/domains/{parent}/subscriptions"),
            member: "subscriptions",
            key: "subscriptionId",
            empty: format!("/domains/{childless}/subscriptions"),
        },
    ]
}

#[tokio::test]
async fn both_listings_bound_a_page_the_same_way() {
    let emulator = Emulator::new(REGION);
    let listings = listings(&emulator).await;
    let (min, max) = PAGE_SIZE_RANGE;
    let held = i64::try_from(COLLECTION_SIZE).expect("a small collection");

    // (label, maxResults, status, how many records the page carries, whether it issues a
    // cursor). A page that is refused carries neither.
    let rows: [(&str, Option<i64>, u16, usize, bool); 9] = [
        ("absent means a full page", None, 200, COLLECTION_SIZE, false),
        ("the published minimum", Some(min), 200, 1, true),
        ("inside the bound", Some(held - 1), 200, COLLECTION_SIZE - 1, true),
        ("exactly the collection's size", Some(held), 200, COLLECTION_SIZE, false),
        ("the published maximum", Some(max), 200, COLLECTION_SIZE, false),
        ("one below the minimum", Some(min - 1), 400, 0, false),
        ("negative", Some(-7), 400, 0, false),
        ("one past the maximum", Some(max + 1), 400, 0, false),
        ("far past the maximum", Some(i64::from(i32::MAX)), 400, 0, false),
    ];

    for (label, max_results, expected_status, expected_len, expects_token) in rows {
        // What each listing answered, so the two can be compared as well as checked.
        let mut answers: Vec<(u16, Value)> = Vec::new();
        for listing in &listings {
            let case = format!("{} with maxResults={max_results:?}: {label}", listing.operation);
            let path = listing.request(&listing.base, max_results, None);
            let (status, body) = emulator.send("GET", &path, None).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
            );
            if expected_status == 200 {
                let keys = listing.keys(&case, &body);
                assert_eq!(
                    keys.len(),
                    expected_len,
                    "case {case:?}: expected {expected_len} records, got {keys:?}"
                );
                assert_eq!(
                    body.get("nextToken").is_some(),
                    expects_token,
                    "case {case:?}: expected a cursor: {expects_token}, got {body}"
                );
            } else {
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: got {body}"
                );
                let requirement = if max_results.expect("a refused row names a value") < min {
                    format!("Member must have value greater than or equal to {min}")
                } else {
                    format!("Member must have value less than or equal to {max}")
                };
                let text = detail_text(&case, &body);
                assert_eq!(
                    text,
                    range_sentence(
                        max_results.expect("a refused row names a value"),
                        "maxResults",
                        &requirement
                    ),
                    "case {case:?}: expected AWS's standard constraint sentence, got {text:?}"
                );
            }
            answers.push((status, body));
        }
        // One rule, two operations: a refusal reads the same on either listing's error
        // channel, and a served page bounds itself the same way.
        let (first_status, first_body) = answers.first().expect("the first listing answered");
        for (status, body) in answers.iter().skip(1) {
            assert_eq!(
                status, first_status,
                "case {label:?}: the two listings must agree on the status, got {first_status} and {status}"
            );
            if *status == 200 {
                continue;
            }
            assert_eq!(
                (error_code(body), detail_text(label, body)),
                (error_code(first_body), detail_text(label, first_body)),
                "case {label:?}: the two listings must refuse with the same code and text, \
                 got {first_body} and {body}"
            );
        }
    }
}

#[tokio::test]
async fn both_listings_resume_only_from_a_cursor_they_issued() {
    let emulator = Emulator::new(REGION);
    let listings = listings(&emulator).await;

    // A cursor each listing really issued, used below as the one the OTHER listing must
    // refuse.
    let mut issued: Vec<String> = Vec::new();
    for listing in &listings {
        let case = format!("{}: a first page of one", listing.operation);
        let path = listing.request(&listing.base, Some(1), None);
        let (status, body) = emulator.send("GET", &path, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} with {body}");
        issued.push(
            body.get("nextToken")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a page of one of {COLLECTION_SIZE} carries a cursor: {body}")
                })
                .to_string(),
        );
    }

    for (index, listing) in listings.iter().enumerate() {
        // The accepting half: paging with only the cursors this listing issued covers the
        // whole collection exactly once, at every page size from one to one past it, and
        // the last page carries no cursor.
        for page_size in 1..=COLLECTION_SIZE + 1 {
            let case = format!("{} paged at {page_size}", listing.operation);
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let path = listing.request(
                    &listing.base,
                    i64::try_from(page_size).ok(),
                    token.as_deref(),
                );
                let (status, body) = emulator.send("GET", &path, None).await;
                assert_eq!(status, 200, "case {case:?}: page {pages} got {status} {body}");
                let keys = listing.keys(&case, &body);
                assert!(
                    keys.len() <= page_size,
                    "case {case:?}: page {pages} carried {} records, got {keys:?}",
                    keys.len()
                );
                seen.extend(keys);
                pages += 1;
                assert!(
                    pages <= COLLECTION_SIZE + 2,
                    "case {case:?}: paging did not finish, saw {seen:?}"
                );
                token = body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                if token.is_none() {
                    break;
                }
            }
            let distinct: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                distinct.len(),
                seen.len(),
                "case {case:?}: a record was reported twice across pages, saw {seen:?}"
            );
            assert_eq!(
                distinct.len(),
                COLLECTION_SIZE,
                "case {case:?}: paging must cover every persisted record, saw {seen:?}"
            );
        }

        // An EMPTY page carries no cursor, whatever page size was asked for: there is
        // nothing left to resume to.
        for page_size in [None, Some(1), Some(PAGE_SIZE_RANGE.1)] {
            let case = format!("{} over an empty collection at {page_size:?}", listing.operation);
            let path = listing.request(&listing.empty, page_size, None);
            let (status, body) = emulator.send("GET", &path, None).await;
            assert_eq!(status, 200, "case {case:?}: got {status} with {body}");
            assert!(
                listing.keys(&case, &body).is_empty(),
                "case {case:?}: expected an empty page, got {body}"
            );
            assert!(
                body.get("nextToken").is_none(),
                "case {case:?}: an empty page must carry no cursor, got {body}"
            );
        }

        // The refusing half: every value this listing did not issue for THIS collection.
        let other = issued[(index + 1) % listings.len()].clone();
        let rows = [
            ("hex of invented text", hex::encode("dom-not-a-cursor")),
            ("not hex at all", "zz-not-a-token".to_string()),
            ("empty", String::new()),
            ("a cursor the other listing issued", other),
            ("far longer than any cursor", "61".repeat(600)),
        ];
        for (label, token) in rows {
            let case = format!("{} with a {label} cursor", listing.operation);
            let path = listing.request(&listing.base, None, Some(&token));
            let (status, body) = emulator.send("GET", &path, None).await;
            assert_eq!(
                status, 400,
                "case {case:?}: an unissued cursor must be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: got {body}"
            );
            let text = detail_text(&case, &body);
            assert_eq!(
                text, "nextToken is not a token this service issued",
                "case {case:?}: expected this service's cursor refusal, got {text:?}"
            );
        }
    }
}
