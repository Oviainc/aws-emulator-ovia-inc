// SPDX-License-Identifier: CC0-1.0

//! What this service's error BODIES carry, end to end over HTTP: the modelled
//! identifier every one of them has, which error class an unusable input is
//! reported in, and which resource a nested lookup names when it finds nothing.
//!
//! Three rules, each asserted over a SET of inputs rather than one request:
//!
//! 1. Every error body this service renders carries the modelled `requestId`
//!    ("The unique identifier associated with the error"), which every exception
//!    shape of this service declares — `@length(min: 1, max: 128)`,
//!    `^[A-Za-z0-9-]+$` (`smithy_query(query="shape",
//!    shape="RequestId,ValidationException,ResourceNotFoundException,
//!    ConflictException,ThrottlingException")`) — and two errors never carry the
//!    same one. The rows span the error CLASS, the operation, and where the
//!    refusal was found: the boundary walk (before any handler ran), a handler's
//!    own input check, a lookup that found nothing, and a resource whose state
//!    refuses the call.
//!
//! 2. `SendAgreementPaymentRequest` validates the agreement it is asked to
//!    charge under as INPUT: an `agreementId` it cannot charge under is answered
//!    as the modelled `ValidationException` (HTTP 400) carrying the
//!    `ValidationExceptionReason` value for that member and one `fields` entry
//!    naming it — never as a missing resource. The rows span the FORM of the id
//!    within `AgreementId`'s pattern and both length bounds, the two ways the
//!    member can be unusable (it names no agreement, or the agreement it names
//!    is no longer active), and the operation's other input members, whose
//!    refusals read alike.
//!
//! 3. A lookup of a resource NESTED under another resolves the parent first, so
//!    `ResourceNotFoundException`'s `resourceType`/`resourceId` — the
//!    machine-readable half of the refusal — name the outermost resource that
//!    failed to resolve. The rows span which of the two ids is unresolvable,
//!    both at once, and a child that exists but belongs to another parent.
//!
//! The suites beside this one cover the other halves: `unknown_ids.rs` the
//! not-found refusals of the operations that resolve one resource,
//! `validation_envelope.rs` the modelled envelope over the model's constraints,
//! and `payment_requests.rs` / `billing_adjustments.rs` the successful paths
//! these refusals are the complement of.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, agreement_over, call, cancel, create, emulator, invoiced_agreement, term,
    variable_payment_term, Answer,
};
use crate::billing_adjustments::{
    entry as adjustment_entry, get as get_adjustment, submit as submit_adjustments,
};

/// `ResourceType`'s two values a billing-adjustment lookup can name
/// (`smithy_query(query="shape", shape="ResourceType")`).
const AGREEMENT: &str = "Agreement";
/// See [`AGREEMENT`].
const BILLING_ADJUSTMENT_REQUEST: &str = "BillingAdjustmentRequest";

/// What a `ValidationExceptionField` states about a member that carried a value
/// the service does not accept. Spelled out here rather than imported, so a
/// change of wording in the service fails this test instead of agreeing with
/// it. The recorded AWS responses for this service carry this class
/// unpunctuated, unlike the absent-member class ("Member is required.").
const FIELD_INVALID: &str = "Member is invalid";

/// `RequestId`'s modelled constraints: `@length(min: 1, max: 128)` and
/// `^[A-Za-z0-9-]+$` (`smithy_query(query="shape", shape="RequestId")`),
/// checked here rather than through the crate's own code so a wrong minter
/// cannot make this test pass.
fn is_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

async fn send(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "SendAgreementPaymentRequest", request).await
}

/// A `SendAgreementPaymentRequest` body naming `agreement` and `term`, with
/// every other member a value the operation accepts — so the ONLY thing a row
/// varies is what it means to vary.
fn send_body(agreement: &str, term_id: &str) -> Value {
    json!({
        "agreementId": agreement,
        "termId": term_id,
        "name": "Variable payment for January",
        "chargeAmount": "12.5",
    })
}

/// `AgreementId` forms that satisfy `^[A-Za-z0-9_/-]+$` and
/// `@length(min: 1, max: 64)` — including both published bounds — and that this
/// emulator's id minter never produces, so each names no agreement.
///
/// `pub(crate)` because the suite over the order a paginated read resolves its
/// agreement and its page token in (`tests/all/listing_resolution_order.rs`)
/// spans the same forms rather than keeping a second copy of them.
pub(crate) fn unresolvable_agreement_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest AgreementId, one character", "A".to_string()),
        (
            "a hyphenated, prefix-shaped id",
            "agmt-ffffffffffffffff".to_string(),
        ),
        (
            "a uuid-shaped id, as a client that keeps its own keys uses",
            "7fa92ff8-f1ab-4a21-8241-0845139308ee".to_string(),
        ),
        (
            "the underscore and slash the pattern admits",
            "tenant_7/agreement_1".to_string(),
        ),
        ("mixed case and digits", "AgmT0123456789".to_string()),
        ("the longest AgreementId, 64 characters", "z".repeat(64)),
    ]
}

/// Assert one `ValidationException` body: the status and code the shape models,
/// the `reason` naming the offending member, one `fields` entry at that
/// member's own request path, and none of the members a not-found refusal would
/// have carried instead.
///
/// `pub(crate)` because the suite that spans the inactive-agreement refusal over
/// every operation that requires an active agreement
/// (`tests/all/inactive_agreement_refusals.rs`) asserts the same envelope rather
/// than keeping a second copy of it.
pub(crate) fn assert_input_fault(case: &str, reason: &str, field: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: an input fault is answered at the modelled HTTP 400, got {} with {:?}",
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
        "case {case:?}: the fields entry must name the member at fault, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/fields/0/message"),
        FIELD_INVALID,
        "case {case:?}: the fields entry must state the member is invalid, got {:?}",
        answer.body
    );
    assert!(
        answer.body["resourceType"].is_null() && answer.body["resourceId"].is_null(),
        "case {case:?}: an input fault is not a missing resource, got {:?}",
        answer.body
    );
    assert!(
        is_request_id(&answer.str_at("/requestId")),
        "case {case:?}: expected a modelled requestId, got {:?}",
        answer.body
    );
}

/// Assert one `ResourceNotFoundException` body names `resource_type` and
/// `resource_id`, at the status this service answers a lookup that found
/// nothing with (`crate::state::NOT_FOUND_STATUS`: the recorded responses for
/// this service carry HTTP 400 with that body, not the shape's
/// `@httpError(404)`).
fn assert_not_found(case: &str, resource_type: &str, resource_id: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a lookup that found nothing answers HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceType"),
        resource_type,
        "case {case:?}: expected resourceType {resource_type:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceId"),
        resource_id,
        "case {case:?}: expected resourceId {resource_id:?}, got {:?}",
        answer.body
    );
}

// ---------------------------------------------------------------------------
// Rule 1: every error body carries the modelled requestId
// ---------------------------------------------------------------------------

/// One error body per row, spanning the error classes this service answers and
/// where each refusal was found — all carrying the modelled identifier, and no
/// two carrying the same one.
#[tokio::test]
async fn every_error_body_carries_a_unique_modelled_request_id() {
    let router = emulator();
    // State the rows refer to: an active agreement with a variable payment
    // term, one that has been cancelled, and a stored agreement request that
    // has already been accepted.
    let active = agreement_over(&router, "ap-ids", vec![variable_payment_term("vpt-ids")]).await;
    let cancelled =
        agreement_over(&router, "ap-idsgone", vec![variable_payment_term("vpt-gone")]).await;
    let answer = cancel(&router, &cancelled).await;
    assert_eq!(
        answer.status, 200,
        "cancelling the agreement must succeed, got {:?}",
        answer.body
    );
    let created = create(
        &router,
        json!({
            "intent": "NEW",
            "agreementProposalIdentifier": "ap-idsaccepted",
            "requestedTerms": [term("eula.v1")],
        }),
    )
    .await;
    let accepted_request = created.str_at("/agreementRequestId");
    let accepted = accept(
        &router,
        json!({"agreementRequestId": accepted_request.clone()}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "accepting the request must succeed, got {:?}",
        accepted.body
    );
    // One stored billing adjustment request, so a child lookup can find one.
    // The adjustment is made against an invoice of its own agreement, which is
    // the one the acceptance of a prepaid charge issued.
    let (adjusted, adjusted_invoice) = invoiced_agreement(&router, "ap-idsadjusted").await;
    let submitted = submit_adjustments(
        &router,
        vec![adjustment_entry(
            &adjusted,
            &adjusted_invoice,
            "5",
            "token-ids-1",
        )],
    )
    .await;
    let adjustment = submitted.str_at("/items/0/billingAdjustmentRequestId");
    assert!(
        !adjustment.is_empty(),
        "submitting one adjustment must store it, got {:?}",
        submitted.body
    );

    // (what the row varies, the operation, the request) — every row is refused.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the boundary walk: a member outside its pattern",
            "DescribeAgreement",
            json!({"agreementId": "not a valid id!!"}),
        ),
        (
            "the boundary walk: a required member left out",
            "DescribeAgreement",
            json!({}),
        ),
        (
            "a handler's own input check: a value outside a documented value set",
            "ListAgreementCancellationRequests",
            json!({"partyType": "Bystander"}),
        ),
        (
            "a handler's own input check: an id naming no agreement",
            "SendAgreementPaymentRequest",
            send_body("agmt-nothing-here", "vpt-ids"),
        ),
        (
            "a handler's own input check: an agreement that is not active",
            "SendAgreementPaymentRequest",
            send_body(&cancelled, "vpt-gone"),
        ),
        (
            "a handler's own input check: a token naming no page",
            "SearchAgreements",
            json!({
                "nextToken": "0123456789abcdef00000001",
                "filters": [{"name": "AgreementType", "values": ["PurchaseAgreement"]}],
            }),
        ),
        (
            "a handler's own input check: a filter member naming no agreement",
            "ListBillingAdjustmentRequests",
            json!({"agreementId": "agmt-nothing-here"}),
        ),
        (
            "a lookup that found nothing: the agreement",
            "DescribeAgreement",
            json!({"agreementId": "agmt-nothing-here"}),
        ),
        (
            "a lookup that found nothing: the parent of a nested read",
            "GetBillingAdjustmentRequest",
            json!({
                "agreementId": "agmt-nothing-here",
                "billingAdjustmentRequestId": adjustment.clone(),
            }),
        ),
        (
            "a lookup that found nothing: the child of a nested read",
            "GetBillingAdjustmentRequest",
            json!({"agreementId": active.clone(), "billingAdjustmentRequestId": "ba-nothing"}),
        ),
        (
            "a resource whose state refuses the call",
            "AcceptAgreementRequest",
            json!({"agreementRequestId": accepted_request}),
        ),
    ];

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (case, operation, request) in rows {
        let answer = call(&router, operation, request).await;
        assert!(
            answer.status >= 400,
            "case {case:?} ({operation}): the row must be refused, got {} with {:?}",
            answer.status,
            answer.body
        );
        let request_id = answer.str_at("/requestId");
        assert!(
            is_request_id(&request_id),
            "case {case:?} ({operation}): every error body carries the modelled requestId \
             (1..128 of ^[A-Za-z0-9-]+$), got {:?}",
            answer.body
        );
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?} ({operation}): {request_id:?} was already reported for another \
             error; the identifier is per error, got {:?}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an unusable agreementId on a payment-request send is an input fault
// ---------------------------------------------------------------------------

/// Every form of an `agreementId` this operation cannot charge under is refused
/// as the modelled input fault naming that member — not as a missing resource.
#[tokio::test]
async fn an_agreement_a_payment_request_cannot_be_sent_under_is_an_input_fault() {
    let router = emulator();
    // A stored agreement, so no refusal below depends on the store being empty.
    let stored = agreement_over(&router, "ap-send", vec![variable_payment_term("vpt-send")]).await;

    for (case, id) in unresolvable_agreement_ids() {
        assert_ne!(
            id, stored,
            "case {case:?}: the id under test must not be the stored agreement"
        );
        let answer = send(&router, send_body(&id, "vpt-send")).await;
        assert_input_fault(case, "INVALID_AGREEMENT_ID", "agreementId", &answer);
    }

    // The same member, unusable the other way the operation states ("The
    // agreement must be active"): `ValidationExceptionReason` names that case
    // `INACTIVE_AGREEMENT`, and it is reported in the same class and on the
    // same member.
    let cancelled =
        agreement_over(&router, "ap-sendgone", vec![variable_payment_term("vpt-gone")]).await;
    let answer = cancel(&router, &cancelled).await;
    assert_eq!(
        answer.status, 200,
        "cancelling the agreement must succeed, got {:?}",
        answer.body
    );
    let answer = send(&router, send_body(&cancelled, "vpt-gone")).await;
    assert_input_fault(
        "an agreement that is no longer active",
        "INACTIVE_AGREEMENT",
        "agreementId",
        &answer,
    );
}

/// The operation's other input members read alike: each unusable member is
/// reported with ITS own modelled reason and a `fields` entry naming IT, so the
/// class is keyed to the member at fault rather than to one of them.
#[tokio::test]
async fn each_unusable_input_member_of_a_send_names_itself() {
    let router = emulator();
    let agreement =
        agreement_over(&router, "ap-members", vec![variable_payment_term("vpt-members")]).await;
    // An agreement whose only accepted term is a legal one, so no term of it
    // can carry a payment request.
    let legal_only = agreement_over(&router, "ap-legal", vec![term("LegalTerm")]).await;

    // (what the row varies, the request, the reason, the member it names)
    for (case, request, reason, field) in [
        (
            "an agreementId naming no agreement",
            send_body("agmt-nothing-here", "vpt-members"),
            "INVALID_AGREEMENT_ID",
            "agreementId",
        ),
        (
            "a termId the agreement does not carry",
            send_body(&agreement, "vpt-not-this-one"),
            "INVALID_TERM_ID",
            "termId",
        ),
        (
            "a termId of a term that is not a VariablePaymentTerm",
            send_body(&legal_only, "LegalTerm"),
            "INVALID_TERM_ID",
            "termId",
        ),
        (
            "a chargeAmount that is not positive",
            json!({
                "agreementId": agreement.clone(),
                "termId": "vpt-members",
                "name": "Variable payment for January",
                "chargeAmount": "0",
            }),
            "INVALID_CHARGE_AMOUNT",
            "chargeAmount",
        ),
    ] {
        let answer = send(&router, request).await;
        assert_input_fault(case, reason, field, &answer);
    }

    // The other half of the rule: the same send, with every member usable, is
    // SERVED — so the refusals above come from the member at fault and not from
    // the operation refusing every send. The stored record is read back through
    // the listing, which computes it from state rather than from this input.
    let served = send(&router, send_body(&agreement, "vpt-members")).await;
    assert_eq!(
        served.status, 200,
        "a send whose every member is usable must be served, got {} with {:?}",
        served.status, served.body
    );
    let payment_request_id = served.str_at("/paymentRequestId");
    let listed = call(
        &router,
        "ListAgreementPaymentRequests",
        json!({"partyType": "Proposer"}),
    )
    .await;
    let stored: Vec<String> = listed.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["paymentRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        stored,
        vec![payment_request_id.clone()],
        "the served send must be the one stored payment request, got {:?}",
        listed.body
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a nested lookup names the resource that failed to resolve
// ---------------------------------------------------------------------------

/// A read of a resource nested under another resolves the parent first: the
/// refusal names the outermost resource that failed to resolve, so a caller
/// repairs the id that is actually wrong.
#[tokio::test]
async fn a_nested_lookup_names_the_resource_that_failed_to_resolve() {
    let router = emulator();
    let agreement =
        adjustment_agreement(&router, "ap-nested", "token-nested-1").await;
    let other = adjustment_agreement(&router, "ap-nested2", "token-nested-2").await;

    let stored = get_adjustment(&router, &agreement.0, &agreement.1).await;
    assert_eq!(
        stored.status, 200,
        "the stored request must read back, got {} with {:?}",
        stored.status, stored.body
    );

    for (case, id) in unresolvable_agreement_ids() {
        // Whatever the child id is — one that names a stored request of another
        // agreement, or one that names nothing at all — the parent is what
        // failed to resolve, so the parent is what the refusal names.
        for (child_case, child) in [
            ("a child id that names a stored request", agreement.1.clone()),
            ("a child id that names none", "ba-nothing".to_string()),
        ] {
            let answer = get_adjustment(&router, &id, &child).await;
            assert_not_found(
                &format!("{case}, with {child_case}"),
                AGREEMENT,
                &id,
                &answer,
            );
        }
    }

    // The parent resolves in these rows, so the CHILD is what failed — the same
    // refusal shape, naming the other resource kind and the other id.
    for (case, parent, child) in [
        (
            "a child id that names nothing under a stored agreement",
            agreement.0.clone(),
            "ba-nothing".to_string(),
        ),
        (
            "a child that exists but belongs to another agreement",
            other.0.clone(),
            agreement.1.clone(),
        ),
    ] {
        let answer = get_adjustment(&router, &parent, &child).await;
        assert_not_found(case, BILLING_ADJUSTMENT_REQUEST, &child, &answer);
    }
}

/// One agreement with one stored billing adjustment request against it,
/// answering both ids.
///
/// The adjustment names the invoice the agreement's own acceptance issued: an
/// entry may only adjust an invoice of the agreement it names.
async fn adjustment_agreement(
    router: &MultiServiceRouter,
    proposal: &str,
    token: &str,
) -> (String, String) {
    let (agreement, invoice) = invoiced_agreement(router, proposal).await;
    let submitted =
        submit_adjustments(router, vec![adjustment_entry(&agreement, &invoice, "7", token)]).await;
    let id = submitted.str_at("/items/0/billingAdjustmentRequestId");
    assert!(
        !id.is_empty(),
        "submitting one adjustment must store it, got {:?}",
        submitted.body
    );
    (agreement, id)
}
