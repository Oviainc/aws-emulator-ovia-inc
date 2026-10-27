// SPDX-License-Identifier: CC0-1.0

//! What an `@idempotencyToken` `clientToken` is scoped to, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises
//! routing, deserialization, the handler, the persisted state and serialization
//! — the same path a client takes.
//!
//! The rule under test: a `clientToken` is "a unique, case-sensitive identifier
//! that you provide to ensure the idempotency of the request" — unique within
//! the request it is provided to, not across the service. So ONE token value
//! carried by DIFFERENT idempotent creates names a resource of each, every one
//! of them decides replay-versus-conflict only against the resource IT created
//! under that token, and two token values differing only in case are two
//! tokens.
//!
//! The suite beside this one (`idempotency_token_conflicts`) pins the other
//! half of the same rule: WHICH members decide whether a reuse is a replay or a
//! conflict. Here the content is held fixed and the TOKEN is varied — across
//! the four operations that take one, across the forms `ClientToken` admits,
//! and across its published length bounds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_cancellation_requests::send as send_cancellation;
use super::agreement_lifecycle::{call, create, emulator, invoiced_agreement, upfront_term, Answer};
use super::billing_adjustments::{entry, get as get_adjustment, submit};
use super::payment_requests::{agreement_with_variable_term, get as get_payment, send as send_payment};

/// `ClientToken` is `@length(min: 1, max: 64)` with `@pattern(^[a-zA-Z0-9-]+$)`,
/// from `smithy_query(query="shape", shape="ClientToken")`.
const CLIENT_TOKEN_MIN_LENGTH: usize = 1;
/// See [`CLIENT_TOKEN_MIN_LENGTH`].
const CLIENT_TOKEN_MAX_LENGTH: usize = 64;

/// The `ValidationExceptionReason` value this service names a `clientToken` it
/// cannot accept with, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`.
const INVALID_CLIENT_TOKEN: &str = "INVALID_CLIENT_TOKEN";

/// `ResourceType`'s values for the resources an idempotent create of this
/// service mints, spelled out here rather than imported so a handler naming
/// another one fails this test.
const AGREEMENT_REQUEST: &str = "AgreementRequest";
/// See [`AGREEMENT_REQUEST`].
const AGREEMENT_CANCELLATION_REQUEST: &str = "AgreementCancellationRequest";
/// See [`AGREEMENT_REQUEST`].
const PAYMENT_REQUEST: &str = "PaymentRequest";

/// The per-entry error code `BatchCreateBillingAdjustmentRequest` reports a
/// token reused for other details under, which is that operation's own channel
/// for the conflict the three single-resource creates answer as HTTP 409.
const CONFLICT_EXCEPTION: &str = "CONFLICT_EXCEPTION";

/// The `VariablePaymentTerm` a payment request is sent under.
const VARIABLE_TERM: &str = "vpt-scope";

/// A token value of `len` characters inside `ClientToken`'s pattern.
fn token_of_length(len: usize) -> String {
    "t-0123456789"
        .chars()
        .cycle()
        .take(len)
        .collect::<String>()
}

async fn get_cancellation(router: &MultiServiceRouter, agreement_id: &str, id: &str) -> Answer {
    call(
        router,
        "GetAgreementCancellationRequest",
        json!({"agreementId": agreement_id, "agreementCancellationRequestId": id}),
    )
    .await
}

/// Assert `answer` is HTTP 409 `ConflictException` over exactly `resource_id`.
fn assert_conflict(case: &str, answer: &Answer, resource_type: &str, resource_id: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceType"),
        resource_type,
        "case {case:?}: expected the conflict to name a {resource_type}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceId"),
        resource_id,
        "case {case:?}: expected the conflict to name {resource_id}, got {:?}",
        answer.body
    );
}

/// The four operations of this service that carry a whole request under one
/// `clientToken`, each driven with the SAME token value so a token leaking out
/// of the operation that minted it shows up as a wrong resource, a wrong
/// conflict or a refusal.
struct Fixtures {
    /// The active agreement with a `VariablePaymentTerm` the cancellation and
    /// payment requests are about.
    paying_agreement: String,
    /// An agreement holding an invoice, and that invoice, for the adjustment.
    invoiced_agreement: String,
    invoice: String,
}

async fn fixtures(router: &MultiServiceRouter) -> Fixtures {
    let paying_agreement =
        agreement_with_variable_term(router, "ap-scopepay", VARIABLE_TERM).await;
    let (invoiced, invoice) = invoiced_agreement(router, "ap-scopeinv").await;
    Fixtures {
        paying_agreement,
        invoiced_agreement: invoiced,
        invoice,
    }
}

/// One token value carried by every idempotent create of this service names a
/// resource of EACH of them: the token is scoped to the operation it was
/// provided to, so a replay reports that operation's own resource and a reuse
/// for other content conflicts over that operation's own resource.
///
/// The rows differ along the FORM of the token: its shortest admissible value,
/// its longest, digits only, letters only, upper case, and the hyphenated form
/// an SDK-generated token takes.
#[tokio::test]
async fn one_token_value_names_a_resource_of_every_create() {
    let longest = token_of_length(CLIENT_TOKEN_MAX_LENGTH);
    let shortest = token_of_length(CLIENT_TOKEN_MIN_LENGTH);
    let cases: Vec<(&str, &str)> = vec![
        ("the shortest admissible token", shortest.as_str()),
        ("the longest admissible token", longest.as_str()),
        ("digits only", "20261024132719"),
        ("letters only", "token"),
        ("upper case", "TOKEN"),
        ("the hyphenated form", "7f3a-4b21-9c0d-5e86"),
    ];

    for (case, token) in cases {
        // A router of its own per row, so what each row reads back is what that
        // row's own calls stored.
        let router = emulator();
        let fixtures = fixtures(&router).await;

        let quote_request = quote_request_of(token);
        let cancellation_request = json!({
            "agreementId": &fixtures.paying_agreement,
            "reasonCode": "BUYER_DISSATISFACTION",
            "description": "the buyer asked to cancel",
            "clientToken": token,
        });
        let payment_request = json!({
            "agreementId": &fixtures.paying_agreement,
            "termId": VARIABLE_TERM,
            "name": "Milestone one",
            "chargeAmount": "10",
            "clientToken": token,
        });
        let adjustment_entry = entry(
            &fixtures.invoiced_agreement,
            &fixtures.invoice,
            "1",
            token,
        );

        // Each create succeeds under the token, even though the three before it
        // already used that same value.
        let quote = create(&router, quote_request.clone()).await;
        assert_eq!(
            quote.status, 200,
            "case {case:?}: creating the quote must succeed, got {} with {:?}",
            quote.status, quote.body
        );
        let quote_id = quote.str_at("/agreementRequestId");

        let cancellation = send_cancellation(&router, cancellation_request.clone()).await;
        assert_eq!(
            cancellation.status, 200,
            "case {case:?}: the same token must still send a cancellation request, got {} \
             with {:?}",
            cancellation.status, cancellation.body
        );
        let cancellation_id = cancellation.str_at("/agreementCancellationRequestId");

        let payment = send_payment(&router, payment_request.clone()).await;
        assert_eq!(
            payment.status, 200,
            "case {case:?}: the same token must still send a payment request, got {} with {:?}",
            payment.status, payment.body
        );
        let payment_id = payment.str_at("/paymentRequestId");

        let adjustment = submit(&router, vec![adjustment_entry.clone()]).await;
        assert_eq!(
            adjustment.status, 200,
            "case {case:?}: the same token must still submit an adjustment, got {} with {:?}",
            adjustment.status, adjustment.body
        );
        let adjustment_id = adjustment.str_at("/items/0/billingAdjustmentRequestId");

        // Four resources, not one: no create answered about another's.
        let minted = vec![
            quote_id.clone(),
            cancellation_id.clone(),
            payment_id.clone(),
            adjustment_id.clone(),
        ];
        let mut distinct = minted.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            minted.len(),
            "case {case:?}: one token value must name a resource of each create, got {minted:?}"
        );
        assert!(
            !minted.iter().any(String::is_empty),
            "case {case:?}: every create must answer with the id it minted, got {minted:?}"
        );

        // Each resource reads back through its own reader, so what the token
        // named is persisted rather than only answered.
        let read_cancellation =
            get_cancellation(&router, &fixtures.paying_agreement, &cancellation_id).await;
        assert_eq!(
            read_cancellation.str_at("/agreementCancellationRequestId"),
            cancellation_id,
            "case {case:?}: the cancellation request must read back, got {} with {:?}",
            read_cancellation.status,
            read_cancellation.body
        );
        let read_payment = get_payment(&router, &fixtures.paying_agreement, &payment_id).await;
        assert_eq!(
            read_payment.str_at("/paymentRequestId"),
            payment_id,
            "case {case:?}: the payment request must read back, got {} with {:?}",
            read_payment.status,
            read_payment.body
        );
        let read_adjustment =
            get_adjustment(&router, &fixtures.invoiced_agreement, &adjustment_id).await;
        assert_eq!(
            read_adjustment.str_at("/billingAdjustmentRequestId"),
            adjustment_id,
            "case {case:?}: the adjustment request must read back, got {} with {:?}",
            read_adjustment.status,
            read_adjustment.body
        );

        // A replay of each request under the token reports that operation's own
        // resource — the lookup never crossed into another operation's.
        let replayed_quote = create(&router, quote_request).await;
        assert_eq!(
            replayed_quote.str_at("/agreementRequestId"),
            quote_id,
            "case {case:?}: a replayed create names its own quote, got {} with {:?}",
            replayed_quote.status,
            replayed_quote.body
        );
        let replayed_cancellation = send_cancellation(&router, cancellation_request.clone()).await;
        assert_eq!(
            replayed_cancellation.str_at("/agreementCancellationRequestId"),
            cancellation_id,
            "case {case:?}: a replayed send names its own cancellation request, got {} with {:?}",
            replayed_cancellation.status,
            replayed_cancellation.body
        );
        let replayed_payment = send_payment(&router, payment_request.clone()).await;
        assert_eq!(
            replayed_payment.str_at("/paymentRequestId"),
            payment_id,
            "case {case:?}: a replayed send names its own payment request, got {} with {:?}",
            replayed_payment.status,
            replayed_payment.body
        );
        let replayed_adjustment = submit(&router, vec![adjustment_entry.clone()]).await;
        assert_eq!(
            replayed_adjustment.str_at("/items/0/billingAdjustmentRequestId"),
            adjustment_id,
            "case {case:?}: a replayed submit names its own adjustment, got {} with {:?}",
            replayed_adjustment.status,
            replayed_adjustment.body
        );

        // And a reuse for OTHER content conflicts over that operation's own
        // resource, naming it rather than one of the three resources the same
        // token value named elsewhere.
        let mut other_quote = quote_request_of(token);
        other_quote["requestedTerms"] = json!([upfront_term("cfg", "P12M", &[("Users", 30)])]);
        assert_conflict(
            case,
            &create(&router, other_quote).await,
            AGREEMENT_REQUEST,
            &quote_id,
        );

        let mut other_cancellation = cancellation_request.clone();
        other_cancellation["reasonCode"] = json!("PRODUCT_DISCONTINUED");
        assert_conflict(
            case,
            &send_cancellation(&router, other_cancellation).await,
            AGREEMENT_CANCELLATION_REQUEST,
            &cancellation_id,
        );

        let mut other_payment = payment_request;
        other_payment["chargeAmount"] = json!("20");
        assert_conflict(
            case,
            &send_payment(&router, other_payment).await,
            PAYMENT_REQUEST,
            &payment_id,
        );

        let mut other_entry = adjustment_entry;
        other_entry["adjustmentAmount"] = json!("2");
        let refused_adjustment = submit(&router, vec![other_entry]).await;
        assert_eq!(
            refused_adjustment.body["errors"]
                .as_array()
                .and_then(|errors| errors.first())
                .and_then(|error| error["code"].as_str()),
            Some(CONFLICT_EXCEPTION),
            "case {case:?}: a reused entry token conflicts over its own adjustment, got {:?}",
            refused_adjustment.body
        );
        assert_eq!(
            refused_adjustment.body["errors"]
                .as_array()
                .and_then(|errors| errors.first())
                .and_then(|error| error["clientToken"].as_str()),
            Some(token),
            "case {case:?}: the per-entry error names the entry's own token, got {:?}",
            refused_adjustment.body
        );
    }
}

/// The base `CreateAgreementRequest` body of
/// [`one_token_value_names_a_resource_of_every_create`] under `token`.
fn quote_request_of(token: &str) -> Value {
    json!({
        "intent": "NEW",
        "agreementProposalIdentifier": "ap-scopeoffer",
        "clientToken": token,
        "requestedTerms": [upfront_term("cfg", "P12M", &[("Users", 3)])],
    })
}

/// A `clientToken` is "case-sensitive": two values differing only in case are
/// two tokens, so each names a quote of its own and neither is a replay or a
/// conflict of the other.
///
/// Each row's two tokens ask for DIFFERENT terms, so a token compared without
/// regard to case would be answered as the conflict this asserts does not
/// happen.
#[tokio::test]
async fn a_client_token_is_case_sensitive() {
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a single letter", "a", "A"),
        ("a whole word", "token", "TOKEN"),
        ("one letter of many", "7f3a-4b21-token", "7f3a-4b21-Token"),
        ("mixed against folded", "MiXeDcAsE", "mixedcase"),
    ];

    for (case, lower, upper) in cases {
        let router = emulator();

        let first = quote_request_of(lower);
        let mut second = quote_request_of(upper);
        second["requestedTerms"] = json!([upfront_term("cfg", "P36M", &[("Users", 300)])]);

        let first = create(&router, first).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {} with {:?}",
            first.status, first.body
        );
        let second = create(&router, second).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: a token differing only in case is a token of its own, got {} \
             with {:?}",
            second.status, second.body
        );
        assert_ne!(
            second.str_at("/agreementRequestId"),
            first.str_at("/agreementRequestId"),
            "case {case:?}: each token names a quote of its own, got {:?} and {:?}",
            first.body,
            second.body
        );
        assert_ne!(
            second.body["chargeSummary"], first.body["chargeSummary"],
            "case {case:?}: each quote prices the terms its own call asked for, got {:?} \
             and {:?}",
            first.body, second.body
        );
    }
}

/// One row of the bounds table: a token value and whether `ClientToken` admits
/// it.
struct BoundCase {
    case: &'static str,
    token: String,
    admitted: bool,
}

/// A `clientToken` is accepted at each of `ClientToken`'s published bounds and
/// refused one past them, as the modelled `ValidationException` naming the
/// member — which is also why no caller can spell a token that imitates how the
/// emulator scopes one.
#[tokio::test]
async fn a_client_token_is_bounded_by_its_modelled_constraints() {
    let cases = vec![
        BoundCase {
            case: "one character under the minimum length",
            token: String::new(),
            admitted: false,
        },
        BoundCase {
            case: "at the minimum length",
            token: token_of_length(CLIENT_TOKEN_MIN_LENGTH),
            admitted: true,
        },
        BoundCase {
            case: "at the maximum length",
            token: token_of_length(CLIENT_TOKEN_MAX_LENGTH),
            admitted: true,
        },
        BoundCase {
            case: "one character over the maximum length",
            token: token_of_length(CLIENT_TOKEN_MAX_LENGTH + 1),
            admitted: false,
        },
        BoundCase {
            case: "a character outside the pattern",
            token: "token#1".to_string(),
            admitted: false,
        },
        BoundCase {
            case: "a space inside the pattern's length",
            token: "token 1".to_string(),
            admitted: false,
        },
    ];

    // The two length rows are only about the bound if they really sit on it.
    assert_eq!(
        token_of_length(CLIENT_TOKEN_MAX_LENGTH).len(),
        CLIENT_TOKEN_MAX_LENGTH,
        "the longest-token row must sit on the published bound"
    );

    let router = emulator();
    for case in cases {
        let answer = create(&router, quote_request_of(&case.token)).await;
        if case.admitted {
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected the token to be accepted, got {} with {:?}",
                case.case, answer.status, answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400, got {} with {:?}",
            case.case, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {:?}: expected a ValidationException, got {:?}",
            case.case,
            answer.body
        );
        assert_eq!(
            answer.reason(),
            INVALID_CLIENT_TOKEN,
            "case {:?}: expected the {INVALID_CLIENT_TOKEN} reason, got {:?}",
            case.case,
            answer.body
        );
        assert_eq!(
            answer.str_at("/fields/0/name"),
            "clientToken",
            "case {:?}: the refusal names the member it is about, got {:?}",
            case.case,
            answer.body
        );
    }
}
