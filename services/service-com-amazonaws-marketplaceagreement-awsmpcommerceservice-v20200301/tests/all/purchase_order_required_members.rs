// SPDX-License-Identifier: CC0-1.0

//! Every member a purchase-order association is MADE OF has to be stated, end to
//! end over HTTP.
//!
//! The rule under test: `PurchaseOrder` is "information about a purchase order
//! association to a charge within an agreement" — the charge it writes to
//! (`chargeId`, "the unique identifier of the charge to associate the purchase
//! order with"), the revision that charge is at (`chargeRevision`, "the revision
//! of the charge") and the reference to record on it
//! (`purchaseOrderReference`, "the purchase order reference to associate with
//! the charge", `smithy_query(query="shape", shape="PurchaseOrder")`). None of
//! the three can be left out: the service publishes a `ValidationExceptionReason`
//! for each of them being absent — `MISSING_CHARGE_ID`,
//! `MISSING_CHARGE_REVISION`, `MISSING_PURCHASE_ORDER_REFERENCE`
//! (`smithy_query(query="shape", shape="ValidationExceptionReason")`) — so an
//! element that omits one is refused with HTTP 400 `ValidationException` under
//! that reason, with `fields[0].name` at the member's own dotted request path,
//! and the list writes NOTHING. The same list with the member stated applies,
//! which is what shows the refusal is of the omission and not of the list.
//!
//! The rows differ in what the rule generalises over: which member is left out,
//! how many elements the list carries, the position of the element at fault, how
//! many charges the agreement holds, and which of the two operations carries the
//! list (`UpdatePurchaseOrders` and `AcceptAgreementRequest$purchaseOrders`).
//! Every charge id comes from the quote that minted it, and the complete element
//! each row is derived from is the one the other purchase-order suites send, so
//! no row can agree with a handler that refuses on something else.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::agreement_lifecycle::{accept, emulator, Answer};
use crate::purchase_order_associations::{order, quote};
use crate::purchase_order_updates::{agreement_holding, references, update};

/// One row: the member one element of the list leaves out, and the refusal that
/// omission earns.
struct Omission {
    case: &'static str,
    /// The member removed from the element at `position`.
    member: &'static str,
    /// The `ValidationExceptionReason` naming that member as absent.
    reason: &'static str,
    /// The sentence that reason states.
    message: &'static str,
    /// How many charges the agreement the row runs over holds.
    charges: usize,
    /// How many elements the list carries, each naming a charge of its own.
    elements: usize,
    /// The 1-based position of the element the member is left out of.
    position: usize,
}

fn omissions() -> Vec<Omission> {
    vec![
        Omission {
            case: "the charge the association writes to",
            member: "chargeId",
            reason: "MISSING_CHARGE_ID",
            message: "Provided charge id is missing.",
            charges: 1,
            elements: 1,
            position: 1,
        },
        Omission {
            case: "the revision that charge is at",
            member: "chargeRevision",
            reason: "MISSING_CHARGE_REVISION",
            message: "Provided charge revision is missing.",
            charges: 1,
            elements: 1,
            position: 1,
        },
        Omission {
            case: "the reference the association records",
            member: "purchaseOrderReference",
            reason: "MISSING_PURCHASE_ORDER_REFERENCE",
            message: "Provided purchase order reference is missing.",
            charges: 2,
            elements: 1,
            position: 1,
        },
        Omission {
            case: "the revision, on the second element of two",
            member: "chargeRevision",
            reason: "MISSING_CHARGE_REVISION",
            message: "Provided charge revision is missing.",
            charges: 2,
            elements: 2,
            position: 2,
        },
        Omission {
            case: "the revision, on the second element of three",
            member: "chargeRevision",
            reason: "MISSING_CHARGE_REVISION",
            message: "Provided charge revision is missing.",
            charges: 3,
            elements: 3,
            position: 2,
        },
        Omission {
            case: "the reference, on the last element of three",
            member: "purchaseOrderReference",
            reason: "MISSING_PURCHASE_ORDER_REFERENCE",
            message: "Provided purchase order reference is missing.",
            charges: 3,
            elements: 3,
            position: 3,
        },
        Omission {
            case: "the charge, on the second element of two",
            member: "chargeId",
            reason: "MISSING_CHARGE_ID",
            message: "Provided charge id is missing.",
            charges: 2,
            elements: 2,
            position: 2,
        },
    ]
}

/// The reference the element at `position` of a row's list associates.
fn reference(position: usize) -> String {
    format!("PO-{position}")
}

/// The list a row sends with every member of every element stated: one
/// association per charge, in the order the quote minted them.
fn complete(charge_ids: &[String], elements: usize) -> Vec<Value> {
    (0..elements)
        .map(|position| order(&charge_ids[position], &reference(position)))
        .collect()
}

/// That same list with `row`'s member left out of the element at its position.
fn with_omission(charge_ids: &[String], row: &Omission) -> Vec<Value> {
    let mut elements = complete(charge_ids, row.elements);
    let at = row.position - 1;
    let element = &mut elements[at];
    let object = element
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {:?}: an association is a JSON object", row.case));
    assert!(
        object.remove(row.member).is_some(),
        "case {:?}: the complete element must state {:?} for the row to leave it out, got {:?}",
        row.case,
        row.member,
        object
    );
    elements
}

/// Assert `answer` is the modelled refusal naming the member `row` left out, on
/// the element it left it out of.
fn assert_refused(operation: &str, answer: &Answer, row: &Omission) {
    let case = row.case;
    let field = format!("purchaseOrders.{}.member.{}", row.position, row.member);
    assert_eq!(
        answer.status, 400,
        "case {case:?} ({operation}): expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?} ({operation}): expected ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.reason(),
        row.reason,
        "case {case:?} ({operation}): expected reason {:?}, got {:?}",
        row.reason,
        answer.body
    );
    assert_eq!(
        answer.str_at("/fields/0/name"),
        field,
        "case {case:?} ({operation}): expected the refusal to name {field:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/message"),
        row.message,
        "case {case:?} ({operation}): expected the message {:?}, got {:?}",
        row.message,
        answer.body
    );
}

/// What the charge report must carry once a row's complete list applied: the
/// reference of each element on the charge it named, and nothing on the rest.
fn associated(charge_ids: &[String], elements: usize) -> BTreeMap<String, Option<String>> {
    let mut expected: BTreeMap<String, Option<String>> = charge_ids
        .iter()
        .map(|charge_id| (charge_id.clone(), None))
        .collect();
    for (position, charge_id) in charge_ids.iter().enumerate().take(elements) {
        expected.insert(charge_id.clone(), Some(reference(position)));
    }
    expected
}

/// Every charge of `charge_ids` carrying no association at all, which is what a
/// refused list must leave behind.
fn unassociated(charge_ids: &[String]) -> BTreeMap<String, Option<String>> {
    charge_ids
        .iter()
        .map(|charge_id| (charge_id.clone(), None))
        .collect()
}

/// `UpdatePurchaseOrders` refuses a list whose element omits any member of the
/// association, associates nothing, and applies that same list once the member
/// is stated.
#[tokio::test]
async fn an_update_states_every_member_of_the_association() {
    for (index, row) in omissions().iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let (agreement, charge_ids) =
            agreement_holding(&router, &format!("ap-offerPOm{index}"), row.charges).await;

        let answered = update(&router, json!(with_omission(&charge_ids, row))).await;
        assert_refused("UpdatePurchaseOrders", &answered, row);

        let reported = references(&router, &agreement).await;
        let nothing = unassociated(&charge_ids);
        assert_eq!(
            reported, nothing,
            "case {case:?}: a refused update must associate nothing, got {reported:?}"
        );

        let applied = update(&router, json!(complete(&charge_ids, row.elements))).await;
        assert_eq!(
            applied.status, 200,
            "case {case:?}: the same list with every member stated must apply, got {} with {:?}",
            applied.status, applied.body
        );
        let reported = references(&router, &agreement).await;
        let expected = associated(&charge_ids, row.elements);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the charge report to carry {expected:?}, got {reported:?}"
        );
    }
}

/// The same rule on the list `AcceptAgreementRequest` carries: an element that
/// omits a member refuses the accept with nothing accepted, and the quote still
/// accepts the list once the member is stated.
#[tokio::test]
async fn an_accepted_list_states_every_member_of_the_association() {
    for (index, row) in omissions().iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let (request_id, charge_ids) =
            quote(&router, &format!("ap-offerPOma{index}"), row.charges).await;

        let answered = accept(
            &router,
            json!({
                "agreementRequestId": request_id,
                "purchaseOrders": with_omission(&charge_ids, row),
            }),
        )
        .await;
        assert_refused("AcceptAgreementRequest", &answered, row);

        // Nothing was accepted: the quote still takes the list it refused, once
        // every member of it is stated.
        let accepted = accept(
            &router,
            json!({
                "agreementRequestId": request_id,
                "purchaseOrders": complete(&charge_ids, row.elements),
            }),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the refused quote must still accept the complete list, got {} with \
             {:?}",
            accepted.status, accepted.body
        );
        let agreement = accepted.str_at("/agreementId");
        let reported = references(&router, &agreement).await;
        let expected = associated(&charge_ids, row.elements);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the accepted agreement to carry {expected:?}, got \
             {reported:?}"
        );
    }
}
