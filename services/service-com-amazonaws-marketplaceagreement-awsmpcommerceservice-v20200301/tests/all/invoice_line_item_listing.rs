// SPDX-License-Identifier: CC0-1.0

//! The input contract of `ListAgreementInvoiceLineItems`, and the fact that it
//! only reports.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the validation seam, the handler, the persisted state and
//! serialization — the same path a client takes. The wire helpers come from the
//! suite beside this one (`agreement_lifecycle`), so there is one harness for
//! the crate.
//!
//! The rules under test, over the CLASS of such requests rather than one
//! example:
//!
//! * Every bound the model publishes for this operation's input is SERVED, and a
//!   value outside one is refused 400 `ValidationException` whose `fields` names
//!   the member at fault and whose `reason` is the value naming that member
//!   where `ValidationExceptionReason` publishes one and its own catch-all
//!   `OTHER` where it does not — a well-formed `agreementId` naming no agreement
//!   reaching the handler and being answered `ResourceNotFoundException`, never
//!   an empty page.
//! * The listing REPORTS: every group it answers is read off the stored invoice,
//!   so reading, paging and filtering leave what a later caller sees untouched,
//!   while a write against the invoice moves what the next read reports.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{call, emulator, invoiced_agreement, invoices_of, Answer};
use crate::billing_adjustments::{entry, submit};

/// `LineItemGroupBy`'s only value, from
/// smithy_query(query="shape", shape="LineItemGroupBy").
const GROUP_BY_INVOICE_ID: &str = "INVOICE_ID";

/// `ValidationException`'s own documentation, which is the sentence the envelope
/// states when the reason names no member
/// (smithy_query(query="shape", shape="ValidationException")).
const UNPLACED_REFUSAL_MESSAGE: &str =
    "The input fails to satisfy the constraints specified by the service.";

/// `ValidationExceptionReason`'s catch-all, for a member it publishes no value
/// for (smithy_query(query="shape", shape="ValidationExceptionReason") lists no
/// GROUP_BY, INVOICE_TYPE, MONTH or YEAR value, and no INVALID_INVOICE_ID).
const REASON_OTHER: &str = "OTHER";

async fn list(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "ListAgreementInvoiceLineItems", request).await
}

/// The groups one listing answered.
fn reported(answer: &Answer) -> Vec<Value> {
    answer.body["agreementInvoiceLineItemGroupSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every group the invoices of `agreement` are reported in, unfiltered.
async fn groups_of(router: &MultiServiceRouter, agreement: &str) -> Vec<Value> {
    let answer = list(
        router,
        json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "listing the invoices of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    reported(&answer)
}

/// One monetary member of a group's `pricingCurrencyAmount`.
fn money(group: &Value, member: &str) -> String {
    group["pricingCurrencyAmount"][member]
        .as_str()
        .unwrap_or_else(|| panic!("pricingCurrencyAmount.{member} must be reported: {group:?}"))
        .to_string()
}

/// The dotted request paths the `fields` of a refusal name, in the order named.
fn field_names(answer: &Answer) -> Vec<String> {
    answer.body["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|field| field["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The sentence a refusal carrying `reason` states: the reason names the member,
/// so the sentence is built from it, and a reason that names none states
/// `ValidationException`'s own documentation.
fn message_of(reason: &str) -> String {
    if let Some(stem) = reason.strip_prefix("INVALID_") {
        format!("Provided {} is invalid.", stem.to_lowercase().replace('_', " "))
    } else if let Some(stem) = reason.strip_prefix("MISSING_") {
        // The absent-member class states the requirement the member is under:
        // `MISSING_AGREEMENT_ID` is "Agreement id is required."
        let mut member = stem.to_lowercase().replace('_', " ");
        if let Some(first) = member.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        format!("{member} is required.")
    } else {
        UNPLACED_REFUSAL_MESSAGE.to_string()
    }
}

/// One request outside a bound the model publishes for this operation's input.
struct Refused {
    case: &'static str,
    /// The request, built around an `agreementId` that names a real agreement
    /// unless the row is about `agreementId` itself, so only the member the row
    /// names can be at fault.
    body: Value,
    /// The dotted request path of the member at fault.
    field: &'static str,
    /// The `ValidationExceptionReason` the refusal carries.
    reason: &'static str,
}

/// Every constraint the model declares on this operation's input is enforced
/// before anything is reported, and each published bound is served.
///
/// The constraints are the model's
/// (smithy_query(query="operation", operation="ListAgreementInvoiceLineItems")):
/// `agreementId` and `groupBy` are `@required`, `agreementId` and `invoiceId`
/// are `ResourceId` (`@length(1..64)`, `@pattern(^[A-Za-z0-9_/-]+$)`), `groupBy`
/// is the `LineItemGroupBy` enum, `invoiceType` the `InvoiceType` enum,
/// `maxResults` is `@range(1..50)`, `nextToken` is `@length(max: 8192)` with
/// `@pattern(^[a-zA-Z0-9+/=_-]+$)`, and `InvoiceBillingPeriod` requires a
/// `month` in 1..12 and a `year` in 2023..2050.
#[tokio::test]
async fn every_published_input_bound_is_served_and_a_value_outside_one_refused() {
    let router = emulator();
    let (agreement, invoice) = invoiced_agreement(&router, "ap-lineitembounds").await;
    let longest_id = "i".repeat(64);
    let over_long_id = "i".repeat(65);
    let over_long_token = "t".repeat(8193);

    let refusals = vec![
        Refused {
            case: "the required grouping left out",
            body: json!({"agreementId": agreement}),
            field: "groupBy",
            reason: REASON_OTHER,
        },
        Refused {
            case: "a grouping outside the one value the enum admits",
            body: json!({"agreementId": agreement, "groupBy": "AGREEMENT_ID"}),
            field: "groupBy",
            reason: REASON_OTHER,
        },
        Refused {
            case: "the modelled grouping in the right words but the wrong case",
            body: json!({"agreementId": agreement, "groupBy": "invoice_id"}),
            field: "groupBy",
            reason: REASON_OTHER,
        },
        Refused {
            case: "the required agreement left out",
            body: json!({"groupBy": GROUP_BY_INVOICE_ID}),
            field: "agreementId",
            reason: "MISSING_AGREEMENT_ID",
        },
        Refused {
            case: "an agreement id under its minimum length",
            body: json!({"agreementId": "", "groupBy": GROUP_BY_INVOICE_ID}),
            field: "agreementId",
            reason: "INVALID_AGREEMENT_ID",
        },
        Refused {
            case: "an agreement id over its maximum length",
            body: json!({"agreementId": over_long_id, "groupBy": GROUP_BY_INVOICE_ID}),
            field: "agreementId",
            reason: "INVALID_AGREEMENT_ID",
        },
        Refused {
            case: "an agreement id outside the resource-id pattern",
            body: json!({"agreementId": "not an id!", "groupBy": GROUP_BY_INVOICE_ID}),
            field: "agreementId",
            reason: "INVALID_AGREEMENT_ID",
        },
        Refused {
            case: "an invoice filter under its minimum length",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceId": "",
            }),
            field: "invoiceId",
            reason: REASON_OTHER,
        },
        Refused {
            case: "an invoice filter over its maximum length",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceId": over_long_id,
            }),
            field: "invoiceId",
            reason: REASON_OTHER,
        },
        Refused {
            case: "an invoice filter outside the resource-id pattern",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceId": "inv oice",
            }),
            field: "invoiceId",
            reason: REASON_OTHER,
        },
        Refused {
            case: "an invoice type outside the enum",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceType": "Invoice",
            }),
            field: "invoiceType",
            reason: REASON_OTHER,
        },
        Refused {
            case: "a page size under its minimum",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "maxResults": 0,
            }),
            field: "maxResults",
            reason: "INVALID_MAX_RESULTS",
        },
        Refused {
            case: "a page size over its maximum",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "maxResults": 51,
            }),
            field: "maxResults",
            reason: "INVALID_MAX_RESULTS",
        },
        Refused {
            case: "a page token outside the token pattern",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "nextToken": "not a token",
            }),
            field: "nextToken",
            reason: "INVALID_NEXT_TOKEN",
        },
        Refused {
            case: "a page token over its maximum length",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "nextToken": over_long_token,
            }),
            field: "nextToken",
            reason: "INVALID_NEXT_TOKEN",
        },
        Refused {
            case: "a billing period month under its minimum",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceBillingPeriod": {"month": 0, "year": 2024},
            }),
            field: "invoiceBillingPeriod.month",
            reason: REASON_OTHER,
        },
        Refused {
            case: "a billing period month over its maximum",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceBillingPeriod": {"month": 13, "year": 2024},
            }),
            field: "invoiceBillingPeriod.month",
            reason: REASON_OTHER,
        },
        Refused {
            case: "a billing period year under its minimum",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceBillingPeriod": {"month": 6, "year": 2022},
            }),
            field: "invoiceBillingPeriod.year",
            reason: REASON_OTHER,
        },
        Refused {
            case: "a billing period year over its maximum",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceBillingPeriod": {"month": 6, "year": 2051},
            }),
            field: "invoiceBillingPeriod.year",
            reason: REASON_OTHER,
        },
        Refused {
            case: "a billing period missing the month it requires",
            body: json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceBillingPeriod": {"year": 2024},
            }),
            field: "invoiceBillingPeriod.month",
            reason: REASON_OTHER,
        },
    ];

    for row in refusals {
        let answer = list(&router, row.body.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {:?}: expected a ValidationException, got {:?}",
            row.case,
            answer.body
        );
        assert_eq!(
            answer.reason(),
            row.reason,
            "case {:?}: expected reason {:?}, got {:?}",
            row.case,
            row.reason,
            answer.body
        );
        assert_eq!(
            answer.str_at("/message"),
            message_of(row.reason),
            "case {:?}: expected the sentence that reason states, got {:?}",
            row.case,
            answer.body
        );
        assert_eq!(
            field_names(&answer),
            vec![row.field.to_string()],
            "case {:?}: expected the fields to name {:?}, got {:?}",
            row.case,
            row.field,
            answer.body
        );
        assert!(
            answer.body["agreementInvoiceLineItemGroupSummaries"].is_null(),
            "case {:?}: a refused request reports nothing, got {:?}",
            row.case,
            answer.body
        );
    }

    // Each published bound, and a value well inside it, is SERVED: the listing
    // answers its report, and every group it reports is a group of an invoice of
    // the agreement named.
    let invoices = invoices_of(&router, &agreement).await;
    let served: Vec<(&str, Value)> = vec![
        (
            "the smallest page size the range publishes",
            json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID, "maxResults": 1}),
        ),
        (
            "a page size inside the range",
            json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID, "maxResults": 25}),
        ),
        (
            "the largest page size the range publishes",
            json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID, "maxResults": 50}),
        ),
        (
            "an invoice filter at the shortest id the pattern admits",
            json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID, "invoiceId": "i"}),
        ),
        (
            "an invoice filter at the longest id the length admits",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceId": longest_id,
            }),
        ),
        (
            "an invoice filter naming an invoice of this agreement",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceId": invoice,
            }),
        ),
        (
            "the invoice type an issued invoice carries",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceType": "INVOICE",
            }),
        ),
        (
            "the other invoice type the enum admits",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceType": "CREDIT_MEMO",
            }),
        ),
        (
            "a billing period at the lowest month and year the ranges publish",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceBillingPeriod": {"month": 1, "year": 2023},
            }),
        ),
        (
            "a billing period at the highest month and year the ranges publish",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceBillingPeriod": {"month": 12, "year": 2050},
            }),
        ),
    ];
    for (case, request) in served {
        let answer = list(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a published bound must be served, got {} with {:?}",
            answer.status, answer.body
        );
        for group in reported(&answer) {
            let id = group["invoiceId"].as_str().unwrap_or_default().to_string();
            assert!(
                invoices.contains(&id),
                "case {case:?}: every group reported must be an invoice of the agreement \
                 ({invoices:?}), got {group:?}"
            );
        }
    }

    // A well-formed id is not refused as a member: it reaches the handler, which
    // answers a reference to an agreement this store does not hold with the
    // not-found this operation models — at both ends of the id length.
    for absent in ["x", &longest_id] {
        let answer = list(
            &router,
            json!({"agreementId": absent, "groupBy": GROUP_BY_INVOICE_ID}),
        )
        .await;
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case \"a well-formed agreement id naming no agreement\" ({absent}): expected a \
             ResourceNotFoundException, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

/// Reading the invoice line items changes nothing a later caller sees, and what
/// is reported follows the stored invoice rather than being fixed at issue time.
#[tokio::test]
async fn listing_the_invoice_line_items_reports_without_changing_anything() {
    let router = emulator();
    let (agreement, invoice) = invoiced_agreement(&router, "ap-lineitemreads").await;

    let before = groups_of(&router, &agreement).await;
    assert_eq!(
        before.len(),
        1,
        "case \"before any read\": the one charge billed on acceptance is one invoice, got \
         {before:?}"
    );

    // Every shape of read this operation serves, including the paging and the
    // refusals: none of them may move what the next caller is reported.
    let reads: Vec<(&str, Value)> = vec![
        (
            "the unfiltered listing, read again",
            json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID}),
        ),
        (
            "the listing filtered to the one invoice",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceId": invoice,
            }),
        ),
        (
            "the listing filtered to the other invoice type, which selects nothing",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceType": "CREDIT_MEMO",
            }),
        ),
        (
            "the listing paged one group at a time",
            json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID, "maxResults": 1}),
        ),
        (
            "a listing whose page token this service never minted",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "nextToken": "madeup",
            }),
        ),
        (
            "a listing of an agreement this store does not hold",
            json!({"agreementId": "agmt-absent", "groupBy": GROUP_BY_INVOICE_ID}),
        ),
    ];
    for (case, request) in reads {
        let answer = list(&router, request).await;
        assert!(
            answer.status == 200 || answer.status == 400,
            "case {case:?}: a read is either reported or refused, got {} with {:?}",
            answer.status,
            answer.body
        );
        let after = groups_of(&router, &agreement).await;
        assert_eq!(
            after, before,
            "case {case:?}: reporting must leave what a later caller sees unchanged, got {after:?}"
        );
    }

    // What the group reports is read off the invoice record: a write against the
    // invoice moves the amount still open to adjustment, and leaves what the
    // invoice billed where it was.
    let billed = money(&before[0], "amount");
    let open = money(&before[0], "maxAdjustmentAmount")
        .parse::<f64>()
        .expect("the open amount is reported as a decimal number");
    let adjustment = 1.0_f64;
    let accepted = submit(
        &router,
        vec![entry(
            &agreement,
            &invoice,
            &format!("{adjustment:.2}"),
            "tok-lineitem-read",
        )],
    )
    .await;
    assert_eq!(
        accepted.body["errors"].as_array().map(Vec::len),
        Some(0),
        "case \"an adjustment inside the open amount\": expected it to be accepted, got {:?}",
        accepted.body
    );

    let after = groups_of(&router, &agreement).await;
    assert_eq!(
        money(&after[0], "amount"),
        billed,
        "case \"after one adjustment\": what the invoice billed does not move, got {after:?}"
    );
    let reported_open = money(&after[0], "maxAdjustmentAmount")
        .parse::<f64>()
        .expect("the open amount is reported as a decimal number");
    assert!(
        (reported_open - (open - adjustment)).abs() < 0.005,
        "case \"after one adjustment\": expected {:.2} open, got {reported_open}",
        open - adjustment
    );
}
