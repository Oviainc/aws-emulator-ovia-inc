// SPDX-License-Identifier: CC0-1.0

//! The time-window bounds of a listing's query as part of its page-token scope,
//! end to end over HTTP.
//!
//! The rule under test is the one both windowed listings of this crate build
//! their page tokens from: each bound of the time window a listing's query names
//! is a part of that query's scope, rendered the one way for every such listing,
//! so a call whose window differs from the one a token was minted for — in
//! either bound, by any amount, or by leaving a bound out — refuses that token
//! with HTTP 400 `ValidationException` / `INVALID_NEXT_TOKEN`, while the
//! identical window resumes it.
//!
//! `ListAgreementInvoiceLineItems` (`afterIssuedTime`/`beforeIssuedTime`) and
//! `ListBillingAdjustmentRequests` (`createdAfter`/`createdBefore`) are driven
//! here because each used to render those bounds with its own copy of that
//! rendering; both now go through the single spelling in `state`, so this suite
//! exercises that one path from both sides.
//!
//! Every neighbouring window below spans every record the token's own window
//! spans — each row asserts that first — so the refusal can only come from the
//! scope and never from a window that selects nothing. The rows differ along
//! what the rule generalises over: which of the two bounds differs, in which
//! direction, by how much (a second and a millisecond, so a rendering that
//! dropped the sub-second part would fail), and whether a bound is moved or left
//! out altogether.
//!
//! The harness (`emulator`, `call`, `agreement_over`, `invoiced_agreement`) is
//! the one `agreement_lifecycle` owns, and the batch helpers are the ones
//! `billing_adjustments` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    agreement_over, call, emulator, invoiced_agreement, is_next_token, term, upfront_term, Answer,
};
use crate::billing_adjustments::{entry, submit};

/// `LineItemGroupBy`'s one value, which `ListAgreementInvoiceLineItems` requires
/// of every call (`smithy_query(query="shape", shape="LineItemGroupBy")`).
const GROUP_BY_INVOICE_ID: &str = "INVOICE_ID";

/// A term id naming a kind whose charge is billed when the agreement request is
/// accepted, so accepting a quote carrying it issues an invoice — one more
/// invoice than the configured prepaid term beside it, so the line-item listing
/// has two groups to page.
const PREPAID_TERM: &str = "FixedUpfrontPricingTerm";

/// The quantity of the one prepaid dimension the configured term buys, large
/// enough that the charge it bills is positive and so invoiced.
const PREPAID_QUANTITY: i32 = 100;

/// One listing whose query carries a window over a stored instant: how it is
/// called, where its records arrive, the member each record carries its id and
/// its bounded instant in, and the query the window is added to.
struct Windowed {
    operation: &'static str,
    /// The member naming the exclusive lower bound.
    after_member: &'static str,
    /// The member naming the exclusive upper bound.
    before_member: &'static str,
    /// The output member the page of records arrives in.
    items_member: &'static str,
    /// The member each record carries its own id in.
    id_member: &'static str,
    /// The member each record reports the bounded instant in.
    instant_member: &'static str,
    /// The query the window is added to, with every member this listing
    /// requires.
    base: Value,
}

impl Windowed {
    /// This listing's query with whichever of the two bounds the caller names;
    /// a `None` bound is left out of the request entirely.
    fn request(&self, after: Option<f64>, before: Option<f64>) -> Value {
        let mut body = self.base.clone();
        if let Some(after) = after {
            body[self.after_member] = json!(after);
        }
        if let Some(before) = before {
            body[self.before_member] = json!(before);
        }
        body
    }

    /// The records one answer reported, in the order reported.
    fn records(&self, answer: &Answer) -> Vec<Value> {
        answer.body[self.items_member]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "a served {} answers its page in {}, got {:?}",
                    self.operation, self.items_member, answer.body
                )
            })
    }

    /// The ids one answer reported, in the order reported.
    fn ids(&self, answer: &Answer) -> Vec<String> {
        self.records(answer)
            .iter()
            .map(|record| {
                record[self.id_member]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!(
                            "every record of {} carries {}, got {record:?}",
                            self.operation, self.id_member
                        )
                    })
                    .to_string()
            })
            .collect()
    }

    /// The bounded instants one answer reported.
    fn instants(&self, answer: &Answer) -> Vec<f64> {
        self.records(answer)
            .iter()
            .map(|record| {
                record[self.instant_member].as_f64().unwrap_or_else(|| {
                    panic!(
                        "every record of {} reports {}, got {record:?}",
                        self.operation, self.instant_member
                    )
                })
            })
            .collect()
    }
}

/// The listing for `request`, asserted to have succeeded.
async fn served(router: &MultiServiceRouter, operation: &str, request: Value) -> Answer {
    let answer = call(router, operation, request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {operation} {request}: the listing must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// How a neighbouring window differs from the one a token was minted for: the
/// offset applied to each bound, where `None` leaves that bound out of the
/// request altogether. Every offset is small enough that the window still spans
/// every stored record.
const NEIGHBOURS: &[(&str, Option<f64>, Option<f64>)] = &[
    (
        "the lower bound a second earlier",
        Some(-1.0),
        Some(0.0),
    ),
    ("the lower bound a second later", Some(1.0), Some(0.0)),
    ("the upper bound a second later", Some(0.0), Some(1.0)),
    ("the upper bound a second earlier", Some(0.0), Some(-1.0)),
    ("both bounds a second wider", Some(-1.0), Some(1.0)),
    (
        "the lower bound a millisecond earlier",
        Some(-0.001),
        Some(0.0),
    ),
    (
        "the lower bound a millisecond later",
        Some(0.001),
        Some(0.0),
    ),
    (
        "the upper bound a millisecond later",
        Some(0.0),
        Some(0.001),
    ),
    (
        "the upper bound a millisecond earlier",
        Some(0.0),
        Some(-0.001),
    ),
    ("the lower bound left out", None, Some(0.0)),
    ("the upper bound left out", Some(0.0), None),
    ("the whole window left out", None, None),
];

/// A token a windowed listing minted resumes exactly the window its query named,
/// and every query whose window differs from it in either bound refuses it.
#[tokio::test]
async fn a_page_token_is_bound_to_both_bounds_of_the_window_that_minted_it() {
    let router = emulator();

    // An agreement over two charges billed on acceptance, so its invoices are
    // two and the line-item listing has a second page to leave a token for.
    let billed = agreement_over(
        &router,
        "ap-windowscope",
        vec![
            upfront_term("term.windowscope@1", "P12M", &[("Users", PREPAID_QUANTITY)]),
            term(PREPAID_TERM),
        ],
    )
    .await;

    // An invoiced agreement carrying two billing adjustment requests, for the
    // same reason.
    let (adjusted, invoice) = invoiced_agreement(&router, "ap-windowscopeadjusted").await;
    let created = submit(
        &router,
        vec![
            entry(&adjusted, &invoice, "1.00", "tok-windowscope-one"),
            entry(&adjusted, &invoice, "2.00", "tok-windowscope-two"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "submitting two billing adjustment requests must succeed, got {} with {:?}",
        created.status, created.body
    );
    assert_eq!(
        created.body["errors"].as_array().map(Vec::len),
        Some(0),
        "both adjustments must be accepted, got {:?}",
        created.body
    );

    let listings = vec![
        Windowed {
            operation: "ListAgreementInvoiceLineItems",
            after_member: "afterIssuedTime",
            before_member: "beforeIssuedTime",
            items_member: "agreementInvoiceLineItemGroupSummaries",
            id_member: "invoiceId",
            instant_member: "issuedTime",
            base: json!({"agreementId": billed, "groupBy": GROUP_BY_INVOICE_ID}),
        },
        Windowed {
            operation: "ListBillingAdjustmentRequests",
            after_member: "createdAfter",
            before_member: "createdBefore",
            items_member: "items",
            id_member: "billingAdjustmentRequestId",
            instant_member: "createdAt",
            base: json!({"agreementId": adjusted}),
        },
    ];

    for listing in &listings {
        let operation = listing.operation;

        // Every record this listing holds, and a window that spans them all:
        // two seconds below the earliest and two above the latest, so every
        // offset a row applies below leaves them all inside it.
        let all = served(&router, operation, listing.base.clone()).await;
        let ids = listing.ids(&all);
        assert!(
            ids.len() >= 2,
            "case {operation}: a bounded page leaves a token only while records remain, \
             so this query must serve at least two, got {ids:?}"
        );
        let instants = listing.instants(&all);
        let after = instants.iter().copied().fold(f64::INFINITY, f64::min) - 2.0;
        let before = instants
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
            + 2.0;

        // The token this window mints, over its first record.
        let mut bounded = listing.request(Some(after), Some(before));
        bounded["maxResults"] = json!(1);
        let first = served(&router, operation, bounded.clone()).await;
        assert_eq!(
            listing.ids(&first),
            ids[..1].to_vec(),
            "case {operation}: expected the first record {:?}, got {:?}",
            ids[..1].to_vec(),
            first.body
        );
        let token = first.body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {operation}: a bounded page over more records leaves a token, got {:?}",
                    first.body
                )
            })
            .to_string();
        assert!(
            is_next_token(&token),
            "case {operation}: the token {token:?} must satisfy NextToken's pattern and bound"
        );

        // The identical window resumes with it, past the record already reported.
        let mut resumed_body = bounded.clone();
        resumed_body["nextToken"] = json!(token.clone());
        let resumed = served(&router, operation, resumed_body).await;
        assert_eq!(
            listing.ids(&resumed),
            ids[1..2].to_vec(),
            "case {operation}: expected the token to resume at {:?}, got {:?}",
            ids[1..2].to_vec(),
            resumed.body
        );

        for (difference, lower, upper) in NEIGHBOURS {
            let case = format!("{operation}, {difference}");
            let neighbour = listing.request(
                lower.map(|offset| after + offset),
                upper.map(|offset| before + offset),
            );

            // The neighbouring window spans every record the token's own window
            // spans, so nothing but the scope can refuse the token below.
            let answer = served(&router, operation, neighbour.clone()).await;
            assert_eq!(
                listing.ids(&answer),
                ids,
                "case {case:?}: the neighbouring window must span the same records {ids:?}, \
                 got {:?}",
                answer.body
            );

            let mut with_token = neighbour.clone();
            with_token["maxResults"] = json!(1);
            with_token["nextToken"] = json!(token.clone());
            let refused = call(&router, operation, with_token.clone()).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: {with_token} must refuse a token minted for another window, \
                 got {} with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?}: the refusal is a ValidationException, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.reason(),
                "INVALID_NEXT_TOKEN",
                "case {case:?}: the reason names the token, got {:?}",
                refused.body
            );
            assert!(
                refused.body.get(listing.items_member).is_none(),
                "case {case:?}: a refused listing reports nothing, got {:?}",
                refused.body
            );
        }
    }
}
