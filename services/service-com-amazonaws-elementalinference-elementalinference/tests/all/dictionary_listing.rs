// SPDX-License-Identifier: CC0-1.0

//! `ListDictionaries` end to end over the wire: the persisted dictionary store,
//! enumerated one page at a time.
//!
//! Every case here drives the real request path through the shared harness
//! (`tests/all/harness.rs`) — the router picks the service from the SigV4 credential
//! scope, the framework deserializes and validates the query string, the handler runs,
//! and the response is serialized as `restJson1` — so what these tests assert is what a
//! client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! case:
//!
//! * `ListDictionaries` reports EVERY persisted dictionary and nothing else, in one
//!   stable id order, at most `maxResults` per page, with a `nextToken` exactly while a
//!   further dictionary remains;
//! * a `DictionarySummary` is a projection of the STORED dictionary — the same `arn`,
//!   `id`, `name`, `language` and reported `status` that `GetDictionary` answers with,
//!   for every lifecycle status a dictionary holds;
//! * a `nextToken` names a POSITION in that order: one that is not a pagination token at
//!   all is refused as HTTP 400 `ValidationException`, and a well-formed one resumes
//!   after its position;
//! * a `maxResults` outside the modelled range never reaches the handler;
//! * listing is a READ: it changes nothing a later caller sees.

use crate::harness::{
    call, expected_dictionary_arn, past_the_transition_span, router, subtitling_output,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Every value the `DictionaryLanguage` enum admits, from
/// `smithy_query(query="shape",shape="DictionaryLanguage")`: `eng`, `fra`, `ita`,
/// `deu`, `spa`, `por`.
const LANGUAGES: &[&str] = &["eng", "fra", "ita", "deu", "spa", "por"];

/// The page size `ListDictionaries` answers with when the request omits `maxResults`.
///
/// `smithy_query(query="operation",operation="ListDictionaries")` gives `maxResults`
/// the range `{min: 1, max: 100}` and the member documents no default, so a request
/// that asks for no page size is answered with at most the modelled maximum.
const DEFAULT_PAGE_SIZE: usize = 100;

/// The five members the model gives `DictionarySummary`
/// (`smithy_query(query="shape",shape="DictionarySummary")`: `arn`, `id`, `name`,
/// `language`, `status` — all required).
const SUMMARY_MEMBERS: &[&str] = &["arn", "id", "name", "language", "status"];

/// Create one dictionary through the wire path and return its id.
async fn create_dictionary(router: &MultiServiceRouter, name: &str, language: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": name, "language": language})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: CreateDictionary answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateDictionary returns an id, got {created}"))
        .to_string()
}

/// `GET /v1/dictionaries` with the `maxResults` and `nextToken` the case asks for.
async fn list_dictionaries(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut query: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = if query.is_empty() {
        "/v1/dictionaries".to_string()
    } else {
        format!("/v1/dictionaries?{}", query.join("&"))
    };
    call(router, "GET", &uri, None).await
}

/// ListDictionaries pages over every persisted dictionary: at most `maxResults` per
/// page (the modelled maximum when the request omits it), one stable id order, each
/// dictionary exactly once, and a `nextToken` only while more remain.
#[tokio::test]
async fn list_dictionaries_pages_over_every_persisted_dictionary() {
    // (dictionaries created, maxResults requested) — spanning an EMPTY store, no
    // pagination at all, a page size of one, a size that does not divide the count, an
    // omitted page size, and a page larger than the whole collection.
    let cases: [(usize, Option<i32>); 7] = [
        (0, None),
        (1, Some(1)),
        (3, Some(1)),
        (5, Some(2)),
        (7, None),
        (12, Some(5)),
        (4, Some(100)),
    ];

    for (count, max_results) in cases {
        let label = format!("{count} dictionaries, maxResults={max_results:?}");
        let router = router();
        // name and language per row index, so the summaries cannot all be alike.
        let mut expected: Vec<(String, String, String)> = Vec::new();
        for index in 0..count {
            let name = format!("listdict{index}");
            let language = LANGUAGES[index % LANGUAGES.len()];
            let id = create_dictionary(&router, &name, language).await;
            expected.push((id, name, language.to_string()));
        }
        // Past the transition span every created dictionary has settled, so the status
        // each summary must report is AVAILABLE.
        past_the_transition_span();

        let page_size = max_results.map_or(DEFAULT_PAGE_SIZE, |max| max as usize);
        // An empty store still answers one page: an empty list and no token.
        let expected_pages = count.div_ceil(page_size).max(1);

        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0usize;
        let mut next_token: Option<String> = None;
        loop {
            let (status, page) =
                list_dictionaries(&router, max_results, next_token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {label:?}: ListDictionaries answers 200, got {status} with {page}"
            );
            pages += 1;
            assert!(
                pages <= expected_pages,
                "case {label:?}: pagination terminates in {expected_pages} pages, got page \
                 {pages}: {page}"
            );

            let dictionaries = page["dictionaries"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {label:?}: the response carries a dictionaries list, got {page}")
                })
                .clone();
            assert!(
                dictionaries.len() <= page_size,
                "case {label:?}: a page holds at most {page_size} dictionaries, got {}",
                dictionaries.len()
            );

            for summary in &dictionaries {
                for member in SUMMARY_MEMBERS {
                    assert!(
                        summary[*member].is_string(),
                        "case {label:?}: a summary carries the required member {member:?}, got \
                         {summary}"
                    );
                }
                let id = summary["id"].as_str().unwrap_or_default().to_string();
                let row = expected
                    .iter()
                    .find(|(created_id, _, _)| created_id == &id)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {label:?}: every listed dictionary was created by this test, \
                             got {summary}"
                        )
                    });
                assert_eq!(
                    summary["arn"],
                    json!(expected_dictionary_arn(&id)),
                    "case {label:?}: the summary carries the stored dictionary's ARN, got {summary}"
                );
                assert_eq!(
                    summary["name"],
                    json!(row.1),
                    "case {label:?}: the summary carries the stored name, got {summary}"
                );
                assert_eq!(
                    summary["language"],
                    json!(row.2),
                    "case {label:?}: the summary carries the stored language, got {summary}"
                );
                assert_eq!(
                    summary["status"],
                    json!("AVAILABLE"),
                    "case {label:?}: a settled dictionary is listed as AVAILABLE, got {summary}"
                );
                seen.push(id);
            }

            next_token = page
                .get("nextToken")
                .and_then(|token| token.as_str())
                .map(str::to_string);
            match &next_token {
                // A token is present only while a further dictionary remains, so the
                // page that carried one was full.
                Some(_) => assert_eq!(
                    dictionaries.len(),
                    page_size,
                    "case {label:?}: a page that carries a nextToken is a full page, got {page}"
                ),
                None => break,
            }
        }

        assert_eq!(
            pages, expected_pages,
            "case {label:?}: {count} dictionaries at {page_size} per page is {expected_pages} \
             pages, got {pages}"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {label:?}: the pages run in one ascending id order, got {seen:?}"
        );
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            count,
            "case {label:?}: every persisted dictionary is listed exactly once, got {seen:?}"
        );
    }
}

/// A summary's `status` is the dictionary's REPORTED status — the same one
/// `GetDictionary` answers with — for every lifecycle status a dictionary holds:
/// `CREATING` while creation is still due, `AVAILABLE` once it settles, `REFERENCED`
/// while a feed output names it, and `DELETING` once a removal is accepted.
#[tokio::test]
async fn a_summary_reports_the_status_get_dictionary_reports() {
    // (how the dictionary reaches the status, the status it then reports)
    let rows = [
        ("read straight after creation", "CREATING"),
        ("creation has fallen due", "AVAILABLE"),
        ("a feed output names it", "REFERENCED"),
        ("a deletion has been accepted", "DELETING"),
    ];

    for (via, expected_status) in rows {
        let label = format!("{via} -> {expected_status}");
        let router = router();
        let id = create_dictionary(&router, "statusdict", "eng").await;

        match via {
            "read straight after creation" => {}
            "creation has fallen due" => past_the_transition_span(),
            "a feed output names it" => {
                past_the_transition_span();
                let (status, feed) = call(
                    &router,
                    "POST",
                    "/v1/feed",
                    Some(json!({
                        "name": "referencing-feed",
                        "outputs": [subtitling_output("subs1", Some(&id))],
                    })),
                )
                .await;
                assert_eq!(
                    status, 202,
                    "case {label:?}: CreateFeed answers 202, got {status} with {feed}"
                );
            }
            "a deletion has been accepted" => {
                past_the_transition_span();
                let (status, deleted) =
                    call(&router, "DELETE", &format!("/v1/dictionary/{id}"), None).await;
                assert_eq!(
                    status, 202,
                    "case {label:?}: DeleteDictionary answers 202, got {status} with {deleted}"
                );
            }
            other => panic!("no such status row: {other}"),
        }

        let (status, page) = list_dictionaries(&router, None, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListDictionaries answers 200, got {status} with {page}"
        );
        let summary = page["dictionaries"]
            .as_array()
            .and_then(|list| list.iter().find(|item| item["id"] == json!(id)))
            .unwrap_or_else(|| {
                panic!("case {label:?}: the dictionary is listed while it is live, got {page}")
            })
            .clone();
        assert_eq!(
            summary["status"],
            json!(expected_status),
            "case {label:?}: the summary reports {expected_status}, got {summary}"
        );

        // And the list and the get cannot disagree about one stored dictionary.
        let (status, fetched) = call(&router, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetDictionary answers 200, got {status} with {fetched}"
        );
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                summary[*member], fetched[*member],
                "case {label:?}: the summary's {member:?} is what GetDictionary reports, got \
                 {summary} against {fetched}"
            );
        }
    }
}

/// A `nextToken` that is not a pagination token at all cannot name a page, so
/// ListDictionaries refuses it as a 400 `ValidationException` rather than silently
/// serving the first page; a well-formed token names a POSITION in the id order.
#[tokio::test]
async fn a_next_token_that_is_not_a_pagination_token_is_refused() {
    let router = router();
    // Not hex at all, hex digits that do not pair up, and a byte that is no hex digit.
    for token in ["not-a-token", "zz", "0", "6g"] {
        let label = format!("nextToken={token}");
        let (status, error) = list_dictionaries(&router, None, Some(token)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a malformed nextToken is a 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {error}"
        );
    }

    let first = create_dictionary(&router, "firstdict", "eng").await;
    let second = create_dictionary(&router, "seconddict", "fra").await;
    let mut ids = [first, second];
    ids.sort();
    past_the_transition_span();

    // A well-formed token names a position, not a dictionary that must still exist:
    // "0" sorts before every id the `DictionaryId` pattern admits, so a page resuming
    // after it starts at the first dictionary in order.
    let before_every_id = hex::encode("0");
    let after_the_first = hex::encode(&ids[0]);
    let after_the_last = hex::encode(&ids[1]);
    for (token, expected_first) in [
        (before_every_id.as_str(), Some(ids[0].clone())),
        // Resuming after the first id continues at the second.
        (after_the_first.as_str(), Some(ids[1].clone())),
        // Resuming after the last id leaves nothing to report.
        (after_the_last.as_str(), None),
    ] {
        let label = format!("nextToken={token}");
        let (status, page) = list_dictionaries(&router, None, Some(token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a well-formed token is served, got {status} with {page}"
        );
        let listed: Vec<String> = page["dictionaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: the response carries a list, got {page}"))
            .iter()
            .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
            .collect();
        match &expected_first {
            Some(id) => assert_eq!(
                listed.first(),
                Some(id),
                "case {label:?}: paging resumes at the next id in order, got {listed:?}"
            ),
            None => assert!(
                listed.is_empty(),
                "case {label:?}: a token past the last id reports nothing, got {listed:?}"
            ),
        }
        assert!(
            page.get("nextToken").is_none_or(Value::is_null),
            "case {label:?}: a page with nothing left to fetch carries no nextToken, got {page}"
        );
    }
}

/// A `maxResults` outside the modelled range never reaches the handler: the boundary
/// refuses it, so no request can ask for an unbounded page.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    create_dictionary(&router, "rangedict", "eng").await;
    // `smithy_query(query="operation",operation="ListDictionaries")` gives maxResults
    // the range {min: 1, max: 100}: each published bound is accepted, one past it is
    // not.
    for (max_results, expected) in [(1, 200), (100, 200), (0, 400), (101, 400)] {
        let label = format!("maxResults={max_results}");
        let (status, body) = list_dictionaries(&router, Some(max_results), None).await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} with {body}"
        );
    }
}

/// Listing is a READ: it reports the stored dictionaries without moving any of them,
/// so two lists with nothing in between are identical and every listed dictionary
/// still reads back exactly as the list described it.
#[tokio::test]
async fn listing_dictionaries_changes_nothing_a_later_caller_sees() {
    // Dictionaries that owe the clock nothing (settled, and settled-and-referenced),
    // so the only thing that could change what the second list reports is the first
    // list itself.
    let router = router();
    let settled = create_dictionary(&router, "settleddict", "ita").await;
    let referenced = create_dictionary(&router, "referenceddict", "deu").await;
    let untouched = create_dictionary(&router, "untoucheddict", "spa").await;
    past_the_transition_span();
    let (status, feed) = call(
        &router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "referencing-feed",
            "outputs": [subtitling_output("subs1", Some(&referenced))],
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateFeed answers 202, got {status} with {feed}"
    );
    // The feed's own creation has fallen due too, so nothing in this store is waiting
    // on the clock while the lists below run.
    past_the_transition_span();

    let (_, first) = list_dictionaries(&router, None, None).await;
    let (_, second) = list_dictionaries(&router, None, None).await;
    assert_eq!(
        first, second,
        "two lists with nothing in between report the same page, got {first} then {second}"
    );

    for id in [&settled, &referenced, &untouched] {
        let (status, fetched) = call(&router, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {id:?}: the dictionary still reads back after the lists, got {status} with \
             {fetched}"
        );
        let summary = first["dictionaries"]
            .as_array()
            .and_then(|list| list.iter().find(|item| item["id"] == json!(id)))
            .unwrap_or_else(|| panic!("case {id:?}: the dictionary is listed, got {first}"));
        assert_eq!(
            summary["status"], fetched["status"],
            "case {id:?}: the listed status is the one still stored, got {summary} against \
             {fetched}"
        );
    }
    // Nothing was consumed either: the referenced dictionary is still REFERENCED and
    // the others still AVAILABLE after both lists.
    let (_, third) = list_dictionaries(&router, None, None).await;
    assert_eq!(
        third, first,
        "a third list still reports the same page, got {third} against {first}"
    );
}
