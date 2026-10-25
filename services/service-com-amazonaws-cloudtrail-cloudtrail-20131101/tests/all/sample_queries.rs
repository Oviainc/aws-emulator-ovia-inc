// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SearchSampleQueries` — the CloudTrail Lake sample-query
//! library.
//!
//! The rules under test:
//!
//! * a search answers entries of the service's CURATED, service-global sample-query
//!   catalog and nothing else: every result carries that entry's own `Name`,
//!   `Description` and `SQL`;
//! * "a list of objects containing the search results ordered from most relevant to
//!   least relevant": the page is in non-increasing `Relevance`, and an entry that
//!   carries more of the phrase outranks one that carries less — so each catalog
//!   entry is the FIRST result for a search of its own name, and a topical phrase
//!   finds the entry about that topic;
//! * an entry carrying none of the phrase's terms is not a result at all;
//! * "The maximum number of results to return on a single page. The default value is
//!   10": a page holds at most `MaxResults` results, 10 when the request names none,
//!   and `MaxResults` outside the modelled `@range(1, 50)` is refused at the
//!   boundary;
//! * `NextToken` appears exactly when matches remain, and the pages of one phrase
//!   partition that phrase's ranking exactly once, whatever the page size;
//! * a `NextToken` belongs to the ranking it was issued for: a token this operation
//!   never handed out, or one replayed with a different `SearchPhrase`, is refused
//!   with the modelled `InvalidParameterException` (HTTP 400);
//! * the search is a READ: it persists nothing, so two identical requests answer
//!   identically, and phrasings that differ only in case or in English filler rank
//!   the catalog the same way.
//!
//! The ordering and pagination rules are invariants, and the catalog is a fixed,
//! small set — so these cases run them EXHAUSTIVELY (every catalog entry, every page
//! size that partitions the ranking differently) rather than sampling them randomly.
//!
//! The MAGNITUDE of a `Relevance` is deliberately not asserted beyond its bounds and
//! its order: the real service ranks with a semantic model this emulator cannot run,
//! and `ops/search_sample_queries.rs` documents the lexical stand-in that replaces
//! it.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::search_sample_queries::SAMPLE_QUERY_CATALOG;

use super::emulator::{Emulator, Reply};

/// One `SearchSampleQueries` call, carrying only the members a case sets.
async fn search(
    emulator: &Emulator,
    phrase: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Reply {
    let mut body = json!({ "SearchPhrase": phrase });
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["NextToken"] = json!(next_token);
    }
    emulator.call("SearchSampleQueries", &body).await
}

/// The `SearchResults` of a successful reply.
fn results_of(case: &str, body: &Value) -> Vec<Value> {
    body["SearchResults"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SearchResults list, got {body:?}"))
        .clone()
}

/// The `Name` of each result, in the order the response carries them.
fn names_of(results: &[Value]) -> Vec<String> {
    results
        .iter()
        .map(|result| result["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert everything that holds of EVERY page, whatever the phrase: each result is a
/// catalog entry reported with the catalog's own text, each `Relevance` is inside
/// `0.0 < r <= 1.0`, and the page is in non-increasing `Relevance`.
fn check_page(case: &str, results: &[Value]) {
    let mut previous = f64::INFINITY;
    for result in results {
        let name = result["Name"].as_str().unwrap_or_default();
        let entry = SAMPLE_QUERY_CATALOG
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {name:?} is not a catalog entry, got {result:?}")
            });
        assert_eq!(
            result["Description"].as_str(),
            Some(entry.description),
            "case {case:?}: {name:?} must report the catalog's description, got {result:?}"
        );
        assert_eq!(
            result["SQL"].as_str(),
            Some(entry.sql),
            "case {case:?}: {name:?} must report the catalog's SQL, got {result:?}"
        );
        let relevance = result["Relevance"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: {name:?} must report a numeric Relevance, got {result:?}")
        });
        assert!(
            relevance > 0.0 && relevance <= 1.0,
            "case {case:?}: {name:?} must carry a relevance in 0.0 < r <= 1.0, got {relevance}"
        );
        assert!(
            relevance <= previous,
            "case {case:?}: results must be ordered from most to least relevant, \
             but {name:?} scores {relevance} after {previous}"
        );
        previous = relevance;
    }
}

/// A phrase every catalog entry's SQL carries and nothing else does, so it ranks the
/// WHOLE catalog at one relevance: the ranking is then the catalog's name order, which
/// makes it the sharpest input for the pagination rules.
const WHOLE_CATALOG_PHRASE: &str = "select";

/// The page size a request that names no `MaxResults` must get, as the MODEL documents
/// it (`SearchSampleQueriesRequest$MaxResults`: "The default value is 10.") rather than
/// as the handler's own constant — so a change of that constant fails this case.
const MODELLED_DEFAULT_PAGE_SIZE: usize = 10;

/// The whole catalog's names in the order a search ranks them when every entry scores
/// the same: the handler breaks a relevance tie on the entry name.
fn catalog_names_in_tie_order() -> Vec<String> {
    let mut names: Vec<String> = SAMPLE_QUERY_CATALOG
        .iter()
        .map(|entry| entry.name.to_string())
        .collect();
    names.sort();
    names
}

/// RULE: an entry that carries more of the phrase outranks one that carries less, so
/// searching an entry's own name puts THAT entry first.
///
/// The row set is the whole catalog — every entry, including the ones no issue cited.
#[tokio::test]
async fn every_catalog_entry_is_the_top_result_for_its_own_name() {
    let emulator = Emulator::new();

    for entry in SAMPLE_QUERY_CATALOG {
        let case = format!("searching the catalog name {:?}", entry.name);
        let reply = search(&emulator, entry.name, Some(50), None).await;
        let body = reply.expect_ok(&case);
        let results = results_of(&case, body);
        check_page(&case, &results);
        assert_eq!(
            names_of(&results).first().map(String::as_str),
            Some(entry.name),
            "case {case:?}: the entry whose name was searched must rank first, got {:?}",
            names_of(&results)
        );
    }
}

/// RULE: a natural-language phrase finds the catalog entry about that topic, not just
/// one whose name it repeats.
///
/// Each row is a phrase a caller might type, paired with the entry it must surface;
/// the rows span distinct topics of the catalog (identity, storage, encryption,
/// compute, networking, sign-in).
#[tokio::test]
async fn a_topical_phrase_surfaces_the_entry_about_that_topic() {
    let emulator = Emulator::new();

    let rows: &[(&str, &str)] = &[
        (
            "console logins without MFA",
            "Console sign-ins without multi-factor authentication",
        ),
        ("find deleted S3 buckets", "Amazon S3 buckets deleted"),
        (
            "security group ingress rules changed",
            "Security group rule changes",
        ),
        (
            "KMS key scheduled for deletion",
            "KMS keys disabled or scheduled for deletion",
        ),
        (
            "lambda function code updates",
            "AWS Lambda functions created or updated",
        ),
        (
            "which identities were denied an API call",
            "Access denied errors by identity",
        ),
        (
            "failed sign-in attempts from an unknown IP",
            "Failed console sign-in attempts",
        ),
    ];

    for (phrase, expected) in rows {
        let case = format!("searching {phrase:?}");
        let reply = search(&emulator, phrase, Some(50), None).await;
        let body = reply.expect_ok(&case);
        let results = results_of(&case, body);
        check_page(&case, &results);
        assert_eq!(
            names_of(&results).first().map(String::as_str),
            Some(*expected),
            "case {case:?}: expected {expected:?} to rank first, got {:?}",
            names_of(&results)
        );
    }
}

/// RULE: an entry carrying none of the phrase's terms is not a search result, so a
/// phrase the catalog has nothing to say about answers an empty list and no token —
/// and a phrase of nothing but English filler carries no term at all, so it answers
/// the same way.
#[tokio::test]
async fn a_phrase_no_entry_carries_has_no_results() {
    let emulator = Emulator::new();

    // Rows differ in form — made-up words, punctuation only, digits, pure filler —
    // because what they have in common is carrying no catalog term, not how they are
    // written.
    for phrase in [
        "zyzzyva quuxly flibbertigibbet",
        "!!! ??? ...",
        "9876543210 1234567890",
        "show me all the",
        "which of the",
        "showed me all the",
    ] {
        let case = format!("searching {phrase:?}");
        let reply = search(&emulator, phrase, None, None).await;
        let body = reply.expect_ok(&case);
        let results = results_of(&case, body);
        assert!(
            results.is_empty(),
            "case {case:?}: a phrase no entry carries must answer no results, got {results:?}"
        );
        assert_eq!(
            body.get("NextToken"),
            None,
            "case {case:?}: an empty answer must carry no NextToken, got {body:?}"
        );
    }
}

/// RULE: a page holds at most `MaxResults` results — 10 when the request names none —
/// and `MaxResults` outside the modelled `@range(1, 50)` is refused.
///
/// Both published bounds are rows, and so is one past each end.
#[tokio::test]
async fn max_results_bounds_a_page_and_defaults_to_ten() {
    let emulator = Emulator::new();
    let catalog_size = SAMPLE_QUERY_CATALOG.len();
    assert!(
        catalog_size > MODELLED_DEFAULT_PAGE_SIZE,
        "the catalog must hold more entries than one default page for this case to \
         mean anything, got {catalog_size}"
    );

    // Inside the range: 1 and 50 are the published bounds.
    for max_results in [1, 2, 7, catalog_size as i64, 50] {
        let case = format!("MaxResults={max_results}");
        let reply = search(&emulator, WHOLE_CATALOG_PHRASE, Some(max_results), None).await;
        let body = reply.expect_ok(&case);
        let results = results_of(&case, body);
        check_page(&case, &results);
        let expected = (max_results as usize).min(catalog_size);
        assert_eq!(
            results.len(),
            expected,
            "case {case:?}: expected {expected} results, got {:?}",
            names_of(&results)
        );
    }

    // No MaxResults at all: "The default value is 10."
    let case = "no MaxResults";
    let reply = search(&emulator, WHOLE_CATALOG_PHRASE, None, None).await;
    let results = results_of(case, reply.expect_ok(case));
    assert_eq!(
        results.len(),
        MODELLED_DEFAULT_PAGE_SIZE,
        "case {case:?}: expected the default page size of {MODELLED_DEFAULT_PAGE_SIZE}, got {:?}",
        names_of(&results)
    );

    // One past each published bound: refused by the modelled range, before the
    // handler runs.
    for max_results in [0, 51] {
        let case = format!("MaxResults={max_results}, outside @range(1, 50)");
        search(&emulator, WHOLE_CATALOG_PHRASE, Some(max_results), None)
            .await
            .expect_error(&case, 400, "ValidationException");
    }
}

/// RULE: `NextToken` appears exactly when matches remain, and the pages of one phrase
/// partition that phrase's ranking exactly once, whatever the page size.
///
/// Every page size that partitions the catalog differently is a row: 1 (a page per
/// entry), sizes that leave a short last page, the exact catalog size (one full page,
/// no token) and a size past it.
#[tokio::test]
async fn pages_partition_one_ranking_exactly_once() {
    let emulator = Emulator::new();
    let catalog_size = SAMPLE_QUERY_CATALOG.len();
    let expected = catalog_names_in_tie_order();

    for page_size in [1, 2, 3, 7, catalog_size as i64, 50] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let reply = search(
                &emulator,
                WHOLE_CATALOG_PHRASE,
                Some(page_size),
                token.as_deref(),
            )
            .await;
            let body = reply.expect_ok(&case);
            let results = results_of(&case, body);
            check_page(&case, &results);
            assert!(
                results.len() <= page_size as usize,
                "case {case:?}: a page may not exceed MaxResults, got {} results",
                results.len()
            );
            walked.extend(names_of(&results));
            pages += 1;
            assert!(
                pages <= catalog_size + 1,
                "case {case:?}: paging must terminate, but it is still going after \
                 {pages} pages holding {walked:?}"
            );
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        walked.len() < catalog_size,
                        "case {case:?}: a NextToken was handed out with nothing left to \
                         read, after {walked:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the pages must walk the whole ranking once, in order"
        );
    }
}

/// RULE: a `NextToken` names a position in the ranking of the phrase it was issued
/// for, so a token this operation never handed out — or one replayed with a different
/// `SearchPhrase` — is refused with the modelled `InvalidParameterException`.
#[tokio::test]
async fn a_page_token_belongs_to_the_ranking_it_was_issued_for() {
    let emulator = Emulator::new();

    // A real token, from a real first page.
    let first = search(&emulator, WHOLE_CATALOG_PHRASE, Some(2), None).await;
    let token = first.expect_ok("the first page")["NextToken"]
        .as_str()
        .expect("a first page of two of the whole catalog must carry a NextToken")
        .to_string();

    // Replayed with the phrase it was issued for, it resumes the ranking.
    let case = "the token replayed with its own phrase";
    let reply = search(
        &emulator,
        WHOLE_CATALOG_PHRASE,
        Some(2),
        Some(token.as_str()),
    )
    .await;
    let resumed = names_of(&results_of(case, reply.expect_ok(case)));
    assert_eq!(
        resumed,
        catalog_names_in_tie_order()[2..4].to_vec(),
        "case {case:?}: the token must resume the ranking where the first page ended"
    );

    // Rows differ in HOW the token is wrong: a different phrase's ranking, a token
    // shape this operation never issues, a position past the end of the ranking.
    let rows: Vec<(String, String, String)> = vec![
        (
            "the token replayed with another phrase".to_string(),
            "root user activity".to_string(),
            token.clone(),
        ),
        (
            "a token this operation never handed out".to_string(),
            WHOLE_CATALOG_PHRASE.to_string(),
            "not-a-token".to_string(),
        ),
        (
            "a token with no phrase fingerprint".to_string(),
            WHOLE_CATALOG_PHRASE.to_string(),
            "ssq1:2".to_string(),
        ),
        (
            "a token whose offset is not a number".to_string(),
            WHOLE_CATALOG_PHRASE.to_string(),
            token.replacen("ssq1:2:", "ssq1:two:", 1),
        ),
        (
            "a token naming the first position, which is never handed out".to_string(),
            WHOLE_CATALOG_PHRASE.to_string(),
            token.replacen("ssq1:2:", "ssq1:0:", 1),
        ),
        (
            "a token past the end of the ranking".to_string(),
            WHOLE_CATALOG_PHRASE.to_string(),
            token.replacen("ssq1:2:", "ssq1:9999:", 1),
        ),
    ];
    for (case, phrase, bad_token) in rows {
        search(&emulator, &phrase, Some(2), Some(bad_token.as_str()))
            .await
            .expect_error(&case, 400, "InvalidParameterException");
    }
}

/// RULE: the search is a READ of a catalog no call changes, so one input always
/// answers the same thing — including through the `NextToken` of an earlier page.
#[tokio::test]
async fn searching_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    let case = "the same phrase searched twice";
    let first = search(&emulator, "root user activity", Some(5), None).await;
    let first_body = first.expect_ok(case).clone();
    // A different phrase in between, in case a search were to leave any trace.
    search(&emulator, "security group rule changes", None, None)
        .await
        .expect_ok("an unrelated search in between");
    let second = search(&emulator, "root user activity", Some(5), None).await;
    assert_eq!(
        second.expect_ok(case),
        &first_body,
        "case {case:?}: a read must answer identically every time"
    );
}

/// RULE: a phrase is matched by its meaningful words, so phrasings that differ only
/// in letter case or in English filler rank the catalog the same way — and a page
/// token from one continues the other.
#[tokio::test]
async fn phrasings_that_differ_only_in_case_or_filler_rank_the_same() {
    let emulator = Emulator::new();

    let canonical = "console logins without MFA";
    let case = format!("the canonical phrasing {canonical:?}");
    let reply = search(&emulator, canonical, Some(50), None).await;
    let expected = names_of(&results_of(&case, reply.expect_ok(&case)));
    assert!(
        expected.len() > 1,
        "case {case:?}: this phrase must rank several entries for the comparison to \
         mean anything, got {expected:?}"
    );

    // Rows differ along case, filler words and punctuation — never along a
    // meaningful word.
    for phrasing in [
        "CONSOLE LOGINS WITHOUT MFA",
        "show me all the console logins without MFA",
        "console logins, without MFA!",
        "Console  logins   without   mfa",
    ] {
        let case = format!("the phrasing {phrasing:?}");
        let reply = search(&emulator, phrasing, Some(50), None).await;
        let ranked = names_of(&results_of(&case, reply.expect_ok(&case)));
        assert_eq!(
            ranked, expected,
            "case {case:?}: a rephrasing that changes no meaningful word must rank \
             the catalog the same way"
        );
    }
}

/// RULE: `SearchPhrase` is required and modelled `@length(2, 1000)`, so the boundary
/// refuses a request without one and one outside those bounds, while both published
/// bounds are accepted.
#[tokio::test]
async fn a_search_phrase_outside_the_modelled_length_is_refused() {
    let emulator = Emulator::new();

    // At the published bounds: 2 characters and 1000.
    let longest = "root user activity ".repeat(60)[..1000].to_string();
    for (case, phrase) in [
        ("the shortest accepted phrase", "s3".to_string()),
        ("the longest accepted phrase", longest.clone()),
    ] {
        let reply = search(&emulator, &phrase, Some(50), None).await;
        let body = reply.expect_ok(case);
        check_page(case, &results_of(case, body));
    }

    // One past each bound, and no phrase at all.
    let too_long = format!("{longest}x");
    for (case, body) in [
        ("one character short of the minimum", json!({"SearchPhrase": "s"})),
        (
            "one character past the maximum",
            json!({"SearchPhrase": too_long}),
        ),
        ("no SearchPhrase at all", json!({})),
    ] {
        emulator
            .call("SearchSampleQueries", &body)
            .await
            .expect_error(case, 400, "ValidationException");
    }
}
