// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRepositoryLinks`.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front door builds (`POST /` + `X-Amz-Target`, awsJson1_0 body), reusing the
//! `connection_lifecycle` suite's harness and the `repository_link_lifecycle` suite's
//! table, so what is asserted here is the service's HTTP behaviour and not a handler
//! called directly. Nothing is seeded by hand: a link gets into the store through
//! `CreateRepositoryLink` and is read back through `ListRepositoryLinks`.
//!
//! RULE — a paginated list operation reports the records this service HOLDS, each
//! rendered from its stored row, in one stable order; a page carries at most the
//! requested size, a continuation token is emitted only when a further page exists and
//! resumes exactly where the previous page stopped, so paging a store of N records
//! yields each of them exactly once; a token this service did not issue names no page
//! and is refused with the modelled "the input is not valid" error
//! (`InvalidInputException`, HTTP 400) rather than read as the first page; and the
//! listing writes nothing, so it cannot change what a later caller sees.
//!
//! The tables below span the classes that rule generalises over — every modelled
//! `ProviderType`, both published bounds of `OwnerId` and `RepositoryName`, links with
//! and without an encryption key, links with 0, 1 and several tags, store sizes from 0
//! up, every page size at and inside the published `MaxResults` bounds plus one past
//! them, and several forged tokens — rather than one recorded request.

use serde_json::{Map, Value, json};

use super::connection_lifecycle::{Emulator, error_code};
use super::repository_link_lifecycle::{Row, assert_link_info, create_body, rows, tags_of};

/// The published bounds of `ListRepositoryLinks.MaxResults` (`range 0..100`), from
/// `smithy_query(query="operation", operation="ListRepositoryLinks")`.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// One `ListRepositoryLinks` call, as `(status, parsed body)`.
async fn list_page(
    emulator: &Emulator,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut body = Map::new();
    if let Some(max) = max_results {
        body.insert("MaxResults".to_string(), json!(max));
    }
    if let Some(token) = next_token {
        body.insert("NextToken".to_string(), json!(token));
    }
    emulator
        .call("ListRepositoryLinks", Value::Object(body))
        .await
}

/// Page through the whole store with `max_results`, following `NextToken` to the end,
/// and return every `RepositoryLinkInfo` in the order the pages reported them.
///
/// Asserts the page-size and token invariants on the way: a page never exceeds the
/// requested size, and a token is emitted only when the page it names is non-empty.
async fn list_every_info(emulator: &Emulator, case: &str, max_results: Option<i64>) -> Vec<Value> {
    let mut infos: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    // A page must make progress, so the loop cannot need more rounds than there are
    // links in any of these fixtures.
    for round in 0..256 {
        let (status, body) = list_page(emulator, max_results, next_token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: round {round}: expected ListRepositoryLinks to answer 200, got \
             {status} {body}"
        );
        let page = body["RepositoryLinks"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: round {round}: no RepositoryLinks list in {body}")
        });
        if let Some(max) = max_results.filter(|max| *max > 0) {
            assert!(
                page.len() as i64 <= max,
                "case {case:?}: round {round}: expected a page of at most {max}, got {} in \
                 {body}",
                page.len()
            );
        }
        infos.extend(page.iter().cloned());
        match body["NextToken"].as_str() {
            Some(token) => {
                assert!(
                    !page.is_empty(),
                    "case {case:?}: round {round}: a continuation token was emitted for an \
                     empty page: {body}"
                );
                next_token = Some(token.to_string());
            }
            None => return infos,
        }
    }
    panic!("case {case:?}: ListRepositoryLinks never stopped emitting a NextToken");
}

/// The `RepositoryLinkId` of one reported record.
fn id_of(case: &str, info: &Value) -> String {
    info["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkId in {info}"))
        .to_string()
}

/// Create one connection per row and register the row's link against it, returning
/// `(connection ARN, the info the create answered with)` in creation order.
async fn seed_table(emulator: &Emulator, table: &[Row]) -> Vec<(String, Value)> {
    let mut seeded: Vec<(String, Value)> = Vec::new();
    for (index, row) in table.iter().enumerate() {
        let case = format!("{} ({})", row.case, row.provider_type);
        // `ConnectionName` is modelled `len 1..32`, so the name stays short.
        let connection_arn = emulator
            .create(
                &case,
                json!({
                    "ConnectionName": format!("link-conn-{index}"),
                    "ProviderType": row.provider_type,
                }),
            )
            .await;
        let (status, created) = emulator
            .call("CreateRepositoryLink", create_body(row, &connection_arn))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateRepositoryLink to answer 200, got {status} {created}"
        );
        let info = created["RepositoryLinkInfo"].clone();
        assert_link_info(&case, row, &connection_arn, &info);
        seeded.push((connection_arn, info));
    }
    seeded
}

/// Register `count` links against one connection, returning their ids in creation order.
async fn seed_count(
    emulator: &Emulator,
    case: &str,
    connection_arn: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let row = Row {
            case: "one of a set",
            provider_type: "GitHub",
            owner_id: format!("owner-{index:03}"),
            repository_name: format!("repo-{index:03}"),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        let (status, body) = emulator
            .call("CreateRepositoryLink", create_body(&row, connection_arn))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected link {index} to be created, got {status} {body}"
        );
        ids.push(id_of(case, &body["RepositoryLinkInfo"]));
    }
    ids
}

/// The reporting half of the RULE, over the whole table: every link the store holds is
/// listed, and each listed record is the very record the create registered — not a
/// second rendering of the request — which `GetRepositoryLink` reports for the same id.
#[tokio::test]
async fn listing_reports_every_stored_link_as_the_record_state_holds() {
    let emulator = Emulator::new();
    let table = rows();
    let seeded = seed_table(&emulator, &table).await;

    let listed = list_every_info(&emulator, "the whole table", None).await;
    assert_eq!(
        listed.len(),
        seeded.len(),
        "expected every seeded link to be listed, got {listed:?}"
    );

    for (index, ((connection_arn, created), info)) in seeded.iter().zip(listed.iter()).enumerate() {
        let row = &table[index];
        let case = format!("{} ({}) at position {index}", row.case, row.provider_type);
        // The listed record satisfies the modelled shape for its row…
        assert_link_info(&case, row, connection_arn, info);
        // …and is identical to the one the create answered with.
        assert_eq!(
            info, created,
            "case {case:?}: expected the listed record to be the one the create registered, \
             got {info}"
        );
        // …and to the one the by-id reader reports.
        let id = id_of(&case, info);
        let (status, body) = emulator
            .call("GetRepositoryLink", json!({ "RepositoryLinkId": id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRepositoryLink to report the listed link, got {status} \
             {body}"
        );
        assert_eq!(
            &body["RepositoryLinkInfo"], info,
            "case {case:?}: expected the listed record and the by-id record to agree, got \
             {body}"
        );
    }
}

/// An empty store lists nothing: `RepositoryLinks` is present and EMPTY (the member is a
/// required list, so "no links" is `[]`), with no continuation token.
#[tokio::test]
async fn an_empty_store_lists_no_links() {
    let emulator = Emulator::new();
    // A connection and a host in the store, so what is empty is the LINK store and not
    // the service's state as a whole.
    emulator
        .create(
            "empty link store",
            json!({ "ConnectionName": "no-links", "ProviderType": "GitHub" }),
        )
        .await;

    for max_results in [None, Some(MAX_RESULTS_MIN), Some(1), Some(MAX_RESULTS_MAX)] {
        let case = format!("MaxResults={max_results:?}");
        let (status, body) = list_page(&emulator, max_results, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an empty store to list 200, got {status} {body}"
        );
        assert_eq!(
            body,
            json!({ "RepositoryLinks": [] }),
            "case {case:?}: expected an empty list and no token, got {body}"
        );
    }
}

/// The paging half of the RULE: for every page size at and inside the published
/// `MaxResults` bounds, and for store sizes either side of those page sizes, paging
/// yields each stored link exactly once in one stable order.
#[tokio::test]
async fn paging_yields_every_link_exactly_once_for_every_page_size() {
    // Store sizes: none, one, and several — the last larger than the smallest page sizes
    // so that a run needs more than one page.
    for count in [0usize, 1, 7] {
        let emulator = Emulator::new();
        let case = format!("{count} links");
        let connection_arn = emulator
            .create(
                &case,
                json!({ "ConnectionName": "paging-conn", "ProviderType": "GitHub" }),
            )
            .await;
        let expected = seed_count(&emulator, &case, &connection_arn, count).await;

        // Page sizes: the published minimum (0, i.e. "no page size requested"), 1, a size
        // smaller than the store, exactly the store's size, one more than it, and the
        // published maximum — plus the request that omits the member entirely.
        let page_sizes = [
            None,
            Some(MAX_RESULTS_MIN),
            Some(1),
            Some(3),
            Some(count as i64),
            Some(count as i64 + 1),
            Some(MAX_RESULTS_MAX),
        ];
        for max_results in page_sizes {
            // `MaxResults=0` is the modelled default and means "no page size requested",
            // so it is the same request as omitting the member.
            let case = format!("{case}, MaxResults={max_results:?}");
            let listed = list_every_info(&emulator, &case, max_results).await;
            let ids: Vec<String> = listed.iter().map(|info| id_of(&case, info)).collect();
            assert_eq!(
                ids, expected,
                "case {case:?}: expected every link exactly once in creation order, got {ids:?}"
            );
        }
    }
}

/// A continuation token resumes exactly where the previous page stopped: the pages of a
/// run partition the store, and a token handed back twice names the same page both times
/// (the cursor is a position in a stable order, not a consumed resource).
#[tokio::test]
async fn a_continuation_token_resumes_where_the_previous_page_stopped() {
    let emulator = Emulator::new();
    let case = "token resumption";
    let connection_arn = emulator
        .create(
            case,
            json!({ "ConnectionName": "token-conn", "ProviderType": "GitHub" }),
        )
        .await;
    let expected = seed_count(&emulator, case, &connection_arn, 5).await;

    let (status, first) = list_page(&emulator, Some(2), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first page to answer 200, got {status} {first}"
    );
    let first_ids: Vec<String> = first["RepositoryLinks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinks in {first}"))
        .iter()
        .map(|info| id_of(case, info))
        .collect();
    assert_eq!(
        first_ids,
        expected[..2].to_vec(),
        "case {case:?}: expected the first two links, got {first_ids:?}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a token with 3 links left: {first}"))
        .to_string();

    // The same token, twice: a cursor is a position, so both reads answer the same page.
    for attempt in 1..=2 {
        let attempt_case = format!("{case}: replay attempt {attempt}");
        let (status, body) = list_page(&emulator, Some(2), Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {attempt_case:?}: expected the resumed page to answer 200, got {status} {body}"
        );
        let ids: Vec<String> = body["RepositoryLinks"]
            .as_array()
            .unwrap_or_else(|| panic!("case {attempt_case:?}: no RepositoryLinks in {body}"))
            .iter()
            .map(|info| id_of(&attempt_case, info))
            .collect();
        assert_eq!(
            ids,
            expected[2..4].to_vec(),
            "case {attempt_case:?}: expected the page after the first two, got {ids:?}"
        );
    }

    // The last page carries the remainder and NO token: a token is emitted only when a
    // further page exists.
    let listed = list_every_info(&emulator, case, Some(2)).await;
    let ids: Vec<String> = listed.iter().map(|info| id_of(case, info)).collect();
    assert_eq!(
        ids, expected,
        "case {case:?}: expected the run to cover the whole store, got {ids:?}"
    );
}

/// The refusal half of the RULE: a `NextToken` this service did not issue names no page
/// and is refused with the modelled `InvalidInputException` at HTTP 400 — never read as
/// the first page, and never answered with the scaffold's 501.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let case = "forged tokens";
    let connection_arn = emulator
        .create(
            case,
            json!({ "ConnectionName": "forged-conn", "ProviderType": "GitHub" }),
        )
        .await;
    seed_count(&emulator, case, &connection_arn, 2).await;

    // Each row satisfies the modelled `SharpNextToken` constraints (`len 1..2048`,
    // pattern `^.*$`), so the handler and not the boundary validator is what answers.
    let forged = [
        ("not a token at all", "not-a-token".to_string()),
        ("a bare integer", "0".to_string()),
        ("sixteen non-hex characters", "zzzzzzzzzzzzzzzz".to_string()),
        ("a hex cursor with a leading space", " 000000000000000".to_string()),
        ("a cursor one character short", "00000000000000".to_string()),
        ("a cursor one character too long", "00000000000000000".to_string()),
        ("a token at the published maximum length", "t".repeat(2048)),
    ];
    for (name, token) in forged {
        let (status, body) = list_page(&emulator, None, Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected a 400 for a token this service did not issue, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {name:?}: expected the modelled InvalidInputException, got {body}"
        );
        assert!(
            body["RepositoryLinks"].is_null(),
            "case {name:?}: a refusal must carry no page, got {body}"
        );
    }
}

/// `MaxResults` is modelled `range 0..100`, so each published bound is accepted and one
/// past either bound is refused before any page is computed.
#[tokio::test]
async fn a_max_results_outside_the_modeled_range_is_refused() {
    let emulator = Emulator::new();
    let case = "MaxResults bounds";
    let connection_arn = emulator
        .create(
            case,
            json!({ "ConnectionName": "bounds-conn", "ProviderType": "GitHub" }),
        )
        .await;
    let expected = seed_count(&emulator, case, &connection_arn, 3).await;

    // (MaxResults, must be accepted)
    let bounds = [
        (MAX_RESULTS_MIN - 1, false),
        (MAX_RESULTS_MIN, true),
        (MAX_RESULTS_MAX, true),
        (MAX_RESULTS_MAX + 1, false),
    ];
    for (max_results, accepted) in bounds {
        let bound_case = format!("MaxResults={max_results}");
        let (status, body) = list_page(&emulator, Some(max_results), None).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {bound_case:?}: a page size inside range 0..100 was refused: {status} \
                 {body}"
            );
            let ids: Vec<String> = body["RepositoryLinks"]
                .as_array()
                .unwrap_or_else(|| panic!("case {bound_case:?}: no RepositoryLinks in {body}"))
                .iter()
                .map(|info| id_of(&bound_case, info))
                .collect();
            assert_eq!(
                ids, expected,
                "case {bound_case:?}: expected the whole store on one page, got {ids:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {bound_case:?}: a page size outside range 0..100 was accepted: {status} \
                 {body}"
            );
            assert!(
                body["RepositoryLinks"].is_null(),
                "case {bound_case:?}: a refusal must carry no page, got {body}"
            );
        }
    }
}

/// The listing tracks the store: a link `DeleteRepositoryLink` removes stops being
/// listed, every other link keeps its place in the order, and a link created afterwards
/// is listed last.
#[tokio::test]
async fn the_listing_follows_what_the_store_holds() {
    let emulator = Emulator::new();
    let case = "store changes";
    let connection_arn = emulator
        .create(
            case,
            json!({ "ConnectionName": "churn-conn", "ProviderType": "GitHub" }),
        )
        .await;
    let seeded = seed_count(&emulator, case, &connection_arn, 4).await;

    // Remove one from the MIDDLE, so the assertion is about the order of what remains
    // and not only about the count.
    let removed = seeded[1].clone();
    let (status, body) = emulator
        .call(
            "DeleteRepositoryLink",
            json!({ "RepositoryLinkId": removed.clone() }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to answer 200, got {status} {body}"
    );

    let after_delete: Vec<String> = list_every_info(&emulator, case, Some(2))
        .await
        .iter()
        .map(|info| id_of(case, info))
        .collect();
    let expected: Vec<String> = seeded
        .iter()
        .filter(|id| **id != removed)
        .cloned()
        .collect();
    assert_eq!(
        after_delete, expected,
        "case {case:?}: expected the deleted link to be gone and the rest in place, got \
         {after_delete:?}"
    );

    // A link created after the delete is listed, last: the order is creation order, and
    // the freed position is not reused.
    let row = Row {
        case: "created after the delete",
        provider_type: "GitHub",
        owner_id: "owner-appended".to_string(),
        repository_name: "repo-appended".to_string(),
        encryption_key_arn: None,
        tags: Vec::new(),
    };
    let (status, created) = emulator
        .call("CreateRepositoryLink", create_body(&row, &connection_arn))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the appended link to be created, got {status} {created}"
    );
    let appended_id = id_of(case, &created["RepositoryLinkInfo"]);
    let after_append: Vec<String> = list_every_info(&emulator, case, None)
        .await
        .iter()
        .map(|info| id_of(case, info))
        .collect();
    let mut expected_after_append = expected.clone();
    expected_after_append.push(appended_id);
    assert_eq!(
        after_append, expected_after_append,
        "case {case:?}: expected the new link listed last, got {after_append:?}"
    );
}

/// Listing is a REPORT: repeated listings answer the same records, and nothing a later
/// caller sees changes — the links' tags are still readable and their
/// (connection, owner, repository) triples are still taken.
#[tokio::test]
async fn listing_changes_nothing_a_later_reader_sees() {
    let emulator = Emulator::new();
    // The rows carrying tags and an encryption key, so the read-back covers the members
    // a listing could perturb.
    let table: Vec<Row> = rows()
        .into_iter()
        .filter(|row| !row.tags.is_empty() || row.encryption_key_arn.is_some())
        .collect();
    assert!(
        !table.is_empty(),
        "the shared table must carry rows with tags or an encryption key, got none"
    );
    let seeded = seed_table(&emulator, &table).await;

    let first = list_every_info(&emulator, "repeated listings", None).await;
    for attempt in 2..=4 {
        let case = format!("listing attempt {attempt}");
        let again = list_every_info(&emulator, &case, Some(1)).await;
        assert_eq!(
            again, first,
            "case {case:?}: expected a repeated listing to answer the same records, got {again:?}"
        );
    }

    for (index, (connection_arn, created)) in seeded.iter().enumerate() {
        let row = &table[index];
        let case = format!("{} ({})", row.case, row.provider_type);
        let arn = created["RepositoryLinkArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkArn in {created}"))
            .to_string();
        let mut expected_tags = row.tags.clone();
        expected_tags.sort();
        assert_eq!(
            tags_of(&emulator, &case, &arn).await,
            expected_tags,
            "case {case:?}: expected the listings to leave the link's tags alone"
        );
        let (status, body) = emulator
            .call("CreateRepositoryLink", create_body(row, connection_arn))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the listed link to still hold its triple, got {status} \
             {body}"
        );
    }
}
