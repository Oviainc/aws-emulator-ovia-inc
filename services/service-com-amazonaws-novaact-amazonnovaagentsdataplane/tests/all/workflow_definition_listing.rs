// SPDX-License-Identifier: CC0-1.0

//! `ListWorkflowDefinitions`: one page of the account's STORED definitions, computed.
//!
//! The rule these cases assert is not "this one request answers these bytes": a listing
//! reports the definitions this account's store holds, each summary member read off the
//! stored record, ordered by the one instant the summary carries (`createdAt`) in the
//! requested `sortOrder` with the name as the tie-break, in pages of at most `maxResults`,
//! each page resuming strictly after the last definition the previous one emitted.
//!
//! So the tables below vary what that rule generalises over — page size (including every
//! published bound and one past it), sort order, how many definitions are stored, the form
//! of their names, which of them is being deleted, and how a resumption token is damaged —
//! and the recorded "five definitions, maxResults=2" case is one row among them.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::state::DELETION_COMPLETION_DELAY;

use crate::workflow_lifecycle::{text, Emulator};

/// `WorkflowDefinitionName` is `^[a-zA-Z0-9_-]{1,40}$`
/// (`smithy_query(query="shape",shape="WorkflowDefinitionSummary")`), so these names span
/// the forms it admits — underscores, hyphens, digits, mixed case, the 40-character
/// maximum and a single character — while their lexical order is also their creation
/// order, which is what makes the expected ordering independent of the host clock's
/// resolution (two definitions created in the same instant are ordered by name).
const DEFINITION_NAMES: [&str; 8] = [
    "01_alpha",
    "02-BETA",
    "03_gamma_123",
    "04",
    "05-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    "06_Mixed-Case_99",
    "07-zzz",
    "9",
];

/// `MaxResults`' modelled range is 1..100
/// (`smithy_query(query="shape",shape="MaxResults")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The byte that separates the fields inside a `nextToken` this operation issues.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// One `ListWorkflowDefinitions` call: `maxResults` and `nextToken` are `@httpQuery`
/// members and `sortOrder` travels in the body (`smithy_query(query="operation",
/// operation="ListWorkflowDefinitions")`).
async fn list(
    emulator: &Emulator,
    sort_order: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let path = if query.is_empty() {
        "/workflow-definitions".to_string()
    } else {
        format!("/workflow-definitions?{}", query.join("&"))
    };
    let body = match sort_order {
        Some(order) => json!({ "sortOrder": order }),
        None => json!({}),
    };
    emulator.call("POST", &path, Some(body)).await
}

/// The `workflowDefinitionName` of every summary in one page, in the order the page
/// carries them.
fn page_names(body: &Value) -> Vec<String> {
    body["workflowDefinitionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("workflowDefinitionSummaries must be a list in {body}"))
        .iter()
        .map(|summary| text(summary, "workflowDefinitionName"))
        .collect()
}

/// Create `names` in order, each through the real `CreateWorkflowDefinition` wire call.
async fn create_all(emulator: &Emulator, names: &[&str]) {
    for name in names {
        let (status, body) = emulator
            .call("PUT", "/workflow-definitions", Some(json!({ "name": name })))
            .await;
        assert_eq!(
            status, 201,
            "case create {name:?}: expected HTTP 201, got {status} {body}"
        );
    }
}

/// Page through the whole listing, asserting the paging invariants on the way: every page
/// but the last is full, no page is bigger than the page size, and a `nextToken` is
/// carried exactly when a further page follows.
async fn drain(
    emulator: &Emulator,
    label: &str,
    sort_order: Option<&str>,
    max_results: Option<i64>,
    expected_page_size: usize,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page more than the store can hold: a listing that does not terminate is a bug,
    // not something to loop forever on.
    for page_index in 0..=DEFINITION_NAMES.len() + 1 {
        let (status, body) = list(emulator, sort_order, max_results, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {label}: page {page_index} expected HTTP 200, got {status} {body}"
        );
        let names = page_names(&body);
        assert!(
            names.len() <= expected_page_size,
            "case {label}: page {page_index} must hold at most {expected_page_size} summaries, got {names:?}"
        );
        collected.extend(names.iter().cloned());
        match body["nextToken"].as_str() {
            Some(next) => {
                assert_eq!(
                    names.len(),
                    expected_page_size,
                    "case {label}: page {page_index} carries a nextToken, so it must be full, got {names:?}"
                );
                assert!(
                    !next.is_empty() && next.len() <= 2048 && !next.contains(char::is_whitespace),
                    "case {label}: page {page_index} nextToken must satisfy NextToken's length 1..2048 and ^\\S*$, got {next:?}"
                );
                token = Some(next.to_string());
            }
            None => return collected,
        }
    }
    panic!("case {label}: the listing never ended, collected {collected:?}");
}

/// Every page size and both orders answer the same definitions, in the order the request
/// asked for, split into pages of the size it asked for.
#[tokio::test]
async fn page_size_and_sort_order_table() {
    let emulator = Emulator::new("us-east-1");
    create_all(&emulator, &DEFINITION_NAMES).await;

    let ascending: Vec<String> = DEFINITION_NAMES.iter().map(|n| n.to_string()).collect();
    let descending: Vec<String> = ascending.iter().rev().cloned().collect();
    let stored = ascending.len();

    // (label, sortOrder, maxResults, the page size that request is answered in)
    let cases: Vec<(&str, Option<&str>, Option<i64>, usize)> = vec![
        ("no sortOrder, no maxResults", None, None, stored),
        ("no sortOrder, the smallest page", None, Some(MAX_RESULTS_MIN), 1),
        ("ascending, one page per definition", Some("Ascending"), Some(1), 1),
        ("ascending, the recorded page size", Some("Ascending"), Some(2), 2),
        ("ascending, a page size that does not divide the store", Some("Ascending"), Some(3), 3),
        ("ascending, exactly the store", Some("Ascending"), Some(stored as i64), stored),
        ("ascending, one page larger than the store", Some("Ascending"), Some(stored as i64 + 1), stored),
        ("ascending, the largest page", Some("Ascending"), Some(MAX_RESULTS_MAX), stored),
        ("descending, one page per definition", Some("Descending"), Some(1), 1),
        ("descending, the recorded page size", Some("Descending"), Some(2), 2),
        ("descending, a page size that does not divide the store", Some("Descending"), Some(5), 5),
        ("descending, the largest page", Some("Descending"), Some(MAX_RESULTS_MAX), stored),
        ("descending, no maxResults", Some("Descending"), None, stored),
    ];

    for (label, sort_order, max_results, page_size) in cases {
        let collected = drain(&emulator, label, sort_order, max_results, page_size.min(stored)).await;
        let expected = if sort_order == Some("Descending") {
            &descending
        } else {
            &ascending
        };
        assert_eq!(
            &collected, expected,
            "case {label}: the whole listing must be every stored definition in the requested order, got {collected:?}"
        );
    }
}

/// A page size outside `MaxResults`' published range is refused by the modelled
/// constraint, never clamped into an answer.
#[tokio::test]
async fn page_size_outside_the_modeled_range_is_refused() {
    let emulator = Emulator::new("us-west-2");
    create_all(&emulator, &["edge_case"]).await;

    for (label, max_results) in [
        ("one below the minimum", MAX_RESULTS_MIN - 1),
        ("one above the maximum", MAX_RESULTS_MAX + 1),
    ] {
        let (status, body) = list(&emulator, None, Some(max_results), None).await;
        assert_eq!(
            status, 400,
            "case {label} ({max_results}): expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"], "ValidationException",
            "case {label} ({max_results}): expected ValidationException, got {body}"
        );
    }

    for (label, max_results) in [
        ("the published minimum", MAX_RESULTS_MIN),
        ("the published maximum", MAX_RESULTS_MAX),
    ] {
        let (status, body) = list(&emulator, None, Some(max_results), None).await;
        assert_eq!(
            status, 200,
            "case {label} ({max_results}): expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            page_names(&body),
            vec!["edge_case".to_string()],
            "case {label} ({max_results}): the one stored definition must be reported, got {body}"
        );
    }
}

/// Nothing stored, nothing reported: the required list is present and empty, and no token
/// invites a page that does not exist.
#[tokio::test]
async fn an_empty_store_answers_an_empty_page() {
    let emulator = Emulator::new("eu-west-1");

    for (label, sort_order, max_results) in [
        ("no members", None, None),
        ("ascending", Some("Ascending"), Some(1)),
        ("descending", Some("Descending"), Some(MAX_RESULTS_MAX)),
    ] {
        let (status, body) = list(&emulator, sort_order, max_results, None).await;
        assert_eq!(
            status, 200,
            "case {label}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            page_names(&body),
            Vec::<String>::new(),
            "case {label}: an empty store reports no summaries, got {body}"
        );
        assert_eq!(
            body["nextToken"],
            Value::Null,
            "case {label}: there is no further page to invite, got {body}"
        );
    }
}

/// Every summary member is the stored definition's own: the same ARN, instant and status
/// the implemented reader answers with for that definition.
#[tokio::test]
async fn each_summary_is_read_off_the_stored_definition() {
    let emulator = Emulator::new("ap-southeast-2");
    // Varying the optional members proves the summary is read from the record rather than
    // from the create request's shape.
    let created: Vec<(&str, Value)> = vec![
        ("plain", json!({ "name": "plain" })),
        (
            "described",
            json!({ "name": "described", "description": "a described definition" }),
        ),
        (
            "exported",
            json!({
                "name": "exported",
                "exportConfig": { "s3BucketName": "a-bucket", "s3KeyPrefix": "runs/exported" },
            }),
        ),
    ];
    for (label, request) in &created {
        let (status, body) = emulator
            .call("PUT", "/workflow-definitions", Some(request.clone()))
            .await;
        assert_eq!(
            status, 201,
            "case {label}: expected HTTP 201, got {status} {body}"
        );
    }

    let (status, listing) = list(&emulator, None, None, None).await;
    assert_eq!(status, 200, "the listing must answer 200, got {status} {listing}");
    let summaries = listing["workflowDefinitionSummaries"]
        .as_array()
        .expect("workflowDefinitionSummaries must be a list")
        .clone();
    assert_eq!(
        summaries.len(),
        created.len(),
        "every created definition must be listed, got {listing}"
    );

    for summary in &summaries {
        let name = text(summary, "workflowDefinitionName");
        let (status, read) = emulator
            .call("GET", &format!("/workflow-definitions/{name}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {name}: the listed definition must read back, got {status} {read}"
        );
        assert_eq!(
            summary["workflowDefinitionArn"], read["arn"],
            "case {name}: the summary ARN is the stored one, got {summary} vs {read}"
        );
        assert_eq!(
            summary["createdAt"], read["createdAt"],
            "case {name}: the summary instant is the stored one, got {summary} vs {read}"
        );
        assert_eq!(
            summary["status"], read["status"],
            "case {name}: the summary status is the stored one, got {summary} vs {read}"
        );
        assert_eq!(
            summary["workflowDefinitionArn"],
            json!(format!(
                "arn:aws:nova-act:ap-southeast-2:{}:workflow-definition/{name}",
                engine::EMULATOR_ACCOUNT_ID
            )),
            "case {name}: the ARN names this service, region and account, got {summary}"
        );
    }
}

/// A definition whose delete is still pending is reported, with the status it is in; once
/// the removal falls due it is reported no more.
#[tokio::test]
async fn a_deleting_definition_is_reported_until_its_removal_falls_due() {
    let emulator = Emulator::new("us-east-1");
    create_all(&emulator, &["kept", "going"]).await;

    let (status, body) = emulator
        .call("DELETE", "/workflow-definitions/going", None)
        .await;
    assert_eq!(status, 202, "the delete must answer 202, got {status} {body}");

    let (status, listing) = list(&emulator, None, None, None).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {listing}");
    let summaries = listing["workflowDefinitionSummaries"]
        .as_array()
        .expect("a list")
        .clone();
    for summary in &summaries {
        let name = text(summary, "workflowDefinitionName");
        let expected = if name == "going" { "DELETING" } else { "ACTIVE" };
        assert_eq!(
            summary["status"], expected,
            "case {name}: expected status {expected}, got {summary}"
        );
    }
    assert_eq!(
        page_names(&listing),
        vec!["kept".to_string(), "going".to_string()],
        "case while deleting: both definitions are still held, oldest first, got {listing}"
    );

    std::thread::sleep(DELETION_COMPLETION_DELAY + Duration::from_millis(250));

    let (status, listing) = list(&emulator, None, None, None).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {listing}");
    assert_eq!(
        page_names(&listing),
        vec!["kept".to_string()],
        "case after the deleting span: the removed definition is gone, got {listing}"
    );
}

/// A resumption carries a POSITION, not an offset: the page after a token holds the
/// definitions that sort after it, even when the definition the token names has since been
/// removed and even when a new one was created meanwhile.
#[tokio::test]
async fn a_page_resumes_after_its_cursor_across_changes_to_the_store() {
    let emulator = Emulator::new("us-east-1");
    create_all(&emulator, &["a1", "a2", "a3", "a4"]).await;

    let (status, first) = list(&emulator, Some("Ascending"), Some(2), None).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {first}");
    assert_eq!(
        page_names(&first),
        vec!["a1".to_string(), "a2".to_string()],
        "case the first page: got {first}"
    );
    let token = text(&first, "nextToken");

    // The definition the cursor names is taken away, and a brand-new one is added. The
    // new definition is the youngest, so it belongs on the LAST page of an ascending
    // listing — after the cursor, hence in the resumption.
    let (status, body) = emulator
        .call("DELETE", "/workflow-definitions/a2", None)
        .await;
    assert_eq!(status, 202, "the delete must answer 202, got {status} {body}");
    std::thread::sleep(DELETION_COMPLETION_DELAY + Duration::from_millis(250));
    create_all(&emulator, &["a5"]).await;

    let (status, second) = list(&emulator, Some("Ascending"), Some(2), Some(&token)).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {second}");
    assert_eq!(
        page_names(&second),
        vec!["a3".to_string(), "a4".to_string()],
        "case resumed after a removed cursor: got {second}"
    );
    let token = text(&second, "nextToken");
    let (status, third) = list(&emulator, Some("Ascending"), Some(2), Some(&token)).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {third}");
    assert_eq!(
        page_names(&third),
        vec!["a5".to_string()],
        "case the definition created mid-listing: got {third}"
    );
    assert_eq!(
        third["nextToken"],
        Value::Null,
        "case the last page: no further page is invited, got {third}"
    );
}

/// A `nextToken` this operation did not issue for this listing is refused with the
/// modelled `ValidationException`, naming the member at fault — never answered with a
/// page from some other position.
#[tokio::test]
async fn a_token_this_listing_did_not_issue_is_refused() {
    let emulator = Emulator::new("us-east-1");
    create_all(&emulator, &["t1", "t2", "t3"]).await;

    let (_, first) = list(&emulator, Some("Ascending"), Some(1), None).await;
    let ascending_token = text(&first, "nextToken");

    let cases: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("hex of nothing this operation encodes", hex::encode("t1")),
        (
            "a field missing",
            hex::encode(format!("Ascending{TOKEN_SEPARATOR}1.000000000")),
        ),
        (
            "a field too many",
            hex::encode(format!(
                "Ascending{TOKEN_SEPARATOR}1.000000000{TOKEN_SEPARATOR}t1{TOKEN_SEPARATOR}t2"
            )),
        ),
        (
            "an order it was not issued for",
            ascending_token.clone(),
        ),
        (
            "an unreadable instant",
            hex::encode(format!(
                "Descending{TOKEN_SEPARATOR}not-an-instant{TOKEN_SEPARATOR}t1"
            )),
        ),
        (
            "a nanosecond field out of range",
            hex::encode(format!(
                "Descending{TOKEN_SEPARATOR}1.1000000000{TOKEN_SEPARATOR}t1"
            )),
        ),
        (
            "an instant past what a clock can name",
            hex::encode(format!(
                "Descending{TOKEN_SEPARATOR}{}.000000000{TOKEN_SEPARATOR}t1",
                u64::MAX
            )),
        ),
        (
            "a name outside WorkflowDefinitionName's pattern",
            hex::encode(format!(
                "Descending{TOKEN_SEPARATOR}1.000000000{TOKEN_SEPARATOR}not a name"
            )),
        ),
    ];

    for (label, token) in cases {
        // Every row is sent on a Descending listing, so the Ascending token above is a
        // token for the other order.
        let (status, body) = list(&emulator, Some("Descending"), Some(1), Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"], "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["reason"], "FieldValidationFailed",
            "case {label:?}: a broken field rule is reported as FieldValidationFailed, got {body}"
        );
        assert_eq!(
            body["fieldList"][0]["name"], "nextToken",
            "case {label:?}: the refusal names the member at fault, got {body}"
        );
        assert!(
            !text(&body, "message").is_empty(),
            "case {label:?}: the refusal says what was wrong, got {body}"
        );
    }

    // And the token it WAS issued for still pages that listing.
    let (status, body) = list(&emulator, Some("Ascending"), Some(1), Some(&ascending_token)).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {body}");
    assert_eq!(
        page_names(&body),
        vec!["t2".to_string()],
        "case the token it was issued for: got {body}"
    );
}

/// A `sortOrder` outside the model's enum is refused, never answered in some other order.
#[tokio::test]
async fn a_sort_order_outside_the_enum_is_refused() {
    let emulator = Emulator::new("us-east-1");
    create_all(&emulator, &["s1"]).await;

    // `SortOrder`'s values are `Ascending | Descending`
    // (`smithy_query(query="shape",shape="SortOrder")`), so each of these is outside it —
    // including the spellings a caller might expect to be accepted.
    for label in ["ASCENDING", "ascending", "Newest", ""] {
        let (status, body) = list(&emulator, Some(label), None, None).await;
        assert_eq!(
            status, 400,
            "case sortOrder {label:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"], "ValidationException",
            "case sortOrder {label:?}: expected ValidationException, got {body}"
        );
    }
}
