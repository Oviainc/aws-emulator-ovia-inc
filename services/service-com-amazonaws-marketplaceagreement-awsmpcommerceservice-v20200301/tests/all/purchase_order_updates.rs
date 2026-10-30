// SPDX-License-Identifier: CC0-1.0

//! `UpdatePurchaseOrders`, and the charge report that reads back the
//! associations this service does hold, end to end over HTTP.
//!
//! The rule under test: associating purchase orders with the charges of an
//! agreement is not an action this service performs for its caller, so a request
//! the modelled constraints on `purchaseOrders` admit is refused at the REQUEST
//! level — the modelled `ValidationException` (HTTP 400,
//! `smithy_query(query="operation", operation="UpdatePurchaseOrders")`) carrying
//! the `UNSUPPORTED_ACTION` reason `ValidationExceptionReason` publishes
//! (`smithy_query(query="shape", shape="ValidationExceptionReason")`) and the
//! sentence of that class, naming no member of the request. The refusal does not
//! depend on what the list names or on what the store holds: no element is
//! resolved against a stored charge, so no identifier of the caller's is ever
//! reported as unknown, and nothing is associated.
//!
//! What the model itself states about the input is still refused first, on the
//! member at fault, by the boundary walk: `purchaseOrders` is `@required` and
//! `@length(1..86)`, an element's `chargeId` is a `@required` `ResourceId` and
//! its `chargeRevision` is `@range(min: 1)`.
//!
//! The rows differ in what the rule generalises over: how long the list is (one
//! element, a few, and the published maximum), whether its elements name charges
//! the store actually holds, how many agreements they span, which optional
//! members they carry (`chargeRevision`, `agreementId`, `purchaseOrderReference`,
//! and the forms a reference takes), and whether the same charge is named twice.
//!
//! The purchase-order associations this service DOES hold are made where it
//! serves them — `AcceptAgreementRequest$purchaseOrders`, whose own suite is
//! `purchase_order_associations` — and `ListAgreementCharges` is the reader these
//! assertions go through, since it is what "allows acceptors to view charges and
//! purchase orders that are associated with an agreement". Every charge id and
//! agreement id is read back from the call that minted it, so no row can agree
//! with a handler that invents ids.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{accept, call, emulator, is_next_token, Answer};
use crate::purchase_order_associations::{order, quote, INITIAL_CHARGE_REVISION};

/// The `agreementType` every agreement this service mints carries, and the one
/// `Charge$agreementType` reports ("for example, `PurchaseAgreement`",
/// `smithy_query(query="shape", shape="Charge")`).
const AGREEMENT_TYPE: &str = "PurchaseAgreement";

/// The one catalog AWS Marketplace exposes, which is the catalog every
/// agreement of this service is created in.
const CATALOG: &str = "AWSMarketplace";

/// The `ValidationExceptionReason` a request for an action this service does not
/// perform is refused under, and the sentence that class states — spelled out
/// here rather than imported, so a change of wording in the service fails this
/// test instead of agreeing with it.
pub(crate) const UNSUPPORTED_ACTION: &str = "UNSUPPORTED_ACTION";
/// See [`UNSUPPORTED_ACTION`].
pub(crate) const UNSUPPORTED_ACTION_MESSAGE: &str = "This action is not supported.";

/// `PurchaseOrders` is `@length(min: 1, max: 86)`
/// (`smithy_query(query="operation", operation="UpdatePurchaseOrders")`), so a
/// list of this many elements is the longest one the model admits.
const MAX_PURCHASE_ORDERS: usize = 86;

/// Create and accept a quote over `charges` charged terms, answering the
/// agreement id and the ids of the charges that agreement holds, in the order
/// its quote minted them.
///
/// `pub(crate)` because `agreement_charges` reports on the same agreements
/// rather than standing up a second harness.
pub(crate) async fn agreement_holding(
    router: &MultiServiceRouter,
    proposal: &str,
    charges: usize,
) -> (String, Vec<String>) {
    agreement_holding_linked(router, proposal, charges, &[]).await
}

/// The same, with the charges at the positions `links` names carrying the
/// purchase order references it pairs them with.
///
/// The association is recorded by the acceptance itself
/// (`AcceptAgreementRequest$purchaseOrders`), which is where this service serves
/// it: the list resolves against the charges the accepted quote minted.
///
/// `pub(crate)` because the suites that report on an associated charge
/// (`agreement_charges`, and the revision and agreement-id suites) read one back
/// through this one harness.
pub(crate) async fn agreement_holding_linked(
    router: &MultiServiceRouter,
    proposal: &str,
    charges: usize,
    links: &[(usize, &str)],
) -> (String, Vec<String>) {
    let (request_id, charge_ids) = quote(router, proposal, charges).await;
    let mut request = json!({"agreementRequestId": request_id});
    if !links.is_empty() {
        request["purchaseOrders"] = Value::Array(
            links
                .iter()
                .map(|(position, reference)| order(&charge_ids[*position], reference))
                .collect(),
        );
    }
    let accepted = accept(router, request).await;
    assert_eq!(
        accepted.status, 200,
        "accepting {request_id} with the associations {links:?} must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    (accepted.str_at("/agreementId"), charge_ids)
}

/// `UpdatePurchaseOrders` over `orders`.
///
/// `pub(crate)` because `purchase_order_charge_revisions` drives the same
/// operation over the same wire helper rather than a second one of its own.
pub(crate) async fn update(router: &MultiServiceRouter, orders: Value) -> Answer {
    call(
        router,
        "UpdatePurchaseOrders",
        json!({"purchaseOrders": orders}),
    )
    .await
}

/// `ListAgreementCharges` with the selectors `request` carries.
pub(crate) async fn charges(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "ListAgreementCharges", request).await
}

/// Every charge the report carries, as a list of its JSON objects.
pub(crate) fn items(answer: &Answer) -> Vec<Value> {
    answer.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a charge report carries an items list, got {:?}", answer.body))
}

/// The purchase order reference each charge of `agreement` carries, keyed by
/// charge id — `None` for a charge with no association, which is the absent
/// member `Charge$purchaseOrderReference` ("if any") states.
///
/// `pub(crate)` because `purchase_order_charge_revisions` reads the same report
/// back rather than building a second reader.
pub(crate) async fn references(
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

/// Assert one answer is the request-level refusal of the action: the modelled
/// envelope, the published reason, the sentence of that class, and no `fields`
/// entry — the refusal is of the whole request, so there is no member for a
/// caller to repair.
pub(crate) fn assert_unsupported_action(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
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
        UNSUPPORTED_ACTION,
        "case {case:?}: expected the reason {UNSUPPORTED_ACTION}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/message"),
        UNSUPPORTED_ACTION_MESSAGE,
        "case {case:?}: expected the message {UNSUPPORTED_ACTION_MESSAGE:?}, got {:?}",
        answer.body
    );
    assert!(
        answer.body["fields"].is_null(),
        "case {case:?}: a refusal of the action names no member, got {:?}",
        answer.body
    );
}

/// The agreements and charges every row is built over: one agreement with two
/// charges, a second with one, and a quote that was never accepted, so its
/// charge belongs to no agreement.
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

/// One list to send, over the fixture the row is built on.
struct Sent {
    case: &'static str,
    orders: fn(&Fixture) -> Value,
}

/// Lists spanning what the rule generalises over — the ones that WOULD resolve
/// against the stored charges among them, so no row can pass by resolving
/// anything.
fn lists() -> Vec<Sent> {
    vec![
        Sent {
            case: "one element naming a charge the store holds",
            orders: |fixture| json!([order(&fixture.charges[0][0], "PO-1")]),
        },
        Sent {
            case: "every charge of one agreement, each once",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][0], "PO-a"),
                    order(&fixture.charges[0][1], "PO-b"),
                ])
            },
        },
        Sent {
            case: "charges of two agreements in one list",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][1], "PO-x"),
                    order(&fixture.charges[1][0], "PO-y"),
                ])
            },
        },
        Sent {
            case: "an element that also names the agreement holding its charge",
            orders: |fixture| {
                json!([{
                    "chargeId": fixture.charges[0][0],
                    "chargeRevision": INITIAL_CHARGE_REVISION,
                    "agreementId": fixture.agreements[0],
                    "purchaseOrderReference": "PO-named",
                }])
            },
        },
        Sent {
            case: "an element naming an agreement that does not hold its charge",
            orders: |fixture| {
                json!([{
                    "chargeId": fixture.charges[0][0],
                    "chargeRevision": INITIAL_CHARGE_REVISION,
                    "agreementId": fixture.agreements[1],
                    "purchaseOrderReference": "PO-wrong",
                }])
            },
        },
        Sent {
            // `PurchaseOrderReference` is `@length(min: 1)` with no pattern
            // (`smithy_query(query="shape", shape="PurchaseOrder")`), so the
            // references differ in form as well as in value.
            case: "references differing in form",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][0], "X"),
                    order(&fixture.charges[0][1], "4500001234"),
                    order(&fixture.charges[1][0], "po/2026-0001 rev2"),
                ])
            },
        },
        Sent {
            case: "an element carrying no reference at all",
            orders: |fixture| json!([{"chargeId": fixture.charges[0][0]}]),
        },
        Sent {
            case: "an element stating a revision the charge is not at",
            orders: |fixture| {
                json!([{
                    "chargeId": fixture.charges[0][0],
                    "chargeRevision": INITIAL_CHARGE_REVISION + 7,
                    "purchaseOrderReference": "PO-rev",
                }])
            },
        },
        Sent {
            case: "the same charge named twice in one list",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][0], "PO-1"),
                    order(&fixture.charges[0][0], "PO-2"),
                ])
            },
        },
        Sent {
            case: "a charge id no quote ever minted",
            orders: |_| json!([order("ch-ffffffffffffffff", "PO-1")]),
        },
        Sent {
            // A charge of a quote nobody accepted belongs to no agreement.
            case: "a charge of a quote that was never accepted",
            orders: |fixture| json!([order(&fixture.unaccepted_charge, "PO-1")]),
        },
        Sent {
            case: "a resolvable element before an unresolvable one",
            orders: |fixture| {
                json!([
                    order(&fixture.charges[0][0], "PO-1"),
                    order("ch-not-mine", "PO-2"),
                ])
            },
        },
        Sent {
            case: "the longest list the model admits, naming no stored charge",
            orders: |_| {
                Value::Array(
                    (0..MAX_PURCHASE_ORDERS)
                        .map(|position| order(&format!("ch-{position:016x}"), "PO-bulk"))
                        .collect(),
                )
            },
        },
        Sent {
            case: "the longest list the model admits, opening on stored charges",
            orders: |fixture| {
                let mut orders: Vec<Value> = fixture
                    .charges
                    .iter()
                    .flatten()
                    .map(|charge| order(charge, "PO-held"))
                    .collect();
                while orders.len() < MAX_PURCHASE_ORDERS {
                    orders.push(order(&format!("ch-{:016x}", orders.len()), "PO-rest"));
                }
                Value::Array(orders)
            },
        },
    ]
}

/// Every list the modelled constraints admit is refused as the unsupported
/// action, and nothing in the store changes: the charges the fixture holds carry
/// no reference before the call and none after it, and a later call is refused
/// the same way rather than acting on anything the refused one left behind.
#[tokio::test]
async fn an_update_is_refused_as_the_unsupported_action_whatever_its_list_names() {
    for row in lists() {
        let case = row.case;
        let router = emulator();
        let fixture = fixture(&router).await;

        // What the store holds before the call: every charge of every agreement
        // the fixture accepted, none of them associated.
        let unassociated: Vec<BTreeMap<String, Option<String>>> = fixture
            .charges
            .iter()
            .map(|ids| ids.iter().map(|id| (id.clone(), None)).collect())
            .collect();

        for attempt in 0..2 {
            let answer = update(&router, (row.orders)(&fixture)).await;
            assert_unsupported_action(&format!("{case} (attempt {attempt})"), &answer);

            for (position, agreement_id) in fixture.agreements.iter().enumerate() {
                let reported = references(&router, agreement_id).await;
                assert_eq!(
                    reported, unassociated[position],
                    "case {case:?} (attempt {attempt}): a refused update associates nothing on \
                     agreement {position}, expected {:?}, got {reported:?}",
                    unassociated[position]
                );
            }
        }
    }
}

/// One request whose input the MODEL itself refuses, and the member the refusal
/// names.
struct Refused {
    case: &'static str,
    request: Value,
    reason: &'static str,
    field: &'static str,
}

/// The modelled constraints on the list and its elements — `purchaseOrders`
/// `@required` and `@length(1..86)`, an element's `chargeId` a `@required`
/// `ResourceId` (`@length(1..64)`, `@pattern(^[A-Za-z0-9_/-]+$)`) and its
/// `chargeRevision` `@range(min: 1)`.
fn malformed() -> Vec<Refused> {
    let one = json!({"chargeId": "ch-0000000000000001", "purchaseOrderReference": "PO-1"});
    vec![
        Refused {
            case: "no list at all",
            request: json!({}),
            reason: "MISSING_PURCHASE_ORDERS",
            field: "purchaseOrders",
        },
        Refused {
            case: "a list below its minimum length",
            request: json!({"purchaseOrders": []}),
            reason: "INVALID_PURCHASE_ORDERS",
            field: "purchaseOrders",
        },
        Refused {
            case: "a list one past its maximum length",
            request: json!({
                "purchaseOrders": vec![one.clone(); MAX_PURCHASE_ORDERS + 1],
            }),
            reason: "INVALID_PURCHASE_ORDERS",
            field: "purchaseOrders",
        },
        Refused {
            case: "an element missing the one member the model requires",
            request: json!({"purchaseOrders": [{"purchaseOrderReference": "PO-1"}]}),
            reason: "MISSING_CHARGE_ID",
            field: "purchaseOrders.1.member.chargeId",
        },
        Refused {
            case: "a later element whose charge id is outside its pattern",
            request: json!({
                "purchaseOrders": [one.clone(), {"chargeId": "not a charge id!!"}],
            }),
            reason: "INVALID_CHARGE_ID",
            field: "purchaseOrders.2.member.chargeId",
        },
        Refused {
            case: "an element whose revision is below its range",
            request: json!({
                "purchaseOrders": [{"chargeId": "ch-0000000000000001", "chargeRevision": 0}],
            }),
            reason: "INVALID_CHARGE_REVISION",
            field: "purchaseOrders.1.member.chargeRevision",
        },
    ]
}

/// A request the model's own constraints refuse is answered about the MEMBER at
/// fault, before the request-level refusal of the action: the caller is told
/// what it can repair when there is something to repair, and told the action is
/// unavailable only when there is not.
#[tokio::test]
async fn the_modelled_constraints_on_the_list_are_refused_on_their_own_member() {
    let router = emulator();
    for row in malformed() {
        let case = row.case;
        let answer = call(&router, "UpdatePurchaseOrders", row.request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
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
            row.reason,
            "case {case:?}: expected the reason {:?}, got {:?}",
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
    }

    // The other half of the rule: the same list with every modelled constraint
    // satisfied is refused as the action rather than about a member.
    let answer = update(
        &router,
        json!([{"chargeId": "ch-0000000000000001", "chargeRevision": 1}]),
    )
    .await;
    assert_unsupported_action("a list the modelled constraints admit", &answer);
}

/// What a charge of an agreement reports, and which charges each selector of the
/// report selects.
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
            Some(INITIAL_CHARGE_REVISION),
            "expected charge {charge:?} to be at revision {INITIAL_CHARGE_REVISION}"
        );
        assert!(
            charge["purchaseOrderReference"].is_null(),
            "expected charge {charge:?} to carry no purchase order reference yet"
        );
    }

    // An association made where this service serves it shows up on that charge
    // and on no other, whichever selector the report is asked under.
    let (third, third_charges) =
        agreement_holding_linked(&router, "ap-offerUPr3", 2, &[(1, "PO-report")]).await;
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
        .chain(third_charges.iter())
        .map(|charge_id| (charge_id.clone(), None))
        .collect();
    expected.insert(third_charges[1].clone(), Some("PO-report".to_string()));
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
    let held = first_charges.len() + second_charges.len() + third_charges.len();
    let selections = vec![
        Selected {
            case: "no selector at all",
            request: json!({}),
            expected: held,
        },
        Selected {
            case: "one agreement",
            request: json!({"agreementId": second}),
            expected: second_charges.len(),
        },
        Selected {
            case: "the agreement holding the associated charge",
            request: json!({"agreementId": third}),
            expected: third_charges.len(),
        },
        Selected {
            case: "the agreement type every agreement carries",
            request: json!({"agreementType": AGREEMENT_TYPE}),
            expected: held,
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
            expected: held,
        },
        Selected {
            case: "one agreement and the type it carries",
            request: json!({"agreementId": first, "agreementType": AGREEMENT_TYPE}),
            expected: first_charges.len(),
        },
        Selected {
            case: "one agreement and an agreement type it does not carry",
            request: json!({"agreementId": first, "agreementType": "VendorInsightsAgreement"}),
            expected: 0,
        },
        Selected {
            // The selector names a resource rather than a value set, so a
            // well-formed id the store holds no agreement for selects nothing —
            // `unmatched_selector_pages` is the suite about that rule.
            case: "an agreement id the store holds no agreement for",
            request: json!({"agreementId": "agmt-ffffffffffffffff"}),
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
