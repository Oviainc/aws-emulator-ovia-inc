// SPDX-License-Identifier: CC0-1.0

//! `ListAgreementInvoiceLineItems` driven on a store no other call has read, end
//! to end over HTTP.
//!
//! Every report here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: a report about the ONE agreement an id member of its
//! input names is computed from the store as the clock has already left it, so
//! the WHOLE answer — not merely its status — never depends on whether some
//! unrelated call looked at the store first. An agreement reaches its own stored
//! end date at that instant whether or not anybody calls, and
//! `EndTimeBehaviorType` names what that date does — `RENEW` is "a new agreement
//! is created from the accepted terms of this agreement"
//! (`smithy_query(query="shape", shape="EndTimeBehavior")`) — so from that date
//! on the renewal is an agreement of the store, holding the invoices its own
//! quote bills on acceptance, which is the "aggregated billing data" this listing
//! reports (`smithy_query(query="operation",
//! operation="ListAgreementInvoiceLineItems")`). Each group is read off the
//! stored invoice, so an invoice the clock issued is dated at the end date that
//! issued it rather than at the instant somebody listed; an `agreementId` no end
//! date ever created is still answered as the agreement that was looked for.
//!
//! The rows differ along what the rule generalises over: which contract length
//! the acceptor purchased, how many whole lengths have passed since it was signed
//! (one, two, as many as the renewal term allows), which link of the chain is
//! asked about, and which shape of request the listing is asked in — unfiltered,
//! at each end of the page-size range, and filtered by each member the operation
//! publishes a filter for.
//!
//! The clock cannot be wound forward 30 days inside a test, so the agreement's
//! own stored window is wound BACK instead: the harness that does it (`days`) is
//! the one `agreement_expiry` owns, the stores and the chain they owe (`Aged`,
//! `aged_rows`, `aged_store`, `chain_after_an_unrelated_read`, `instant`) are
//! `unsettled_store_single_agreement_reads`', the wire harness (`call`,
//! `describe`) is `agreement_lifecycle`'s and the terms of an auto-renewing
//! purchase (`renewing_terms`) are `agreement_renewals`'.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_expiry::days;
use crate::agreement_lifecycle::{call, describe, Answer};
use crate::agreement_renewals::renewing_terms;
use crate::unsettled_store_single_agreement_reads::{
    aged_rows, aged_store, chain_after_an_unrelated_read, instant, Aged,
};

/// `LineItemGroupBy`'s only value, from
/// `smithy_query(query="shape", shape="LineItemGroupBy")`.
const GROUP_BY_INVOICE_ID: &str = "INVOICE_ID";

/// The invoice type an invoice an acceptance billed carries, from
/// `smithy_query(query="shape", shape="InvoiceType")` ("Valid values are INVOICE
/// and CREDIT_MEMO").
const INVOICE: &str = "INVOICE";

/// See [`INVOICE`]: the other value that enum admits, which no invoice an
/// acceptance bills carries.
const CREDIT_MEMO: &str = "CREDIT_MEMO";

/// An `agreementId` within `ResourceId`'s `^[A-Za-z0-9_/-]+$` that names no
/// agreement any end date of these stores could have created.
const UNKNOWN_AGREEMENT: &str = "agmt-ffffffffffffffff";

/// An `invoiceId` within that same pattern that names no invoice this emulator's
/// minter ever produced, so the filter carrying it selects nothing.
const UNKNOWN_INVOICE: &str = "inv-ffffffffffffffff";

/// The page sizes `maxResults` publishes, from `@range(min: 1, max: 50)`.
const SMALLEST_PAGE: i32 = 1;
/// See [`SMALLEST_PAGE`].
const LARGEST_PAGE: i32 = 50;

/// The status this service answers a lookup that found nothing with, which the
/// crate's `unknown_ids` suite pins against the recorded AWS responses.
const NOT_FOUND_STATUS: u16 = 400;

/// The error this operation models for "a resource which does not exist", from
/// `smithy_query(query="operation", operation="ListAgreementInvoiceLineItems")`.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The `resourceType` such a refusal names when the agreement is what was looked
/// for, from `smithy_query(query="shape", shape="ResourceType")`.
const AGREEMENT: &str = "Agreement";

/// The invoices one link of these chains is billed: its quote carries a single
/// configured prepaid charge, which is the one charge billed on acceptance and so
/// the one invoice that acceptance issues.
const INVOICES_PER_LINK: usize = 1;

/// How close two instants the wire spells in epoch seconds have to be to count as
/// the same: a double carries an agreement's instants to well under a
/// millisecond.
const INSTANT_TOLERANCE: f64 = 1e-3;

/// The report `request` asks of the listing under test.
async fn list(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "ListAgreementInvoiceLineItems", request).await
}

/// The unfiltered report of one agreement's invoices: `agreementId` and the
/// required grouping, and nothing else.
fn report_of(agreement: &str) -> Value {
    json!({"agreementId": agreement, "groupBy": GROUP_BY_INVOICE_ID})
}

/// The same report with the filter members of one row carried beside them.
fn report_with(agreement: &str, filters: &Value) -> Value {
    let mut body = report_of(agreement);
    for (member, value) in filters.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    body
}

/// The groups one report carries.
fn groups(answer: &Answer) -> Vec<Value> {
    answer.body["agreementInvoiceLineItemGroupSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The unrelated call that used to be what admitted the renewal: the charge
/// report over the whole store, which settles every chain the store holds.
async fn an_unrelated_call(router: &MultiServiceRouter, case: &str) {
    let answer = call(router, "ListAgreementCharges", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the unrelated call must succeed, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Assert every group of a report belongs to the agreement that was asked about
/// and is read off an invoice an acceptance billed.
fn assert_groups_are_of(case: &str, agreement: &str, answer: &Answer) {
    for group in groups(answer) {
        assert_eq!(
            group["agreementId"].as_str(),
            Some(agreement),
            "case {case:?}: every group reported is a group of an invoice of {agreement}, got \
             {group:?}"
        );
        assert_eq!(
            group["invoiceType"].as_str(),
            Some(INVOICE),
            "case {case:?}: an invoice an acceptance billed carries {INVOICE}, got {group:?}"
        );
    }
}

/// The invoices of every agreement of the chain an end date created are reported
/// by the listing that exists to report them, even when it is the FIRST call the
/// store sees — and an unrelated call changes that report in no way.
#[tokio::test]
async fn the_invoices_of_the_renewal_the_clock_owes_are_reported_without_an_earlier_call() {
    for (index, row) in aged_rows().into_iter().enumerate() {
        let proposal = format!("ap-unsettledinvoices{index}");
        let (probe, source) = aged_store(&row, &proposal).await;
        let chain = chain_after_an_unrelated_read(&probe, &source).await;
        assert_eq!(
            chain.len(),
            row.renewals + 1,
            "case {:?}: expected the source and {} renewal(s), got {chain:?}",
            row.case,
            row.renewals
        );

        for (link, agreement) in chain.iter().enumerate() {
            let case = format!("{} — link {link} of the chain", row.case);
            // A store of its own, built exactly the same way, where the listing
            // under test is the FIRST call it sees. Ids are minted from the
            // store's own counter, so the identical setup mints the identical
            // ones — which the source id pins.
            let (router, same) = aged_store(&row, &proposal).await;
            assert_eq!(
                same, source,
                "case {case:?}: the identical setup must mint the identical agreement id"
            );

            let first = list(&router, report_of(agreement)).await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: the agreement {agreement} the clock owes is one this listing \
                 holds, got {} with {:?}",
                first.status, first.body
            );
            assert_eq!(
                groups(&first).len(),
                INVOICES_PER_LINK,
                "case {case:?}: expected the {INVOICES_PER_LINK} invoice(s) the prepaid charge of \
                 {agreement} is billed, got {:?}",
                first.body
            );
            assert_groups_are_of(&case, agreement, &first);

            // An invoice the clock issued is dated at the end date that issued
            // it — the renewal's own start — rather than at the instant this
            // listing ran. The source's invoice is dated at the acceptance this
            // harness wound the window back FROM, so only the renewals carry
            // that equality.
            if link > 0 {
                let described = describe(&router, agreement).await;
                assert_eq!(
                    described.status, 200,
                    "case {case:?}: reading {agreement} back must succeed, got {} with {:?}",
                    described.status, described.body
                );
                let started = instant(&described.body, "startTime");
                for group in groups(&first) {
                    let issued = instant(&group, "issuedTime");
                    assert!(
                        (issued - started).abs() < INSTANT_TOLERANCE,
                        "case {case:?}: expected the invoice of {agreement} to be issued at the \
                         start {started} the end date gave it, got {issued}"
                    );
                }
            }

            // The unrelated call, and the identical request again: the same
            // body, so this report never depended on that call.
            an_unrelated_call(&router, &case).await;
            let again = list(&router, report_of(agreement)).await;
            assert_eq!(
                again.body, first.body,
                "case {case:?}: an unrelated call must not change the report, expected {:?}, got \
                 {:?}",
                first.body, again.body
            );
        }
    }
}

/// One shape of the report, and what it selects of the invoices the renewal's
/// acceptance billed.
struct Shape {
    case: &'static str,
    /// The members the row carries beside `agreementId` and `groupBy`.
    filters: Value,
    /// How many groups the row selects, or `None` where what it selects depends
    /// on the calendar month the store happened to be built in — such a row pins
    /// the rule itself (the identical answer before and after an unrelated call)
    /// rather than a count.
    selects: Option<usize>,
}

/// Every shape of request this listing serves, around the ONE invoice the
/// renewal's acceptance billed: the filter members are the ones the operation
/// publishes (`smithy_query(query="operation",
/// operation="ListAgreementInvoiceLineItems")` — `invoiceId`, `invoiceType`,
/// `invoiceBillingPeriod`, `maxResults`), each carried at a value that selects
/// that invoice and at one that selects nothing.
fn shapes(invoice: &str, period: &Value) -> Vec<Shape> {
    let mut other_period = period.clone();
    // The month before the one the invoice was issued in, inside
    // `InvoiceBillingPeriod$month`'s own 1..12 range.
    if let Some(month) = period["month"].as_i64() {
        other_period["month"] = json!(if month > 1 { month - 1 } else { month + 1 });
    }
    vec![
        Shape {
            case: "unfiltered",
            filters: json!({}),
            selects: Some(INVOICES_PER_LINK),
        },
        Shape {
            case: "paged at the smallest page size the range publishes",
            filters: json!({"maxResults": SMALLEST_PAGE}),
            selects: Some(INVOICES_PER_LINK),
        },
        Shape {
            case: "paged at the largest page size the range publishes",
            filters: json!({"maxResults": LARGEST_PAGE}),
            selects: Some(INVOICES_PER_LINK),
        },
        Shape {
            case: "filtered to the invoice the renewal's acceptance billed",
            filters: json!({"invoiceId": invoice}),
            selects: Some(INVOICES_PER_LINK),
        },
        Shape {
            case: "filtered to a well-formed invoice id naming no invoice",
            filters: json!({"invoiceId": UNKNOWN_INVOICE}),
            selects: Some(0),
        },
        Shape {
            case: "filtered to the type an issued invoice carries",
            filters: json!({"invoiceType": INVOICE}),
            selects: Some(INVOICES_PER_LINK),
        },
        Shape {
            case: "filtered to the other type the enum admits",
            filters: json!({"invoiceType": CREDIT_MEMO}),
            selects: Some(0),
        },
        Shape {
            case: "filtered to the billing period the invoice was issued in",
            filters: json!({"invoiceBillingPeriod": period}),
            selects: None,
        },
        Shape {
            case: "filtered to another billing period",
            filters: json!({"invoiceBillingPeriod": other_period}),
            selects: None,
        },
    ]
}

/// Every shape of request the listing serves answers the renewal the clock owes
/// the same way whether or not an unrelated call looked at the store first, so
/// the rule holds over the class of these reports rather than over one of them.
#[tokio::test]
async fn every_shape_of_the_report_answers_the_same_without_an_earlier_call() {
    let row = Aged {
        case: "a month's purchase a day past its end date",
        requested: renewing_terms("P1M"),
        signed_ago: days(31),
        renewals: 1,
    };
    let proposal = "ap-unsettledinvoiceshapes";

    let (probe, source) = aged_store(&row, proposal).await;
    let chain = chain_after_an_unrelated_read(&probe, &source).await;
    assert_eq!(
        chain.len(),
        row.renewals + 1,
        "{}: expected the source and {} renewal(s), got {chain:?}",
        row.case,
        row.renewals
    );
    let renewal = chain
        .last()
        .cloned()
        .expect("the chain holds at least the source");
    let reported = groups(&list(&probe, report_of(&renewal)).await);
    let group = reported.first().cloned().unwrap_or_else(|| {
        panic!("the renewal {renewal} is billed its prepaid charge, got {reported:?}")
    });
    let invoice = group["invoiceId"]
        .as_str()
        .unwrap_or_else(|| panic!("every group names its invoice, got {group:?}"))
        .to_string();
    let period = group["invoiceBillingPeriod"].clone();

    for shape in shapes(&invoice, &period) {
        let case = shape.case;
        let (router, same) = aged_store(&row, proposal).await;
        assert_eq!(
            same, source,
            "case {case:?}: the identical setup must mint the identical agreement id"
        );

        let request = report_with(&renewal, &shape.filters);
        let first = list(&router, request.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the renewal {renewal} the clock owes is an agreement this listing \
             holds, got {} with {:?}",
            first.status, first.body
        );
        if let Some(selects) = shape.selects {
            assert_eq!(
                groups(&first).len(),
                selects,
                "case {case:?}: expected {selects} group(s), got {:?}",
                first.body
            );
        }
        assert_groups_are_of(case, &renewal, &first);

        an_unrelated_call(&router, case).await;
        let again = list(&router, request).await;
        assert_eq!(
            again.body, first.body,
            "case {case:?}: an unrelated call must not change the report, expected {:?}, got {:?}",
            first.body, again.body
        );
    }
}

/// The complement: an `agreementId` no end date of the store could have created
/// is still answered as the agreement that was looked for, whether or not an
/// unrelated call looked first — settling admits the agreements the clock owed
/// and nothing else.
#[tokio::test]
async fn an_agreement_id_no_end_date_created_is_still_not_found_by_this_report() {
    let row = Aged {
        case: "a month's purchase a day past its end date",
        requested: renewing_terms("P1M"),
        signed_ago: days(31),
        renewals: 1,
    };
    let (router, _source) = aged_store(&row, "ap-unsettledinvoiceunknown").await;

    let first = list(&router, report_of(UNKNOWN_AGREEMENT)).await;
    assert_not_found("the first call the store sees", &first);
    an_unrelated_call(&router, "after an unrelated call").await;
    let again = list(&router, report_of(UNKNOWN_AGREEMENT)).await;
    assert_not_found("after an unrelated call", &again);
}

/// Assert one refusal is the modelled not-found for the agreement that was
/// looked for, at the status this service answers a lookup that found nothing
/// with, reporting nothing beside it.
fn assert_not_found(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, NOT_FOUND_STATUS,
        "case {case:?}: a lookup that found nothing answers HTTP {NOT_FOUND_STATUS}, got {} with \
         {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        NOT_FOUND_CODE,
        "case {case:?}: expected a {NOT_FOUND_CODE}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceId"),
        UNKNOWN_AGREEMENT,
        "case {case:?}: expected the refusal to name {UNKNOWN_AGREEMENT}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceType"),
        AGREEMENT,
        "case {case:?}: expected the refusal to name the {AGREEMENT} kind, got {:?}",
        answer.body
    );
    assert!(
        answer.body["agreementInvoiceLineItemGroupSummaries"].is_null(),
        "case {case:?}: a refused request reports nothing, got {:?}",
        answer.body
    );
}
