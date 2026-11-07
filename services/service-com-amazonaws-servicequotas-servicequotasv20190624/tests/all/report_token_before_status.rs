// SPDX-License-Identifier: CC0-1.0

//! Whether a `NextToken` is good is a property of the REQUEST, not of how far the
//! clock has carried the resource it reads.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the wire
//! behaviour. The harness (`Client`, `base64_of`) is the one `quota_workflow.rs` owns.
//!
//! The RULE, asserted over a TABLE of malformed tokens rather than over one recorded
//! request: a paginated read refuses a `NextToken` that names no position in the
//! listing it was issued for — with `InvalidPaginationTokenException` (HTTP 400), on
//! EVERY call, whatever status the resource being read carries. The same token
//! therefore cannot be answered 200 by a report that is still generating and 400 by
//! the same report a few hundred milliseconds later; and a token that DOES name a
//! position in the report's listing is answered 200 in either status, so the refusal
//! stays as narrow as the rule.
//!
//! `utilization_report.rs::a_report_this_emulator_did_not_mint_is_refused` pins the
//! COMPLETED half of the table; what this suite adds is the same rows against a report
//! the clock has not finished, plus the accepted counterpart.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::UTILIZATION_REPORT_PAGE_SIZE;

use crate::quota_workflow::{base64_of, client, emulator, Client};

/// Comfortably past `crate::state::REPORT_COMPLETION_DELAY` (500 ms), so a report
/// started before this wait has completed afterwards.
const PAST_COMPLETION: Duration = Duration::from_millis(1_200);

/// Regional endpoints, one per table row: a start reuses the region's live report, so
/// a row that needs its own freshly `PENDING` report needs its own region.
///
/// Source: the regional endpoints `src/state.rs` keys this service's state by — any
/// region in the credential scope, as `quota_workflow.rs` addresses them.
const REGIONS: &[&str] = &[
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
    "eu-west-1",
    "eu-west-2",
    "eu-central-1",
    "ap-south-1",
    "ap-northeast-1",
    "ap-southeast-2",
    "sa-east-1",
    "ca-central-1",
];

/// The ways a `NextToken` can name no position, as `(case, token)` — built against the
/// report id the token is handed to.
///
/// The rows span the forms the refusal has to cover: bytes that are not base64 at all,
/// base64 of bytes that are not text, text that is not `<listing>:<offset>`, another
/// operation's listing, another report's listing of THIS operation, a non-numeric
/// offset, and a numeric offset past the end of the report.
fn bogus_tokens(report_id: &str) -> Vec<(String, String)> {
    let listing = format!("GetQuotaUtilizationReport:{report_id}");
    vec![
        (
            "bytes that are not base64".to_string(),
            "not+a+token".to_string(),
        ),
        (
            // `//4=` is base64 of 0xFF 0xFE, which no UTF-8 text decodes from.
            "base64 of bytes that are not text".to_string(),
            "//4=".to_string(),
        ),
        (
            "base64 of text that names no listing".to_string(),
            base64_of("foo"),
        ),
        (
            "a token of another operation's listing".to_string(),
            base64_of("ListServices:1"),
        ),
        (
            "a token of another report's listing".to_string(),
            base64_of("GetQuotaUtilizationReport:some-other-report:1"),
        ),
        (
            "this report's listing with no offset".to_string(),
            base64_of(&listing),
        ),
        (
            "this report's listing with a non-numeric offset".to_string(),
            base64_of(&format!("{listing}:eleven")),
        ),
        (
            "this report's listing with a negative offset".to_string(),
            base64_of(&format!("{listing}:-1")),
        ),
        (
            "this report's listing past its last record".to_string(),
            base64_of(&format!("{listing}:999999")),
        ),
    ]
}

/// Start a report and return its `ReportId`, asserting the start reports it `PENDING`
/// (so the rows that follow are handed a report whose generation has just begun).
async fn start_pending_report(caller: &Client, case: &str) -> String {
    let started = caller.ok("StartQuotaUtilizationReport", json!({})).await;
    assert_eq!(
        started["Status"],
        json!("PENDING"),
        "case {case}: a just-started report must be PENDING, got {started}"
    );
    started["ReportId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a ReportId, got {started}"))
        .to_string()
}

/// The `Status` a read of `report_id` reports right now.
async fn status_now(caller: &Client, report_id: &str) -> String {
    let read = caller
        .ok("GetQuotaUtilizationReport", json!({"ReportId": report_id}))
        .await;
    read["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("a report must report a Status, got {read}"))
        .to_string()
}

/// Every record of a COMPLETED report, walked with the tokens the service issues.
///
/// A report longer than one page reports its first page and a `NextToken`, so the whole
/// snapshot takes the walk — reading one unasked page would be reading one page of it.
async fn whole_report(caller: &Client, report_id: &str) -> Vec<Value> {
    let mut records: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({"ReportId": report_id});
        if let Some(next) = &token {
            body["NextToken"] = json!(next);
        }
        let page = caller.ok("GetQuotaUtilizationReport", body).await;
        records.extend(
            page["Quotas"]
                .as_array()
                .unwrap_or_else(|| panic!("a completed report publishes Quotas, got {page}"))
                .clone(),
        );
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return records;
        }
    }
}

/// Assert one bogus token is refused as the model declares.
async fn assert_refused(caller: &Client, case: &str, report_id: &str, token: &str) {
    let (status, code) = caller
        .refused(
            "GetQuotaUtilizationReport",
            json!({"ReportId": report_id, "NextToken": token}),
        )
        .await;
    assert_eq!(
        (status, code.contains("InvalidPaginationTokenException")),
        (400, true),
        "case {case}: expected HTTP 400 InvalidPaginationTokenException, got {status} {code}"
    );
}

/// RULE: a token naming no position is refused while the report is still generating —
/// the status the clock has reached does not suspend the check.
#[tokio::test]
async fn a_token_naming_no_position_is_refused_while_the_report_is_generating() {
    let engine = emulator();

    // Every row gets its OWN region, so every row is handed a report that was PENDING
    // when the row started.
    let rows = bogus_tokens("placeholder");
    assert!(
        rows.len() <= REGIONS.len(),
        "case table: {} rows need a region each, only {} are listed",
        rows.len(),
        REGIONS.len()
    );

    // How many rows were still provably unfinished when they were answered. The read
    // that follows the refusal can only report a status the report had reached AFTER
    // the refusal, and the statuses only move forwards, so a non-COMPLETED status there
    // proves the refusal itself was answered before completion.
    let mut unfinished_rows = 0_usize;

    for (index, region) in REGIONS.iter().enumerate().take(rows.len()) {
        let caller = client(&engine, region);
        let report_id = start_pending_report(&caller, region).await;
        let (case, token) = bogus_tokens(&report_id)
            .into_iter()
            .nth(index)
            .expect("one row per region");
        let case = format!("{case} in {region}");

        assert_refused(&caller, &case, &report_id, &token).await;

        let status = status_now(&caller, &report_id).await;
        if status != "COMPLETED" {
            unfinished_rows += 1;
        }
    }

    assert!(
        unfinished_rows >= 1,
        "case table: no row was answered inside the report's generation window \
         (REPORT_COMPLETION_DELAY is 500 ms), so the unfinished branch went untested; \
         0 of {} rows",
        rows.len()
    );
}

/// RULE, the other end of the same report's life: the identical tokens are refused once
/// the clock has completed the report, so the answer never flips with the status.
#[tokio::test]
async fn the_same_tokens_are_refused_once_the_report_has_completed() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");
    let report_id = start_pending_report(&caller, "a completed report").await;

    // Before the clock has completed it, and after: the SAME rows, the same answers.
    for phase in ["still generating", "completed"] {
        if phase == "completed" {
            tokio::time::sleep(PAST_COMPLETION).await;
            assert_eq!(
                status_now(&caller, &report_id).await,
                "COMPLETED",
                "case {phase}: the clock must have completed the report by now"
            );
        }
        for (case, token) in bogus_tokens(&report_id) {
            assert_refused(&caller, &format!("{case}, {phase}"), &report_id, &token).await;
        }
    }
}

/// The refusal is exactly as wide as the rule: a token that DOES name a position in
/// this report's listing is served in either status — status only while the report is
/// generating, records once it has completed.
#[tokio::test]
async fn a_token_naming_a_position_in_the_report_is_served_in_either_status() {
    let engine = emulator();
    let caller = client(&engine, "eu-central-1");
    let report_id = start_pending_report(&caller, "an accepted token").await;
    let listing = format!("GetQuotaUtilizationReport:{report_id}");

    // Rows: no token at all, the empty token (the same request as none), and positions
    // inside the report's frozen snapshot.
    let rows: Vec<(String, Option<String>)> = vec![
        ("no NextToken".to_string(), None),
        ("the empty NextToken".to_string(), Some(String::new())),
        (
            "the first position".to_string(),
            Some(base64_of(&format!("{listing}:0"))),
        ),
        (
            "a position inside the report".to_string(),
            Some(base64_of(&format!("{listing}:3"))),
        ),
    ];

    for (case, token) in &rows {
        let mut body = json!({"ReportId": &report_id});
        if let Some(token) = token {
            body["NextToken"] = json!(token);
        }
        let served = caller.ok("GetQuotaUtilizationReport", body).await;
        assert_eq!(
            served["ReportId"],
            json!(&report_id),
            "case {case}: a read must answer for the report it was asked for, got {served}"
        );
        // Whichever status the clock has reached, the token is accepted; what differs is
        // only whether the report publishes records yet.
        let status = served["Status"].as_str().unwrap_or_default().to_string();
        if status == "COMPLETED" {
            assert!(
                served["Quotas"].is_array(),
                "case {case}: a completed report must publish Quotas, got {served}"
            );
        } else {
            assert!(
                served["Quotas"].is_null() && served["TotalCount"].is_null(),
                "case {case}: an unfinished report must publish no records, got {served}"
            );
        }
    }

    // And once the clock has completed it, the same positions serve the snapshot from
    // exactly the offsets they name.
    tokio::time::sleep(PAST_COMPLETION).await;
    // The whole snapshot, walked with the tokens the service issues: a report longer
    // than one page (`UTILIZATION_REPORT_PAGE_SIZE`) reports its first page and a
    // `NextToken`, so the records a position names are the slice that starts there
    // BOUNDED by one page.
    let all = whole_report(&caller, &report_id).await;
    assert!(
        all.len() > 3,
        "case the completed report: the snapshot must be longer than the offsets this \
         suite names, got {} records",
        all.len()
    );
    for offset in [0_usize, 3] {
        let case = format!("offset {offset} of the completed report");
        let served = caller
            .ok(
                "GetQuotaUtilizationReport",
                json!({
                    "ReportId": &report_id,
                    "NextToken": base64_of(&format!("{listing}:{offset}")),
                }),
            )
            .await;
        let records = served["Quotas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected Quotas, got {served}"))
            .clone();
        let expected: Vec<Value> = all[offset..]
            .iter()
            .take(UTILIZATION_REPORT_PAGE_SIZE)
            .cloned()
            .collect();
        assert_eq!(
            records,
            expected,
            "case {case}: a token resumes at the position it names, expected {} records, got {}",
            expected.len(),
            records.len()
        );
    }
}
