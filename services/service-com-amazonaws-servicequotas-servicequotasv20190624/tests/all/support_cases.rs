// SPDX-License-Identifier: CC0-1.0

//! `CreateSupportCase`: opening the support case that processes a quota increase
//! request, end to end over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the HTTP
//! behaviour and the persisted state behind it. The harness (`Client`, `client`,
//! `emulator`) is the one `quota_workflow.rs` owns.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one case:
//!
//! * a support case is opened only for a request that is still awaiting a decision, and
//!   opening one WRITES that request: it keeps its `Id`, `DesiredValue` and `Created`,
//!   and reads back with a `CaseId` and the `CASE_OPENED` status the model's
//!   `RequestStatus` documents for a request a support case is processing
//!   (`smithy_query(query="shape", shape="RequestStatus")`, and the `Status` member of
//!   `RequestedServiceQuotaChange`). A case is the review the decision waits on, so once
//!   the decision comes due the increase is approved and its desired value is in force —
//!   whatever the submitter had said about `SupportCaseAllowed`, because a case now
//!   reviews the request;
//! * a request that has no decision left to wait on gets no case: an unknown
//!   `RequestId` (or one from another Region) is `NoSuchResourceException`, a request
//!   still awaiting its decision that already carries a case is
//!   `ResourceAlreadyExistsException`, and a decided request is
//!   `InvalidResourceStateException` whether or not a case was opened for it — each on
//!   the client-fault status this awsJson front end uses, and each leaving the request
//!   exactly as it found it.

use std::time::Duration;

use serde_json::{json, Value};

use crate::error_envelope::error_of;
use crate::quota_workflow::{client, emulator, Client};

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once a support case is open on it), so a
/// request whose case was opened before this wait has settled afterwards.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// The per-quota history listing, whose name is too long to repeat in every row.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// One row of the support-case table: a quota increase request to make, and how it says
/// (or does not say) whether Service Quotas may open a case itself.
struct CaseRow {
    /// Why this row is in the table.
    why: &'static str,
    service_code: &'static str,
    quota_code: &'static str,
    desired: f64,
    /// `SupportCaseAllowed` as the request carries it; `None` omits the member.
    support_case_allowed: Option<bool>,
}

/// A request whose support case has been opened, waiting for the decision to come due.
struct Opened {
    case: String,
    id: String,
    case_id: String,
    /// The `(ServiceCode, QuotaCode)` body a quota read takes.
    pair: Value,
    desired: f64,
}

/// Make one quota increase request and return its minted `Id`.
async fn raise(caller: &Client, row: &CaseRow) -> String {
    let mut body = json!({
        "ServiceCode": row.service_code,
        "QuotaCode": row.quota_code,
        "DesiredValue": row.desired,
    });
    if let Some(flag) = row.support_case_allowed {
        body["SupportCaseAllowed"] = json!(flag);
    }
    let accepted = caller.ok("RequestServiceQuotaIncrease", body).await;
    let requested = &accepted["RequestedQuota"];
    assert_eq!(
        requested["Status"].as_str(),
        Some("PENDING"),
        "case {}: a new request is PENDING, got {accepted}",
        row.why
    );
    assert!(
        requested["CaseId"].is_null(),
        "case {}: a new request carries no CaseId, got {accepted}",
        row.why
    );
    requested["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {}: the accepted request reports an Id", row.why))
        .to_string()
}

/// The stored request `id`, as `GetRequestedServiceQuotaChange` reports it.
///
/// `pub(crate)` so the suite that reads one request back at several endpoints shares
/// this reader instead of growing a copy of it.
pub(crate) async fn read_request(caller: &Client, id: &str) -> Value {
    caller
        .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
        .await["RequestedQuota"]
        .clone()
}

/// Opening a support case moves the request to `CASE_OPENED` with a `CaseId`, and the
/// decision that case reviews then approves the increase.
#[tokio::test]
async fn opening_a_support_case_writes_the_request_and_the_case_review_approves_it() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // The rows differ in the service and quota they name, in whether the quota is
    // counted per Region or account-wide, and in the FORM of `SupportCaseAllowed`
    // (absent, explicitly true, explicitly false) — the seeded defaults are
    // `src/state.rs::seed_default_quotas` (vpc/L-F678F1CE = 5, ec2/L-0263D0A3 = 5,
    // budgets/Budgets = 20000 and global, dynamodb/L-F98FE922 = 2500,
    // vpc/L-A4707A72 = 5).
    let rows = vec![
        CaseRow {
            why: "SupportCaseAllowed absent",
            service_code: "vpc",
            quota_code: "L-F678F1CE",
            desired: 20.0,
            support_case_allowed: None,
        },
        CaseRow {
            why: "SupportCaseAllowed explicitly true",
            service_code: "ec2",
            quota_code: "L-0263D0A3",
            desired: 100.0,
            support_case_allowed: Some(true),
        },
        CaseRow {
            why: "an account-wide quota",
            service_code: "budgets",
            quota_code: "Budgets",
            desired: 30_000.0,
            support_case_allowed: Some(true),
        },
        CaseRow {
            why: "a submitter who opted out of the case Service Quotas opens itself",
            service_code: "dynamodb",
            quota_code: "L-F98FE922",
            desired: 5_000.0,
            support_case_allowed: Some(false),
        },
        CaseRow {
            why: "another quota of a service already carrying a case",
            service_code: "vpc",
            quota_code: "L-A4707A72",
            desired: 50.0,
            support_case_allowed: Some(false),
        },
    ];

    let mut opened: Vec<Opened> = Vec::new();
    for row in &rows {
        let case = format!("{} ({}/{})", row.why, row.service_code, row.quota_code);
        let id = raise(&caller, row).await;
        let before = read_request(&caller, &id).await;

        let answer = caller
            .ok("CreateSupportCase", json!({"RequestId": id}))
            .await;
        // `CreateSupportCaseResponse` declares no members, so a served call carries an
        // empty body — the state it wrote is what a later read reports.
        assert_eq!(
            answer,
            json!({}),
            "case {case}: expected an empty response body, got {answer}"
        );

        let after = read_request(&caller, &id).await;
        assert_eq!(
            after["Status"].as_str(),
            Some("CASE_OPENED"),
            "case {case}: expected CASE_OPENED once a case is open, got {after}"
        );
        let case_id = after["CaseId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a CaseId, got {after}"))
            .to_string();
        assert!(
            !case_id.is_empty(),
            "case {case}: expected a non-empty CaseId, got {after}"
        );
        // Nothing else about the request moved: the case is opened ON the stored
        // request, it does not mint a new one.
        for member in ["Id", "ServiceCode", "QuotaCode", "DesiredValue", "Created"] {
            assert_eq!(
                after[member], before[member],
                "case {case}: expected {member} unchanged by opening a case, got {after} \
                 from {before}"
            );
        }
        // The case id is state, not a per-call value: a second read reports the same one.
        let again = read_request(&caller, &id).await;
        assert_eq!(
            again["CaseId"].as_str(),
            Some(case_id.as_str()),
            "case {case}: a second read must report the same CaseId, got {again}"
        );
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        // While a case is open the request is still OPEN, so the quota has no second
        // one — the same rule a PENDING request stands behind.
        let (refused_status, refused_body) = caller
            .call(
                "RequestServiceQuotaIncrease",
                &json!({
                    "ServiceCode": row.service_code,
                    "QuotaCode": row.quota_code,
                    "DesiredValue": row.desired + 1.0,
                }),
            )
            .await;
        let (refused_code, _) = error_of(&refused_body);
        assert_eq!(
            (
                refused_status,
                refused_code.contains("ResourceAlreadyExistsException")
            ),
            (400, true),
            "case {case}: a quota whose request has a case open takes no second request, \
             got {refused_status} {refused_code}"
        );

        opened.push(Opened {
            case,
            id,
            case_id,
            pair,
            desired: row.desired,
        });
    }

    // Every case carries its own id: one case per request, never a shared string.
    for (left, right) in opened.iter().zip(opened.iter().skip(1)) {
        assert_ne!(
            left.case_id, right.case_id,
            "case {} vs {}: two requests must not share a CaseId",
            left.case, right.case
        );
    }

    tokio::time::sleep(PAST_DECISION).await;

    for entry in &opened {
        let settled = read_request(&caller, &entry.id).await;
        assert_eq!(
            settled["Status"].as_str(),
            Some("APPROVED"),
            "case {}: the review a support case opened approves the increase, got {settled}",
            entry.case
        );
        // The case the review belongs to is still reported.
        assert_eq!(
            settled["CaseId"].as_str(),
            Some(entry.case_id.as_str()),
            "case {}: a decided request keeps its CaseId, got {settled}",
            entry.case
        );
        let applied = caller.ok("GetServiceQuota", entry.pair.clone()).await;
        assert_eq!(
            applied["Quota"]["Value"].as_f64(),
            Some(entry.desired),
            "case {}: expected {} in force once the case review approved it, got {applied}",
            entry.case,
            entry.desired
        );
        // The AWS default is untouched by any of this.
        let default = caller
            .ok("GetAWSDefaultServiceQuota", entry.pair.clone())
            .await;
        assert_ne!(
            default["Quota"]["Value"].as_f64(),
            Some(entry.desired),
            "case {}: the AWS default must not carry the increased value, got {default}",
            entry.case
        );
    }
}

/// A request a support case is open on is reported as `CASE_OPENED` by the history
/// listings, and its `Status` filter selects it.
#[tokio::test]
async fn the_history_reports_a_request_whose_case_is_open_as_case_opened() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");
    let pair = json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"});

    let id = raise(
        &caller,
        &CaseRow {
            why: "a request to open a case on",
            service_code: "vpc",
            quota_code: "L-F678F1CE",
            desired: 30.0,
            support_case_allowed: None,
        },
    )
    .await;
    caller
        .ok("CreateSupportCase", json!({"RequestId": id}))
        .await;

    // Both the per-quota and the account-wide listing read the same stored record, and
    // each is asked both unfiltered and filtered on the status the request now carries.
    let listings: Vec<(&str, Value)> = vec![
        (HISTORY_BY_QUOTA, pair.clone()),
        (
            HISTORY_BY_QUOTA,
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "Status": "CASE_OPENED"}),
        ),
        ("ListRequestedServiceQuotaChangeHistory", json!({})),
        (
            "ListRequestedServiceQuotaChangeHistory",
            json!({"Status": "CASE_OPENED"}),
        ),
    ];
    for (listing, body) in &listings {
        let reported = caller.ok(listing, body.clone()).await;
        let entries = reported["RequestedQuotas"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let matching: Vec<&Value> = entries
            .iter()
            .filter(|entry| entry["Id"].as_str() == Some(id.as_str()))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {listing} {body}: expected the request reported exactly once, got {reported}"
        );
        assert_eq!(
            matching[0]["Status"].as_str(),
            Some("CASE_OPENED"),
            "case {listing} {body}: expected CASE_OPENED, got {reported}"
        );
        assert!(
            matching[0]["CaseId"].as_str().is_some(),
            "case {listing} {body}: expected the CaseId reported, got {reported}"
        );
    }

    // A status the request does NOT carry selects nothing, so the filter is reading the
    // stored status rather than ignoring it.
    for status in ["PENDING", "APPROVED", "NOT_APPROVED", "DENIED"] {
        let reported = caller
            .ok(
                HISTORY_BY_QUOTA,
                json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "Status": status}),
            )
            .await;
        let ids: Vec<&str> = reported["RequestedQuotas"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry["Id"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !ids.contains(&id.as_str()),
            "case the {status} filter: a request whose case is open must not be selected, \
             got {reported}"
        );
    }
}

/// A request with no decision left to wait on gets no support case, and the refusal says
/// which condition failed while the request stays exactly as it was.
#[tokio::test]
async fn a_request_that_is_not_awaiting_a_decision_gets_no_case() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");
    let elsewhere = client(&engine, "ap-southeast-2");

    // A request whose case is already open.
    let with_case = raise(
        &caller,
        &CaseRow {
            why: "a request whose case is already open",
            service_code: "vpc",
            quota_code: "L-F678F1CE",
            desired: 20.0,
            support_case_allowed: None,
        },
    )
    .await;
    caller
        .ok("CreateSupportCase", json!({"RequestId": with_case}))
        .await;
    let case_id = read_request(&caller, &with_case).await["CaseId"]
        .as_str()
        .expect("the opened case reports a CaseId")
        .to_string();

    // Asking again while that case is still open: one request has at most one case, so
    // the second call is refused and mints nothing.
    let (status, body) = caller
        .call("CreateSupportCase", &json!({"RequestId": with_case}))
        .await;
    let (code, message) = error_of(&body);
    assert_eq!(
        (status, code.contains("ResourceAlreadyExistsException")),
        (400, true),
        "case a second CreateSupportCase while the case is open: got {status} {code}"
    );
    assert!(
        message
            .as_deref()
            .is_some_and(|text| text.contains(&with_case)),
        "case a second CreateSupportCase while the case is open: the refusal must name the \
         RequestId, got {message:?}"
    );
    let unchanged = read_request(&caller, &with_case).await;
    assert_eq!(
        (
            unchanged["Status"].as_str(),
            unchanged["CaseId"].as_str()
        ),
        (Some("CASE_OPENED"), Some(case_id.as_str())),
        "case a second CreateSupportCase while the case is open: expected the request and \
         its CaseId unchanged, got {unchanged}"
    );

    // A request that will have settled APPROVED, and one that will have settled
    // NOT_APPROVED, by the time a case is asked for.
    let approved = raise(
        &caller,
        &CaseRow {
            why: "a request that settles APPROVED",
            service_code: "ec2",
            quota_code: "L-0263D0A3",
            desired: 100.0,
            support_case_allowed: Some(true),
        },
    )
    .await;
    let not_approved = raise(
        &caller,
        &CaseRow {
            why: "a request that settles NOT_APPROVED",
            service_code: "dynamodb",
            quota_code: "L-F98FE922",
            desired: 5_000.0,
            support_case_allowed: Some(false),
        },
    )
    .await;

    tokio::time::sleep(PAST_DECISION).await;

    // Each row: the caller, the `RequestId`, the error code the condition carries, and
    // the status the request must still report afterwards. The ids vary in FORM too — a
    // syntactically valid id nothing was ever minted for, and a real id of another
    // Region's state.
    let rows: Vec<(&Client, &str, &str, &str, &str)> = vec![
        (
            &caller,
            "0requestthatwasnevermintedhere",
            "an id nothing was minted for",
            "NoSuchResourceException",
            "",
        ),
        (
            &caller,
            "a-b-c",
            "a short id of another form",
            "NoSuchResourceException",
            "",
        ),
        (
            &elsewhere,
            with_case.as_str(),
            "a real id read from another Region",
            "NoSuchResourceException",
            "",
        ),
        (
            &caller,
            with_case.as_str(),
            "a request whose case review has approved it",
            "InvalidResourceStateException",
            "APPROVED",
        ),
        (
            &caller,
            approved.as_str(),
            "a request that was approved",
            "InvalidResourceStateException",
            "APPROVED",
        ),
        (
            &caller,
            not_approved.as_str(),
            "a request that was not approved",
            "InvalidResourceStateException",
            "NOT_APPROVED",
        ),
    ];

    for (who, request_id, why, expected_code, expected_status) in rows {
        let case = format!("{why} ({request_id})");
        let (status, body) = who
            .call("CreateSupportCase", &json!({"RequestId": request_id}))
            .await;
        let (code, message) = error_of(&body);
        // Every modeled client fault of this awsJson service is HTTP 400; the `__type`
        // is what tells one condition from another.
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            code.contains(expected_code),
            "case {case}: expected {expected_code}, got {code}"
        );
        // Never the scaffold's 501: the operation is implemented, so a refusal is a
        // judgement about the request.
        assert!(
            !code.contains("NotImplemented"),
            "case {case}: expected a modeled refusal, got {code}"
        );
        let message = message.unwrap_or_default();
        if expected_code == "NoSuchResourceException" {
            // A request id that resolves to nothing is refused in the service's own
            // FIXED sentence for that fact, so the id the caller supplied is NOT echoed
            // back (`refusal_wording.rs` spans that rule).
            assert!(
                !message.is_empty() && !message.contains(request_id),
                "case {case}: expected a refusal that states the fact without naming the \
                 RequestId, got {message:?}"
            );
        } else {
            // A refusal about the STATE of a request this endpoint DID resolve names it,
            // so a caller can tell which of its requests was meant.
            assert!(
                message.contains(request_id),
                "case {case}: the refusal must name the RequestId, got {message:?}"
            );
        }

        if !expected_status.is_empty() {
            let unchanged = read_request(who, request_id).await;
            assert_eq!(
                unchanged["Status"].as_str(),
                Some(expected_status),
                "case {case}: a refused call leaves the request alone, got {unchanged}"
            );
        }
    }

    // The request whose case was opened still carries the SAME one: no refused call
    // minted another.
    let still = read_request(&caller, &with_case).await;
    assert_eq!(
        still["CaseId"].as_str(),
        Some(case_id.as_str()),
        "case a refused second CreateSupportCase: expected the first CaseId kept, got {still}"
    );
    // And a decided request that never had a case still reports none.
    let decided = read_request(&caller, &not_approved).await;
    assert!(
        decided["CaseId"].is_null(),
        "case a refused CreateSupportCase on a decided request: expected no CaseId, got \
         {decided}"
    );
}
