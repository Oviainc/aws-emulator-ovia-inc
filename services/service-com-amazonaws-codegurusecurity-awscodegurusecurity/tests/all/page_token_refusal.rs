// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE refusal every paginated reader of this service
//! shares: a `nextToken` the service did not issue.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), reusing
//! the harness of `crate::scan_lifecycle`, so the restJson1 wire shape and the shared
//! engine store are exercised exactly as a client would exercise them.
//!
//! The rule under test: a page token is opaque and only this service mints one
//! (`state::encode_page_token`), so a `nextToken` the service could not have issued fails
//! a specified constraint on the request and is refused with the modelled
//! `ValidationException` (HTTP 400, `reason` `fieldValidationFailed`) naming the member at
//! fault — identically whichever paginated reader the request reached, because all of them
//! report it through the single `state::page_token_refusal` constructor.
//!
//! The cases vary what the rule generalises over: which paginated operation the token
//! reached (`GetFindings`, `ListScans` and `ListFindingsMetrics`, whose cursors are a
//! finding id, a scan name and a UTC day), and the form of the foreign token (not hex at
//! all, hex of an odd number of digits, hex of bytes that are no text, a cursor a caller
//! guessed, and a token at the longest length the model accepts). The token this service
//! DOES issue is driven through each reader too, so the refusal is pinned to the token's
//! origin rather than to passing one at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::ops::types::{
    Finding, ValidationException,
};
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    self as cgs_state, STANDARD_SCAN_SETTLE_MILLIS, STATUS_OPEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::metrics_summary::iso;
use crate::scan_lifecycle::{call, harness, start_scan, store_of, Answer};

/// One day in seconds.
const DAY: i64 = 24 * 60 * 60;

/// `2024-03-01T00:00:00Z`: the first day of the interval the metrics reader is driven
/// over.
const DAY_ZERO: i64 = 1_709_251_200;

/// The scan the readers are driven against.
const SCAN: &str = "token-scan";

/// The paginated readers of this service: the three operations that model a `nextToken`
/// input member (`smithy_query(query="digest")` — `GetFindings`, `ListScans` and
/// `ListFindingsMetrics`).
const READERS: &[&str] = &["GetFindings", "ListScans", "ListFindingsMetrics"];

/// The modelled `nextToken` member name every reader of this service paginates on.
const MEMBER: &str = "nextToken";

/// The wording each reader answered before the refusal was reported from one place,
/// pinned here so no reader — and not the shared constructor either — can reword what a
/// client reads without this suite saying so.
const REFUSAL_MESSAGE: &str = "nextToken is not a token this service issued";

/// The `fieldList` entry's wording; see [`REFUSAL_MESSAGE`].
const REFUSAL_FIELD_MESSAGE: &str = "pass back the nextToken from the previous page unchanged";

/// `op`'s modelled route, with the page controls the row names.
async fn paged_read(
    router: &MultiServiceRouter,
    op: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if op == "ListFindingsMetrics" {
        query.push(format!("startDate={}", iso(DAY_ZERO)));
        query.push(format!("endDate={}", iso(DAY_ZERO + DAY)));
    }
    if op == "GetFindings" {
        query.push("status=All".to_string());
    }
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let path = match op {
        "GetFindings" => format!("/findings/{SCAN}"),
        "ListScans" => "/scans".to_string(),
        "ListFindingsMetrics" => "/metrics/findings".to_string(),
        other => panic!("no route for {other}"),
    };
    let query = query.join("&");
    call(router, "GET", &format!("{path}?{query}"), None).await
}

/// Findings of `scan_name` as a run's analysis records them, seeded straight into the
/// store so a page boundary is reached without one upload and one run per finding.
fn seed_findings(engine: &Arc<engine::InMemoryEngine>, scan_name: &str, ids: &[&str]) {
    let store = store_of(engine);
    let mut guard = store.lock();
    for id in ids {
        guard.findings.insert(
            (scan_name.to_string(), (*id).to_string()),
            Finding {
                id: Some((*id).to_string()),
                status: Some(STATUS_OPEN.to_string()),
                title: Some(format!("seeded finding {id}")),
                ..Finding::default()
            },
        );
    }
}

/// Wait past the clock-owed transition of the run `start_scan` starts, so the readers
/// observe a settled scan.
async fn wait_for_settle() {
    let span = u64::try_from(STANDARD_SCAN_SETTLE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span + 150)).await;
}

/// The foreign tokens the rows drive. Each satisfies the modelled `NextToken` constraints
/// (`smithy_query(query="shape",shape="NextToken")`: len 1..2048, pattern `^[\S]+$`), so
/// each reaches the handler, and none is a cursor `state::encode_page_token` could have
/// produced for any reader.
fn foreign_tokens() -> Vec<(String, String)> {
    vec![
        ("not hex at all".to_string(), "not-a-token".to_string()),
        (
            "an odd number of hex digits".to_string(),
            "abc".to_string(),
        ),
        (
            "hex of bytes that are no text".to_string(),
            "ff".to_string(),
        ),
        ("a cursor a caller guessed".to_string(), "page-2".to_string()),
        // The longest value the modelled `NextToken` length constraint accepts.
        (
            "the longest token the model accepts".to_string(),
            "z".repeat(2048),
        ),
    ]
}

/// The `fieldList` the shared constructor reports, as the wire carries it.
fn expected_field_list(refusal: &ValidationException) -> Value {
    Value::Array(
        refusal
            .field_list
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|field| json!({ "name": field.name, "message": field.message }))
            .collect(),
    )
}

#[tokio::test]
async fn every_paginated_reader_refuses_an_unissued_token_the_same_way() {
    let (engine, router) = harness();
    // A scan that exists and has settled, and findings to report: were the token one this
    // service issued, each reader below would answer the read, so the refusal is about
    // the token and not about the state of the store.
    start_scan(&router, SCAN).await;
    wait_for_settle().await;
    seed_findings(&engine, SCAN, &["f1", "f2"]);

    let expected = cgs_state::page_token_refusal(MEMBER);
    assert_eq!(
        (
            expected.message.as_str(),
            expected.reason.as_str(),
            expected.error_code.as_str()
        ),
        (REFUSAL_MESSAGE, "fieldValidationFailed", "ValidationException"),
        "the shared refusal must keep the wording every reader answered, got {expected:?}"
    );
    assert_eq!(
        expected
            .field_list
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|field| (field.name.as_str(), field.message.as_str()))
            .collect::<Vec<_>>(),
        vec![(MEMBER, REFUSAL_FIELD_MESSAGE)],
        "the shared refusal must report exactly the member at fault, got {:?}",
        expected.field_list
    );
    let fields = expected_field_list(&expected);
    for (label, token) in foreign_tokens() {
        let mut answered: Vec<(&str, Value)> = Vec::new();
        for op in READERS {
            let case = format!("{op}({label})");
            let answer = paged_read(&router, op, None, Some(&token)).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: an unissued nextToken is refused, got {} {}",
                answer.status, answer.json
            );
            assert_eq!(
                answer.error_type, "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {:?} {}",
                answer.error_type, answer.json
            );
            assert_eq!(
                answer.get("errorCode").as_str(),
                Some(expected.error_code.as_str()),
                "case {case:?}: expected errorCode {:?}, got {}",
                expected.error_code,
                answer.json
            );
            assert_eq!(
                answer.get("message").as_str(),
                Some(expected.message.as_str()),
                "case {case:?}: expected message {:?}, got {}",
                expected.message,
                answer.json
            );
            assert_eq!(
                answer.get("reason").as_str(),
                Some(expected.reason.as_str()),
                "case {case:?}: expected reason {:?}, got {}",
                expected.reason,
                answer.json
            );
            assert_eq!(
                answer.get("fieldList"),
                &fields,
                "case {case:?}: expected fieldList {fields}, got {}",
                answer.json
            );
            answered.push((op, answer.json));
        }

        // The readers do not merely each refuse: they answer the SAME refusal, which is
        // what one shared constructor buys and what three copies could not keep true.
        let (first_op, first) = answered
            .first()
            .expect("one answer per paginated reader")
            .clone();
        for (op, body) in &answered[1..] {
            assert_eq!(
                body, &first,
                "case {label:?}: {op} and {first_op} must refuse an unissued nextToken \
                 identically, got {body} and {first}"
            );
        }
    }
}

#[tokio::test]
async fn every_paginated_reader_resumes_from_the_token_it_issued() {
    let (engine, router) = harness();
    start_scan(&router, SCAN).await;
    // A second scan, so the account-wide listing has a page boundary of its own.
    start_scan(&router, "other-scan").await;
    wait_for_settle().await;
    seed_findings(&engine, SCAN, &["f1", "f2"]);

    for op in READERS {
        let case = format!("{op}(its own nextToken)");
        let first = paged_read(&router, op, Some(1), None).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first page is answered, got {} {}",
            first.status, first.json
        );
        let token = first
            .get(MEMBER)
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a page of one must carry a nextToken, got {}", first.json)
            })
            .to_string();
        let second = paged_read(&router, op, Some(1), Some(&token)).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: the token this service issued resumes the listing, got {} {}",
            second.status, second.json
        );
    }
}

#[tokio::test]
async fn the_refusal_names_the_member_it_is_built_for() {
    // The shared constructor reports whichever member carried the token, so a reader
    // paginating on another member name is refused with that member named rather than with
    // this service's `nextToken` hard-coded into the text.
    for member in [MEMBER, "pageToken", "token"] {
        let refusal = cgs_state::page_token_refusal(member);
        assert!(
            refusal.message.starts_with(member),
            "case {member:?}: the message must name the member, got {:?}",
            refusal.message
        );
        let fields = refusal.field_list.clone().unwrap_or_default();
        assert_eq!(
            fields.len(),
            1,
            "case {member:?}: exactly the member at fault is reported, got {fields:?}"
        );
        assert_eq!(
            fields[0].name, member,
            "case {member:?}: fieldList must name the member, got {:?}",
            fields[0].name
        );
        assert_eq!(
            refusal.reason, "fieldValidationFailed",
            "case {member:?}: the modelled reason is fieldValidationFailed, got {:?}",
            refusal.reason
        );
    }
}
