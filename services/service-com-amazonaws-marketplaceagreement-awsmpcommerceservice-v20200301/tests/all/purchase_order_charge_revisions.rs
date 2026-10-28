// SPDX-License-Identifier: CC0-1.0

//! `PurchaseOrder$chargeRevision` as the precondition it is, end to end over
//! HTTP.
//!
//! The rule under test: a member that restates the current revision of the
//! resource a call writes is the caller's precondition on that resource, so an
//! association whose `chargeRevision` ("the revision of the charge",
//! `smithy_query(query="shape", shape="PurchaseOrder")`) is not the revision the
//! charge it names is at (`Charge$revision`, "the revision number of the
//! charge", `smithy_query(query="shape", shape="Charge")`) is refused in the
//! service's modelled `ValidationException` (HTTP 400) under the
//! `ValidationExceptionReason` that names the member
//! (`INVALID_CHARGE_REVISION`), writing nothing — and an association that states
//! no revision at all states no precondition, so it is refused under the reason
//! for that member being absent (`MISSING_CHARGE_REVISION`) rather than applied
//! blind. The rule holds on both operations the member reaches the store
//! through: `UpdatePurchaseOrders` and `AcceptAgreementRequest$purchaseOrders`.
//!
//! The rows differ in what the rule generalises over: whether the member is
//! stated at all, how far the stated revision is from the charge's own (one
//! revision, many, `ChargeRevision`'s published minimum, below it, negative, the
//! largest value a `long` carries), how many charges the agreement holds, which
//! of them the list names, the position of the element at fault, how the charge
//! came into existence (minted by the accepted quote, or created by an approved
//! payment request), and which operation carries the list.
//!
//! Every revision a row states is computed from the revision
//! `ListAgreementCharges` reports for the charge it names, never written by
//! hand, so no row can agree with a handler that compares against a revision of
//! its own invention.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{accept, call, emulator, Answer};
use crate::payment_requests::{agreement_with_variable_term, get, sent};
use crate::purchase_order_associations::{order_at, quote};
use crate::purchase_order_updates::{agreement_holding, charges, items, references, update};

/// The `ValidationExceptionReason` the model publishes for a revision the charge
/// is not at (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
const INVALID_CHARGE_REVISION: &str = "INVALID_CHARGE_REVISION";

/// The reason that model publishes for the same member not being stated at all.
const MISSING_CHARGE_REVISION: &str = "MISSING_CHARGE_REVISION";

/// The sentence this service's `ValidationException` states for that reason,
/// which names the member the request got wrong.
const INVALID_CHARGE_REVISION_MESSAGE: &str = "Provided charge revision is invalid.";

/// The sentence stated for the member being absent.
const MISSING_CHARGE_REVISION_MESSAGE: &str = "Provided charge revision is missing.";

/// `ChargeRevision` is `@range(min: 1)`
/// (`smithy_query(query="shape", shape="ChargeRevision")`), so this is the
/// lowest revision any charge can be at and the published bound a row states
/// and steps past.
const MINIMUM_REVISION: i64 = 1;

/// The term id the payment-request rows send their requests under.
const VARIABLE_TERM: &str = "VariablePaymentTerm";

/// The revision `ListAgreementCharges` reports for `charge_id` — the value an
/// association's `chargeRevision` has to agree with, read from the service's own
/// report rather than assumed.
async fn reported_revision(
    router: &MultiServiceRouter,
    agreement: &str,
    charge_id: &str,
) -> i64 {
    let reported = charges(router, json!({"agreementId": agreement})).await;
    assert_eq!(
        reported.status, 200,
        "listing the charges of {agreement} must succeed, got {} with {:?}",
        reported.status, reported.body
    );
    items(&reported)
        .iter()
        .find(|charge| charge["id"].as_str() == Some(charge_id))
        .and_then(|charge| charge["revision"].as_i64())
        .unwrap_or_else(|| {
            panic!("charge {charge_id} of {agreement} must report a revision, got {reported:?}",
                reported = reported.body)
        })
}

/// What an element of a row states as its `chargeRevision`.
#[derive(Clone, Copy, Debug)]
enum Stated {
    /// The member is left out of the element entirely.
    Omitted,
    /// The revision the charge report states for the charge the element names.
    Reported,
    /// That reported revision, this many revisions away from it.
    OffsetFromReported(i64),
    /// A value stated outright, whatever revision the charge is at.
    Absolute(i64),
}

/// The value an element states, against the `reported` revision of the charge it
/// names.
fn stated(revision: Stated, reported: i64) -> Option<i64> {
    match revision {
        Stated::Omitted => None,
        Stated::Reported => Some(reported),
        Stated::OffsetFromReported(offset) => Some(reported + offset),
        Stated::Absolute(value) => Some(value),
    }
}

/// The `reason` and `message` an element stating `revision` is refused under: a
/// member left out is the MISSING_ reason, a member stating a revision the
/// charge is not at the INVALID_ one.
fn refusal_of(revision: Stated) -> (&'static str, &'static str) {
    match revision {
        Stated::Omitted => (MISSING_CHARGE_REVISION, MISSING_CHARGE_REVISION_MESSAGE),
        _ => (INVALID_CHARGE_REVISION, INVALID_CHARGE_REVISION_MESSAGE),
    }
}

/// One element of a row's list: the position of the charge it names among the
/// agreement's charges, and the revision it states about it.
type Element = (usize, Stated);

/// One row of the precondition table: the charges the agreement holds, the
/// elements of the list, and the 1-based position of the element the call must
/// be refused for — `None` when the whole list is applied.
struct Row {
    case: &'static str,
    charges: usize,
    elements: &'static [Element],
    refused_at: Option<usize>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no revision stated at all",
            charges: 2,
            elements: &[(0, Stated::Omitted)],
            refused_at: Some(1),
        },
        Row {
            case: "the revision the charge is at",
            charges: 2,
            elements: &[(1, Stated::Reported)],
            refused_at: None,
        },
        Row {
            // `ChargeRevision`'s published minimum, which is the revision a
            // charge nothing has revised is at.
            case: "the lowest revision the model allows",
            charges: 1,
            elements: &[(0, Stated::Absolute(MINIMUM_REVISION))],
            refused_at: None,
        },
        Row {
            case: "one revision past the charge",
            charges: 1,
            elements: &[(0, Stated::OffsetFromReported(1))],
            refused_at: Some(1),
        },
        Row {
            case: "many revisions past the charge",
            charges: 2,
            elements: &[(1, Stated::OffsetFromReported(98))],
            refused_at: Some(1),
        },
        Row {
            case: "the largest revision a long carries",
            charges: 1,
            elements: &[(0, Stated::Absolute(i64::MAX))],
            refused_at: Some(1),
        },
        Row {
            // One past the published bound on the other side: `@range(min: 1)`
            // admits no revision below the minimum.
            case: "one below the lowest revision the model allows",
            charges: 1,
            elements: &[(0, Stated::Absolute(MINIMUM_REVISION - 1))],
            refused_at: Some(1),
        },
        Row {
            case: "a negative revision",
            charges: 2,
            elements: &[(0, Stated::Absolute(-7))],
            refused_at: Some(1),
        },
        Row {
            case: "every element stating the revision its charge is at",
            charges: 3,
            elements: &[(0, Stated::Reported), (1, Stated::Reported), (2, Stated::Reported)],
            refused_at: None,
        },
        Row {
            case: "a wrong revision on the second element of two",
            charges: 2,
            elements: &[(0, Stated::Reported), (1, Stated::OffsetFromReported(1))],
            refused_at: Some(2),
        },
        Row {
            case: "a wrong revision on the first element of two",
            charges: 3,
            elements: &[(0, Stated::Absolute(42)), (2, Stated::Reported)],
            refused_at: Some(1),
        },
        Row {
            case: "a wrong revision on the last element of three",
            charges: 3,
            elements: &[
                (0, Stated::Reported),
                (1, Stated::Reported),
                (2, Stated::OffsetFromReported(3)),
            ],
            refused_at: Some(3),
        },
        Row {
            case: "no revision on the last element of three",
            charges: 3,
            elements: &[(0, Stated::Reported), (1, Stated::Reported), (2, Stated::Omitted)],
            refused_at: Some(3),
        },
        Row {
            case: "no revision on the first element of two",
            charges: 2,
            elements: &[(0, Stated::Omitted), (1, Stated::Reported)],
            refused_at: Some(1),
        },
        Row {
            case: "no revision on the second element of two",
            charges: 3,
            elements: &[(0, Stated::Reported), (2, Stated::Omitted)],
            refused_at: Some(2),
        },
    ]
}

/// The reference the element at `position` of a row's list associates.
fn reference(position: usize) -> String {
    format!("PO-{position}")
}

/// Assert `answer` is the modelled refusal of the `chargeRevision` of the
/// `position`-th element of the request's list, under the reason `stated_as`
/// earns.
fn assert_refused(case: &str, answer: &Answer, position: usize, stated_as: Stated) {
    let field = format!("purchaseOrders.{position}.member.chargeRevision");
    let (reason, message) = refusal_of(stated_as);
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
        reason,
        "case {case:?}: expected reason {reason:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/fields/0/name"),
        field,
        "case {case:?}: expected the refusal to name {field:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/message"),
        message,
        "case {case:?}: expected the message {message:?}, got {:?}",
        answer.body
    );
}

/// `UpdatePurchaseOrders` applies a list exactly when every element's stated
/// revision is the revision its charge is at, and a list with one element at
/// another revision is refused whole, associating nothing.
#[tokio::test]
async fn an_update_states_the_revision_the_charge_is_at() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let (agreement, charge_ids) =
            agreement_holding(&router, &format!("ap-offerCR{index}"), row.charges).await;

        let mut orders: Vec<Value> = Vec::new();
        for (position, (charge, revision)) in row.elements.iter().enumerate() {
            let charge_id = &charge_ids[*charge];
            let reported = reported_revision(&router, &agreement, charge_id).await;
            orders.push(order_at(
                charge_id,
                &reference(position),
                stated(*revision, reported),
            ));
        }

        let answered = update(&router, Value::Array(orders)).await;

        // What the row's elements asked for, as the charge-keyed report states
        // it: every association when the list resolved, none at all when it was
        // refused.
        let mut expected: BTreeMap<String, Option<String>> = charge_ids
            .iter()
            .map(|charge_id| (charge_id.clone(), None))
            .collect();
        match row.refused_at {
            None => {
                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with {:?}",
                    answered.status, answered.body
                );
                for (position, (charge, _)) in row.elements.iter().enumerate() {
                    expected.insert(charge_ids[*charge].clone(), Some(reference(position)));
                }
            }
            Some(position) => {
                assert_refused(case, &answered, position, row.elements[position - 1].1)
            }
        }
        let reported = references(&router, &agreement).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the charge report to carry {expected:?}, got {reported:?}"
        );

        // The refusal is of the stated revision, not of the operation: the same
        // list applies once every element states the revision its charge is at.
        if row.refused_at.is_some() {
            let corrected: Vec<Value> = {
                let mut corrected = Vec::new();
                for (position, (charge, _)) in row.elements.iter().enumerate() {
                    let charge_id = &charge_ids[*charge];
                    let reported = reported_revision(&router, &agreement, charge_id).await;
                    corrected.push(order_at(charge_id, &reference(position), Some(reported)));
                }
                corrected
            };
            let retried = update(&router, Value::Array(corrected)).await;
            assert_eq!(
                retried.status, 200,
                "case {case:?}: the same list at the charges' own revisions must apply, got {} \
                 with {:?}",
                retried.status, retried.body
            );
        }
    }
}

/// The same precondition on the list `AcceptAgreementRequest` carries: the
/// charges of the agreement being accepted are at the revision the report states
/// for them afterwards, and a list naming any other revision is refused with
/// nothing accepted.
#[tokio::test]
async fn an_accepted_list_states_the_revision_its_charges_are_at() {
    // The revision a charge an accepted quote mints is at, read from the report
    // of one such agreement rather than assumed — the value the rows below are
    // computed from.
    let router = emulator();
    let (probe, probe_charges) = agreement_holding(&router, "ap-offerCRprobe", 1).await;
    let minted_at = reported_revision(&router, &probe, &probe_charges[0]).await;
    assert_eq!(
        minted_at, MINIMUM_REVISION,
        "a charge nothing has revised is at revision {MINIMUM_REVISION}, got {minted_at}"
    );

    struct Accepted {
        case: &'static str,
        charges: usize,
        /// One element per purchase order, naming a charge of the quote and the
        /// revision it states about it.
        elements: &'static [Element],
        refused_at: Option<usize>,
    }
    let acceptances = [
        Accepted {
            case: "no revision stated at all",
            charges: 1,
            elements: &[(0, Stated::Omitted)],
            refused_at: Some(1),
        },
        Accepted {
            case: "no revision on the second element of two",
            charges: 2,
            elements: &[(0, Stated::Reported), (1, Stated::Omitted)],
            refused_at: Some(2),
        },
        Accepted {
            case: "the revision the quote's charges are minted at",
            charges: 2,
            elements: &[(0, Stated::Reported), (1, Stated::Reported)],
            refused_at: None,
        },
        Accepted {
            case: "one revision past the charge",
            charges: 1,
            elements: &[(0, Stated::OffsetFromReported(1))],
            refused_at: Some(1),
        },
        Accepted {
            case: "a wrong revision on the second element of two",
            charges: 3,
            elements: &[(0, Stated::Reported), (2, Stated::Absolute(77))],
            refused_at: Some(2),
        },
        Accepted {
            case: "below the lowest revision the model allows",
            charges: 1,
            elements: &[(0, Stated::Absolute(MINIMUM_REVISION - 1))],
            refused_at: Some(1),
        },
    ];

    for (index, row) in acceptances.iter().enumerate() {
        // Each row accepts its own quote, so the rows share the probe above and
        // nothing else.
        let case = row.case;
        let (request_id, charge_ids) =
            quote(&router, &format!("ap-offerCRa{index}"), row.charges).await;
        let orders: Vec<Value> = row
            .elements
            .iter()
            .enumerate()
            .map(|(position, (charge, revision))| {
                order_at(
                    &charge_ids[*charge],
                    &reference(position),
                    stated(*revision, minted_at),
                )
            })
            .collect();

        let answered = accept(
            &router,
            json!({"agreementRequestId": request_id, "purchaseOrders": orders}),
        )
        .await;

        match row.refused_at {
            None => {
                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: accepting must succeed, got {} with {:?}",
                    answered.status, answered.body
                );
                let agreement = answered.str_at("/agreementId");
                let mut expected: BTreeMap<String, Option<String>> = charge_ids
                    .iter()
                    .map(|charge_id| (charge_id.clone(), None))
                    .collect();
                for (position, (charge, _)) in row.elements.iter().enumerate() {
                    expected.insert(charge_ids[*charge].clone(), Some(reference(position)));
                }
                let reported = references(&router, &agreement).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the accepted agreement to carry {expected:?}, got \
                     {reported:?}"
                );
            }
            Some(position) => {
                assert_refused(case, &answered, position, row.elements[position - 1].1);
                // Nothing was accepted: the quote is still acceptable, which is
                // only true if the refusal ran before anything was written.
                let retried = accept(&router, json!({"agreementRequestId": request_id})).await;
                assert_eq!(
                    retried.status, 200,
                    "case {case:?}: the refused quote must still be acceptable, got {} with {:?}",
                    retried.status, retried.body
                );
            }
        }
    }
}

/// The precondition holds for a charge the approval of a payment request
/// created, not only for one an accepted quote minted: the revision checked is
/// the one the report states for THAT charge.
#[tokio::test]
async fn the_revision_of_a_charge_an_approval_created_is_the_one_checked() {
    let router = emulator();
    let agreement =
        agreement_with_variable_term(&router, "ap-offerCRpr", VARIABLE_TERM).await;
    let payment_request = sent(&router, &agreement, VARIABLE_TERM, "charge-revision", "120").await;
    let approved = call(
        &router,
        "AcceptAgreementPaymentRequest",
        json!({"agreementId": agreement, "paymentRequestId": payment_request}),
    )
    .await;
    assert_eq!(
        approved.status, 200,
        "approving {payment_request} must succeed, got {} with {:?}",
        approved.status, approved.body
    );
    // The charge the approval created, read from the record the approval moved.
    let charge_id = get(&router, &agreement, &payment_request).await.str_at("/chargeId");
    let reported = reported_revision(&router, &agreement, &charge_id).await;

    // Nothing of the agreement carries an association yet, which is what the
    // refusal below must leave untouched.
    let unassociated = references(&router, &agreement).await;
    assert!(
        unassociated.contains_key(&charge_id),
        "expected the charge report to carry the approved charge {charge_id}, got \
         {unassociated:?}"
    );
    assert!(
        unassociated.values().all(Option::is_none),
        "expected no charge of {agreement} to carry an association yet, got {unassociated:?}"
    );

    // A revision the charge is not at is refused and associates nothing, and so
    // is an element that states none at all.
    let refused = update(
        &router,
        json!([order_at(&charge_id, "PO-approval", Some(reported + 1))]),
    )
    .await;
    assert_refused(
        "a charge created by an approval",
        &refused,
        1,
        Stated::OffsetFromReported(1),
    );
    let unstated = update(&router, json!([order_at(&charge_id, "PO-approval", None)])).await;
    assert_refused(
        "a charge created by an approval, with no revision stated",
        &unstated,
        1,
        Stated::Omitted,
    );
    let after_refusal = references(&router, &agreement).await;
    assert_eq!(
        after_refusal, unassociated,
        "a refused update must associate nothing, got {after_refusal:?}"
    );

    // The revision the report states applies.
    let applied = update(
        &router,
        json!([order_at(&charge_id, "PO-approval", Some(reported))]),
    )
    .await;
    assert_eq!(
        applied.status, 200,
        "an update at the charge's own revision must succeed, got {} with {:?}",
        applied.status, applied.body
    );
    let mut expected = unassociated.clone();
    expected.insert(charge_id.clone(), Some("PO-approval".to_string()));
    let associated = references(&router, &agreement).await;
    assert_eq!(
        associated, expected,
        "expected the association to be recorded on the approved charge, {expected:?}, got \
         {associated:?}"
    );
}
