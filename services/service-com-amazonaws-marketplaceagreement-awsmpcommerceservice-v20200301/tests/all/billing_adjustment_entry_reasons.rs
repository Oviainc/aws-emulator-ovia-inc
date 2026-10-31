// SPDX-License-Identifier: CC0-1.0

//! What a refused `BatchCreateBillingAdjustmentRequest` tells a client to branch
//! on, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes. The wire helpers come from the suites beside this
//! one (`agreement_lifecycle`, `billing_adjustments`), so there is one harness
//! for the crate.
//!
//! The rule under test: every validation refusal of this operation names the
//! modelled `ValidationExceptionReason` for the rule that was broken. A refusal
//! of the whole call names it in the `reason` member of its
//! `ValidationException`; a refusal of ONE entry has no such member
//! (`smithy_query(query="shape", shape="BatchCreateBillingAdjustmentError")`
//! carries only `code`, `message` and `clientToken`), so its message opens with
//! that reason and then states what about the entry broke it — the same message
//! shape for every entry refused under `VALIDATION_EXCEPTION`, whichever rule it
//! broke, including the bound on how many requests the store holds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{call, emulator, invoiced_agreement, Answer};
use crate::billing_adjustments::{entry, submit};

/// The currency the invoices of this service are denominated in, spelled out
/// here rather than imported so a handler reporting another one fails this test.
const CURRENCY: &str = "USD";

/// The values `ValidationExceptionReason` publishes that a refusal of this
/// operation can name, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`, spelled out
/// here rather than imported so a handler naming a value the model does not
/// publish fails this test.
const MODELLED_REASONS: &[&str] = &[
    "MISSING_BILLING_ADJUSTMENTS",
    "BILLING_ADJUSTMENTS_LIMIT_EXCEEDED",
    "MULTIPLE_AGREEMENT_IDS",
    "INVALID_CLIENT_TOKEN",
    "INVALID_ADJUSTMENT_AMOUNT",
    "INVALID_CURRENCY_CODE",
    "EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT",
];

/// The largest batch one call accepts, which is the emulator's own documented
/// bound (`state::BILLING_ADJUSTMENT_BATCH_LIMIT`), spelled out so a change to
/// it has to be made deliberately.
const BATCH_LIMIT: usize = 50;

/// The largest number of billing adjustment requests the store holds, which is
/// the emulator's own documented bound
/// (`state::BILLING_ADJUSTMENT_REQUEST_QUOTA`), spelled out for the same reason.
const STORE_QUOTA: usize = 1_000;

/// The `errors` of a batch answer.
fn batch_errors(answer: &Answer) -> Vec<Value> {
    answer.body["errors"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("errors is a required member, got {:?}", answer.body))
}

/// The `items` of a batch answer.
fn batch_items(answer: &Answer) -> Vec<Value> {
    answer.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("items is a required member, got {:?}", answer.body))
}

/// One entry, with the currency and amount the row names.
fn adjusting(agreement: &str, invoice: &str, amount: &str, currency: &str, token: &str) -> Value {
    let mut submitted = entry(agreement, invoice, amount, token);
    submitted["currencyCode"] = json!(currency);
    submitted
}

/// The amount `invoice` billed, as `ListAgreementInvoiceLineItems` reports it —
/// which is also the whole of what that invoice has open to adjustment before
/// any adjustment is requested against it.
async fn billed(router: &MultiServiceRouter, agreement: &str) -> String {
    let answer = call(
        router,
        "ListAgreementInvoiceLineItems",
        json!({"agreementId": agreement, "groupBy": "INVOICE_ID"}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "listing the invoices of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementInvoiceLineItemGroupSummaries"][0]["pricingCurrencyAmount"]["amount"]
        .as_str()
        .unwrap_or_else(|| panic!("the invoice must report what it billed, got {:?}", answer.body))
        .to_string()
}

/// The one message shape a per-entry `VALIDATION_EXCEPTION` carries: the
/// modelled reason, then what about the entry broke the rule.
fn assert_entry_reason(case: &str, reason: &str, refusal: &Value) {
    assert_eq!(
        refusal["code"].as_str(),
        Some("VALIDATION_EXCEPTION"),
        "case {case:?}: expected a VALIDATION_EXCEPTION entry, got {refusal:?}"
    );
    let message = refusal["message"].as_str().unwrap_or_default();
    let Some((named, detail)) = message.split_once(": ") else {
        panic!(
            "case {case:?}: expected the message to open with a ValidationExceptionReason and \
             then state what broke, got {message:?}"
        );
    };
    assert_eq!(
        named, reason,
        "case {case:?}: expected the message to name {reason}, got {message:?}"
    );
    assert!(
        MODELLED_REASONS.contains(&named),
        "case {case:?}: {named:?} is not a value ValidationExceptionReason publishes, got \
         {message:?}"
    );
    assert!(
        !detail.trim().is_empty(),
        "case {case:?}: expected the reason to be followed by what broke the rule, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// One entry refused
// ---------------------------------------------------------------------------

/// How a row names the amount to adjust.
#[derive(Debug, Clone, Copy)]
enum Amount {
    /// A literal amount.
    Literal(&'static str),
    /// A tenth of a cent past what the invoice billed.
    PastTheInvoice,
}

/// One row: the entry's amount and currency, and the modelled reason its refusal
/// names.
struct Row {
    case: &'static str,
    amount: Amount,
    currency: &'static str,
    reason: &'static str,
}

/// Every entry refused under `VALIDATION_EXCEPTION` names its modelled reason in
/// its message, whichever rule it broke.
///
/// The rows differ along what the rule generalises over: which rule the entry
/// breaks (an amount that is not an amount at all, an amount too long or too
/// large for this service to carry, an amount past what the invoice has open, a
/// currency the invoice is not denominated in) and so which
/// `ValidationExceptionReason` the refusal names. Each row runs against its own
/// emulator, so no row is held to headroom another row consumed.
#[tokio::test]
async fn every_refused_entry_names_the_modelled_reason_it_broke() {
    let rows = [
        Row {
            case: "an amount of zero",
            amount: Amount::Literal("0"),
            currency: CURRENCY,
            reason: "INVALID_ADJUSTMENT_AMOUNT",
        },
        Row {
            case: "an amount of nothing but zeros",
            amount: Amount::Literal("0.00"),
            currency: CURRENCY,
            reason: "INVALID_ADJUSTMENT_AMOUNT",
        },
        Row {
            case: "an amount longer than this service accepts an amount being",
            amount: Amount::Literal("1111111111111111111111111111111111111111"),
            currency: CURRENCY,
            reason: "INVALID_ADJUSTMENT_AMOUNT",
        },
        Row {
            case: "an amount too large to count in the units this API carries",
            amount: Amount::Literal("99999999999999999999999999999999"),
            currency: CURRENCY,
            reason: "INVALID_ADJUSTMENT_AMOUNT",
        },
        Row {
            case: "a tenth of a cent past what the invoice billed",
            amount: Amount::PastTheInvoice,
            currency: CURRENCY,
            reason: "EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT",
        },
        Row {
            case: "orders of magnitude more than the invoice billed",
            amount: Amount::Literal("99999999"),
            currency: CURRENCY,
            reason: "EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT",
        },
        Row {
            case: "a currency the invoice is not denominated in",
            amount: Amount::Literal("1"),
            currency: "EUR",
            reason: "INVALID_CURRENCY_CODE",
        },
        Row {
            case: "another currency code the member admits",
            amount: Amount::Literal("1"),
            currency: "JPY",
            reason: "INVALID_CURRENCY_CODE",
        },
    ];

    for row in &rows {
        let router = emulator();
        let (agreement, invoice) = invoiced_agreement(&router, "ap-entryreason").await;
        let amount = match row.amount {
            Amount::Literal(amount) => amount.to_string(),
            // One more digit on the end of the billed amount is strictly more
            // than the billed amount, and still inside
            // `PositiveAmountUpto8Decimals`'s eight fraction digits.
            Amount::PastTheInvoice => format!("{}1", billed(&router, &agreement).await),
        };
        let answer = submit(
            &router,
            vec![adjusting(
                &agreement,
                &invoice,
                &amount,
                row.currency,
                "tok-entryreason",
            )],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: a per-entry outcome is reported in the body, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        assert!(
            batch_items(&answer).is_empty(),
            "case {:?}: a refused entry creates nothing, got {:?}",
            row.case,
            answer.body
        );
        let refusals = batch_errors(&answer);
        assert_eq!(
            refusals.len(),
            1,
            "case {:?}: expected one per-entry error, got {:?}",
            row.case,
            answer.body
        );
        assert_eq!(
            refusals[0]["clientToken"].as_str(),
            Some("tok-entryreason"),
            "case {:?}: the error names the entry it is about, got {:?}",
            row.case,
            answer.body
        );
        assert_entry_reason(row.case, row.reason, &refusals[0]);
    }
}

// ---------------------------------------------------------------------------
// The bound on the store, reported the same way
// ---------------------------------------------------------------------------

/// An entry refused because the store already holds the most requests this
/// emulator keeps names its modelled reason the same way its siblings do.
///
/// The two cases differ along where the bound falls relative to the call: a batch
/// that crosses it partway through (the entry that still fits is stored, the rest
/// are refused) and a call made when the store is already full (every entry
/// refused, nothing created).
#[tokio::test]
async fn the_store_bound_refusal_names_the_modelled_reason_like_its_siblings() {
    const REASON: &str = "BILLING_ADJUSTMENTS_LIMIT_EXCEEDED";

    let router = emulator();
    let (agreement, invoice) = invoiced_agreement(&router, "ap-storebound").await;

    // Fill the store to one slot short of the bound, in batches of the largest
    // size one call accepts. Each entry adjusts one unit of the invoice, which
    // bills far more than the bound's worth, so no entry of the fill is held
    // back by what the invoice has open.
    let mut stored = 0usize;
    while stored < STORE_QUOTA - 1 {
        let count = BATCH_LIMIT.min(STORE_QUOTA - 1 - stored);
        let entries: Vec<Value> = (0..count)
            .map(|index| entry(&agreement, &invoice, "1", &format!("tok-fill-{stored}-{index}")))
            .collect();
        let answer = submit(&router, entries).await;
        assert_eq!(
            answer.status, 200,
            "case \"filling the store at {stored}\": expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            batch_errors(&answer).len(),
            0,
            "case \"filling the store at {stored}\": every entry still fits, got {:?}",
            answer.body
        );
        stored += count;
    }

    // A batch that crosses the bound: one slot is free, so one entry is stored
    // and the other two are refused, each under its own clientToken.
    let crossing: Vec<Value> = (0..3)
        .map(|index| entry(&agreement, &invoice, "1", &format!("tok-cross-{index}")))
        .collect();
    let crossed = submit(&router, crossing).await;
    assert_eq!(
        crossed.status, 200,
        "case \"a batch crossing the bound\": expected 200, got {} with {:?}",
        crossed.status, crossed.body
    );
    assert_eq!(
        batch_items(&crossed).len(),
        1,
        "case \"a batch crossing the bound\": only the entry that still fits is stored, got {:?}",
        crossed.body
    );
    let refusals = batch_errors(&crossed);
    assert_eq!(
        refusals.len(),
        2,
        "case \"a batch crossing the bound\": expected one error per entry that does not fit, \
         got {:?}",
        crossed.body
    );
    for refusal in &refusals {
        assert_entry_reason("a batch crossing the bound", REASON, refusal);
    }

    // The store is now full, so no entry of a further call is accepted.
    let refused = submit(
        &router,
        vec![
            entry(&agreement, &invoice, "1", "tok-full-1"),
            entry(&agreement, &invoice, "2", "tok-full-2"),
        ],
    )
    .await;
    assert_eq!(
        refused.status, 200,
        "case \"a call against a full store\": the batch is answered per entry, got {} with {:?}",
        refused.status, refused.body
    );
    assert!(
        batch_items(&refused).is_empty(),
        "case \"a call against a full store\": nothing may be created, got {:?}",
        refused.body
    );
    let full = batch_errors(&refused);
    assert_eq!(
        full.len(),
        2,
        "case \"a call against a full store\": expected one error per entry, got {:?}",
        refused.body
    );
    for refusal in &full {
        assert_entry_reason("a call against a full store", REASON, refusal);
    }
    // One channel, one message shape: the entry refused for the bound reads like
    // the entry refused for a bad amount, differing only in what follows the
    // reason.
    let other = submit(
        &router,
        vec![adjusting(&agreement, &invoice, "0", CURRENCY, "tok-full-3")],
    )
    .await;
    assert_entry_reason(
        "a bad amount against a full store",
        "INVALID_ADJUSTMENT_AMOUNT",
        &batch_errors(&other)[0],
    );
}

// ---------------------------------------------------------------------------
// The whole call refused
// ---------------------------------------------------------------------------

/// A rule about the batch ITSELF refuses the whole call as the modelled
/// `ValidationException`, whose `reason` member names the same modelled value a
/// per-entry refusal names in its message.
///
/// The rows differ along which rule about the batch is broken: no entries at all,
/// more entries than one call carries, entries spanning more than one agreement,
/// and two entries sharing a `clientToken`.
#[tokio::test]
async fn every_refused_batch_carries_the_modelled_reason_it_broke() {
    let router = emulator();
    let (agreement, invoice) = invoiced_agreement(&router, "ap-batchreason").await;
    let (other_agreement, other_invoice) =
        invoiced_agreement(&router, "ap-batchreasonother").await;

    let past_the_limit: Vec<Value> = (0..=BATCH_LIMIT)
        .map(|index| entry(&agreement, &invoice, "1", &format!("tok-over-{index}")))
        .collect();
    let cases: Vec<(&str, Vec<Value>, &str)> = vec![
        ("no entries at all", Vec::new(), "MISSING_BILLING_ADJUSTMENTS"),
        (
            "one entry more than a call carries",
            past_the_limit,
            "BILLING_ADJUSTMENTS_LIMIT_EXCEEDED",
        ),
        (
            "entries spanning two agreements",
            vec![
                entry(&agreement, &invoice, "1", "tok-span-1"),
                entry(&other_agreement, &other_invoice, "1", "tok-span-2"),
            ],
            "MULTIPLE_AGREEMENT_IDS",
        ),
        (
            "two entries sharing a clientToken",
            vec![
                entry(&agreement, &invoice, "1", "tok-same"),
                entry(&agreement, &invoice, "2", "tok-same"),
            ],
            "INVALID_CLIENT_TOKEN",
        ),
    ];

    for (case, entries, reason) in cases {
        let answer = submit(&router, entries).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a batch-wide refusal is the modelled ValidationException, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            reason,
            "case {case:?}: expected reason {reason}, got {:?}",
            answer.body
        );
        assert!(
            MODELLED_REASONS.contains(&answer.reason()),
            "case {case:?}: {:?} is not a value ValidationExceptionReason publishes",
            answer.reason()
        );
    }
}
