// SPDX-License-Identifier: CC0-1.0

//! An agreement that is no longer active, refused end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: an operation that acts only on an ACTIVE agreement
//! refuses one whose `AgreementStatus` has already reached a terminal value as
//! the modelled INPUT fault on the member that named it — HTTP 400
//! `ValidationException` with `reason` `INACTIVE_AGREEMENT`, the one
//! `ValidationExceptionReason` value the model publishes for that condition, and
//! one `fields` entry naming `agreementId` — rather than as a conflict over the
//! resource, which carries no reason a client could branch on. The refusal does
//! not depend on WHICH operation is called, on WHICH terminal status the
//! agreement reached, or on HOW it got there, and it stores nothing.
//!
//! The rows differ along what the rule generalises over: the operation called,
//! the terminal status the agreement carries (`CANCELLED`, `REPLACED`) and the
//! path that put it there (a direct cancel, an approved cancellation request, a
//! replacement agreement). The last suite is the complement: the same calls on
//! an ACTIVE agreement are served, so the guard refuses the condition rather
//! than the operation.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_cancellation_requests::cancellation_over;
use crate::agreement_lifecycle::{
    accept, agreement_over, call, cancel, create, describe, emulator, term, variable_payment_term,
};
use crate::error_bodies::assert_input_fault;

/// The `VariablePaymentTerm` every agreement here is accepted on, so a payment
/// request sent under it is refused for the agreement's status rather than for a
/// term the agreement does not carry.
const PAYMENT_TERM: &str = "vpt-inactive";

/// The `ValidationExceptionReason` the model publishes for an `agreementId` that
/// names an agreement which is not active, and the member a refusal of it names.
/// Spelled out here rather than imported from the crate, so a handler that
/// reported another reason fails this test instead of agreeing with it.
const INACTIVE_AGREEMENT: &str = "INACTIVE_AGREEMENT";
/// See [`INACTIVE_AGREEMENT`].
const AGREEMENT_ID: &str = "agreementId";

/// How an agreement leaves `ACTIVE`: every path this service has to a terminal
/// `AgreementStatus`.
#[derive(Debug, Clone, Copy)]
enum LeftActive {
    /// The acceptor cancelled it directly (`CancelAgreement`).
    Cancelled,
    /// The buyer approved the seller's cancellation request
    /// (`AcceptAgreementCancellationRequest`), whose approval is what cancels
    /// the agreement.
    CancellationApproved,
    /// A replacement agreement superseded it (`AcceptAgreementRequest` on a
    /// `REPLACE` request naming it as the source).
    Replaced,
}

impl LeftActive {
    /// The `AgreementStatus` this path leaves the agreement in, from
    /// `smithy_query(query="shape", shape="AgreementStatus")`: "CANCELLED – The
    /// agreement was cancelled" and "REPLACED – The agreement was replaced using
    /// an agreement replacement offer".
    fn status(self) -> &'static str {
        match self {
            Self::Cancelled | Self::CancellationApproved => "CANCELLED",
            Self::Replaced => "REPLACED",
        }
    }
}

/// The operations of this service that act only on an active agreement.
///
/// From the operation docs: `CancelAgreement` "allows an acceptor to cancel an
/// active agreement", `SendAgreementCancellationRequest` "allows sellers
/// (proposers) to submit a cancellation request for an active agreement", and
/// `SendAgreementPaymentRequest` states "the agreement must be active".
const NEEDS_AN_ACTIVE_AGREEMENT: [&str; 3] = [
    "CancelAgreement",
    "SendAgreementCancellationRequest",
    "SendAgreementPaymentRequest",
];

/// The request `operation` takes over `agreement_id`, with every OTHER member a
/// value the operation accepts — so the only thing a row varies is the status of
/// the agreement `agreementId` names.
fn request_for(operation: &str, agreement_id: &str) -> Value {
    match operation {
        "CancelAgreement" => json!({"agreementId": agreement_id}),
        "SendAgreementCancellationRequest" => {
            json!({"agreementId": agreement_id, "reasonCode": "OTHER"})
        }
        "SendAgreementPaymentRequest" => json!({
            "agreementId": agreement_id,
            "termId": PAYMENT_TERM,
            "name": "Variable payment for January",
            "chargeAmount": "12.50",
        }),
        other => panic!("{other:?} is not one of the operations under test"),
    }
}

/// One agreement, active, over the terms every row here needs.
async fn active_agreement(router: &MultiServiceRouter, proposal: &str) -> String {
    agreement_over(
        router,
        proposal,
        vec![term("eula.v1"), variable_payment_term(PAYMENT_TERM)],
    )
    .await
}

/// One agreement that reached a terminal status by the path `how` names,
/// asserted to report that status before anything is tested against it.
async fn agreement_that_left_active(
    router: &MultiServiceRouter,
    proposal: &str,
    how: LeftActive,
) -> String {
    let agreement_id = active_agreement(router, proposal).await;
    match how {
        LeftActive::Cancelled => {
            let cancelled = cancel(router, &agreement_id).await;
            assert_eq!(
                cancelled.status, 200,
                "case {how:?}: cancelling the active agreement must succeed, got {} with {:?}",
                cancelled.status, cancelled.body
            );
        }
        LeftActive::CancellationApproved => {
            let request_id = cancellation_over(router, &agreement_id, "OTHER").await;
            let approved = call(
                router,
                "AcceptAgreementCancellationRequest",
                json!({
                    "agreementId": agreement_id,
                    "agreementCancellationRequestId": request_id,
                }),
            )
            .await;
            assert_eq!(
                approved.status, 200,
                "case {how:?}: approving the cancellation request must succeed, got {} with {:?}",
                approved.status, approved.body
            );
        }
        LeftActive::Replaced => {
            let created = create(
                router,
                json!({
                    "intent": "REPLACE",
                    "agreementProposalIdentifier": "ap-replacement",
                    "sourceAgreementIdentifier": agreement_id,
                    "requestedTerms": [term("eula.v2")],
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {how:?}: the replacement request must be created, got {} with {:?}",
                created.status, created.body
            );
            let accepted = accept(
                router,
                json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
            )
            .await;
            assert_eq!(
                accepted.status, 200,
                "case {how:?}: accepting the replacement must succeed, got {} with {:?}",
                accepted.status, accepted.body
            );
        }
    }

    let described = describe(router, &agreement_id).await;
    assert_eq!(
        described.body["status"].as_str(),
        Some(how.status()),
        "case {how:?}: the agreement must report {} before the refusal is tested, got {:?}",
        how.status(),
        described.body
    );
    agreement_id
}

// ---------------------------------------------------------------------------
// The rule: the refusal class, status, reason and member
// ---------------------------------------------------------------------------

/// Every operation that needs an active agreement refuses an inactive one with
/// the SAME modelled input fault, whichever terminal status it carries and
/// whichever path put it there.
#[tokio::test]
async fn an_inactive_agreement_is_refused_as_the_modelled_input_fault() {
    let router = emulator();

    for (proposal, how) in [
        ("ap-direct", LeftActive::Cancelled),
        ("ap-approved", LeftActive::CancellationApproved),
        ("ap-replaced", LeftActive::Replaced),
    ] {
        let agreement_id = agreement_that_left_active(&router, proposal, how).await;
        for operation in NEEDS_AN_ACTIVE_AGREEMENT {
            let answer = call(&router, operation, request_for(operation, &agreement_id)).await;
            let case = format!("{operation} on an agreement that is {} ({how:?})", how.status());
            assert_input_fault(&case, INACTIVE_AGREEMENT, AGREEMENT_ID, &answer);
        }
    }
}

/// A refused call changes nothing: the agreement keeps the terminal status it
/// had, and the refusal is the same the second time — so the class a client
/// branches on does not depend on how many times it asked.
#[tokio::test]
async fn a_refused_call_leaves_the_agreement_as_it_found_it() {
    let router = emulator();

    for (proposal, how) in [
        ("ap-stable1", LeftActive::Cancelled),
        ("ap-stable2", LeftActive::Replaced),
    ] {
        let agreement_id = agreement_that_left_active(&router, proposal, how).await;
        for operation in NEEDS_AN_ACTIVE_AGREEMENT {
            for attempt in 1..=2 {
                let answer = call(&router, operation, request_for(operation, &agreement_id)).await;
                let case = format!("{operation} attempt {attempt} on a {} agreement", how.status());
                assert_input_fault(&case, INACTIVE_AGREEMENT, AGREEMENT_ID, &answer);
            }
        }

        let described = describe(&router, &agreement_id).await;
        assert_eq!(
            described.body["status"].as_str(),
            Some(how.status()),
            "case {how:?}: a refused call must not move the status, got {:?}",
            described.body
        );

        // Nothing was stored for the refused sends either, as seen from EITHER
        // documented party: neither path out of ACTIVE used here creates a
        // cancellation request or a payment request, so every listing is empty.
        for operation in [
            "ListAgreementCancellationRequests",
            "ListAgreementPaymentRequests",
        ] {
            for party_type in ["Acceptor", "Proposer"] {
                let listed = call(&router, operation, json!({"partyType": party_type})).await;
                assert_eq!(
                    listed.status, 200,
                    "case {how:?}: {operation} as {party_type} must succeed, got {} with {:?}",
                    listed.status, listed.body
                );
                assert_eq!(
                    listed.body["items"].as_array().map(Vec::len),
                    Some(0),
                    "case {how:?}: a refused send stores nothing, {operation} as {party_type} \
                     answered {:?}",
                    listed.body
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The complement: an ACTIVE agreement is served
// ---------------------------------------------------------------------------

/// The same calls on an ACTIVE agreement are served, so the refusal above is
/// about the agreement's status rather than about the operation.
#[tokio::test]
async fn an_active_agreement_is_served_by_each_of_those_operations() {
    let router = emulator();

    // One agreement per operation, so each row's call is made on an agreement
    // that is still ACTIVE: `CancelAgreement` is what would take it out.
    for (index, operation) in NEEDS_AN_ACTIVE_AGREEMENT.into_iter().enumerate() {
        let agreement_id = active_agreement(&router, &format!("ap-active{index}")).await;
        let answer = call(&router, operation, request_for(operation, &agreement_id)).await;
        assert_eq!(
            answer.status, 200,
            "case {operation}: an ACTIVE agreement must be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.reason(),
            INACTIVE_AGREEMENT,
            "case {operation}: an ACTIVE agreement is not the inactive refusal, got {:?}",
            answer.body
        );
    }
}
