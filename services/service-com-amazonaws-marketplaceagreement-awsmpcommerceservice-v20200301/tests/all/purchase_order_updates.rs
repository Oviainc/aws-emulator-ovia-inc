// SPDX-License-Identifier: CC0-1.0

//! `UpdatePurchaseOrders`, and the charge report that reads its work back, end
//! to end over HTTP.
//!
//! The rule under test: `UpdatePurchaseOrders` "allows acceptors to associate
//! purchase orders with agreement charges after an agreement is created"
//! (`smithy_query(query="doc", shape="UpdatePurchaseOrders")`), and one
//! `PurchaseOrder` is "information about a purchase order association to a
//! charge within an agreement" whose `chargeId` is "the unique identifier of the
//! charge to associate the purchase order with" and whose
//! `purchaseOrderReference` is "the purchase order reference to associate with
//! the charge" (`smithy_query(query="shape", shape="PurchaseOrder")`). So each
//! element of the list records its reference ON the charge it names, in the
//! agreement that holds that charge — replacing the reference that charge
//! carried, because a charge carries one — and a list is refused in the
//! service's modelled `ValidationException` (HTTP 400), associating NOTHING,
//! when an element names a charge no agreement holds (`INVALID_CHARGE_ID`),
//! names one twice (`DUPLICATE_CHARGES`), carries no reference
//! (`MISSING_PURCHASE_ORDER_REFERENCE`) or states an `agreementId` that is not
//! the agreement holding the charge (`INVALID_AGREEMENT_ID`), with `fields[0]`
//! naming that element's own member path.
//!
//! The rows differ in what the rule generalises over: how many charges the
//! agreement holds, which of them the list names, how many agreements one call
//! spans, how many calls run, the position of the element at fault, the form of
//! the reference, and whether the element also names its agreement. Every charge
//! id and agreement id is read back from the call that minted it, so no row can
//! agree with a handler that invents ids.
//!
//! `ListAgreementCharges` is the reader these assertions go through: it is what
//! "allows acceptors to view charges and purchase orders that are associated
//! with an agreement", so the association is checked against the service's own
//! report rather than against handler internals. Its scaffold issue is
//! `issues/things-that-do-not-work/20261024T010602Z-marketplace-agreement-listagreementcharges-stub.md`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{accept, call, emulator, is_next_token, Answer};
use crate::purchase_order_associations::{order, quote};

/// The `agreementType` every agreement this service mints carries, and the one
/// `Charge$agreementType` reports ("for example, `PurchaseAgreement`",
/// `smithy_query(query="shape", shape="Charge")`).
const AGREEMENT_TYPE: &str = "PurchaseAgreement";

/// The one catalog AWS Marketplace exposes, which is the catalog every
/// agreement of this service is created in.
const CATALOG: &str = "AWSMarketplace";

/// A charge is minted once and never revised here, so every reported charge is
/// at `ChargeRevision`'s first value (`@range(min: 1)`,
/// `smithy_query(query="shape", shape="ChargeRevision")`).
const INITIAL_REVISION: i64 = 1;

/// Create and accept a quote over `charges` charged terms, answering the
/// agreement id and the ids of the charges that agreement holds, in the order
/// its quote minted them.
async fn agreement_holding(
    router: &MultiServiceRouter,
    proposal: &str,
    charges: usize,
) -> (String, Vec<String>) {
    let (request_id, charge_ids) = quote(router, proposal, charges).await;
    let accepted = accept(router, json!({"agreementRequestId": request_id})).await;
    assert_eq!(
        accepted.status, 200,
        "accepting {request_id} must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    (accepted.str_at("/agreementId"), charge_ids)
}

/// `UpdatePurchaseOrders` over `orders`.
async fn update(router: &MultiServiceRouter, orders: Value) -> Answer {
    call(
        router,
        "UpdatePurchaseOrders",
        json!({"purchaseOrders": orders}),
    )
    .await
}

/// `ListAgreementCharges` with the selectors `request` carries.
async fn charges(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "ListAgreementCharges", request).await
}

/// Every charge the report carries, as a list of its JSON objects.
fn items(answer: &Answer) -> Vec<Value> {
    answer.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a charge report carries an items list, got {:?}", answer.body))
}

/// The purchase order reference each charge of `agreement` carries, keyed by
/// charge id — `None` for a charge with no association, which is the absent
/// member `Charge$purchaseOrderReference` ("if any") states.
async fn references(
    router: &MultiServiceRouter,
    agreement: &str,
) -> BTreeMap<String, Option<String>> {
    let reported = charges(router, json!({"agreementId": agreement})).await;
    assert_eq!(
        reported.status, 200,
        "listing the charges of {agreement} must succeed, got {} with {:?}",
        reported.status, reported.body
    );
    items(&reported)
        .iter()
        .map(|charge| {
            (
                charge["id"].as_str().unwrap_or_default().to_string(),
                charge["purchaseOrderReference"]
                    .as_str()
                    .map(str::to_string),
            )
        })
        .collect()
}

/// One row of the association table: the agreements to set up (one entry per
/// agreement, carrying how many charges it holds), the calls to make over them,
/// and the reference each charge must carry afterwards.
///
/// A call names its orders as `(agreement, charge, reference)` triples over the
/// positions set up, so a row never writes an id by hand.
struct Recorded {
    case: &'static str,
    agreements: &'static [usize],
    calls: &'static [&'static [(usize, usize, &'static str)]],
    expected: &'static [(usize, usize, &'static str)],
    /// Whether every element also states the `agreementId` of the agreement
    /// holding the charge it names — an optional member of `PurchaseOrder`.
    names_agreement: bool,
}

fn recordings() -> Vec<Recorded> {
    vec![
        Recorded {
            case: "the only charge of an agreement",
            agreements: &[1],
            calls: &[&[(0, 0, "PO-1")]],
            expected: &[(0, 0, "PO-1")],
            names_agreement: false,
        },
        Recorded {
            case: "one charge of several, leaving the others unassociated",
            agreements: &[3],
            calls: &[&[(0, 1, "PO-mid")]],
            expected: &[(0, 1, "PO-mid")],
            names_agreement: false,
        },
        Recorded {
            case: "every charge of the agreement in one call",
            agreements: &[4],
            calls: &[&[(0, 0, "PO-a"), (0, 1, "PO-b"), (0, 2, "PO-c"), (0, 3, "PO-d")]],
            expected: &[(0, 0, "PO-a"), (0, 1, "PO-b"), (0, 2, "PO-c"), (0, 3, "PO-d")],
            names_agreement: false,
        },
        Recorded {
            case: "two charges named out of the order the quote minted them in",
            agreements: &[3],
            calls: &[&[(0, 2, "PO-last"), (0, 0, "PO-first")]],
            expected: &[(0, 0, "PO-first"), (0, 2, "PO-last")],
            names_agreement: false,
        },
        Recorded {
            case: "charges of two agreements in one call",
            agreements: &[2, 2],
            calls: &[&[(0, 1, "PO-x"), (1, 0, "PO-y")]],
            expected: &[(0, 1, "PO-x"), (1, 0, "PO-y")],
            names_agreement: false,
        },
        Recorded {
            case: "a second call replaces the reference the charge carried",
            agreements: &[2],
            calls: &[&[(0, 0, "PO-first")], &[(0, 0, "PO-second")]],
            expected: &[(0, 0, "PO-second")],
            names_agreement: false,
        },
        Recorded {
            case: "a second call on another charge leaves the first association",
            agreements: &[2],
            calls: &[&[(0, 0, "PO-kept")], &[(0, 1, "PO-added")]],
            expected: &[(0, 0, "PO-kept"), (0, 1, "PO-added")],
            names_agreement: false,
        },
        Recorded {
            case: "the same charge named twice over two calls, in one of three",
            agreements: &[3],
            calls: &[
                &[(0, 2, "PO-one")],
                &[(0, 2, "PO-two")],
                &[(0, 2, "PO-three")],
            ],
            expected: &[(0, 2, "PO-three")],
            names_agreement: false,
        },
        Recorded {
            // `PurchaseOrderReference` is `@length(min: 1)` with no pattern
            // (`smithy_query(query="shape", shape="PurchaseOrder")`), so the
            // references differ in form as well as in value.
            case: "references differing in form",
            agreements: &[3],
            calls: &[&[(0, 0, "X"), (0, 1, "4500001234"), (0, 2, "po/2026-0001 rev2")]],
            expected: &[(0, 0, "X"), (0, 1, "4500001234"), (0, 2, "po/2026-0001 rev2")],
            names_agreement: false,
        },
        Recorded {
            case: "an element that also names the agreement holding its charge",
            agreements: &[2],
            calls: &[&[(0, 1, "PO-named")]],
            expected: &[(0, 1, "PO-named")],
            names_agreement: true,
        },
        Recorded {
            case: "elements of two agreements, each also naming its agreement",
            agreements: &[1, 3],
            calls: &[&[(1, 0, "PO-b0"), (0, 0, "PO-a0"), (1, 2, "PO-b2")]],
            expected: &[(0, 0, "PO-a0"), (1, 0, "PO-b0"), (1, 2, "PO-b2")],
            names_agreement: true,
        },
    ]
}

/// Every association a call records is the reference the charge it names carries
/// afterwards, and no other charge of that agreement carries one.
#[tokio::test]
async fn an_association_is_recorded_on_the_charge_it_names() {
    for row in recordings() {
        let case = row.case;
        let router = emulator();

        // The agreements the row asks for, each with its own charges.
        let mut agreements: Vec<(String, Vec<String>)> = Vec::new();
        for (position, charges) in row.agreements.iter().enumerate() {
            agreements
                .push(agreement_holding(&router, &format!("ap-offerUP{position}"), *charges).await);
        }

        for (position, orders) in row.calls.iter().enumerate() {
            let body: Vec<Value> = orders
                .iter()
                .map(|(agreement, charge, reference)| {
                    let (agreement_id, charge_ids) = &agreements[*agreement];
                    let mut element = order(&charge_ids[*charge], reference);
                    if row.names_agreement {
                        element["agreementId"] = json!(agreement_id);
                    }
                    element
                })
                .collect();
            let answered = update(&router, Value::Array(body)).await;
            assert_eq!(
                answered.status, 200,
                "case {case:?}: call {position} must succeed, got {} with {:?}",
                answered.status, answered.body
            );
            assert_eq!(
                answered.body,
                json!({}),
                "case {case:?}: call {position} answers the modelled empty output, got {:?}",
                answered.body
            );
        }

        // What the row expects, as the charge-id-keyed map the report answers.
        for (position, (agreement_id, charge_ids)) in agreements.iter().enumerate() {
            let mut expected: BTreeMap<String, Option<String>> = charge_ids
                .iter()
                .map(|charge_id| (charge_id.clone(), None))
                .collect();
            for (agreement, charge, reference) in row.expected {
                if *agreement == position {
                    expected.insert(charge_ids[*charge].clone(), Some((*reference).to_string()));
                }
            }
            let reported = references(&router, agreement_id).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected agreement {position} to report {expected:?}, got \
                 {reported:?}"
            );
        }
    }
}

/// The agreements and charges every refusal row is built over: one agreement
/// with two charges, a second with one, and a quote that was never accepted, so
/// its charge belongs to no agreement.
struct Fixture {
    agreements: Vec<String>,
    charges: Vec<Vec<String>>,
    unaccepted_charge: String,
}

async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let (first, first_charges) = agreement_holding(router, "ap-offerUPa", 2).await;
    let (second, second_charges) = agreement_holding(router, "ap-offerUPb", 1).await;
    let (_, pending) = quote(router, "ap-offerUPc", 1).await;
    Fixture {
        agreements: vec![first, second],
        charges: vec![first_charges, second_charges],
        unaccepted_charge: pending[0].clone(),
    }
}

/// One row of the refusal table: the purchase orders to send over the fixture,
/// and the refusal they earn.
struct Refused {
    case: &'static str,
    orders: fn(&Fixture) -> Value,
    reason: &'static str,
    field: &'static str,
}

fn refusals() -> Vec<Refused> {
    vec![
        Refused {
            case: "a charge id no quote ever minted",
            orders: |_| json!([order("ch-ffffffffffffffff", "PO-1")]),
            reason: "INVALID_CHARGE_ID",
            field: "purchaseOrders.1.member.chargeId",
        },
        Refused {
            // A charge of a quote nobody accepted belongs to no agreement, so
            // there is no agreement charge to associate it with.
            case: "a charge of a quote that was never accepted",
            orders: |fixture| json!([order(&fixture.unaccepted_charge, "PO-1")]),
            reason: "INVALID_CHARGE_ID",
            field: "purchaseOrders.1.member.chargeId",
        },
        Refused {
            case: "a resolvable element before an unresolvable one",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][0], "PO-1"),
                    order("ch-not-mine", "PO-2"),
                ])
            },
            reason: "INVALID_CHARGE_ID",
            field: "purchaseOrders.2.member.chargeId",
        },
        Refused {
            case: "the same charge named twice in a row",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][0], "PO-1"),
                    order(&fixture.charges[0][0], "PO-2"),
                ])
            },
            reason: "DUPLICATE_CHARGES",
            field: "purchaseOrders.2.member.chargeId",
        },
        Refused {
            case: "a charge named again after a charge of another agreement",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][0], "PO-1"),
                    order(&fixture.charges[1][0], "PO-2"),
                    order(&fixture.charges[0][0], "PO-3"),
                ])
            },
            reason: "DUPLICATE_CHARGES",
            field: "purchaseOrders.3.member.chargeId",
        },
        Refused {
            case: "an element with nothing to associate",
            orders: |fixture| json!([{"chargeId": fixture.charges[0][1]}]),
            reason: "MISSING_PURCHASE_ORDER_REFERENCE",
            field: "purchaseOrders.1.member.purchaseOrderReference",
        },
        Refused {
            case: "a later element with nothing to associate",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][0], "PO-1"),
                    {"chargeId": fixture.charges[0][1], "chargeRevision": 2},
                ])
            },
            reason: "MISSING_PURCHASE_ORDER_REFERENCE",
            field: "purchaseOrders.2.member.purchaseOrderReference",
        },
        Refused {
            case: "an element naming an agreement that does not hold its charge",
            orders: |fixture| {
                json!([{
                    "chargeId": fixture.charges[0][0],
                    "agreementId": fixture.agreements[1],
                    "purchaseOrderReference": "PO-1",
                }])
            },
            reason: "INVALID_AGREEMENT_ID",
            field: "purchaseOrders.1.member.agreementId",
        },
        Refused {
            case: "a later element naming an agreement that does not exist",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][0], "PO-1"),
                    {
                        "chargeId": fixture.charges[1][0],
                        "agreementId": "agmt-ffffffffffffffff",
                        "purchaseOrderReference": "PO-2",
                    },
                ])
            },
            reason: "INVALID_AGREEMENT_ID",
            field: "purchaseOrders.2.member.agreementId",
        },
    ]
}

/// Every list that does not resolve against the charges the store holds is
/// refused in the modelled envelope, and associates nothing — not even the
/// elements before the one at fault.
#[tokio::test]
async fn an_unresolvable_update_is_refused_and_associates_nothing() {
    for row in refusals() {
        let case = row.case;
        let router = emulator();
        let fixture = fixture(&router).await;

        let answer = update(&router, (row.orders)(&fixture)).await;

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

        for (position, agreement_id) in fixture.agreements.iter().enumerate() {
            let reported = references(&router, agreement_id).await;
            let unassociated: BTreeMap<String, Option<String>> = fixture.charges[position]
                .iter()
                .map(|charge_id| (charge_id.clone(), None))
                .collect();
            assert_eq!(
                reported, unassociated,
                "case {case:?}: a refused update must associate nothing on agreement \
                 {position}, got {reported:?}"
            );
        }

        // The refusal is of the list, not of the operation: the same charges
        // take an association once the list resolves.
        let retried = update(&router, json!([order(&fixture.charges[0][0], "PO-ok")])).await;
        assert_eq!(
            retried.status, 200,
            "case {case:?}: a resolvable update after the refusal must succeed, got {} with {:?}",
            retried.status, retried.body
        );
    }
}

/// What a charge of an agreement reports, before and after an association, and
/// which charges each selector of the report selects.
#[tokio::test]
async fn the_charge_report_is_computed_from_the_agreement_that_holds_the_charge() {
    let router = emulator();
    let (first, first_charges) = agreement_holding(&router, "ap-offerUPr1", 3).await;
    let (second, second_charges) = agreement_holding(&router, "ap-offerUPr2", 2).await;

    // Every charge of an agreement is reported, with the identity and the
    // agreement its quote minted it under, and no association yet.
    let reported = charges(&router, json!({"agreementId": first})).await;
    assert_eq!(
        reported.status, 200,
        "listing the charges of {first} must succeed, got {} with {:?}",
        reported.status, reported.body
    );
    let listed: Vec<String> = items(&reported)
        .iter()
        .map(|charge| charge["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed, first_charges,
        "expected the charges the quote minted, {first_charges:?}, got {listed:?}"
    );
    for charge in items(&reported) {
        assert_eq!(
            charge["agreementId"].as_str(),
            Some(first.as_str()),
            "expected charge {charge:?} to name agreement {first:?}"
        );
        assert_eq!(
            charge["agreementType"].as_str(),
            Some(AGREEMENT_TYPE),
            "expected charge {charge:?} to name agreement type {AGREEMENT_TYPE:?}"
        );
        assert_eq!(
            charge["revision"].as_i64(),
            Some(INITIAL_REVISION),
            "expected charge {charge:?} to be at revision {INITIAL_REVISION}"
        );
        assert!(
            charge["purchaseOrderReference"].is_null(),
            "expected charge {charge:?} to carry no purchase order reference yet"
        );
    }

    // An association shows up on that charge and on no other, whichever
    // selector the report is asked under.
    let associated = update(&router, json!([order(&first_charges[1], "PO-report")])).await;
    assert_eq!(
        associated.status, 200,
        "associating {:?} must succeed, got {} with {:?}",
        first_charges[1], associated.status, associated.body
    );
    let every = charges(&router, json!({})).await;
    assert_eq!(
        every.status, 200,
        "listing every charge must succeed, got {} with {:?}",
        every.status, every.body
    );
    let references_everywhere: BTreeMap<String, Option<String>> = items(&every)
        .iter()
        .map(|charge| {
            (
                charge["id"].as_str().unwrap_or_default().to_string(),
                charge["purchaseOrderReference"]
                    .as_str()
                    .map(str::to_string),
            )
        })
        .collect();
    let mut expected: BTreeMap<String, Option<String>> = first_charges
        .iter()
        .chain(second_charges.iter())
        .map(|charge_id| (charge_id.clone(), None))
        .collect();
    expected.insert(first_charges[1].clone(), Some("PO-report".to_string()));
    assert_eq!(
        references_everywhere, expected,
        "expected the unfiltered report to carry {expected:?}, got {references_everywhere:?}"
    );

    // Each selector is an AND over the agreement that resulted in the charge.
    struct Selected {
        case: &'static str,
        request: Value,
        expected: usize,
    }
    let selections = vec![
        Selected {
            case: "no selector at all",
            request: json!({}),
            expected: first_charges.len() + second_charges.len(),
        },
        Selected {
            case: "one agreement",
            request: json!({"agreementId": second}),
            expected: second_charges.len(),
        },
        Selected {
            case: "the agreement type every agreement carries",
            request: json!({"agreementType": AGREEMENT_TYPE}),
            expected: first_charges.len() + second_charges.len(),
        },
        Selected {
            // The other value `DescribeAgreementOutput$agreementType`
            // documents, which no agreement of this service carries.
            case: "an agreement type no agreement carries",
            request: json!({"agreementType": "VendorInsightsAgreement"}),
            expected: 0,
        },
        Selected {
            case: "the catalog every agreement is created in",
            request: json!({"catalog": CATALOG}),
            expected: first_charges.len() + second_charges.len(),
        },
        Selected {
            case: "a catalog no agreement is created in",
            request: json!({"catalog": "AWSMarketplace.test"}),
            expected: 0,
        },
        Selected {
            case: "one agreement and the type it carries",
            request: json!({"agreementId": first, "agreementType": AGREEMENT_TYPE}),
            expected: first_charges.len(),
        },
        Selected {
            case: "one agreement and a catalog it is not in",
            request: json!({"agreementId": first, "catalog": "AWSMarketplace.test"}),
            expected: 0,
        },
    ];
    for selection in selections {
        let answer = charges(&router, selection.request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the report must succeed, got {} with {:?}",
            selection.case, answer.status, answer.body
        );
        assert_eq!(
            items(&answer).len(),
            selection.expected,
            "case {:?}: expected {} charges, got {:?}",
            selection.case,
            selection.expected,
            answer.body
        );
    }

    // An `agreementId` the store holds no agreement for is not a selector that
    // matches nothing: it is a value the member does not accept.
    let unknown = charges(&router, json!({"agreementId": "agmt-ffffffffffffffff"})).await;
    assert_eq!(
        unknown.status, 400,
        "an unknown agreement must be refused, got {} with {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.reason(),
        "INVALID_AGREEMENT_ID",
        "expected the INVALID_AGREEMENT_ID reason, got {:?}",
        unknown.body
    );
}

/// The report pages over the charges it selects: every charge once, in one
/// order, and a token only while charges remain.
#[tokio::test]
async fn the_charge_report_pages_over_what_it_selects() {
    let router = emulator();
    let (agreement, minted) = agreement_holding(&router, "ap-offerUPpage", 4).await;

    for page_size in 1..=minted.len() {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"agreementId": agreement, "maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let page = charges(&router, request).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size}: a page must succeed, got {} with {:?}",
                page.status, page.body
            );
            let ids: Vec<String> = items(&page)
                .iter()
                .map(|charge| charge["id"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                ids.len() <= page_size,
                "case page size {page_size}: a page carries at most {page_size} charges, got {:?}",
                page.body
            );
            seen.extend(ids);
            match page.body["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        is_next_token(next),
                        "case page size {page_size}: {next:?} is not a modelled nextToken"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, minted,
            "case page size {page_size}: expected every charge once, in the order the quote \
             minted them, got {seen:?}"
        );
    }

    // A token minted for one listing does not resume another: it is bound to
    // the query it was minted for.
    let first_page = charges(&router, json!({"agreementId": agreement, "maxResults": 1})).await;
    let token = first_page.str_at("/nextToken");
    assert!(
        is_next_token(&token),
        "the first page must carry a token, got {:?}",
        first_page.body
    );
    let foreign = charges(
        &router,
        json!({"agreementType": AGREEMENT_TYPE, "maxResults": 1, "nextToken": token}),
    )
    .await;
    assert_eq!(
        foreign.status, 400,
        "a token of another listing must be refused, got {} with {:?}",
        foreign.status, foreign.body
    );
    assert_eq!(
        foreign.reason(),
        "INVALID_NEXT_TOKEN",
        "expected the INVALID_NEXT_TOKEN reason, got {:?}",
        foreign.body
    );
}
