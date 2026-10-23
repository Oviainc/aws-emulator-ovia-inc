// SPDX-License-Identifier: CC0-1.0

//! `ListStoredQueries`, end to end over the wire.
//!
//! Four rules are pinned here, each over a table of inputs rather than one case:
//!
//! 1. COMPLETENESS: following the `NextToken` chain from the first page reports every
//!    stored query the account holds exactly once, whatever the page size and however
//!    many queries are saved; no page carries more entries than the page size asked
//!    for, every page but the last is full, and only the last page omits a
//!    `NextToken`;
//! 2. a page reports each query's PERSISTED identity and description — the same
//!    `QueryId`, `QueryArn` and `Description` `GetStoredQuery` reports — and never the
//!    expression, which `StoredQueryMetadata` does not declare;
//! 3. a `NextToken` this operation did not issue is refused with the modelled
//!    `InvalidNextTokenException`, and a `MaxResults` outside the published range with
//!    `ValidationException`;
//! 4. listing REPORTS state: it changes nothing a later caller sees, and a token taken
//!    from one page still resumes correctly after more queries are saved.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` already builds;
//! every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::Emulator;

/// The published maximum of `ListStoredQueriesRequest$MaxResults` (target `Limit`,
/// range 0..100) from
/// `smithy_query(query="operation", operation="ListStoredQueries")` on this service's
/// model. It is also the page size the operation documents for a request that asks for
/// none ("The default is 100").
const MAX_RESULTS_MAX: usize = 100;

/// A minimal query expression; its text is never asserted on, only saved.
const SELECT_BUCKETS: &str =
    "SELECT resourceId, resourceType WHERE resourceType = 'AWS::S3::Bucket'";

/// Save `name` with `description`, carrying a description only when one is given.
async fn save(emulator: &Emulator, name: &str, description: Option<&str>) {
    let mut query = json!({ "QueryName": name, "Expression": SELECT_BUCKETS });
    if let Some(description) = description {
        query["Description"] = json!(description);
    }
    emulator
        .ok("PutStoredQuery", &json!({ "StoredQuery": query }))
        .await;
}

/// One page of `ListStoredQueries`: the metadata entries and the token that follows.
struct Page {
    entries: Vec<Value>,
    next_token: Option<String>,
}

/// Request one page, with `max_results` and `next_token` present only when given.
async fn list(emulator: &Emulator, max_results: Option<usize>, next_token: Option<&str>) -> Page {
    let mut body = json!({});
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    let answered = emulator.ok("ListStoredQueries", &body).await;
    let entries = answered["StoredQueryMetadata"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("ListStoredQueries must answer a StoredQueryMetadata list, got {answered}")
        });
    Page {
        entries,
        next_token: answered
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Walk the whole `NextToken` chain at `max_results`, returning every page in order.
async fn all_pages(emulator: &Emulator, max_results: Option<usize>) -> Vec<Page> {
    let mut pages = vec![list(emulator, max_results, None).await];
    // The chain is bounded: the quota is 300 queries and the smallest page carries one,
    // so a chain longer than this cannot terminate and the test must fail rather than
    // loop forever.
    for _ in 0..512 {
        let Some(token) = pages
            .last()
            .and_then(|page| page.next_token.clone()) else {
            return pages;
        };
        pages.push(list(emulator, max_results, Some(&token)).await);
    }
    panic!("the NextToken chain did not terminate within 512 pages");
}

/// The `QueryName` of each entry of `page`, in the order the page reports them.
fn names_of(page: &Page) -> Vec<String> {
    page.entries
        .iter()
        .map(|entry| {
            entry["QueryName"]
                .as_str()
                .unwrap_or_else(|| panic!("every metadata entry carries a QueryName, got {entry}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn every_saved_query_is_reported_exactly_once_across_the_pages() {
    // Rows differ along HOW MANY queries the account holds — none, one, a handful, and
    // one past the default page size — crossed with the page size asked for: each
    // published bound of `MaxResults` that selects a page (1 and 100), sizes that do
    // and do not divide the count, 0 ("Config uses the default") and no `MaxResults`
    // member at all.
    let counts = [0_usize, 1, 2, 7, MAX_RESULTS_MAX + 1];
    let page_sizes = [Some(1_usize), Some(3), Some(MAX_RESULTS_MAX), Some(0), None];

    for count in counts {
        for max_results in page_sizes {
            let case = format!("{count} queries at MaxResults {max_results:?}");
            let emulator = Emulator::new();
            let mut saved: Vec<String> = Vec::new();
            for index in 0..count {
                let name = format!("query-{index:03}");
                save(&emulator, &name, None).await;
                saved.push(name);
            }

            // A `MaxResults` of 0 or none asked for is the documented default of 100.
            let effective = match max_results {
                Some(size) if size > 0 => size,
                _ => MAX_RESULTS_MAX,
            };

            let pages = all_pages(&emulator, max_results).await;
            let reported: Vec<String> = pages.iter().flat_map(names_of).collect();

            let mut unique = reported.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(
                unique.len(),
                reported.len(),
                "case {case:?}: every query must be reported once, got {reported:?}"
            );
            let mut expected = saved.clone();
            expected.sort();
            assert_eq!(
                unique, expected,
                "case {case:?}: expected the pages to report every saved query, got \
                 {reported:?}"
            );

            // Pagination is only meaningful over a stable total ordering, which is
            // what makes a token resumable: the names come back strictly increasing
            // across the whole chain.
            assert!(
                reported.windows(2).all(|pair| pair[0] < pair[1]),
                "case {case:?}: expected a strictly increasing listing, got {reported:?}"
            );

            let sizes: Vec<usize> = pages.iter().map(|page| page.entries.len()).collect();
            for (index, size) in sizes.iter().enumerate() {
                assert!(
                    *size <= effective,
                    "case {case:?}: page {index} must carry at most {effective} entries, \
                     got {size}"
                );
            }
            // Every page but the last is full, and only the last page omits a token.
            for (index, size) in sizes.iter().enumerate().take(sizes.len() - 1) {
                assert_eq!(
                    *size, effective,
                    "case {case:?}: page {index} must be full when a NextToken follows \
                     it, got {size}"
                );
            }
            assert!(
                pages
                    .last()
                    .is_some_and(|page| page.next_token.is_none()),
                "case {case:?}: the last page must omit a NextToken, got {:?}",
                pages.last().map(|page| page.next_token.clone())
            );
            let expected_pages = count.div_ceil(effective).max(1);
            assert_eq!(
                pages.len(),
                expected_pages,
                "case {case:?}: expected {expected_pages} page(s) at a page size of \
                 {effective}, got {} with sizes {sizes:?}",
                pages.len()
            );
        }
    }
}

#[tokio::test]
async fn a_page_reports_the_persisted_identity_and_description_of_each_query() {
    // Rows differ along the DESCRIPTION the query was saved with — absent, empty (the
    // published minimum length of `QueryDescription`, 0..256), non-Latin scripts,
    // emoji, embedded whitespace and the published maximum length — and along the form
    // of the name, since `QueryName`'s pattern (^[a-zA-Z0-9-_]+$) allows digits,
    // hyphens, underscores and mixed case.
    let longest_description = "d".repeat(256);
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("no description at all", "ascii-query", None),
        (
            "an empty description, the published minimum length",
            "empty_description",
            Some(""),
        ),
        ("non-Latin scripts", "UPPER-Query-1", Some("ресурсы 資源")),
        ("emoji", "emoji_query_2", Some("buckets 🪣👨‍👩‍👧‍👦")),
        (
            "newlines and tabs",
            "whitespace-query",
            Some("line one\nline two\tindented"),
        ),
        (
            "a description at the published maximum length",
            "longest-description",
            Some(longest_description.as_str()),
        ),
    ];

    let emulator = Emulator::new();
    for (_, name, description) in &rows {
        save(&emulator, name, *description).await;
    }

    let pages = all_pages(&emulator, None).await;
    let reported: Vec<Value> = pages
        .into_iter()
        .flat_map(|page| page.entries.into_iter())
        .collect();

    for (case, name, description) in &rows {
        let entry = reported
            .iter()
            .find(|entry| entry["QueryName"].as_str() == Some(*name))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {name:?} among the listing, got {reported:?}")
            });

        // The listing must report what the store holds, which is what the service's own
        // read path reports.
        let stored = emulator
            .ok("GetStoredQuery", &json!({ "QueryName": name }))
            .await;
        let stored = &stored["StoredQuery"];
        assert_eq!(
            entry["QueryId"], stored["QueryId"],
            "case {case:?}: expected the persisted QueryId {:?}, got {:?}",
            stored["QueryId"], entry["QueryId"]
        );
        assert_eq!(
            entry["QueryArn"], stored["QueryArn"],
            "case {case:?}: expected the persisted QueryArn {:?}, got {:?}",
            stored["QueryArn"], entry["QueryArn"]
        );
        match description {
            Some(expected) => assert_eq!(
                entry["Description"].as_str(),
                Some(*expected),
                "case {case:?}: expected the persisted Description {expected:?}, got {:?}",
                entry["Description"]
            ),
            None => assert!(
                entry.get("Description").is_none(),
                "case {case:?}: expected no Description for a query saved without one, \
                 got {:?}",
                entry["Description"]
            ),
        }
        // `StoredQueryMetadata` declares no `Expression`, so a page never carries one.
        assert!(
            entry.get("Expression").is_none(),
            "case {case:?}: metadata declares no Expression, got {entry}"
        );
    }

    assert_eq!(
        reported.len(),
        rows.len(),
        "expected one entry per saved query, got {reported:?}"
    );
}

#[tokio::test]
async fn an_update_in_place_is_reported_as_one_query_with_its_original_identity() {
    // Rows differ along what the second save carries: a new expression alone, a new
    // description too, and the same body again (the idempotent case the operation
    // documents).
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        (
            "a new expression, no description",
            None,
            "SELECT resourceId WHERE resourceType = 'AWS::EC2::Instance'",
        ),
        (
            "a new expression and a new description",
            Some("the replacement description 🔁"),
            "SELECT resourceId, awsRegion WHERE resourceType = 'AWS::IAM::Role'",
        ),
        ("the same body again", Some("the original"), SELECT_BUCKETS),
    ];

    for (case, description, expression) in rows {
        let emulator = Emulator::new();
        let name = "query-under-update";
        save(&emulator, name, Some("the original")).await;
        save(&emulator, "other-query", None).await;

        let before = all_pages(&emulator, None).await;
        let before: Vec<Value> = before
            .into_iter()
            .flat_map(|page| page.entries.into_iter())
            .collect();
        let created = before
            .iter()
            .find(|entry| entry["QueryName"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("case {case:?}: expected {name:?} listed, got {before:?}"))
            .clone();

        let mut query = json!({ "QueryName": name, "Expression": expression });
        if let Some(description) = description {
            query["Description"] = json!(description);
        }
        emulator
            .ok("PutStoredQuery", &json!({ "StoredQuery": query }))
            .await;

        let after = all_pages(&emulator, None).await;
        let after: Vec<Value> = after
            .into_iter()
            .flat_map(|page| page.entries.into_iter())
            .collect();
        assert_eq!(
            after.len(),
            before.len(),
            "case {case:?}: an update must not add a second query, got {after:?}"
        );
        let updated = after
            .iter()
            .find(|entry| entry["QueryName"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("case {case:?}: expected {name:?} listed, got {after:?}"));
        assert_eq!(
            updated["QueryId"], created["QueryId"],
            "case {case:?}: an update keeps the QueryId {:?}, got {:?}",
            created["QueryId"], updated["QueryId"]
        );
        assert_eq!(
            updated["QueryArn"], created["QueryArn"],
            "case {case:?}: an update keeps the QueryArn {:?}, got {:?}",
            created["QueryArn"], updated["QueryArn"]
        );
        let expected_description = description.unwrap_or("the original");
        assert_eq!(
            updated["Description"].as_str(),
            Some(expected_description),
            "case {case:?}: expected the Description {expected_description:?}, got {:?}",
            updated["Description"]
        );
    }
}

#[tokio::test]
async fn a_next_token_the_operation_did_not_issue_is_refused() {
    // Rows differ along the FORM of the bad token: not base64 at all, base64 of text
    // that is no query name (empty, past `QueryName`'s published 1..64 length, outside
    // its published pattern) and a token another paginated read of this service issues.
    let longest_name = "n".repeat(64);
    let rows: Vec<(&str, String)> = vec![
        ("not base64 at all", "not-a-token!!".to_string()),
        ("base64 of empty text", base64("")),
        (
            "base64 of a name one past the published maximum length",
            base64(&"n".repeat(65)),
        ),
        (
            "base64 of text outside the published name pattern",
            base64("not a valid name"),
        ),
        (
            "base64 of a key carrying a separator another read uses",
            base64("210987654321#us-east-1"),
        ),
    ];

    for (case, token) in rows {
        let emulator = Emulator::new();
        save(&emulator, "alpha-query", None).await;
        save(&emulator, &longest_name, None).await;

        let (status, code) = emulator
            .refused("ListStoredQueries", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: a NextToken this operation did not issue must be refused \
             400 InvalidNextTokenException, got {status} {code}"
        );

        // The refusal changes nothing: both queries are still reported.
        let page = list(&emulator, None, None).await;
        assert_eq!(
            names_of(&page).len(),
            2,
            "case {case:?}: a refused page must leave the store alone, got {:?}",
            names_of(&page)
        );
    }
}

#[tokio::test]
async fn a_max_results_outside_the_published_range_is_refused() {
    // The published range of `MaxResults` is 0..100, so the rows are one past each
    // published bound; the bounds themselves are exercised by the pagination table.
    for max_results in [-1_i64, i64::try_from(MAX_RESULTS_MAX).unwrap_or(100) + 1] {
        let case = format!("MaxResults {max_results}");
        let emulator = Emulator::new();
        save(&emulator, "alpha-query", None).await;

        let (status, code) = emulator
            .refused("ListStoredQueries", &json!({ "MaxResults": max_results }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a MaxResults outside the published range must be refused \
             400 ValidationException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn listing_reports_state_without_changing_it() {
    // Rows differ along the page size, so the read is checked both when it pages and
    // when one page carries everything.
    for max_results in [Some(1_usize), Some(2), None] {
        let case = format!("MaxResults {max_results:?}");
        let emulator = Emulator::new();
        for index in 0..5 {
            save(&emulator, &format!("query-{index}"), None).await;
        }

        let first = all_pages(&emulator, max_results).await;
        let first: Vec<Vec<String>> = first.iter().map(names_of).collect();
        let again = all_pages(&emulator, max_results).await;
        let again: Vec<Vec<String>> = again.iter().map(names_of).collect();
        assert_eq!(
            first, again,
            "case {case:?}: listing twice must report the same pages, got {first:?} \
             then {again:?}"
        );

        // A query read through the service's own read path after the listing is
        // unchanged by it.
        let stored = emulator
            .ok("GetStoredQuery", &json!({ "QueryName": "query-0" }))
            .await;
        assert_eq!(
            stored["StoredQuery"]["Expression"].as_str(),
            Some(SELECT_BUCKETS),
            "case {case:?}: a listing must not change a stored query, got {stored}"
        );
    }
}

#[tokio::test]
async fn a_token_resumes_correctly_after_more_queries_are_saved() {
    // Rows differ along WHERE the queries saved between the pages sort relative to the
    // token: entirely before it, entirely after it, and on both sides.
    let rows: Vec<(&str, &[&str])> = vec![
        ("inserted before the resume point", &["aaa-early", "aab-early"]),
        ("inserted after the resume point", &["zzz-late", "zzy-late"]),
        ("inserted on both sides", &["aaa-early", "zzz-late"]),
    ];

    for (case, inserted) in rows {
        let emulator = Emulator::new();
        let seeded = ["mmm-1", "mmm-2", "mmm-3", "mmm-4"];
        for name in seeded {
            save(&emulator, name, None).await;
        }

        let first = list(&emulator, Some(2), None).await;
        let token = first
            .next_token
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: expected a NextToken after a full page"));
        let first_names = names_of(&first);

        for name in inserted {
            save(&emulator, name, None).await;
        }

        // The token holds the key the next page resumes at, so the rest of the listing
        // is exactly the names that sort at or after it — including any saved since,
        // and never a name the first page already reported.
        let mut rest: Vec<String> = Vec::new();
        let mut next = Some(token);
        while let Some(token) = next {
            let page = list(&emulator, Some(2), Some(&token)).await;
            rest.extend(names_of(&page));
            next = page.next_token.clone();
        }

        let resume_at = first_names
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the first page carried no names"));
        let mut expected: Vec<String> = seeded
            .iter()
            .chain(inserted.iter())
            .map(|name| (*name).to_string())
            .filter(|name| *name > resume_at)
            .collect();
        expected.sort();
        let mut got = rest.clone();
        got.sort();
        assert_eq!(
            got, expected,
            "case {case:?}: expected the pages after {resume_at:?} to report \
             {expected:?}, got {rest:?}"
        );
        assert!(
            rest.iter().all(|name| !first_names.contains(name)),
            "case {case:?}: a resumed page must not repeat the first page \
             {first_names:?}, got {rest:?}"
        );
    }
}

/// `text` as the base64 the framework's page tokens are encoded with.
fn base64(text: &str) -> String {
    aws_smithy_types::base64::encode(text)
}
