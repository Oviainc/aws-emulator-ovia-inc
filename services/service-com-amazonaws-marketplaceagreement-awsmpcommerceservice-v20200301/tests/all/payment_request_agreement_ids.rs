// SPDX-License-Identifier: CC0-1.0

//! `agreementId` on the operations that act on a PAYMENT REQUEST, end to end
//! over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the validation seam, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test: on an operation whose own resource is a payment request,
//! `agreementId` is an INPUT member that says which agreement the request was
//! sent under rather than a resource the call looks for — "the unique identifier
//! of the agreement associated with the payment request"
//! (`smithy_query(query="operation", operation="GetAgreementPaymentRequest")`) —
//! so a value that resolves to no agreement of the account is refused as a fault
//! in that member: HTTP 400 with the service's modelled `ValidationException`
//! carrying `reason` `INVALID_AGREEMENT_ID`, the sentence that reason states and
//! one `fields` entry naming `agreementId`, and none of the members a missing
//! resource would have carried. `ResourceNotFoundException` stays the answer for
//! the one resource those calls do look for, the payment request itself.
//!
//! The complement is the other half of the rule, and is what keeps it from being
//! "every agreement id is an input fault": an operation whose own resource IS the
//! agreement, and the reads of the OTHER child families, answer the same value
//! with the modelled not-found naming `Agreement`.
//!
//! The rows differ along what the rule generalises over: which operation carries
//! the member (all five operations of the payment-request family, spanning the
//! read, the three decisions and the send, plus the listing that resolves the
//! same member), the FORM of the id within `AgreementId`'s published pattern and
//! both its published length bounds (the forms come from `error_bodies`, which
//! owns that catalog), and — for the complement — which resource the call does
//! look for.
//!
//! No agreement id or payment request id is written by hand where the rule is
//! about a stored one: both are read back from the call that minted them.

use serde_json::{json, Value};

use crate::agreement_lifecycle::{agreement_over, call, emulator, term, Answer};
use crate::error_bodies::{assert_input_fault, unresolvable_agreement_ids};
use crate::payment_requests::{agreement_with_variable_term, sent, PENDING_APPROVAL};

/// The `ValidationExceptionReason` the model publishes for an `agreementId` the
/// service cannot accept (`smithy_query(query="shape",
/// shape="ValidationExceptionReason")`).
const INVALID_AGREEMENT_ID: &str = "INVALID_AGREEMENT_ID";

/// The resource kinds a lookup that found nothing names in `resourceType` —
/// `ResourceType`'s values (`smithy_query(query="shape",
/// shape="ResourceType")`). Spelled out here rather than imported from the crate
/// so a handler naming another kind fails these rows instead of agreeing with
/// them.
const AGREEMENT: &str = "Agreement";
/// See [`AGREEMENT`].
const PAYMENT_REQUEST: &str = "PaymentRequest";

/// The status this service answers a lookup that found nothing with, which the
/// crate's `unknown_ids` suite pins against the recorded AWS responses (the
/// shape itself is modelled `@httpError(404)`, which no recorded response of
/// this service shows).
const NOT_FOUND_STATUS: u16 = 400;

/// The term id every agreement in this suite carries its `VariablePaymentTerm`
/// under — the term a payment request is sent against.
const TERM_ID: &str = "VariablePaymentTerm";

/// A `paymentRequestId` within `^pr-[a-zA-Z0-9]+$` that this emulator's minter
/// never produces, for the rows where the CHILD is what cannot resolve.
const UNKNOWN_PAYMENT_REQUEST: &str = "pr-ffffffffffffffff";

/// Child ids of the other families, each within its own modelled pattern
/// (`^acr-[a-zA-Z0-9]+$`, `^ba-[a-zA-Z0-9]+$`) and none of them minted here.
const UNKNOWN_CANCELLATION_REQUEST: &str = "acr-ffffffffffffffff";
/// See [`UNKNOWN_CANCELLATION_REQUEST`].
const UNKNOWN_BILLING_ADJUSTMENT: &str = "ba-ffffffffffffffff";

/// One call that carries `agreementId` as the input of an operation on a payment
/// request: the operation, and the other members the model makes `@required`
/// (each a value the call accepts, so the only thing a row varies is the
/// agreement it names).
struct PaymentRequestCall {
    operation: &'static str,
    others: Value,
}

impl PaymentRequestCall {
    fn request(&self, agreement: &str) -> Value {
        let mut body = self.others.clone();
        body["agreementId"] = json!(agreement);
        body
    }
}

/// Every operation of this service whose own resource is a payment request, plus
/// the listing that resolves the same member. The required members come from
/// `smithy_query(query="operation", operation="GetAgreementPaymentRequest,
/// AcceptAgreementPaymentRequest,RejectAgreementPaymentRequest,
/// CancelAgreementPaymentRequest,SendAgreementPaymentRequest,
/// ListAgreementPaymentRequests")`.
fn payment_request_calls() -> Vec<PaymentRequestCall> {
    vec![
        PaymentRequestCall {
            operation: "GetAgreementPaymentRequest",
            others: json!({"paymentRequestId": UNKNOWN_PAYMENT_REQUEST}),
        },
        PaymentRequestCall {
            operation: "AcceptAgreementPaymentRequest",
            others: json!({"paymentRequestId": UNKNOWN_PAYMENT_REQUEST}),
        },
        PaymentRequestCall {
            operation: "RejectAgreementPaymentRequest",
            others: json!({
                "paymentRequestId": UNKNOWN_PAYMENT_REQUEST,
                "rejectionReason": "The invoice has already been settled",
            }),
        },
        PaymentRequestCall {
            operation: "CancelAgreementPaymentRequest",
            others: json!({"paymentRequestId": UNKNOWN_PAYMENT_REQUEST}),
        },
        PaymentRequestCall {
            operation: "SendAgreementPaymentRequest",
            others: json!({
                "termId": TERM_ID,
                "name": "Milestone one delivery",
                "chargeAmount": "12.5",
            }),
        },
        PaymentRequestCall {
            operation: "ListAgreementPaymentRequests",
            others: json!({"partyType": "Acceptor"}),
        },
    ]
}

/// One call whose own resource is NOT a payment request, and the resource it
/// looks for when its `agreementId` resolves to nothing.
struct ResourceCall {
    operation: &'static str,
    others: Value,
}

impl ResourceCall {
    fn request(&self, agreement: &str) -> Value {
        let mut body = self.others.clone();
        body["agreementId"] = json!(agreement);
        body
    }
}

/// The complement: the operations whose own resource is the agreement, and the
/// reads of the other child families, which resolve `agreementId` as a resource.
fn resource_calls() -> Vec<ResourceCall> {
    vec![
        ResourceCall {
            operation: "DescribeAgreement",
            others: json!({}),
        },
        ResourceCall {
            operation: "GetAgreementTerms",
            others: json!({}),
        },
        ResourceCall {
            operation: "GetAgreementEntitlements",
            others: json!({}),
        },
        ResourceCall {
            operation: "CancelAgreement",
            others: json!({}),
        },
        ResourceCall {
            operation: "GetAgreementCancellationRequest",
            others: json!({"agreementCancellationRequestId": UNKNOWN_CANCELLATION_REQUEST}),
        },
        ResourceCall {
            operation: "GetBillingAdjustmentRequest",
            others: json!({"billingAdjustmentRequestId": UNKNOWN_BILLING_ADJUSTMENT}),
        },
    ]
}

/// Assert one refusal is the modelled not-found naming `resource_type` and
/// `resource_id`, at the status this service answers a lookup that found nothing
/// with.
fn assert_not_found(case: &str, resource_type: &str, resource_id: &str, answer: &Answer) {
    assert_eq!(
        answer.status, NOT_FOUND_STATUS,
        "case {case:?}: a lookup that found nothing answers HTTP {NOT_FOUND_STATUS}, got {} with \
         {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected a ResourceNotFoundException, got {:?}",
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

/// An `agreementId` that resolves to no agreement is the input fault on that
/// member, on every operation whose own resource is a payment request, for every
/// form the member's pattern admits.
#[tokio::test]
async fn an_unresolvable_agreement_id_is_an_input_fault_on_every_payment_request_call() {
    let router = emulator();
    // A stored agreement with a term a payment request can be sent under, so no
    // refusal below depends on the store being empty.
    let stored = agreement_with_variable_term(&router, "ap-pragreement", TERM_ID).await;

    for under_test in payment_request_calls() {
        for (id_case, id) in unresolvable_agreement_ids() {
            assert_ne!(
                id, stored,
                "case {id_case:?}: the id under test must not be the stored agreement"
            );
            let case = format!("{} over {id_case}", under_test.operation);
            let answer = call(&router, under_test.operation, under_test.request(&id)).await;
            assert_input_fault(&case, INVALID_AGREEMENT_ID, "agreementId", &answer);
        }
    }
}

/// The complement that keeps the rule from swallowing the modelled not-found:
/// the same values, on the calls whose own resource IS the agreement or whose
/// child belongs to another family, are answered as the missing `Agreement`.
#[tokio::test]
async fn an_unresolvable_agreement_id_is_the_missing_agreement_everywhere_else() {
    let router = emulator();
    // A stored agreement, so the store is not merely empty.
    let _stored = agreement_over(&router, "ap-prcomplement", vec![term("eula.v1")]).await;

    for under_test in resource_calls() {
        for (id_case, id) in unresolvable_agreement_ids() {
            let case = format!("{} over {id_case}", under_test.operation);
            let answer = call(&router, under_test.operation, under_test.request(&id)).await;
            assert_not_found(&case, AGREEMENT, &id, &answer);
        }
    }
}

/// The other half of the rule: with an `agreementId` that DOES resolve, the same
/// calls are served or refused about the CHILD they name — never about
/// `agreementId`. So the refusals above come from the resolution of that member
/// and not from the form of the id or from the operation refusing every call.
#[tokio::test]
async fn a_resolvable_agreement_id_is_never_the_member_at_fault() {
    let router = emulator();
    let agreement = agreement_with_variable_term(&router, "ap-prresolves", TERM_ID).await;
    let stored = sent(&router, &agreement, TERM_ID, "Milestone probe", "15").await;

    // The decisions and the read, over a child id this store holds nothing
    // under: the agreement resolved, so the refusal is the missing payment
    // request.
    for under_test in payment_request_calls() {
        let answer = call(
            &router,
            under_test.operation,
            under_test.request(&agreement),
        )
        .await;
        let case = format!("{} over an agreement that exists", under_test.operation);
        match under_test.operation {
            // Both of these name no child at all: the send stores a new request
            // under the agreement, and the listing reports the ones it holds.
            "SendAgreementPaymentRequest" | "ListAgreementPaymentRequests" => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the call must be served, got {} with {:?}",
                    answer.status, answer.body
                );
            }
            _ => assert_not_found(&case, PAYMENT_REQUEST, UNKNOWN_PAYMENT_REQUEST, &answer),
        }
    }

    // And the request those refusals named the parent of was neither decided nor
    // touched: the read reports it exactly as the send stored it.
    let read = call(
        &router,
        "GetAgreementPaymentRequest",
        json!({"agreementId": &agreement, "paymentRequestId": &stored}),
    )
    .await;
    assert_eq!(
        read.status, 200,
        "the stored pair must be served, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.str_at("/status"),
        PENDING_APPROVAL,
        "a refused call must not have decided {stored}, got {:?}",
        read.body
    );
}
