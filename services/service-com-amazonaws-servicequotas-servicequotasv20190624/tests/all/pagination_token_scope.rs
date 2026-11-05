// SPDX-License-Identifier: CC0-1.0

//! Which listing a `NextToken` of this service is good for.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the wire
//! behaviour. The harness (`Client`, `walk_pages`) is the one `quota_workflow.rs` owns.
//!
//! The RULE, asserted over a TABLE rather than over one recorded request: a page token
//! names an offset, and an offset means something only inside ONE ordered result set, so
//! a token is good exactly for the listing the inputs that SELECT records chose — the
//! `ServiceCode`, `QuotaCode`, `Status` and level members, not the `MaxResults` that only
//! cuts the page. Replayed against any other selection of the same operation it is
//! `InvalidPaginationTokenException` (HTTP 400) rather than a page read out of the middle
//! of that other listing; replayed against its own selection it resumes exactly where
//! its page stopped, whatever page size the continuation asks for.
//!
//! `quota_workflow.rs::a_next_token_this_service_did_not_issue_is_refused` pins the
//! coarser half of the same rule (a token of another OPERATION, and bytes no listing
//! issued); what this suite adds is the same-operation rows, one per member a listing
//! narrows itself by.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::APPLIED_QUOTA_PAGE_SIZE;

use crate::quota_workflow::{client, emulator, walk_pages, Client};

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open), so a request
/// made before this wait has settled afterwards and the history listings stop moving.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// The account-wide history listing, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";

/// The per-quota history listing.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// Adjustable quotas to raise, as `(ServiceCode, QuotaCode, DesiredValue)`.
///
/// Source: the seeded catalog in `src/state.rs` (rows AWS publishes as `Adjustable`),
/// with a desired value above each quota's published default.
const RAISEABLE: &[(&str, &str, f64)] = &[
    ("vpc", "L-F678F1CE", 10.0),
    ("vpc", "L-A4707A72", 25.0),
    ("lambda", "L-B99A9384", 2_000.0),
    ("dynamodb", "L-F98FE922", 5_000.0),
];

/// The quota raised TWICE, so the per-quota history holds more than one record and can
/// therefore cut a page short.
const TWICE_RAISED: (&str, &str, f64) = ("vpc", "L-F678F1CE", 11.0);

/// One row of the table: two selections of ONE operation, each of which a token of the
/// other must not resume.
struct Row {
    /// What names the case in every assertion.
    case: String,
    /// The operation both selections are of.
    operation: &'static str,
    /// The output member carrying the items.
    items: &'static str,
    /// One selection.
    one: Value,
    /// Another selection of the same operation.
    other: Value,
}

/// Raise `desired` on one quota.
async fn raise(caller: &Client, service_code: &str, quota_code: &str, desired: f64) {
    caller
        .ok(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "DesiredValue": desired,
            }),
        )
        .await;
}

/// Store a settled request history: several quotas raised and decided, one of them
/// raised a second time, so every history listing is non-empty and no longer moving.
async fn seed_settled_history(caller: &Client) {
    for (service_code, quota_code, desired) in RAISEABLE {
        raise(caller, service_code, quota_code, *desired).await;
    }
    // Every step the clock owes comes due, so the stored requests are decided.
    tokio::time::sleep(PAST_DECISION).await;
    let (service_code, quota_code, desired) = TWICE_RAISED;
    raise(caller, service_code, quota_code, desired).await;
    tokio::time::sleep(PAST_DECISION).await;
}

/// The items a response carries under `member`.
fn items(case: &str, response: &Value, member: &str) -> Vec<Value> {
    response[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a {member} list, got {response}"))
        .clone()
}

/// The page a listing of this service serves a caller — a first call or a continuation
/// — that names no `MaxResults`: its OWN unasked page size, which for the applied-quota
/// listing is `APPLIED_QUOTA_PAGE_SIZE` and for the others the whole remainder.
fn unasked_page(operation: &str) -> usize {
    if operation == "ListServiceQuotas" {
        APPLIED_QUOTA_PAGE_SIZE
    } else {
        usize::MAX
    }
}

/// The `NextToken` of `body`'s first page at `size`, or `None` when that page is the
/// whole selection (nothing remains for a token to resume).
async fn first_page_token(
    caller: &Client,
    operation: &str,
    body: &Value,
    size: i64,
) -> Option<String> {
    let mut asked = body.clone();
    asked["MaxResults"] = json!(size);
    caller.ok(operation, asked).await["NextToken"]
        .as_str()
        .map(str::to_string)
}

/// The whole table: for each member a listing of this service narrows itself by, two
/// selections that differ along it.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // `ServiceCode`, over services whose default sets differ in length (`iam` publishes
    // a handful, `ec2` hundreds) — including the pair the token is longest-lived for.
    for (one, other) in [("ec2", "iam"), ("vpc", "dynamodb")] {
        rows.push(Row {
            case: format!("ListAWSDefaultServiceQuotas {one} vs {other}"),
            operation: "ListAWSDefaultServiceQuotas",
            items: "Quotas",
            one: json!({"ServiceCode": one}),
            other: json!({"ServiceCode": other}),
        });
    }

    // The applied listing narrows by `ServiceCode`, by `QuotaCode` and by the plane:
    // a token of the whole service must not resume the one-quota listing, and a token of
    // the ACCOUNT plane must not resume the RESOURCE plane (which here is SHORTER than
    // the token's offset, the case the old code answered with an empty page).
    rows.push(Row {
        case: "ListServiceQuotas ec2 vs iam".to_string(),
        operation: "ListServiceQuotas",
        items: "Quotas",
        one: json!({"ServiceCode": "ec2"}),
        other: json!({"ServiceCode": "iam"}),
    });
    rows.push(Row {
        case: "ListServiceQuotas vpc, whole service vs one QuotaCode".to_string(),
        operation: "ListServiceQuotas",
        items: "Quotas",
        one: json!({"ServiceCode": "vpc"}),
        other: json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"}),
    });
    rows.push(Row {
        case: "ListServiceQuotas ec2, the ACCOUNT plane vs the RESOURCE plane".to_string(),
        operation: "ListServiceQuotas",
        items: "Quotas",
        one: json!({"ServiceCode": "ec2"}),
        other: json!({"ServiceCode": "ec2", "QuotaAppliedAtLevel": "RESOURCE"}),
    });

    // The account-wide history narrows by `ServiceCode`, by `Status` and by the level
    // the increase was requested at.
    for (what, other) in [
        ("ServiceCode", json!({"ServiceCode": "vpc"})),
        ("Status=APPROVED", json!({"Status": "APPROVED"})),
        ("Status=PENDING", json!({"Status": "PENDING"})),
        (
            "QuotaRequestedAtLevel=ALL",
            json!({"QuotaRequestedAtLevel": "ALL"}),
        ),
    ] {
        rows.push(Row {
            case: format!("{HISTORY} every request vs {what}"),
            operation: HISTORY,
            items: "RequestedQuotas",
            one: json!({}),
            other,
        });
    }

    // The per-quota history narrows by the pair and by `Status`.
    rows.push(Row {
        case: format!("{HISTORY_BY_QUOTA} two quotas of one service"),
        operation: HISTORY_BY_QUOTA,
        items: "RequestedQuotas",
        one: json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"}),
        other: json!({"ServiceCode": "vpc", "QuotaCode": "L-A4707A72"}),
    });
    rows.push(Row {
        case: format!("{HISTORY_BY_QUOTA} one quota, every status vs Status=PENDING"),
        operation: HISTORY_BY_QUOTA,
        items: "RequestedQuotas",
        one: json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"}),
        other: json!({
            "ServiceCode": "vpc",
            "QuotaCode": "L-F678F1CE",
            "Status": "PENDING",
        }),
    });

    rows
}

/// RULE: a token is good only for the selection that issued it, so replaying it against
/// any other selection of the SAME operation is `InvalidPaginationTokenException` — never
/// a page read out of the middle of that other listing.
#[tokio::test]
async fn a_token_of_one_selection_does_not_resume_another() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");
    seed_settled_history(&caller).await;

    let mut refusals = 0_usize;
    for Row {
        case,
        operation,
        one,
        other,
        ..
    } in &rows()
    {
        // Both directions: whichever of the two selections is long enough to cut a page
        // short issues the token, and the other must refuse it.
        for (from, to) in [(one, other), (other, one)] {
            let Some(token) = first_page_token(&caller, operation, from, 1).await else {
                // This selection fits in one page, so it issues no token to replay.
                continue;
            };
            let case = format!("{case}: a token of {from} replayed against {to}");
            let mut body = to.clone();
            body["NextToken"] = json!(token);
            let (status, code) = caller.refused(operation, body).await;
            assert_eq!(
                (status, code.contains("InvalidPaginationTokenException")),
                (400, true),
                "case {case}: expected HTTP 400 InvalidPaginationTokenException, got {status} \
                 {code}"
            );
            refusals += 1;
        }
    }

    // The table must actually have exercised a token per row, in at least one direction.
    assert!(
        refusals >= rows().len(),
        "case table: expected a refusal for every row, got {refusals} over {} rows",
        rows().len()
    );
}

/// RULE, the other half: `MaxResults` cuts the page and selects nothing, so a token
/// resumes its OWN selection exactly where its page stopped — at any page size the
/// continuation asks for, and at none.
#[tokio::test]
async fn a_token_resumes_the_selection_that_issued_it() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");
    seed_settled_history(&caller).await;

    // Every selection of the table, each walked on its own terms.
    let selections: Vec<(String, &'static str, &'static str, Value)> = rows()
        .into_iter()
        .flat_map(|row| {
            [
                (
                    format!("{} [{}]", row.case, row.one),
                    row.operation,
                    row.items,
                    row.one,
                ),
                (
                    format!("{} [{}]", row.case, row.other),
                    row.operation,
                    row.items,
                    row.other,
                ),
            ]
        })
        .collect();

    let mut resumed_rows = 0_usize;
    for (case, operation, member, body) in &selections {
        // The selection itself, as an unasked call serves it: the whole set in one page.
        let whole = walk_pages(&caller, operation, body, member, None).await;
        if whole.len() < 2 {
            // Nothing to cut short; the single-page shape is pinned by the suites that
            // own the unasked page size.
            continue;
        }

        // Rows: page sizes that divide none of these sets evenly, and a continuation
        // that asks for another size or for none at all.
        for size in [1_i64, 2, 3] {
            let case = format!("{case} MaxResults={size}");
            let taken = (size as usize).min(whole.len());
            let mut asked = body.clone();
            asked["MaxResults"] = json!(size);
            let first = caller.ok(operation, asked).await;
            assert_eq!(
                items(&case, &first, member),
                whole[..taken].to_vec(),
                "case {case}: the first page must be the first {taken} records of this selection"
            );
            let token = first["NextToken"].as_str().map(str::to_string);
            assert_eq!(
                token.is_some(),
                taken < whole.len(),
                "case {case}: a NextToken follows this page exactly while records remain ({taken} \
                 of {} served), got {first}",
                whole.len()
            );
            let Some(token) = token else {
                // This page IS the whole selection, so there is nothing to resume.
                continue;
            };

            for continuation in [Some(1_i64), Some(size + 4), None] {
                let case = format!("{case} resumed at MaxResults={continuation:?}");
                let mut resume = body.clone();
                resume["NextToken"] = json!(&token);
                if let Some(size) = continuation {
                    resume["MaxResults"] = json!(size);
                }
                let next = caller.ok(operation, resume).await;
                let served = items(&case, &next, member);
                let expected = match continuation {
                    Some(size) => (taken + size as usize).min(whole.len()),
                    // A continuation that names no page size is bounded by the
                    // LISTING'S own unasked page, the same way its first page is.
                    None => taken.saturating_add(unasked_page(operation)).min(whole.len()),
                };
                assert_eq!(
                    served,
                    whole[taken..expected].to_vec(),
                    "case {case}: a continuation resumes at offset {taken} of its own selection, \
                     expected {} records, got {}",
                    expected - taken,
                    served.len()
                );
            }
            resumed_rows += 1;
        }
    }

    assert!(
        resumed_rows >= 9,
        "case table: expected several selections long enough to resume, got {resumed_rows}"
    );
}
