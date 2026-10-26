// SPDX-License-Identifier: CC0-1.0

//! The balance a payment request is charged against, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test is the one `SendAgreementPaymentRequest` states about the
//! amount it is asked to charge: it is "a positive decimal value in the currency
//! of the accepted term" and it "must not exceed the remaining available balance
//! under the `VariablePaymentTerm` `maxTotalChargeAmount`", with "a
//! `ValidationException` … if the `chargeAmount` exceeds the available balance".
//! So the term the send names publishes BOTH the currency every report of the
//! request carries and the ceiling the requests sent under it are measured
//! against, and the balance that bounds one send is that ceiling less what the
//! term's earlier requests still stand to charge.
//!
//! Both the ceiling and the currency are read back over the wire from
//! `GetAgreementTerms`, so no constant in this file can stand in for what the
//! accepted term actually publishes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{call, emulator, Answer};
use crate::payment_requests::{agreement_with_variable_term, items, list, send, PENDING_APPROVAL};

/// `PaymentRequestStatus`'s values for the three decisions on a stored request
/// (`smithy_query(query="shape", shape="PaymentRequestStatus")`).
const APPROVED: &str = "APPROVED";
/// See [`APPROVED`].
const REJECTED: &str = "REJECTED";
/// See [`APPROVED`].
const CANCELLED: &str = "CANCELLED";

/// How many fraction digits an amount of this API carries at most, from
/// `PositiveAmountUpto8Decimals`' `^[0-9]*(\.[0-9]{0,8})?$` — so an amount is an
/// exact whole number of these units and the sums below lose nothing.
const FRACTION_DIGITS: usize = 8;

/// Ten to the power of [`FRACTION_DIGITS`]: how many of the smallest unit make
/// one currency unit.
const UNITS: u128 = 100_000_000;

/// `amount` as a count of the smallest unit, computed here rather than through
/// the crate's own parser so a wrong one cannot make this suite pass.
fn units_of(amount: &str) -> u128 {
    let (whole, fraction) = amount.split_once('.').unwrap_or((amount, ""));
    assert!(
        fraction.len() <= FRACTION_DIGITS,
        "{amount:?} carries more than {FRACTION_DIGITS} fraction digits"
    );
    let digits = |part: &str| -> u128 {
        part.bytes().fold(0u128, |total, digit| {
            assert!(digit.is_ascii_digit(), "{amount:?} is not a decimal amount");
            total * 10 + u128::from(digit - b'0')
        })
    };
    let padded = format!("{fraction:0<width$}", width = FRACTION_DIGITS);
    digits(whole) * UNITS + digits(&padded)
}

/// `count` of the smallest unit, spelled as an amount the member's pattern
/// admits: the whole units, a point, and the eight fraction digits.
fn amount_of(count: u128) -> String {
    format!(
        "{whole}.{fraction:0width$}",
        whole = count / UNITS,
        fraction = count % UNITS,
        width = FRACTION_DIGITS
    )
}

/// The accepted `VariablePaymentTerm` of `agreement` that `term_id` names, as
/// `GetAgreementTerms` reports it.
async fn variable_payment_term_of(
    router: &MultiServiceRouter,
    agreement: &str,
    term_id: &str,
) -> Value {
    let answer = call(
        router,
        "GetAgreementTerms",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reporting the terms of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["acceptedTerms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|term| term["variablePaymentTerm"].clone())
        .find(|term| term["id"].as_str() == Some(term_id))
        .unwrap_or_else(|| {
            panic!(
                "{agreement} must carry a variablePaymentTerm {term_id:?}, got {:?}",
                answer.body
            )
        })
}

/// The ceiling that term publishes — `maxTotalChargeAmount`, "the maximum total
/// amount that can be charged to the customer through variable payment requests
/// under this term" — in the smallest unit.
async fn ceiling_of(router: &MultiServiceRouter, agreement: &str, term_id: &str) -> u128 {
    let term = variable_payment_term_of(router, agreement, term_id).await;
    let published = term["maxTotalChargeAmount"].as_str().unwrap_or_else(|| {
        panic!("the accepted term must publish a maxTotalChargeAmount, got {term:?}")
    });
    let ceiling = units_of(published);
    assert!(
        ceiling > 0,
        "a ceiling of {published:?} would admit no payment request at all"
    );
    ceiling
}

/// The currency that term publishes — `currencyCode`, "defines the currency for
/// the prices mentioned in the term".
async fn currency_of(router: &MultiServiceRouter, agreement: &str, term_id: &str) -> String {
    let term = variable_payment_term_of(router, agreement, term_id).await;
    let currency = term["currencyCode"].as_str().unwrap_or_else(|| {
        panic!("the accepted term must publish a currencyCode, got {term:?}")
    });
    // `CurrencyCode` is `@length(min: 3, max: 3)` `^[A-Z]+$`.
    assert!(
        currency.len() == 3 && currency.chars().all(|c| c.is_ascii_uppercase()),
        "{currency:?} must satisfy CurrencyCode's ^[A-Z]+$ and its length of 3"
    );
    currency.to_string()
}

/// Send one payment request of `charge_amount` under `term_id`.
async fn send_amount(
    router: &MultiServiceRouter,
    agreement: &str,
    term_id: &str,
    name: &str,
    charge_amount: &str,
) -> Answer {
    send(
        router,
        json!({
            "agreementId": agreement,
            "termId": term_id,
            "name": name,
            "chargeAmount": charge_amount,
        }),
    )
    .await
}

/// How many payment requests the store holds for `agreement`.
async fn stored(router: &MultiServiceRouter, agreement: &str) -> usize {
    let answer = list(
        router,
        json!({"partyType": "Proposer", "agreementId": agreement, "maxResults": 50}),
    )
    .await;
    items(&answer).len()
}

/// One row of the balance table: the amounts already sent under the term, and
/// the one the row then asks to charge.
struct Row {
    case: &'static str,
    /// Each a count of the smallest unit, sent in order before the row's own
    /// amount.
    prior: Vec<u128>,
    /// The amount the row asks to charge, in the same unit.
    amount: u128,
    /// Whether the remaining balance admits it.
    accepted: bool,
}

/// A send is admitted exactly while the term's ceiling less what its earlier
/// requests still stand to charge covers the amount asked for; anything past
/// that balance is refused with the modelled `ValidationException` naming
/// `chargeAmount`, and stores nothing.
///
/// The rows differ along what the rule generalises over: how much of the balance
/// the earlier requests consumed (none, half, all of it, three eighths spread
/// over three requests), and where the asked-for amount falls relative to what
/// is left — well inside it, exactly on it, one smallest unit past it, and
/// orders of magnitude past it.
#[tokio::test]
async fn a_send_is_bounded_by_the_terms_remaining_balance() {
    let router = emulator();
    let term_id = "vpt-ceiling";
    // The ceiling belongs to the term the offer publishes, so it is read from an
    // agreement of its own before the rows that spend it are built.
    let probe = agreement_with_variable_term(&router, "ap-ceilingprobe", term_id).await;
    let ceiling = ceiling_of(&router, &probe, term_id).await;
    let currency = currency_of(&router, &probe, term_id).await;

    let rows = vec![
        Row {
            case: "one charge well inside the ceiling",
            prior: Vec::new(),
            amount: ceiling / 4,
            accepted: true,
        },
        Row {
            case: "one charge of the smallest positive amount",
            prior: Vec::new(),
            amount: 1,
            accepted: true,
        },
        Row {
            case: "one charge of exactly the ceiling",
            prior: Vec::new(),
            amount: ceiling,
            accepted: true,
        },
        Row {
            case: "one charge a single unit past the ceiling",
            prior: Vec::new(),
            amount: ceiling + 1,
            accepted: false,
        },
        Row {
            case: "one charge a thousand times the ceiling",
            prior: Vec::new(),
            amount: ceiling * 1_000,
            accepted: false,
        },
        Row {
            case: "two charges that together are exactly the ceiling",
            prior: vec![ceiling / 2],
            amount: ceiling - ceiling / 2,
            accepted: true,
        },
        Row {
            case: "a second charge a single unit past what is left",
            prior: vec![ceiling / 2],
            amount: ceiling - ceiling / 2 + 1,
            accepted: false,
        },
        Row {
            case: "the smallest positive charge once the ceiling is consumed",
            prior: vec![ceiling],
            amount: 1,
            accepted: false,
        },
        Row {
            case: "four eighths of the ceiling, sent one at a time",
            prior: vec![ceiling / 8, ceiling / 8, ceiling / 8],
            amount: ceiling / 8,
            accepted: true,
        },
        Row {
            case: "a charge past what three eighths of the ceiling left",
            prior: vec![ceiling / 8, ceiling / 8, ceiling / 8],
            amount: ceiling - 3 * (ceiling / 8) + 1,
            accepted: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // A fresh agreement per row, so each row is about its own balance.
        let agreement =
            agreement_with_variable_term(&router, &format!("ap-ceiling{index}"), term_id).await;
        for (position, prior) in row.prior.iter().enumerate() {
            let answer = send_amount(
                &router,
                &agreement,
                term_id,
                &format!("Earlier charge {position}"),
                &amount_of(*prior),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: the earlier charge {} is inside the balance, got {} with {:?}",
                row.case,
                amount_of(*prior),
                answer.status,
                answer.body
            );
        }

        let asked = amount_of(row.amount);
        let answer = send_amount(
            &router,
            &agreement,
            term_id,
            "The charge under test",
            &asked,
        )
        .await;
        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {:?}: {asked} is inside the remaining balance, got {} with {:?}",
                row.case, answer.status, answer.body
            );
            assert_eq!(
                answer.str_at("/status"),
                PENDING_APPROVAL,
                "case {:?}: an accepted send stores a {PENDING_APPROVAL} request, got {:?}",
                row.case,
                answer.body
            );
            assert_eq!(
                answer.str_at("/chargeAmount"),
                asked,
                "case {:?}: the answer reports the amount asked for, got {:?}",
                row.case,
                answer.body
            );
            // "The currency code for the charge amount" is the accepted term's
            // own, which this row read back from `GetAgreementTerms`.
            assert_eq!(
                answer.body["currencyCode"].as_str(),
                Some(currency.as_str()),
                "case {:?}: the currency is the term's {currency}, got {:?}",
                row.case,
                answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {:?}: {asked} is past the remaining balance, got {} with {:?}",
                row.case, answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {:?}: the refusal is a ValidationException, got {:?}",
                row.case,
                answer.body
            );
            assert_eq!(
                answer.reason(),
                "INVALID_CHARGE_AMOUNT",
                "case {:?}: the refusal names the charge amount, got {:?}",
                row.case,
                answer.body
            );
        }

        // A refused send stores nothing; an accepted one stores exactly one
        // request beside the row's earlier ones.
        let expected = row.prior.len() + usize::from(row.accepted);
        let held = stored(&router, &agreement).await;
        assert_eq!(
            held, expected,
            "case {:?}: the agreement must hold {expected} payment requests, got {held}",
            row.case
        );
    }
}

/// One row of the decision table: what was done to the request holding the whole
/// ceiling, and whether its amount returns to the term's balance.
struct Decision {
    case: &'static str,
    /// The operation that decides the request, or `None` to leave it awaiting
    /// the buyer.
    operation: Option<&'static str>,
    /// The `PaymentRequestStatus` the request then carries.
    status: &'static str,
    /// Whether the balance it consumed is available again.
    releases: bool,
}

/// Only a request that can still be charged holds the term's balance: one
/// awaiting the buyer's decision and one the buyer approved keep it, while a
/// request no charge can follow — rejected by the buyer or cancelled by the
/// seller — releases exactly what it held, so the whole ceiling can be charged
/// again.
///
/// The rows are every decision this service can put a stored request into, and
/// each is checked by asking to charge the WHOLE ceiling afterwards: that
/// succeeds only where the earlier request released it.
#[tokio::test]
async fn only_a_chargeable_request_holds_the_terms_balance() {
    let router = emulator();
    let term_id = "vpt-decided";
    let rows = [
        Decision {
            case: "a request still awaiting the buyer's decision",
            operation: None,
            status: PENDING_APPROVAL,
            releases: false,
        },
        Decision {
            case: "a request the buyer approved",
            operation: Some("AcceptAgreementPaymentRequest"),
            status: APPROVED,
            releases: false,
        },
        Decision {
            case: "a request the buyer rejected",
            operation: Some("RejectAgreementPaymentRequest"),
            status: REJECTED,
            releases: true,
        },
        Decision {
            case: "a request the seller cancelled",
            operation: Some("CancelAgreementPaymentRequest"),
            status: CANCELLED,
            releases: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let agreement =
            agreement_with_variable_term(&router, &format!("ap-decided{index}"), term_id).await;
        let ceiling = ceiling_of(&router, &agreement, term_id).await;
        let whole = amount_of(ceiling);

        let first = send_amount(
            &router,
            &agreement,
            term_id,
            "The whole ceiling",
            &whole,
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: a charge of the whole ceiling must succeed, got {} with {:?}",
            row.case, first.status, first.body
        );
        let payment_request_id = first.str_at("/paymentRequestId");

        if let Some(operation) = row.operation {
            let decided = call(
                &router,
                operation,
                json!({"agreementId": &agreement, "paymentRequestId": &payment_request_id}),
            )
            .await;
            assert_eq!(
                decided.status, 200,
                "case {:?}: {operation} must succeed, got {} with {:?}",
                row.case, decided.status, decided.body
            );
        }

        // The decision is what the store holds, read back through the reader.
        let read = call(
            &router,
            "GetAgreementPaymentRequest",
            json!({"agreementId": &agreement, "paymentRequestId": &payment_request_id}),
        )
        .await;
        assert_eq!(
            read.str_at("/status"),
            row.status,
            "case {:?}: the stored request must be {}, got {:?}",
            row.case,
            row.status,
            read.body
        );

        let again = send_amount(
            &router,
            &agreement,
            term_id,
            "The ceiling once more",
            &whole,
        )
        .await;
        if row.releases {
            assert_eq!(
                again.status, 200,
                "case {:?}: the released balance admits the whole ceiling again, got {} \
                 with {:?}",
                row.case, again.status, again.body
            );
        } else {
            assert_eq!(
                again.status, 400,
                "case {:?}: the held balance admits nothing more, got {} with {:?}",
                row.case, again.status, again.body
            );
            assert_eq!(
                again.reason(),
                "INVALID_CHARGE_AMOUNT",
                "case {:?}: the refusal names the charge amount, got {:?}",
                row.case,
                again.body
            );
        }
    }
}

/// Every report of a payment request carries the currency of the term it was
/// sent under: the send's own answer, `GetAgreementPaymentRequest` and the
/// `PaymentRequestSummary` a listing projects all read it from that one accepted
/// term.
///
/// The rows differ along the `TermId` the agreement carries its variable payment
/// term under — the shortest the pattern admits, one naming the term's kind, one
/// carrying the punctuation `^[A-Za-z0-9+=;,.@\-_]+$` allows, and the longest
/// `@length(min: 1, max: 256)` admits.
#[tokio::test]
async fn every_report_of_a_payment_request_carries_the_terms_currency() {
    let router = emulator();
    let term_ids = [
        ("the shortest TermId the pattern admits", "v".to_string()),
        ("a term id naming the term's kind", "VariablePaymentTerm".to_string()),
        (
            "the punctuation the pattern allows",
            "term+vp=1;a,b.c@d-e_f".to_string(),
        ),
        ("the longest TermId, 256 characters", "t".repeat(256)),
    ];

    for (index, (case, term_id)) in term_ids.iter().enumerate() {
        let agreement =
            agreement_with_variable_term(&router, &format!("ap-currency{index}"), term_id).await;
        let currency = currency_of(&router, &agreement, term_id).await;
        let ceiling = ceiling_of(&router, &agreement, term_id).await;
        // Half the ceiling: an amount the balance admits whatever the published
        // ceiling is.
        let asked = amount_of(ceiling / 2);

        let sent = send_amount(
            &router,
            &agreement,
            term_id,
            "Charge in the term's currency",
            &asked,
        )
        .await;
        assert_eq!(
            sent.status, 200,
            "case {case:?}: sending must succeed, got {} with {:?}",
            sent.status, sent.body
        );
        let payment_request_id = sent.str_at("/paymentRequestId");
        assert_eq!(
            sent.body["currencyCode"].as_str(),
            Some(currency.as_str()),
            "case {case:?}: the send reports the term's {currency}, got {:?}",
            sent.body
        );

        let read = call(
            &router,
            "GetAgreementPaymentRequest",
            json!({"agreementId": &agreement, "paymentRequestId": &payment_request_id}),
        )
        .await;
        assert_eq!(
            read.body["currencyCode"].as_str(),
            Some(currency.as_str()),
            "case {case:?}: the detail reports the term's {currency}, got {:?}",
            read.body
        );

        let listed = list(
            &router,
            json!({"partyType": "Proposer", "agreementId": &agreement}),
        )
        .await;
        let summary = items(&listed)
            .into_iter()
            .find(|item| item["paymentRequestId"].as_str() == Some(payment_request_id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: {payment_request_id} must be listed, got {:?}", listed.body)
            });
        assert_eq!(
            summary["currencyCode"].as_str(),
            Some(currency.as_str()),
            "case {case:?}: the summary reports the term's {currency}, got {summary:?}"
        );
    }
}
