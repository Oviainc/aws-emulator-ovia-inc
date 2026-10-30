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
//! blind. The rule holds where this service performs the association:
//! `AcceptAgreementRequest$purchaseOrders`. `UpdatePurchaseOrders` performs none
//! (`purchase_order_updates`), so there a list is refused as the unsupported
//! action whatever revision it states — except for a revision the MODEL itself
//! refuses (`ChargeRevision` is `@range(min: 1)`), which the boundary walk
//! reports on that element's own member before any handler runs.
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
use crate::purchase_order_updates::{
    agreement_holding, assert_unsupported_action, charges, items, references, update,
};

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

/// One row of the precondition table: the charges the agreement holds and the
/// elements of the list.
struct Row {
    case: &'static str,
    charges: usize,
    elements: &'static [Element],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no revision stated at all",
            charges: 2,
            elements: &[(0, Stated::Omitted)],
        },
        Row {
            case: "the revision the charge is at",
            charges: 2,
            elements: &[(1, Stated::Reported)],
        },
        Row {
            // `ChargeRevision`'s published minimum, which is the revision a
            // charge nothing has revised is at.
            case: "the lowest revision the model allows",
            charges: 1,
            elements: &[(0, Stated::Absolute(MINIMUM_REVISION))],
        },
        Row {
            case: "one revision past the charge",
            charges: 1,
            elements: &[(0, Stated::OffsetFromReported(1))],
        },
        Row {
            case: "many revisions past the charge",
            charges: 2,
            elements: &[(1, Stated::OffsetFromReported(98))],
        },
        Row {
            case: "the largest revision a long carries",
            charges: 1,
            elements: &[(0, Stated::Absolute(i64::MAX))],
        },
        Row {
            // One past the published bound on the other side: `@range(min: 1)`
            // admits no revision below the minimum.
            case: "one below the lowest revision the model allows",
            charges: 1,
            elements: &[(0, Stated::Absolute(MINIMUM_REVISION - 1))],
        },
        Row {
            case: "a negative revision",
            charges: 2,
            elements: &[(0, Stated::Absolute(-7))],
        },
        Row {
            case: "every element stating the revision its charge is at",
            charges: 3,
            elements: &[(0, Stated::Reported), (1, Stated::Reported), (2, Stated::Reported)],
        },
        Row {
            case: "a wrong revision on the second element of two",
            charges: 2,
            elements: &[(0, Stated::Reported), (1, Stated::OffsetFromReported(1))],
        },
        Row {
            case: "a wrong revision on the first element of two",
            charges: 3,
            elements: &[(0, Stated::Absolute(42)), (2, Stated::Reported)],
        },
        Row {
            case: "a wrong revision on the last element of three",
            charges: 3,
            elements: &[
                (0, Stated::Reported),
                (1, Stated::Reported),
                (2, Stated::OffsetFromReported(3)),
            ],
        },
        Row {
            case: "no revision on the last element of three",
            charges: 3,
            elements: &[(0, Stated::Reported), (1, Stated::Reported), (2, Stated::Omitted)],
        },
        Row {
            case: "no revision on the first element of two",
            charges: 2,
            elements: &[(0, Stated::Omitted), (1, Stated::Reported)],
        },
        Row {
            case: "no revision on the second element of two",
            charges: 3,
            elements: &[(0, Stated::Reported), (2, Stated::Omitted)],
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

/// The 1-based position of the first element whose stated revision the MODEL
/// itself refuses: `ChargeRevision` is `@range(min: 1)`, so a value below that
/// bound is refused on that element's own member by the boundary walk, before
/// any handler runs.
fn below_the_range(orders: &[Value]) -> Option<usize> {
    orders
        .iter()
        .position(|element| {
            element["chargeRevision"]
                .as_i64()
                .is_some_and(|revision| revision < MINIMUM_REVISION)
        })
        .map(|position| position + 1)
}

/// `UpdatePurchaseOrders` associates nothing whatever revision its elements
/// state — the precondition is not what it refuses a list for, so a list the
/// model admits is refused as the unsupported action, and only a revision below
/// `ChargeRevision`'s own bound is reported on the member that states it.
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

        let answered = update(&router, Value::Array(orders.clone())).await;
        match below_the_range(&orders) {
            Some(position) => {
                assert_refused(case, &answered, position, row.elements[position - 1].1)
            }
            None => assert_unsupported_action(case, &answered),
        }

        // Nothing was associated, whichever refusal answered the list.
        let unassociated: BTreeMap<String, Option<String>> = charge_ids
            .iter()
            .map(|charge_id| (charge_id.clone(), None))
            .collect();
        let reported = references(&router, &agreement).await;
        assert_eq!(
            reported, unassociated,
            "case {case:?}: expected the charge report to carry {unassociated:?}, got {reported:?}"
        );
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

/// A charge the approval of a payment request created is reported with a
/// revision of its own, like one an accepted quote minted — and it takes no
/// association from `UpdatePurchaseOrders` whatever revision the list states,
/// because that operation performs none: the reference such a charge carries is
/// the one its approval recorded (`payment_request_approval`).
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
    assert_eq!(
        reported, MINIMUM_REVISION,
        "a charge nothing has revised is at revision {MINIMUM_REVISION}, got {reported}"
    );

    // Nothing of the agreement carries an association, which is what the
    // refusals below must leave untouched.
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

    // The revision the report states, one past it, and none at all: every list
    // the model admits is refused as the unsupported action, and associates
    // nothing.
    for (case, revision) in [
        ("the revision the report states", Some(reported)),
        ("one revision past the charge", Some(reported + 1)),
        ("no revision stated at all", None),
    ] {
        let answered = update(
            &router,
            json!([order_at(&charge_id, "PO-approval", revision)]),
        )
        .await;
        assert_unsupported_action(&format!("a charge created by an approval: {case}"), &answered);
        let after = references(&router, &agreement).await;
        assert_eq!(
            after, unassociated,
            "case {case:?}: a refused update must associate nothing, got {after:?}"
        );
    }
}
