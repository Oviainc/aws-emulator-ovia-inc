// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListReviewTemplates` — "List review templates"
//! (`smithy_query(query="operation", operation="ListReviewTemplates")`).
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. The list reports one `ReviewTemplateSummary` per STORED review template, projected
//!    from that template's own state: every member `ReviewTemplateSummary` models and no
//!    other, agreeing with the `GetReviewTemplate` of the same template on each member
//!    the two shapes share. A template the store no longer holds is not reported, and a
//!    store that holds none reports an empty page.
//! 2. `MaxResults`/`NextToken` page over the stored templates: however small the page,
//!    following `NextToken` enumerates every template exactly once in one stable order,
//!    and the page that holds the rest of the selection carries no `NextToken`. The
//!    published `@range(1, 50)` bounds of `MaxResults` are served and a value past either
//!    of them is refused.
//! 3. A `NextToken` this service did not issue cannot be decoded, so it is a
//!    `400 ValidationException` classified `CANNOT_PARSE` and naming `NextToken` — not an
//!    empty page.
//! 4. Listing is REPORTING: it writes nothing, so two identical reads report the same
//!    templates and a read between two pages changes neither.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::workload_harness::{created_arn, minimal_template, Answer, Client};

/// Every member `ReviewTemplateSummary` declares, from
/// `smithy_query(query="shape", shape="ReviewTemplateSummary")`. A summary carries these
/// and nothing else.
const SUMMARY_MEMBERS: [&str; 7] = [
    "Description",
    "Lenses",
    "Owner",
    "TemplateArn",
    "TemplateName",
    "UpdateStatus",
    "UpdatedAt",
];

/// The members `ReviewTemplate` carries that `ReviewTemplateSummary` does NOT declare
/// (same `smithy_query`), so a summary must omit them.
const NON_SUMMARY_MEMBERS: [&str; 4] = ["Notes", "Tags", "QuestionCounts", "ShareInvitationId"];

/// The `ReviewTemplates` a successful `ListReviewTemplates` answered with.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListReviewTemplates 200, got {answer:?}"
    );
    answer
        .body
        .get("ReviewTemplates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a successful list must carry ReviewTemplates, got {answer:?}")
        })
}

fn arn_of(summary: &Value) -> String {
    summary
        .get("TemplateArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every stored template, read by following `NextToken` from the first page to the last
/// with pages of `page_size` (or in one request when `page_size` is `None`). Answers the
/// summaries in the order the pages served them.
async fn walk(client: &Client, case: &str, page_size: Option<i64>) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One more request than there can be pages, so a handler that never stops paging
    // fails here instead of hanging.
    for request in 0..64 {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("MaxResults={size}"));
        }
        if let Some(cursor) = &token {
            query.push(format!("NextToken={cursor}"));
        }
        let answer = client.list_review_templates(&query.join("&")).await;
        let page = summaries(&answer, case);
        if let Some(size) = page_size {
            assert!(
                i64::try_from(page.len()).expect("a page fits in an i64") <= size,
                "case {case:?}: request {request} returned {} summaries for MaxResults={size}",
                page.len()
            );
        }
        walked.extend(page);
        token = match answer.body.get("NextToken").and_then(Value::as_str) {
            Some(cursor) => {
                assert!(
                    !cursor.is_empty()
                        && cursor.chars().all(|character| character.is_ascii_alphanumeric()
                            || matches!(character, '+' | '/' | '=' | '_' | '-')),
                    "case {case:?}: NextToken must match ^[A-Za-z0-9+/=_-]+$, got {cursor:?}"
                );
                Some(cursor.to_string())
            }
            None => return walked,
        };
    }
    panic!("case {case:?}: paging did not terminate, got {} summaries", walked.len())
}

/// Create `count` review templates and answer their ARNs.
async fn seed(client: &Client, prefix: &str, count: usize) -> Vec<String> {
    let mut arns = Vec::new();
    for index in 0..count {
        let case = format!("{prefix}-{index:03}");
        let created = client
            .create_review_template(minimal_template(&case, &format!("tok-{case}")))
            .await;
        arns.push(created_arn(&created, &case));
    }
    arns
}

/// Rule 1. The rows differ along what the create carried — the lens count, how a lens is
/// named, whether notes and tags are present, and the name/description lengths at their
/// published bounds — because each summary is projected from the stored template's own
/// state and not from one recorded request.
#[tokio::test]
async fn every_stored_template_is_reported_as_its_modelled_summary() {
    // (case, request overrides)
    let cases: Vec<(&str, Value)> = vec![
        ("minimal", json!({})),
        (
            "notes-and-tags",
            json!({ "Notes": "notes the summary does not carry", "Tags": { "Env": "test" } }),
        ),
        ("no-lenses", json!({ "Lenses": [] })),
        // "For Amazon Web Services official lenses, this is either the lens alias … or
        // the lens ARN" (`LensAlias` doc): the summary reports the lens as the template
        // stores it.
        (
            "lens-by-arn",
            json!({ "Lenses": ["arn:aws:wellarchitected:us-east-1::lens/wellarchitected"] }),
        ),
        (
            "two-lenses",
            json!({ "Lenses": ["wellarchitected", "serverless"] }),
        ),
        // `TemplateName` is @length(3, 100) and `Description` @length(3, 250): each at its
        // published minimum, then at its published maximum.
        (
            "name-and-description-at-the-minimum-length",
            json!({ "TemplateName": "abc", "Description": "xyz" }),
        ),
        (
            "name-and-description-at-the-maximum-length",
            json!({ "TemplateName": "n".repeat(100), "Description": "d".repeat(250) }),
        ),
    ];

    // One client, so every template shares a store and one list reports them all.
    let client = Client::new();
    let mut requests: Vec<(String, Value, String)> = Vec::new();
    for (case, overrides) in cases {
        let mut request = minimal_template(&format!("summary-{case}"), &format!("tok-{case}"));
        for (name, value) in overrides.as_object().expect("overrides are an object") {
            request[name] = value.clone();
        }
        let arn = created_arn(&client.create_review_template(request.clone()).await, case);
        requests.push((case.to_string(), request, arn));
    }

    let listed = walk(&client, "all-templates", None).await;
    assert_eq!(
        listed.len(),
        requests.len(),
        "case \"all-templates\": expected one summary per stored template, got {listed:?}"
    );

    for (case, request, arn) in &requests {
        let summary = listed
            .iter()
            .find(|summary| &arn_of(summary) == arn)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the list must report the template {arn:?}, got {listed:?}")
            });

        // Exactly the modelled members, and no member the summary shape does not declare.
        let reported: BTreeSet<&str> = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a summary is an object, got {summary:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        let expected: BTreeSet<&str> = SUMMARY_MEMBERS.into_iter().collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the summary members {expected:?}, got {summary:?}"
        );
        for absent in NON_SUMMARY_MEMBERS {
            assert!(
                summary.get(absent).is_none(),
                "case {case:?}: ReviewTemplateSummary models no {absent}, got {summary:?}"
            );
        }

        // The members the REQUEST set come back as sent.
        for name in ["TemplateName", "Description", "Lenses"] {
            assert_eq!(
                summary.get(name),
                request.get(name),
                "case {case:?}: {name} must be reported as stored, got {summary:?}"
            );
        }

        // The members the SERVICE computes.
        assert_eq!(
            summary.get("Owner").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: the creating account owns the template, got {summary:?}"
        );
        assert_eq!(
            summary.get("UpdateStatus").and_then(Value::as_str),
            Some("CURRENT"),
            "case {case:?}: a template whose lenses are current reports CURRENT, got {summary:?}"
        );

        // The summary agrees with the full shape on every member the two share.
        let fetched = client.get_review_template(arn).await;
        let template = fetched
            .body
            .get("ReviewTemplate")
            .cloned()
            .unwrap_or(Value::Null);
        for name in SUMMARY_MEMBERS {
            assert_eq!(
                summary.get(name),
                template.get(name),
                "case {case:?}: the summary and GetReviewTemplate must agree on {name}, \
                 got {summary:?} and {template:?}"
            );
        }
    }
}

/// Rule 1's other half: a store that holds no template reports an empty page, and a
/// template the store no longer holds leaves the list while its neighbours stay.
#[tokio::test]
async fn the_list_reports_exactly_what_the_store_holds() {
    let client = Client::new();

    let empty = client.list_review_templates("").await;
    assert!(
        summaries(&empty, "empty-store").is_empty(),
        "case \"empty-store\": a store with no template reports no summary, got {empty:?}"
    );
    assert!(
        empty.body.get("NextToken").is_none(),
        "case \"empty-store\": an empty page carries no NextToken, got {empty:?}"
    );

    let arns = seed(&client, "removed", 4).await;
    // The rows differ along WHICH of the stored templates is dropped — the first in the
    // list order, one in the middle and the last — so the list is reporting the store
    // rather than a fixed set.
    let mut remaining: Vec<String> = arns.clone();
    remaining.sort();
    for case in ["first", "middle", "last"] {
        let position = match case {
            "first" => 0,
            "middle" => remaining.len() / 2,
            _ => remaining.len() - 1,
        };
        let dropped = remaining.remove(position);
        let deleted = client
            .delete_review_template(&dropped, &format!("tok-drop-{case}"))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteReviewTemplate 200, got {deleted:?}"
        );

        let listed = walk(&client, case, None).await;
        let reported: Vec<String> = listed.iter().map(arn_of).collect();
        assert_eq!(
            reported, remaining,
            "case {case:?}: the list must report exactly the templates the store holds"
        );
    }
}

/// Rule 2. The rows are every page size from one template per page up to past the stored
/// count, plus the published `@range(1, 50)` maximum and a request that omits
/// `MaxResults` — however the selection is cut, the pages enumerate the same templates in
/// the same order, exactly once each.
#[tokio::test]
async fn paging_enumerates_every_template_exactly_once() {
    let client = Client::new();
    let created = seed(&client, "paged", 7).await;
    let mut expected: Vec<String> = created.clone();
    // The emulator's order is `TemplateArn`, the only member of a template that is both
    // unique and fixed for its whole life.
    expected.sort();

    for (case, page_size) in [
        ("omitted", None),
        ("published-minimum", Some(1)),
        ("two-per-page", Some(2)),
        ("three-per-page", Some(3)),
        ("one-short-of-the-count", Some(6)),
        ("exactly-the-count", Some(7)),
        ("more-than-the-count", Some(8)),
        ("published-maximum", Some(50)),
    ] {
        let walked: Vec<String> = walk(&client, case, page_size).await.iter().map(arn_of).collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must enumerate every template exactly once in ARN order"
        );
    }

    // A page that holds the rest of the selection carries no `NextToken`; one that does
    // not, does.
    for (case, max_results, expects_token) in [
        ("short-page", 3, true),
        ("page-of-the-whole-selection", 7, false),
        ("page-larger-than-the-selection", 50, false),
    ] {
        let answer = client
            .list_review_templates(&format!("MaxResults={max_results}"))
            .await;
        assert_eq!(
            answer.body.get("NextToken").and_then(Value::as_str).is_some(),
            expects_token,
            "case {case:?}: NextToken must be present only while templates remain, got {answer:?}"
        );
    }

    // The published bounds are served; a value past either of them is refused before the
    // handler runs.
    for (case, max_results, expected_status) in [
        ("published-minimum", 1, 200),
        ("published-maximum", 50, 200),
        ("below-the-minimum", 0, 400),
        ("above-the-maximum", 51, 400),
    ] {
        let answer = client
            .list_review_templates(&format!("MaxResults={max_results}"))
            .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for MaxResults={max_results}, \
             got {answer:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {answer:?}"
            );
        }
    }
}

/// Rule 3. The rows differ along the undecodable token's form — each one satisfying the
/// modelled `NextToken` `@pattern("^[A-Za-z0-9+/=_-]+$")`, so the refusal is about the
/// token's content and not its characters — and include cursors ANOTHER paginated
/// operation of this service issues, which are not cursors here.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let client = Client::new();
    seed(&client, "token", 3).await;

    let issued = client
        .list_review_templates("MaxResults=1")
        .await
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("three templates and a page of one leave a NextToken")
        .to_string();

    let rows: Vec<(&str, String)> = vec![
        ("not-hex", "zzzz".to_string()),
        ("hex-but-not-a-cursor", "6162".to_string()),
        ("single-character", "a".to_string()),
        ("truncated-issued-token", issued[..issued.len() / 2].to_string()),
        ("issued-token-with-a-suffix", format!("{issued}00")),
        // hex of {"n":"a","i":"b"} — the cursor `ListWorkloads` issues.
        (
            "a-workloads-cursor",
            "7b226e223a2261222c2269223a2262227d".to_string(),
        ),
        // hex of {"s":"a"} — the cursor `ListWorkloadShares` issues.
        ("a-workload-shares-cursor", "7b2273223a2261227d".to_string()),
    ];

    for (case, token) in rows {
        let answer = client
            .list_review_templates(&format!("NextToken={token}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token this service did not issue is a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: an opaque token that will not decode is CANNOT_PARSE, got {answer:?}"
        );
        assert_eq!(
            answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .map(|fields| fields
                    .iter()
                    .map(|field| field.get("Name").and_then(Value::as_str).unwrap_or_default())
                    .collect::<Vec<_>>()),
            Some(vec!["NextToken"]),
            "case {case:?}: the finding must name NextToken, got {answer:?}"
        );
    }

    // The token this service DID issue still pages, so the refusals above are about the
    // token and not about the operation.
    let resumed = client
        .list_review_templates(&format!("NextToken={issued}"))
        .await;
    assert_eq!(
        resumed.status, 200,
        "case \"issued-token\": a token this service issued must page, got {resumed:?}"
    );
}

/// Rule 4. The rows are the page sizes a read can be cut into: for each, listing twice
/// reports the same templates, and a full re-read taken between two pages of a walk
/// changes neither the rest of that walk nor the stored templates.
#[tokio::test]
async fn listing_writes_nothing() {
    let client = Client::new();
    let created = seed(&client, "reporting", 5).await;
    let mut expected: Vec<String> = created.clone();
    expected.sort();

    for (case, page_size) in [
        ("omitted", None),
        ("one-per-page", Some(1)),
        ("two-per-page", Some(2)),
    ] {
        let first: Vec<String> = walk(&client, case, page_size).await.iter().map(arn_of).collect();
        let second: Vec<String> = walk(&client, case, page_size).await.iter().map(arn_of).collect();
        assert_eq!(
            first, expected,
            "case {case:?}: the first read must report every stored template"
        );
        assert_eq!(
            second, first,
            "case {case:?}: a second read must report the same templates"
        );
    }

    // A read taken BETWEEN two pages does not move the cursor's selection.
    let first_page = client.list_review_templates("MaxResults=2").await;
    let mut seen: Vec<String> = summaries(&first_page, "interleaved")
        .iter()
        .map(arn_of)
        .collect();
    let token = first_page
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("five templates and a page of two leave a NextToken")
        .to_string();
    let interleaved: Vec<String> = walk(&client, "interleaved-full-read", None)
        .await
        .iter()
        .map(arn_of)
        .collect();
    assert_eq!(
        interleaved, expected,
        "case \"interleaved\": the read between two pages must report every template"
    );
    let rest = client
        .list_review_templates(&format!("MaxResults=50&NextToken={token}"))
        .await;
    seen.extend(summaries(&rest, "interleaved").iter().map(arn_of));
    assert_eq!(
        seen, expected,
        "case \"interleaved\": a read between two pages must not change what the rest of \
         the walk reports"
    );
}
