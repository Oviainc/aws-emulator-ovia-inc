// SPDX-License-Identifier: CC0-1.0

//! Purchase-order associations on `AcceptAgreementRequest`, end to end over HTTP.
//!
//! The rule under test: a purchase-order association must resolve against the
//! charges of the agreement it is being associated with — `PurchaseOrder` is
//! "information about a purchase order association to a charge within an
//! agreement", its `chargeId` is "the unique identifier of the charge to
//! associate the purchase order with" and its `purchaseOrderReference` is "the
//! purchase order reference to associate with the charge"
//! (`smithy_query(query="shape", shape="PurchaseOrder")`). So a supplied list is
//! accepted only when every element names one of the charges the accepted quote
//! minted into `ChargeSummary$expectedCharges`, names it at most once across the
//! list, and carries the reference the association records; otherwise the call
//! is refused with the service's modelled `ValidationException` (HTTP 400)
//! carrying the `ValidationExceptionReason` for the offending member —
//! `INVALID_CHARGE_ID`, `DUPLICATE_CHARGES` or
//! `MISSING_PURCHASE_ORDER_REFERENCE` — and `fields[0].name` at that member's
//! own dotted request path.
//!
//! The rows below differ in what the rule generalises over: how many charges the
//! quote minted (none, one, several), which of them the list names, how many
//! elements the list has, the position of the offending element, and which of
//! the three conditions it misses. The charge ids are never written by hand —
//! they are read back from the quote that minted them, so a test row cannot
//! agree with a handler that invents ids.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, call, create, emulator, expected_charge_ids, new_request, term,
};

/// Term ids naming kinds that are charged, so a quote over `n` of them mints `n`
/// charges. Each is a value of the term catalog `AcceptedTerm` models
/// (`smithy_query(query="shape", shape="AcceptedTerm")`), and
/// `ExpectedCharge$timing` documents when each is charged
/// (`ON_ACCEPTANCE`/`BILLING_PERIOD`/`SCHEDULED`).
const CHARGED_TERMS: &[&str] = &[
    "UsageBasedPricingTerm",
    "RecurringPaymentTerm",
    "PaymentScheduleTerm",
    "FixedUpfrontPricingTerm",
];

/// A term id naming a kind that incurs no charge at all, so a quote over it
/// mints none.
const UNCHARGED_TERM: &str = "eula.v1";

/// Create a quote whose requested terms imply `charges` charges, answering its
/// `agreementRequestId` and the charge ids it minted.
async fn quote(
    router: &MultiServiceRouter,
    proposal: &str,
    charges: usize,
) -> (String, Vec<String>) {
    assert!(
        charges <= CHARGED_TERMS.len(),
        "the charged-term catalog has only {} entries, {charges} asked for",
        CHARGED_TERMS.len()
    );
    let terms: Vec<Value> = if charges == 0 {
        vec![term(UNCHARGED_TERM)]
    } else {
        CHARGED_TERMS.iter().copied().take(charges).map(term).collect()
    };
    let created = create(router, new_request(proposal, terms)).await;
    assert_eq!(
        created.status, 200,
        "creating a quote over {charges} charged terms must succeed, got {} with {:?}",
        created.status, created.body
    );
    let ids = expected_charge_ids(&created);
    assert_eq!(
        ids.len(),
        charges,
        "a quote over {charges} charged terms must mint {charges} charges, got {:?}",
        created.body
    );
    (created.str_at("/agreementRequestId"), ids)
}

/// One purchase order naming `charge_id` and carrying `reference`.
fn order(charge_id: &str, reference: &str) -> Value {
    json!({"chargeId": charge_id, "purchaseOrderReference": reference})
}

/// How many agreements the store holds under this proposal, read through
/// `SearchAgreements` — so "the refused call stored nothing" is checked against
/// the service's own report rather than against the handler's internals.
async fn agreements_for(router: &MultiServiceRouter, proposal: &str) -> usize {
    let found = call(
        router,
        "SearchAgreements",
        json!({"filters": [{"name": "OfferId", "values": [proposal]}]}),
    )
    .await;
    assert_eq!(
        found.status, 200,
        "searching agreements of {proposal} must succeed, got {} with {:?}",
        found.status, found.body
    );
    found.body["agreementViewSummaries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// One row of the refusal table: the quote to accept, the purchase orders to
/// accept it with, and the refusal they earn.
struct Refused {
    case: &'static str,
    /// How many charges the quote mints.
    charges: usize,
    /// The purchase orders, built from the charge ids that quote minted.
    orders: fn(&[String]) -> Value,
    reason: &'static str,
    field: &'static str,
}

fn refusals() -> Vec<Refused> {
    vec![
        Refused {
            case: "a charge id no quote ever minted",
            charges: 1,
            orders: |_| json!([order("ch-1", "PO-77")]),
            reason: "INVALID_CHARGE_ID",
            field: "purchaseOrders.1.member.chargeId",
        },
        Refused {
            case: "a charge id of a quote that implies no charge at all",
            charges: 0,
            orders: |_| json!([order("ch-0000000000000001", "PO-1")]),
            reason: "INVALID_CHARGE_ID",
            field: "purchaseOrders.1.member.chargeId",
        },
        Refused {
            case: "one resolvable charge and one that is not, the bad one second",
            charges: 2,
            orders: |charges| json!([order(&charges[0], "PO-1"), order("ch-not-mine", "PO-2")]),
            reason: "INVALID_CHARGE_ID",
            field: "purchaseOrders.2.member.chargeId",
        },
        Refused {
            case: "the same charge named twice in a row",
            charges: 1,
            orders: |charges| json!([order(&charges[0], "PO-1"), order(&charges[0], "PO-2")]),
            reason: "DUPLICATE_CHARGES",
            field: "purchaseOrders.2.member.chargeId",
        },
        Refused {
            case: "the same charge named again after another charge",
            charges: 3,
            orders: |charges| {
                json!([
                    order(&charges[0], "PO-1"),
                    order(&charges[1], "PO-2"),
                    order(&charges[0], "PO-3"),
                ])
            },
            reason: "DUPLICATE_CHARGES",
            field: "purchaseOrders.3.member.chargeId",
        },
        Refused {
            case: "the last charge named twice, with distinct references",
            charges: 2,
            orders: |charges| json!([order(&charges[1], "PO-a"), order(&charges[1], "PO-b")]),
            reason: "DUPLICATE_CHARGES",
            field: "purchaseOrders.2.member.chargeId",
        },
        Refused {
            case: "an order with nothing to associate",
            charges: 1,
            orders: |charges| json!([{"chargeId": charges[0]}]),
            reason: "MISSING_PURCHASE_ORDER_REFERENCE",
            field: "purchaseOrders.1.member.purchaseOrderReference",
        },
        Refused {
            case: "a second order with nothing to associate",
            charges: 2,
            orders: |charges| {
                json!([order(&charges[0], "PO-1"), {"chargeId": charges[1], "chargeRevision": 2}])
            },
            reason: "MISSING_PURCHASE_ORDER_REFERENCE",
            field: "purchaseOrders.2.member.purchaseOrderReference",
        },
    ]
}

/// Every purchase-order list that does not resolve against the accepted quote's
/// charges is refused in the modelled envelope, and the accept stores nothing.
#[tokio::test]
async fn an_unresolvable_purchase_order_is_refused_and_nothing_is_accepted() {
    for row in refusals() {
        let case = row.case;
        let router = emulator();
        let proposal = "ap-offerPO";
        let (request_id, charges) = quote(&router, proposal, row.charges).await;

        let answer = accept(
            &router,
            json!({
                "agreementRequestId": request_id,
                "purchaseOrders": (row.orders)(&charges),
            }),
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
            row.reason,
            "case {case:?}: expected reason {:?}, got {:?}",
            row.reason,
            answer.body
        );
        assert_eq!(
            answer.str_at("/fields/0/name"),
            row.field,
            "case {case:?}: expected the refusal to name {:?}, got {:?}",
            row.field,
            answer.body
        );
        assert_eq!(
            agreements_for(&router, proposal).await,
            0,
            "case {case:?}: a refused accept must mint no agreement, got {:?}",
            answer.body
        );

        // The quote was not consumed either: accepting it without the
        // unresolvable list still works, which is only true if the refusal ran
        // before anything was written.
        let retried = accept(&router, json!({"agreementRequestId": request_id})).await;
        assert_eq!(
            retried.status, 200,
            "case {case:?}: the refused quote must still be acceptable, got {} with {:?}",
            retried.status, retried.body
        );
    }
}

/// One row of the acceptance table: the quote, and the purchase orders it is
/// accepted with.
struct Accepted {
    case: &'static str,
    charges: usize,
    /// `None` leaves `purchaseOrders` out of the request entirely — the member
    /// is optional on `AcceptAgreementRequest`.
    orders: Option<fn(&[String]) -> Value>,
}

fn acceptances() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "no purchase orders at all, over a quote with no charges",
            charges: 0,
            orders: None,
        },
        Accepted {
            case: "no purchase orders at all, over a quote with charges",
            charges: 2,
            orders: None,
        },
        Accepted {
            case: "the quote's only charge",
            charges: 1,
            orders: Some(|charges| json!([order(&charges[0], "PO-1")])),
        },
        Accepted {
            case: "the last charge of several, and no other",
            charges: 3,
            orders: Some(|charges| json!([order(&charges[2], "PO-last")])),
        },
        Accepted {
            case: "every charge of the quote, each once",
            charges: 4,
            orders: Some(|charges| {
                Value::Array(
                    charges
                        .iter()
                        .enumerate()
                        .map(|(position, id)| order(id, &format!("PO-{position}")))
                        .collect(),
                )
            }),
        },
        Accepted {
            case: "two charges named out of the order the quote minted them in",
            charges: 2,
            orders: Some(|charges| {
                json!([order(&charges[1], "PO-second"), order(&charges[0], "PO-first")])
            }),
        },
        Accepted {
            case: "a charge named with a revision beside its reference",
            charges: 1,
            orders: Some(|charges| {
                json!([{
                    "chargeId": charges[0],
                    "chargeRevision": 1,
                    "purchaseOrderReference": "PO-rev",
                }])
            }),
        },
    ]
}

/// A purchase-order list that resolves against the accepted quote's charges is
/// accepted, and the agreement it creates is the one `DescribeAgreement`
/// reports.
#[tokio::test]
async fn a_resolvable_purchase_order_is_accepted_and_the_agreement_is_described() {
    for row in acceptances() {
        let case = row.case;
        let router = emulator();
        let proposal = "ap-offerPOok";
        let (request_id, charges) = quote(&router, proposal, row.charges).await;

        let mut request = json!({"agreementRequestId": request_id});
        if let Some(orders) = row.orders {
            request["purchaseOrders"] = orders(&charges);
        }
        let accepted = accept(&router, request).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: accepting must succeed, got {} with {:?}",
            accepted.status, accepted.body
        );

        let agreement_id = accepted.str_at("/agreementId");
        let described = call(
            &router,
            "DescribeAgreement",
            json!({"agreementId": agreement_id}),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: describing {agreement_id} must succeed, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_at("/agreementId"),
            agreement_id,
            "case {case:?}: expected the described id to be {agreement_id:?}, got {:?}",
            described.body
        );
        assert_eq!(
            described.str_at("/proposalSummary/offerId"),
            proposal,
            "case {case:?}: expected the agreement to carry proposal {proposal:?}, got {:?}",
            described.body
        );
        assert_eq!(
            described.str_at("/status"),
            "ACTIVE",
            "case {case:?}: an accepted agreement is ACTIVE, got {:?}",
            described.body
        );

        // The quote is spent: a second accept is the documented conflict, which
        // is what proves the accepted list was persisted rather than discarded.
        let again = accept(&router, json!({"agreementRequestId": request_id})).await;
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {case:?}: a second accept of {request_id} is a conflict, got {} with {:?}",
            again.status,
            again.body
        );
    }
}
