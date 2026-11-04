// SPDX-License-Identifier: CC0-1.0

//! `ListFilters`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `ListFilters` names exactly the filters the request's detector holds — every one
//!    `CreateFilter` saved and nothing else — in filter-name order, and each name it
//!    reports resolves through `GetFilter`.
//! 2. It is PAGINATED (`smithy_query(operation="ListFilters")` reports
//!    `inputToken`/`outputToken` `NextToken`, `items` `FilterNames`, `pageSize`
//!    `MaxResults`): walking the pages with any page size yields each name exactly once
//!    in the unpaged order, the last page carries no `NextToken`, and no page is longer
//!    than `FilterNames`'s own `len 0..50` whatever the caller asks for.
//! 3. It REPORTS: listing changes nothing a later call sees.
//! 4. A filter belongs to one detector, which is regional, so a list names only the
//!    filters that detector holds, through the Region's endpoint that created it.
//! 5. A `detectorId` the store does not resolve, and a `nextToken` this service never
//!    issued, are refused with HTTP 400 under the `InvalidInputException` class naming
//!    the member at fault — never a hollow empty page, and never echoing the caller's
//!    value.

use serde_json::{json, Value};

use crate::client_rejections::page_token_refusal;
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::DETECTOR_ID_MEMBER;

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// `FilterNames` is `len 0..50` and `MaxResults` is `range 1..50`
/// (`smithy_query(operation="ListFilters")`), so one page carries at most 50 names
/// however many filters the detector holds.
const FILTER_NAMES_MAX: usize = 50;

/// A `FindingCriteria` every create can send: `CreateFilter.FindingCriteria` is
/// required (`smithy_query(operation="CreateFilter")`).
fn criteria() -> Value {
    json!({"criterion": {"severity": {"greaterThanOrEqual": 1}}})
}

async fn create_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(json!({"name": name, "findingCriteria": criteria()})),
    )
    .await
}

/// Save a filter, failing the test if the create did not succeed.
async fn saved(router: &Router, detector_id: &str, name: &str, case: &str) {
    let (status, body) = create_filter(router, detector_id, name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-filter {name:?} expected 200, got {status} {body:?}"
    );
}

/// One `ListFilters` call. `query` is the raw query string (`""` for none).
async fn list_filters(router: &Router, detector_id: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/filter{query}"),
        None,
    )
    .await
}

/// The `FilterNames` of one page, failing the test if the list did not succeed.
async fn page_names(router: &Router, detector_id: &str, query: &str, case: &str) -> Vec<String> {
    let (status, body) = list_filters(router, detector_id, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-filters{query} expected 200, got {status} {body:?}"
    );
    names_in(&body, case)
}

fn names_in(body: &Value, case: &str) -> Vec<String> {
    body["filterNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a filterNames array, got {body:?}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a filter name, got {value:?}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of `ListFilters` with `page_size` (`None` asks for the service's
/// own page size) and return the names in the order the pages reported them, plus the
/// length of each page.
async fn walk_pages(
    router: &Router,
    detector_id: &str,
    page_size: Option<i64>,
    case: &str,
) -> (Vec<String>, Vec<usize>) {
    let mut names = Vec::new();
    let mut page_lengths = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token the service never advances cannot loop forever.
    for _ in 0..512 {
        let mut query = String::new();
        if let Some(size) = page_size {
            query.push_str(&format!("?maxResults={size}"));
        }
        if let Some(token) = token.as_deref() {
            query.push_str(if query.is_empty() { "?" } else { "&" });
            query.push_str(&format!("nextToken={token}"));
        }
        let (status, body) = list_filters(router, detector_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: list-filters{query} expected 200, got {status} {body:?}"
        );
        let page = names_in(&body, case);
        assert!(
            page.len() <= FILTER_NAMES_MAX,
            "case {case:?}: a page must carry at most {FILTER_NAMES_MAX} names, got {}",
            page.len()
        );
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: a page must not exceed maxResults={size}, got {}",
                page.len()
            );
        }
        page_lengths.push(page.len());
        names.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return (names, page_lengths),
        }
    }
    panic!("case {case:?}: list-filters never ran out of pages");
}

/// One row: the case it stands for and the filter names created on a detector of its
/// own, in the order the creates happen.
///
/// The rows span what the rule generalises over — COUNT (none, one, several, more than
/// one page), the `Name` alphabet ("Valid characters include period (.), underscore
/// (_), dash (-), and alphanumeric characters",
/// `smithy_query(query="doc",shape="CreateFilterRequest",member="Name")`), SIZE (both
/// ends of `len 3..64`) and ORDER (created in an order that is not the order a list
/// reports).
fn name_rows() -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("a detector holding no filters", Vec::new()),
        (
            "a single filter at the minimum name length",
            vec!["abc".to_string()],
        ),
        (
            "names created in an order that is not sorted order",
            vec![
                "zeta".to_string(),
                "alpha".to_string(),
                "mid".to_string(),
                "Beta".to_string(),
            ],
        ),
        (
            "the documented name alphabet: periods, underscores, dashes, digits",
            vec![
                "high.severity_findings-01".to_string(),
                "0-leading-digit".to_string(),
                "UPPER_lower.9".to_string(),
                "a.b".to_string(),
            ],
        ),
        (
            "the longest and the shortest name the model accepts, side by side",
            vec!["n".repeat(64), "n".repeat(3), "n".repeat(32)],
        ),
        (
            "more filters than one page can carry",
            (0..FILTER_NAMES_MAX + 5)
                .map(|index| format!("paged-{index:03}"))
                .collect(),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Rule 1: the list names exactly the stored filters, in filter-name order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_names_every_stored_filter_in_order() {
    for (case, created) in name_rows() {
        let router = new_router();
        let detector = enabled_detector(&router).await;

        // Before any create the list is empty — not absent, not canned.
        let empty = page_names(&router, &detector, "", case).await;
        assert!(
            empty.is_empty(),
            "case {case:?}: a detector with no filters must list nothing, got {empty:?}"
        );

        for name in &created {
            saved(&router, &detector, name, case).await;
        }

        // Filter-name order over every name the creates saved: the names a caller
        // gets back are the stored keys, sorted, whatever order the creates arrived
        // in.
        let mut expected = created.clone();
        expected.sort();
        let (listed, _) = walk_pages(&router, &detector, None, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the stored names in filter-name order {expected:?}, got {listed:?}"
        );

        // Every name the list reports resolves through the implemented reader, so the
        // list is the stored filters and not a synthesised answer.
        for name in &listed {
            let (status, body) = call(
                &router,
                "GET",
                &format!("/detector/{detector}/filter/{name}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: get-filter {name:?} expected 200, got {status} {body:?}"
            );
            let reported = body["name"].as_str().unwrap_or_default();
            assert_eq!(
                reported, name,
                "case {case:?}: get-filter must report the listed name {name:?}, got {reported:?}"
            );
        }
    }
}

/// A filter a create did NOT save is never named, and a create the service refused
/// leaves the list exactly as it was.
#[tokio::test]
async fn a_refused_create_adds_nothing_to_the_list() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    saved(&router, &detector, "kept.filter", "the stored filter").await;

    // Each row: the case, and a create the service refuses — a name outside the
    // modelled `len 3..64`, a name outside the documented alphabet, and a duplicate
    // of the name already held.
    let rows: Vec<(&str, String)> = vec![
        ("a name shorter than the modelled minimum", "ab".to_string()),
        ("a name longer than the modelled maximum", "n".repeat(65)),
        ("a name carrying whitespace", "has a space".to_string()),
        ("a name already held by this detector", "kept.filter".to_string()),
    ];

    for (case, name) in rows {
        let (status, body) = create_filter(&router, &detector, &name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused with 400, got {status} {body:?}"
        );
        let listed = page_names(&router, &detector, "", case).await;
        assert_eq!(
            listed,
            vec!["kept.filter".to_string()],
            "case {case:?}: a refused create must not change the list, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: paging with any page size yields each name exactly once, in order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_yields_each_filter_exactly_once_in_order() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // More filters than one page can carry, so every page size below is exercised on
    // a list that really spans pages.
    let created: Vec<String> = (0..FILTER_NAMES_MAX + 7)
        .map(|index| format!("page-{index:03}"))
        .collect();
    for name in &created {
        saved(&router, &detector, name, "the paged detector").await;
    }
    let mut expected = created.clone();
    expected.sort();

    // The page sizes span `MaxResults`'s whole modelled range (`range 1..50`): both
    // published bounds, the divisors and non-divisors of the list length in between,
    // and the service's own page size when the caller names none.
    let mut page_sizes: Vec<Option<i64>> = (1..=FILTER_NAMES_MAX as i64).map(Some).collect();
    page_sizes.push(None);

    for size in page_sizes {
        let case = match size {
            Some(size) => format!("maxResults={size}"),
            None => "no maxResults".to_string(),
        };
        let (listed, page_lengths) = walk_pages(&router, &detector, size, &case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: paging must yield every name exactly once in order, got {listed:?}"
        );
        // Every page but the last is full: that is what makes the token walk
        // terminate in the number of calls a client budgets for.
        let per_page = size.unwrap_or(FILTER_NAMES_MAX as i64).min(FILTER_NAMES_MAX as i64) as usize;
        for (index, length) in page_lengths.iter().enumerate() {
            if index + 1 < page_lengths.len() {
                assert_eq!(
                    *length, per_page,
                    "case {case:?}: page {index} must be full at {per_page}, got {length}"
                );
            } else {
                assert!(
                    *length <= per_page && *length > 0,
                    "case {case:?}: the last page must carry 1..={per_page} names, got {length}"
                );
            }
        }
    }

    // A list that fits in one page carries no token at all, at every page size that
    // covers it.
    let short = new_router();
    let short_detector = enabled_detector(&short).await;
    for name in ["one.filter", "two.filter"] {
        saved(&short, &short_detector, name, "the short detector").await;
    }
    for size in [2, 3, FILTER_NAMES_MAX as i64] {
        let case = format!("a complete page at maxResults={size}");
        let (status, body) = list_filters(&short, &short_detector, &format!("?maxResults={size}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a complete page must carry no nextToken, got {:?}",
            body["nextToken"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: listing reports; it changes nothing a later call sees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_changes_nothing_a_later_call_sees() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    for name in ["c.filter", "a.filter", "b.filter"] {
        saved(&router, &detector, name, "the reported detector").await;
    }

    let first = page_names(&router, &detector, "", "the first list").await;
    // Several lists, some of them paged, in between.
    for size in [1, 2, 3] {
        let case = format!("an interleaved page at maxResults={size}");
        walk_pages(&router, &detector, Some(size), &case).await;
    }
    let again = page_names(&router, &detector, "", "the list after the walks").await;
    assert_eq!(
        again, first,
        "case \"a repeated list\": listing must not change what a later list sees, got {again:?} after {first:?}"
    );

    // Nor what a reader sees: the stored filters still report their own metadata,
    // unchanged by any number of lists.
    for name in &first {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/detector/{detector}/filter/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: get-filter after the lists expected 200, got {status} {body:?}"
        );
        let version = body["version"].as_i64().unwrap_or_default();
        assert_eq!(
            version, 1,
            "case {name:?}: listing must not version a filter, got {version}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: filters belong to one detector, served by one Region.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_is_scoped_to_its_detector_and_region() {
    let router = new_router();
    let home = enabled_detector(&router).await;
    for name in ["home-one", "home-two"] {
        saved(&router, &home, name, "the us-east-1 detector").await;
    }

    // A detector of its own in another Region, with filters of its own: one Region's
    // list must name neither the other Region's filters nor its detector.
    let (status, body) = call_in(&router, "eu-west-1", "POST", "/detector", Some(json!({"enable": true}))).await;
    assert_eq!(
        status, 200,
        "case \"create-detector in eu-west-1\": expected 200, got {status} {body:?}"
    );
    let away = body["detectorId"]
        .as_str()
        .expect("a detectorId in eu-west-1")
        .to_string();
    for name in ["away-one", "away-two", "away-three"] {
        let (status, body) = call_in(
            &router,
            "eu-west-1",
            "POST",
            &format!("/detector/{away}/filter"),
            Some(json!({"name": name, "findingCriteria": criteria()})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: create-filter in eu-west-1 expected 200, got {status} {body:?}"
        );
    }

    let home_names = page_names(&router, &home, "", "the us-east-1 list").await;
    assert_eq!(
        home_names,
        vec!["home-one".to_string(), "home-two".to_string()],
        "case \"the us-east-1 list\": expected only this detector's filters, got {home_names:?}"
    );

    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "GET",
        &format!("/detector/{away}/filter"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the eu-west-1 list\": expected 200, got {status} {body:?}"
    );
    let away_names = names_in(&body, "the eu-west-1 list");
    assert_eq!(
        away_names,
        vec![
            "away-one".to_string(),
            "away-three".to_string(),
            "away-two".to_string()
        ],
        "case \"the eu-west-1 list\": expected only that detector's filters in order, got {away_names:?}"
    );

    // Each detector is served only through the Region that created it.
    for (region, detector_id) in [("eu-west-1", home.as_str()), ("us-west-2", away.as_str())] {
        let case = format!("{detector_id} through {region}");
        let (status, body) = call_in(
            &router,
            region,
            "GET",
            &format!("/detector/{detector_id}/filter"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: another Region's endpoint must not list the filters, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: an unresolvable detector and an unissued token are classified 400s.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_detector_or_token_is_refused_without_echoing_it() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    for name in ["refusal-one", "refusal-two"] {
        saved(&router, &detector, name, "the stored filters").await;
    }

    // Each row: the case, the detectorId the list uses, the query it carries, and the
    // member the refusal must name — `None` where the refusal's own class names none,
    // which is how a page token this service did not issue is refused.
    let rows: Vec<(&str, String, String, Option<&str>)> = vec![
        (
            "a well-formed detectorId no create minted",
            ABSENT_DETECTOR_ID.to_string(),
            String::new(),
            Some(DETECTOR_ID_MEMBER),
        ),
        (
            "a value that cannot be a detectorId at all",
            "not-a-detector".to_string(),
            String::new(),
            Some(DETECTOR_ID_MEMBER),
        ),
        (
            "a detectorId at the modelled minimum length",
            "x".to_string(),
            String::new(),
            Some(DETECTOR_ID_MEMBER),
        ),
        (
            "a nextToken that is not a token this service issues",
            detector.clone(),
            "?nextToken=not-a-token".to_string(),
            None,
        ),
        (
            "a nextToken pointing past the end of the list",
            detector.clone(),
            "?nextToken=9999".to_string(),
            None,
        ),
        (
            "a nextToken pointing at the entry just past the last one",
            detector.clone(),
            "?nextToken=2".to_string(),
            None,
        ),
    ];

    for (case, detector_id, query, member) in rows {
        let (status, body) = list_filters(&router, &detector_id, &query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        let class = error_code(&body);
        assert_eq!(
            class, INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        match member {
            Some(member) => assert!(
                message.contains(member),
                "case {case:?}: the refusal must name {member}, got {message:?}"
            ),
            None => assert_eq!(
                message,
                page_token_refusal(),
                "case {case:?}: expected the page-token refusal, got {message:?}"
            ),
        }
        for echoed in ["not-a-detector", "not-a-token", "9999", ABSENT_DETECTOR_ID] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
            );
        }
        // The refusal leaves the store alone: the stored filters still list.
        let listed = page_names(&router, &detector, "", case).await;
        assert_eq!(
            listed,
            vec!["refusal-one".to_string(), "refusal-two".to_string()],
            "case {case:?}: a refusal must not change the list, got {listed:?}"
        );
    }

    // The token the service DOES issue is accepted on the same list: the refusals
    // above are about tokens it never handed out, not about paging itself.
    let (status, body) = list_filters(&router, &detector, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case \"the first page\": expected 200, got {status} {body:?}"
    );
    let token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"the first page\": expected a nextToken, got {body:?}"));
    let second = page_names(
        &router,
        &detector,
        &format!("?maxResults=1&nextToken={token}"),
        "the issued token",
    )
    .await;
    assert_eq!(
        second,
        vec!["refusal-two".to_string()],
        "case \"the issued token\": expected the second page, got {second:?}"
    );
}
