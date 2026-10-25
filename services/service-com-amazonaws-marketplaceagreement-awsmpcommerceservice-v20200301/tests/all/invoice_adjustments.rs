// SPDX-License-Identifier: CC0-1.0

//! The invoice a billing adjustment is made against, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes. The wire helpers come from the suite beside this
//! one (`agreement_lifecycle`), so there is one harness for the crate.
//!
//! The rule under test: an invoice is what bills one charge of an agreement, so
//! one `BatchCreateBillingAdjustmentRequest` entry is accepted only when the
//! invoice it names is an invoice of the agreement it names, its `currencyCode`
//! is the currency that invoice is denominated in, and its `adjustmentAmount` is
//! within what that invoice still has open to adjustment — and an entry that
//! breaks any of those is reported on the per-entry `errors` channel and stores
//! nothing. `ListAgreementInvoiceLineItems` reports the same invoices, grouped
//! by invoice, with the amount each billed and the amount of it still open.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, call, create, emulator, invoiced_agreement, invoices_of, is_next_token, new_request,
    term, upfront_term, Answer,
};
use crate::billing_adjustments::{entry, submit};

/// The currency this service prices every agreement in, spelled out here rather
/// than imported so a handler reporting another one fails this test.
const CURRENCY: &str = "USD";

/// `InvoiceType`'s two values, from
/// smithy_query(query="shape", shape="InvoiceType").
const INVOICE: &str = "INVOICE";
/// See [`INVOICE`].
const CREDIT_MEMO: &str = "CREDIT_MEMO";

/// `LineItemGroupBy`'s only value, from
/// smithy_query(query="shape", shape="LineItemGroupBy").
const GROUP_BY_INVOICE_ID: &str = "INVOICE_ID";

async fn list_invoices(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "ListAgreementInvoiceLineItems", request).await
}

/// Every group the invoices of `agreement` are reported in, in the order they
/// are reported.
async fn groups_of(router: &MultiServiceRouter, agreement: &str) -> Vec<Value> {
    let answer = list_invoices(
        router,
        json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "listing the invoices of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementInvoiceLineItemGroupSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The one group an agreement with a single invoice is reported in.
async fn one_group(router: &MultiServiceRouter, agreement: &str) -> Value {
    let groups = groups_of(router, agreement).await;
    assert_eq!(
        groups.len(),
        1,
        "{agreement} must have exactly one invoice, got {groups:?}"
    );
    groups[0].clone()
}

/// One monetary member of a group's `pricingCurrencyAmount`.
fn money(group: &Value, member: &str) -> String {
    group["pricingCurrencyAmount"][member]
        .as_str()
        .unwrap_or_else(|| panic!("pricingCurrencyAmount.{member} must be reported: {group:?}"))
        .to_string()
}

/// The `code` of each per-entry error a batch answered, in the order answered.
fn error_codes(answer: &Answer) -> Vec<String> {
    answer.body["errors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|error| error["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `billingAdjustmentRequestId` of each item a batch answered.
fn item_ids(answer: &Answer) -> Vec<String> {
    answer.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["billingAdjustmentRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Submit one entry, with the currency the row names rather than the default.
async fn submit_one(
    router: &MultiServiceRouter,
    agreement: &str,
    invoice: &str,
    amount: &str,
    currency: &str,
    token: &str,
) -> Answer {
    let mut submitted = entry(agreement, invoice, amount, token);
    submitted["currencyCode"] = json!(currency);
    submit(router, vec![submitted]).await
}

// ---------------------------------------------------------------------------
// An entry is resolved against the invoice it adjusts
// ---------------------------------------------------------------------------

/// What the service does with one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The entry is accepted and a request is stored for it.
    Stored,
    /// The entry is reported on the per-entry `errors` channel under this
    /// `BillingAdjustmentErrorCode`, and — where the refusal is about a member
    /// the request got wrong — its message names this
    /// `ValidationExceptionReason`, which is the only place a per-entry error
    /// can carry one.
    Refused(&'static str, Option<&'static str>),
}

/// How a row names the invoice to adjust.
#[derive(Debug, Clone, Copy)]
enum Invoice {
    /// The invoice the agreement's own acceptance issued.
    Own,
    /// An invoice of ANOTHER agreement of the same account.
    Other,
    /// A literal id that names no invoice of this account.
    Literal(&'static str),
}

/// How a row names the amount to adjust.
#[derive(Debug, Clone, Copy)]
enum Amount {
    /// A literal amount.
    Literal(&'static str),
    /// Exactly what the invoice billed, which is the ceiling the invoice
    /// publishes as `maxAdjustmentAmount`.
    WholeInvoice,
    /// A tenth of a cent past what the invoice billed.
    PastTheInvoice,
}

/// One row: the entry's invoice, amount and currency, and what the service does
/// with it.
struct Row {
    case: &'static str,
    invoice: Invoice,
    amount: Amount,
    currency: &'static str,
    expected: Outcome,
}

/// An entry is accepted only when the invoice, currency and amount it names hold
/// up against the invoice being adjusted.
///
/// The rows differ along what the rule generalises over: which invoice the entry
/// names (its agreement's own, another agreement's, one that exists nowhere, one
/// shaped like an id this service mints), where its amount falls relative to what
/// the invoice billed (well inside, the smallest the model admits, exactly the
/// published ceiling, a tenth of a cent past it, orders of magnitude past it,
/// longer than this service accepts an amount being), and which currency it is
/// in (the invoice's own, and two other codes `CurrencyCode` admits). Each row
/// runs against its own emulator, so the entries never share one invoice's
/// headroom.
#[tokio::test]
async fn an_entry_is_resolved_against_the_invoice_it_adjusts() {
    let rows = vec![
        Row {
            case: "part of what its own invoice billed",
            invoice: Invoice::Own,
            amount: Amount::Literal("10"),
            currency: CURRENCY,
            expected: Outcome::Stored,
        },
        Row {
            case: "the smallest amount the model admits",
            invoice: Invoice::Own,
            amount: Amount::Literal("0.00000001"),
            currency: CURRENCY,
            expected: Outcome::Stored,
        },
        Row {
            case: "the whole of what its own invoice billed",
            invoice: Invoice::Own,
            amount: Amount::WholeInvoice,
            currency: CURRENCY,
            expected: Outcome::Stored,
        },
        Row {
            case: "a tenth of a cent past what the invoice billed",
            invoice: Invoice::Own,
            amount: Amount::PastTheInvoice,
            currency: CURRENCY,
            expected: Outcome::Refused(
                "VALIDATION_EXCEPTION",
                Some("EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT"),
            ),
        },
        Row {
            case: "orders of magnitude more than the invoice billed",
            invoice: Invoice::Own,
            amount: Amount::Literal("99999999"),
            currency: CURRENCY,
            expected: Outcome::Refused(
                "VALIDATION_EXCEPTION",
                Some("EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT"),
            ),
        },
        Row {
            case: "an amount longer than this service accepts an amount being",
            // Past `state::BILLING_ADJUSTMENT_AMOUNT_MAX_LEN`, which is what
            // stands in for the `@length` `PositiveAmountUpto8Decimals` does
            // not declare.
            invoice: Invoice::Own,
            amount: Amount::Literal("1111111111111111111111111111111111111111"),
            currency: CURRENCY,
            expected: Outcome::Refused(
                "VALIDATION_EXCEPTION",
                Some("INVALID_ADJUSTMENT_AMOUNT"),
            ),
        },
        Row {
            case: "an id that names no invoice at all",
            invoice: Invoice::Literal("not-an-invoice-of-this-agreement"),
            amount: Amount::Literal("1"),
            currency: CURRENCY,
            expected: Outcome::Refused("RESOURCE_NOT_FOUND_EXCEPTION", None),
        },
        Row {
            case: "an id shaped like one this service mints",
            invoice: Invoice::Literal("inv-0000000000000000"),
            amount: Amount::Literal("1"),
            currency: CURRENCY,
            expected: Outcome::Refused("RESOURCE_NOT_FOUND_EXCEPTION", None),
        },
        Row {
            case: "another agreement's invoice",
            invoice: Invoice::Other,
            amount: Amount::Literal("1"),
            currency: CURRENCY,
            expected: Outcome::Refused("RESOURCE_NOT_FOUND_EXCEPTION", None),
        },
        Row {
            case: "a currency the invoice is not denominated in",
            invoice: Invoice::Own,
            amount: Amount::Literal("1"),
            currency: "EUR",
            expected: Outcome::Refused("VALIDATION_EXCEPTION", Some("INVALID_CURRENCY_CODE")),
        },
        Row {
            case: "another currency code the member admits",
            invoice: Invoice::Own,
            amount: Amount::Literal("1"),
            currency: "GBP",
            expected: Outcome::Refused("VALIDATION_EXCEPTION", Some("INVALID_CURRENCY_CODE")),
        },
    ];

    for row in rows {
        let router = emulator();
        let (agreement, own) = invoiced_agreement(&router, "ap-rule").await;
        let (_, other) = invoiced_agreement(&router, "ap-ruleother").await;
        let billed = money(&one_group(&router, &agreement).await, "amount");

        let invoice = match row.invoice {
            Invoice::Own => own.clone(),
            Invoice::Other => other.clone(),
            Invoice::Literal(id) => id.to_string(),
        };
        let amount = match row.amount {
            Amount::Literal(amount) => amount.to_string(),
            Amount::WholeInvoice => billed.clone(),
            // One more digit on the end of the billed amount is strictly more
            // than the billed amount, and still inside
            // `PositiveAmountUpto8Decimals`'s eight fraction digits.
            Amount::PastTheInvoice => format!("{billed}1"),
        };

        let answer = submit_one(
            &router,
            &agreement,
            &invoice,
            &amount,
            row.currency,
            "tok-rule",
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: a per-entry outcome is reported in the body, got {} with {:?}",
            row.case, answer.status, answer.body
        );

        match row.expected {
            Outcome::Stored => {
                assert_eq!(
                    error_codes(&answer),
                    Vec::<String>::new(),
                    "case {:?}: expected the entry to be accepted, got {:?}",
                    row.case,
                    answer.body
                );
                assert_eq!(
                    item_ids(&answer).len(),
                    1,
                    "case {:?}: expected one created item, got {:?}",
                    row.case,
                    answer.body
                );
            }
            Outcome::Refused(code, reason) => {
                assert_eq!(
                    error_codes(&answer),
                    vec![code.to_string()],
                    "case {:?}: expected one {code} error, got {:?}",
                    row.case,
                    answer.body
                );
                assert!(
                    item_ids(&answer).is_empty(),
                    "case {:?}: a refused entry creates nothing, got {:?}",
                    row.case,
                    answer.body
                );
                let message = answer.body["errors"][0]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !message.is_empty(),
                    "case {:?}: message is a required member, got {:?}",
                    row.case,
                    answer.body
                );
                // A per-entry error carries no `reason` member, so the
                // modelled reason a client branches on is named in the
                // message.
                if let Some(reason) = reason {
                    assert!(
                        message.contains(reason),
                        "case {:?}: expected the message to name {reason}, got {message:?}",
                        row.case
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// What the invoice has left to adjust
// ---------------------------------------------------------------------------

/// The amount an invoice has open to adjustment is computed from what it billed
/// and the adjustments already requested against it, and it is the ceiling each
/// further entry is held to.
#[tokio::test]
async fn the_open_amount_follows_the_adjustments_already_requested() {
    let router = emulator();
    let (agreement, invoice) = invoiced_agreement(&router, "ap-open").await;

    let group = one_group(&router, &agreement).await;
    let billed = money(&group, "amount");
    assert_eq!(
        money(&group, "maxAdjustmentAmount"),
        billed,
        "case \"before any adjustment\": the whole of what the invoice billed is open to \
         adjustment, got {group:?}"
    );
    assert_eq!(
        money(&group, "currencyCode"),
        CURRENCY,
        "case \"before any adjustment\": the invoice is denominated in the currency the \
         agreement was priced in, got {group:?}"
    );
    assert_eq!(
        group["invoiceId"].as_str(),
        Some(invoice.as_str()),
        "case \"before any adjustment\": the group names its invoice, got {group:?}"
    );
    assert_eq!(
        group["agreementId"].as_str(),
        Some(agreement.as_str()),
        "case \"before any adjustment\": the group names its agreement, got {group:?}"
    );
    assert_eq!(
        group["invoiceType"].as_str(),
        Some(INVOICE),
        "case \"before any adjustment\": an invoice that bills a charge is an INVOICE, got \
         {group:?}"
    );
    assert!(
        group["issuedTime"].as_f64().is_some(),
        "case \"before any adjustment\": the group reports when the invoice was issued, got \
         {group:?}"
    );
    let month = group["invoiceBillingPeriod"]["month"].as_i64();
    let year = group["invoiceBillingPeriod"]["year"].as_i64();
    assert!(
        month.is_some_and(|month| (1..=12).contains(&month))
            && year.is_some_and(|year| (2023..=2050).contains(&year)),
        "case \"before any adjustment\": the billing period must be inside the bounds \
         InvoiceBillingPeriod declares (month 1-12, year 2023-2050), got {group:?}"
    );

    // One adjustment of part of the invoice: what is open drops by exactly what
    // it asked for.
    let first = "10000";
    let answer = submit_one(&router, &agreement, &invoice, first, CURRENCY, "tok-open-1").await;
    assert_eq!(
        error_codes(&answer),
        Vec::<String>::new(),
        "case \"an adjustment inside the open amount\": expected it to be accepted, got {:?}",
        answer.body
    );
    let open = money(&one_group(&router, &agreement).await, "maxAdjustmentAmount");
    assert!(
        open != billed && !open.is_empty(),
        "case \"after the first adjustment\": the open amount must have dropped from {billed}, \
         got {open}"
    );

    // A second entry for the whole invoice now asks for more than is open.
    let refused = submit_one(
        &router,
        &agreement,
        &invoice,
        &billed,
        CURRENCY,
        "tok-open-2",
    )
    .await;
    assert_eq!(
        error_codes(&refused),
        vec!["VALIDATION_EXCEPTION".to_string()],
        "case \"an adjustment past the open amount\": expected a per-entry validation error, \
         got {:?}",
        refused.body
    );
    assert_eq!(
        money(&one_group(&router, &agreement).await, "maxAdjustmentAmount"),
        open,
        "case \"after a refused adjustment\": a refused entry stores nothing, so the open \
         amount does not move"
    );

    // Exactly what is open is accepted, and nothing is left afterwards.
    let rest = submit_one(&router, &agreement, &invoice, &open, CURRENCY, "tok-open-3").await;
    assert_eq!(
        error_codes(&rest),
        Vec::<String>::new(),
        "case \"an adjustment of exactly the open amount\": expected it to be accepted, got {:?}",
        rest.body
    );
    let exhausted = money(&one_group(&router, &agreement).await, "maxAdjustmentAmount");
    assert!(
        exhausted
            .chars()
            .all(|digit| digit == '0' || digit == '.'),
        "case \"after the invoice was fully adjusted\": nothing is left open, got {exhausted}"
    );
    let past = submit_one(
        &router,
        &agreement,
        &invoice,
        "0.00000001",
        CURRENCY,
        "tok-open-4",
    )
    .await;
    assert_eq!(
        error_codes(&past),
        vec!["VALIDATION_EXCEPTION".to_string()],
        "case \"an adjustment of a fully adjusted invoice\": expected a per-entry validation \
         error, got {:?}",
        past.body
    );
}

// ---------------------------------------------------------------------------
// Which charges are invoiced at all
// ---------------------------------------------------------------------------

/// One row of the invoicing table: the terms the agreement was created over.
struct Billed {
    case: &'static str,
    terms: Vec<Value>,
}

/// The invoices an acceptance issues are exactly the charges of its own quote
/// that are charged on acceptance and carry an amount, each billing the amount
/// the quote priced it at.
///
/// The rows differ along what the requested terms imply: a term that carries no
/// charge at all, charges timed on a billing period and on a future date, one
/// and then two charges timed on acceptance, and a prepaid charge over a
/// dimension the published rates do not price (so the quote reports no amount
/// for it). The timings are `ExpectedCharge$timing`'s documented meanings.
#[tokio::test]
async fn an_acceptance_invoices_the_charges_it_bills() {
    let rows = vec![
        Billed {
            case: "a legal term carries no charge",
            terms: vec![term("eula.v1")],
        },
        Billed {
            case: "a usage-based charge is incurred on each billing period",
            terms: vec![term("UsageBasedPricingTerm")],
        },
        Billed {
            case: "a payment schedule is charged at a future date",
            terms: vec![term("PaymentScheduleTerm")],
        },
        Billed {
            case: "a fixed upfront charge is incurred on acceptance",
            terms: vec![term("FixedUpfrontPricingTerm")],
        },
        Billed {
            case: "a configured prepaid charge is incurred on acceptance",
            terms: vec![upfront_term(
                "term.cfg@1",
                "P12M",
                &[("Users", 10), ("Admins", 2)],
            )],
        },
        Billed {
            case: "two charges on acceptance are two invoices",
            terms: vec![
                upfront_term("term.cfg@2", "P24M", &[("Hosts", 3)]),
                term("FixedUpfrontPricingTerm"),
            ],
        },
        Billed {
            case: "a dimension the published rates do not price has no amount to bill",
            terms: vec![upfront_term("term.cfg@3", "P12M", &[("Unpriced", 5)])],
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let router = emulator();
        let quoted = create(
            &router,
            new_request(&format!("ap-billed{index}"), row.terms.clone()),
        )
        .await;
        assert_eq!(
            quoted.status, 200,
            "case {:?}: quoting must succeed, got {} with {:?}",
            row.case, quoted.status, quoted.body
        );
        // The amounts the quote itself priced, for the charges it timed on
        // acceptance: what an invoice of this acceptance may bill.
        let on_acceptance: Vec<String> = quoted.body["chargeSummary"]["expectedCharges"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|charge| charge["timing"].as_str() == Some("ON_ACCEPTANCE"))
            .filter_map(|charge| charge["amount"].as_str().map(str::to_string))
            .collect();
        let currency = quoted.body["chargeSummary"]["currencyCode"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let accepted = accept(
            &router,
            json!({"agreementRequestId": quoted.str_at("/agreementRequestId")}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: accepting must succeed, got {} with {:?}",
            row.case, accepted.status, accepted.body
        );
        let agreement = accepted.str_at("/agreementId");

        let groups = groups_of(&router, &agreement).await;
        let billed: Vec<String> = groups
            .iter()
            .map(|group| money(group, "amount"))
            .collect();
        assert_eq!(
            billed, on_acceptance,
            "case {:?}: expected one invoice per charge incurred on acceptance billing what the \
             quote priced it at ({on_acceptance:?}), got {groups:?}",
            row.case
        );
        for group in &groups {
            assert_eq!(
                money(group, "currencyCode"),
                currency,
                "case {:?}: an invoice is denominated in the currency the quote priced, got \
                 {group:?}",
                row.case
            );
        }

        // An agreement with nothing invoiced has nothing to adjust: the ids of
        // another agreement's invoices do not become its own.
        if groups.is_empty() {
            let (_, elsewhere) =
                invoiced_agreement(&router, &format!("ap-billed{index}else")).await;
            let answer = submit_one(
                &router,
                &agreement,
                &elsewhere,
                "1",
                CURRENCY,
                "tok-billed",
            )
            .await;
            assert_eq!(
                error_codes(&answer),
                vec!["RESOURCE_NOT_FOUND_EXCEPTION".to_string()],
                "case {:?}: an agreement with no invoice has nothing to adjust, got {:?}",
                row.case,
                answer.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ListAgreementInvoiceLineItems
// ---------------------------------------------------------------------------

/// Every filter of the listing selects on the stored invoice, the page token
/// resumes the listing it was minted for, and an `agreementId` naming no
/// agreement is a not-found rather than an empty page.
#[tokio::test]
async fn every_filter_selects_on_the_stored_invoice() {
    let router = emulator();
    // Two charges incurred on acceptance, so the agreement has two invoices to
    // select between and to page over.
    let quoted = create(
        &router,
        new_request(
            "ap-invoicefilters",
            vec![
                upfront_term("term.cfg@1", "P12M", &[("Users", 4)]),
                term("FixedUpfrontPricingTerm"),
            ],
        ),
    )
    .await;
    let accepted = accept(
        &router,
        json!({"agreementRequestId": quoted.str_at("/agreementRequestId")}),
    )
    .await;
    let agreement = accepted.str_at("/agreementId");
    let invoices = invoices_of(&router, &agreement).await;
    assert_eq!(
        invoices.len(),
        2,
        "two charges on acceptance must be two invoices, got {invoices:?}"
    );
    // An invoice of ANOTHER agreement, for the row that names one.
    let (_, other_invoice) = invoiced_agreement(&router, "ap-invoicefiltersother").await;

    let groups = groups_of(&router, &agreement).await;
    let issued = groups[0]["issuedTime"].as_f64().unwrap_or_else(|| {
        panic!("a group reports when its invoice was issued, got {:?}", groups[0])
    });
    let period = groups[0]["invoiceBillingPeriod"].clone();
    let month = period["month"].as_i64().unwrap_or_default();
    let year = period["year"].as_i64().unwrap_or_default();
    // The next month in the modelled 1-12 range, which no invoice of this run
    // was issued in.
    let next_month = (month % 12) + 1;

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter reports every invoice of the agreement",
            json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID}),
            invoices.clone(),
        ),
        (
            "invoiceId selects the one invoice it names",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceId": invoices[1],
            }),
            vec![invoices[1].clone()],
        ),
        (
            "invoiceId naming another agreement's invoice selects nothing",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceId": other_invoice,
            }),
            vec![],
        ),
        (
            "invoiceType INVOICE selects the invoices that bill a charge",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceType": INVOICE,
            }),
            invoices.clone(),
        ),
        (
            "the other invoice type the member admits selects nothing",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceType": CREDIT_MEMO,
            }),
            vec![],
        ),
        (
            "the billing period the invoices were issued in selects them",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceBillingPeriod": {"month": month, "year": year},
            }),
            invoices.clone(),
        ),
        (
            "another billing period selects nothing",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "invoiceBillingPeriod": {"month": next_month, "year": year},
            }),
            vec![],
        ),
        (
            "afterIssuedTime a second before the acceptance selects every invoice",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "afterIssuedTime": issued - 1.0,
            }),
            invoices.clone(),
        ),
        (
            "afterIssuedTime a second past the acceptance selects nothing",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "afterIssuedTime": issued + 1.0,
            }),
            vec![],
        ),
        (
            "beforeIssuedTime a second past the acceptance selects every invoice",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "beforeIssuedTime": issued + 1.0,
            }),
            invoices.clone(),
        ),
        (
            "beforeIssuedTime a second before the acceptance selects nothing",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "beforeIssuedTime": issued - 1.0,
            }),
            vec![],
        ),
        (
            "the two bounds together select the invoices between them",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "afterIssuedTime": issued - 1.0,
                "beforeIssuedTime": issued + 1.0,
            }),
            invoices.clone(),
        ),
        (
            "maxResults bounds the page to the first invoice",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "maxResults": 1,
            }),
            vec![invoices[0].clone()],
        ),
    ];

    for (case, request, expected) in cases {
        let answer = list_invoices(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: listing must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        let reported: Vec<String> = answer.body["agreementInvoiceLineItemGroupSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|group| group["invoiceId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.body
        );
    }

    // The pages together report each invoice once, and the token that resumes
    // them belongs to the listing it was minted for.
    let first_page = list_invoices(
        &router,
        json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID, "maxResults": 1}),
    )
    .await;
    let token = first_page.str_at("/nextToken");
    assert!(
        is_next_token(&token),
        "a page token must match NextToken's pattern, got {token:?}"
    );
    let second_page = list_invoices(
        &router,
        json!({
            "agreementId": agreement,
            "groupBy": GROUP_BY_INVOICE_ID,
            "maxResults": 1,
            "nextToken": token,
        }),
    )
    .await;
    assert_eq!(
        second_page.str_at("/agreementInvoiceLineItemGroupSummaries/0/invoiceId"),
        invoices[1],
        "case \"the second page\": expected the invoice the first page did not report, got {:?}",
        second_page.body
    );
    assert!(
        second_page.body["nextToken"].is_null(),
        "case \"the second page\": the last page carries no nextToken, got {:?}",
        second_page.body
    );

    for (case, request) in [
        (
            "a token this service never minted",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "nextToken": "not-a-token",
            }),
        ),
        (
            "a token minted for the unfiltered listing cannot resume a filtered one",
            json!({
                "agreementId": agreement,
                "groupBy": GROUP_BY_INVOICE_ID,
                "maxResults": 1,
                "invoiceType": INVOICE,
                "nextToken": first_page.str_at("/nextToken"),
            }),
        ),
    ] {
        let answer = list_invoices(&router, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.reason(),
            "INVALID_NEXT_TOKEN",
            "case {case:?}: expected reason INVALID_NEXT_TOKEN, got {:?}",
            answer.body
        );
    }

    // An `agreementId` naming no agreement is a reference to a resource that
    // does not exist, which this operation models.
    let missing = list_invoices(
        &router,
        json!({"agreementId": "agmt-absent", "groupBy": GROUP_BY_INVOICE_ID}),
    )
    .await;
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "case \"an agreementId naming no agreement\": expected a ResourceNotFoundException, got \
         {:?}",
        missing.body
    );
}
