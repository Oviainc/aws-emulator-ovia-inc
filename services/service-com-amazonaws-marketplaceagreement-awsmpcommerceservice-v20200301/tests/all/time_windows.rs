// SPDX-License-Identifier: CC0-1.0

//! A request that names BOTH ends of one instant's window, end to end over HTTP.
//!
//! The rule under test: where a listing bounds one stored instant with a pair of
//! request members — an exclusive lower bound ("after the specified timestamp")
//! and an exclusive upper one ("before the specified timestamp") — a lower bound
//! at or after the upper one spans no instant at all, so the pair states a
//! period rather than selecting on one and is refused with HTTP 400
//! `ValidationException` carrying the reason `ValidationExceptionReason`
//! publishes for a period, naming both members that bound it. A window that
//! spans any instant stays a selector: it reports whatever falls inside it, and
//! an empty page is a legitimate answer.
//!
//! Every operation of this crate whose input carries such a pair is a row
//! (`ListBillingAdjustmentRequests`' `createdAfter`/`createdBefore` and
//! `ListAgreementInvoiceLineItems`' `afterIssuedTime`/`beforeIssuedTime`,
//! `smithy_query(query="operation", operation="…")`), so the rule is pinned over
//! the class rather than on the one operation it was reported against. The rows
//! differ in what it generalises over: how far the wrong way round the bounds
//! are (a year, a second, a millisecond, and the degenerate pair at the same
//! instant), where the window sits relative to the stored records (wholly
//! before, straddling, wholly after), and what the store holds — a refusal
//! checked only while scanning would answer an empty page instead.
//!
//! The harness (`emulator`, `call`, `invoiced_agreement`) is the one
//! `agreement_lifecycle` owns, and the batch helpers are the ones
//! `billing_adjustments` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{call, emulator, invoiced_agreement, Answer};
use crate::billing_adjustments::{entry, get, submit};

/// The reason a period no instant can fall in is reported under — the one value
/// `ValidationExceptionReason` publishes about a period
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`), spelled
/// out here rather than imported so a handler reporting another one fails.
const PERIOD_REASON: &str = "INVALID_INVOICE_ADJUSTMENT_PERIOD";

/// The sentence that reason states, as this service words every refusal of a
/// member it names.
const PERIOD_MESSAGE: &str = "Provided invoice adjustment period is invalid.";

/// `LineItemGroupBy`'s one value, which `ListAgreementInvoiceLineItems` requires
/// of every call (`smithy_query(query="shape", shape="LineItemGroupBy")`).
const GROUP_BY_INVOICE_ID: &str = "INVOICE_ID";

/// Seconds in a year, for the row whose bounds are a year the wrong way round.
const YEAR: f64 = 365.0 * 24.0 * 60.0 * 60.0;

/// One listing that bounds a stored instant with a pair of request members. Each
/// row is self-describing: the members it bounds with, the extra member every
/// call of it must carry, what it reports, and which stored instant it bounds.
struct Window {
    /// The wire name of the operation.
    operation: &'static str,
    /// The member naming the exclusive lower bound.
    after: &'static str,
    /// The member naming the exclusive upper bound.
    before: &'static str,
    /// The output member carrying what the listing reports.
    reports: &'static str,
    /// `(member, value)` this listing requires of every call beside the
    /// agreement it is scoped to, or `None` when it requires nothing more.
    required: Option<(&'static str, &'static str)>,
    /// The stored instant this listing bounds, read off the fixture.
    pivot: fn(&Held) -> f64,
}

const WINDOWS: &[Window] = &[
    Window {
        operation: "ListBillingAdjustmentRequests",
        after: "createdAfter",
        before: "createdBefore",
        reports: "items",
        required: None,
        pivot: |held| held.created_at,
    },
    Window {
        operation: "ListAgreementInvoiceLineItems",
        after: "afterIssuedTime",
        before: "beforeIssuedTime",
        reports: "agreementInvoiceLineItemGroupSummaries",
        required: Some(("groupBy", GROUP_BY_INVOICE_ID)),
        pivot: |held| held.issued_at,
    },
];

/// The call one row makes: the agreement it is scoped to, whatever else the
/// listing requires, and whichever of the two bounds the row names.
fn request(window: &Window, agreement: &str, after: Option<f64>, before: Option<f64>) -> Value {
    let mut body = json!({"agreementId": agreement});
    if let Some((member, value)) = window.required {
        body[member] = json!(value);
    }
    if let Some(after) = after {
        body[window.after] = json!(after);
    }
    if let Some(before) = before {
        body[window.before] = json!(before);
    }
    body
}

/// How many records a served listing reported.
fn reported(window: &Window, answer: &Answer) -> usize {
    answer.body[window.reports]
        .as_array()
        .map(Vec::len)
        .unwrap_or_else(|| {
            panic!(
                "a served {} answers a page in {}, got {:?}",
                window.operation, window.reports, answer.body
            )
        })
}

/// The `fields` entries of a refusal, as the members they name.
fn named_members(answer: &Answer) -> Vec<String> {
    answer.body["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|field| field["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A store holding one invoice and one billing adjustment request against it,
/// with the instant each of the two listings bounds.
struct Held {
    router: MultiServiceRouter,
    agreement: String,
    /// When the stored adjustment request was created, which
    /// `ListBillingAdjustmentRequests` bounds.
    created_at: f64,
    /// When the stored invoice was issued, which
    /// `ListAgreementInvoiceLineItems` bounds.
    issued_at: f64,
}

/// An agreement with one invoice and one adjustment request against it, so both
/// listings have exactly one record to report.
async fn held() -> Held {
    let router = emulator();
    let (agreement, invoice) = invoiced_agreement(&router, "ap-window").await;
    let created = submit(
        &router,
        vec![entry(&agreement, &invoice, "10", "tok-window")],
    )
    .await;
    let id = created.body["items"][0]["billingAdjustmentRequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("the batch must create one request, got {:?}", created.body))
        .to_string();
    let detail = get(&router, &agreement, &id).await;
    let created_at = detail.body["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("createdAt is a required timestamp, got {:?}", detail.body));
    let groups = call(
        &router,
        "ListAgreementInvoiceLineItems",
        json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID}),
    )
    .await;
    let issued_at = groups.body["agreementInvoiceLineItemGroupSummaries"][0]["issuedTime"]
        .as_f64()
        .unwrap_or_else(|| {
            panic!(
                "a group reports when its invoice was issued, got {:?}",
                groups.body
            )
        });
    Held {
        router,
        agreement,
        created_at,
        issued_at,
    }
}

/// A pair of bounds the wrong way round, as offsets from the stored instant the
/// listing bounds: how far apart they are, and where the window sits relative to
/// that instant.
const INVERTED: &[(&str, f64, f64)] = &[
    ("the bounds a year the wrong way round", YEAR, -YEAR),
    ("a second the wrong way round", 1.0, -1.0),
    ("a millisecond the wrong way round", 0.001, -0.001),
    ("both bounds at the same instant", 0.0, 0.0),
    ("a window wholly before the record, inverted", -10.0, -20.0),
    ("a window wholly after the record, inverted", 20.0, 10.0),
];

/// A pair of bounds the wrong way round is refused as a malformed period,
/// whatever the store holds and wherever the window sits.
#[tokio::test]
async fn an_inverted_window_is_refused_as_a_malformed_period() {
    let held = held().await;
    let empty = emulator();
    // The second store holds nothing and the call names an agreement nothing
    // holds, so a period checked only while scanning — or only after the
    // agreement was resolved — would answer something else here.
    let stores: Vec<(&str, &MultiServiceRouter, &str)> = vec![
        (
            "a store holding an invoice and an adjustment",
            &held.router,
            held.agreement.as_str(),
        ),
        ("an empty store", &empty, "agmt-does-not-exist"),
    ];

    for window in WINDOWS {
        let pivot = (window.pivot)(&held);
        for (store, router, agreement) in &stores {
            for (margin, after, before) in INVERTED {
                let case = format!("{} {margin} ({store})", window.operation);
                let answer = call(
                    router,
                    window.operation,
                    request(
                        window,
                        agreement,
                        Some(pivot + after),
                        Some(pivot + before),
                    ),
                )
                .await;
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.reason(),
                    PERIOD_REASON,
                    "case {case:?}: expected reason {PERIOD_REASON}, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.str_at("/message"),
                    PERIOD_MESSAGE,
                    "case {case:?}: expected {PERIOD_MESSAGE:?}, got {:?}",
                    answer.body
                );
                assert_eq!(
                    named_members(&answer),
                    vec![window.after.to_string(), window.before.to_string()],
                    "case {case:?}: the refusal names both members that bound the period, \
                     got {:?}",
                    answer.body
                );
                assert!(
                    answer.body.get(window.reports).is_none(),
                    "case {case:?}: a refused listing reports nothing, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// A window that spans any instant at all stays a selector: it is served, and it
/// reports exactly the records that fall inside it.
///
/// The rows differ along how wide the window is, which of the two bounds the
/// call names, and whether the stored record falls inside it — so the refusal
/// above is shown to be of the pair that spans nothing, not of the members.
#[tokio::test]
async fn a_window_that_spans_an_instant_is_served() {
    let held = held().await;
    // (case, lower bound, upper bound, how many records fall inside)
    let cases: &[(&str, Option<f64>, Option<f64>, usize)] = &[
        ("neither bound", None, None, 1),
        ("only the lower bound, below the record", Some(-1.0), None, 1),
        ("only the upper bound, above it", None, Some(1.0), 1),
        (
            "a window of two seconds around the record",
            Some(-1.0),
            Some(1.0),
            1,
        ),
        (
            "a window of two days around it",
            Some(-86_400.0),
            Some(86_400.0),
            1,
        ),
        (
            "a window of two years around it",
            Some(-YEAR),
            Some(YEAR),
            1,
        ),
        (
            "a window wholly before the record selects nothing",
            Some(-20.0),
            Some(-10.0),
            0,
        ),
        (
            "a window wholly after it selects nothing",
            Some(10.0),
            Some(20.0),
            0,
        ),
    ];

    for window in WINDOWS {
        let pivot = (window.pivot)(&held);
        for (case, after, before, expected) in cases {
            let case = format!("{} {case}", window.operation);
            let answer = call(
                &held.router,
                window.operation,
                request(
                    window,
                    &held.agreement,
                    after.map(|offset| pivot + offset),
                    before.map(|offset| pivot + offset),
                ),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a window that spans an instant must be served, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                reported(window, &answer),
                *expected,
                "case {case:?}: expected {expected} records, got {:?}",
                answer.body
            );
        }
    }
}
