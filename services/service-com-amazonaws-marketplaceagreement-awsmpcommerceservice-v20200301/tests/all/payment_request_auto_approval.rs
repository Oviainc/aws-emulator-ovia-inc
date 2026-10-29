// SPDX-License-Identifier: CC0-1.0

//! A payment request the CLOCK approves, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: the acceptor's configuration of the
//! `VariablePaymentTerm` a payment request is sent under decides who approves
//! that request, and a configuration that approves on expiration makes the
//! approval the CLOCK's — `expirationDuration` is "the duration after which a
//! payment request is automatically approved if no further action is taken. This
//! only applies when the payment request approval strategy is set to
//! `AUTO_APPROVE_ON_EXPIRATION`" (`smithy_query(query="shape",
//! shape="VariablePaymentTermConfiguration")`). So once that duration has
//! elapsed from the send with no decision taken, the request IS approved — with
//! the charge an approval creates, reported by every reader of the request and
//! by the charge report of its agreement — and no later decision can move it;
//! until it has, and under the strategy that waits for the buyer, the request
//! stays `PENDING_APPROVAL` and only the buyer's own decision moves it.
//!
//! The transition is dated at the instant it was DUE rather than at the instant
//! somebody looked, and reading is not what causes it.
//!
//! The rows differ along what the rule generalises over: which strategy the
//! acceptor configured, the form of the duration they configured it with (every
//! field `ISO8601Duration`'s pattern admits — days, hours, minutes, seconds,
//! fractional seconds, all of them at once, and a duration of no length at all),
//! how far past or short of that duration the request is read (well past, the
//! exact instant it is due, a second short of it, not aged at all), and whether
//! the term names a duration — or a configuration — at all.
//!
//! The clock cannot be wound forward ten days inside a test, so the request's
//! own stored instants are wound BACK instead ([`sent_ago`]), the way
//! `agreement_expiry` ages an agreement's window: a record whose instants all
//! sit `d` earlier is exactly what a handler sees once `d` has passed, and the
//! handlers read the wall clock. The harness (`agreement_over`, `call`,
//! `emulator_over_engine`, `term`) is the one `agreement_lifecycle` owns, the
//! payment-request wire helpers (`get`, `items`, `list`, `send`) are
//! `payment_requests`', the accepted-term reader is `payment_request_balance`'s
//! and the charge report helpers are `purchase_order_updates`'.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{agreement_over, call, emulator_over_engine, term};
use crate::payment_request_balance::variable_payment_term_of;
use crate::payment_requests::{get, items, list, send};
use crate::purchase_order_updates::{charges, items as charge_items};

/// `PaymentRequestStatus`'s values this suite expects, from
/// `smithy_query(query="shape", shape="PaymentRequestStatus")`:
/// `SendAgreementPaymentRequest` documents a request as "created in
/// `PENDING_APPROVAL` status", and an approved one carries `APPROVED`. Spelled
/// out here rather than imported from the crate, so a handler storing another
/// value fails these rows instead of agreeing with them.
const PENDING_APPROVAL: &str = "PENDING_APPROVAL";
/// See [`PENDING_APPROVAL`].
const APPROVED: &str = "APPROVED";

/// `PaymentRequestApprovalStrategy`'s values, from
/// `smithy_query(query="shape", shape="PaymentRequestApprovalStrategy")` —
/// "values include `AUTO_APPROVE_ON_EXPIRATION` and `WAIT_FOR_APPROVAL`".
const AUTO_APPROVE_ON_EXPIRATION: &str = "AUTO_APPROVE_ON_EXPIRATION";
/// See [`AUTO_APPROVE_ON_EXPIRATION`].
const WAIT_FOR_APPROVAL: &str = "WAIT_FOR_APPROVAL";

/// The resource kind a conflict names in `resourceType` — one of
/// `ResourceType`'s values (`smithy_query(query="shape", shape="ResourceType")`).
const PAYMENT_REQUEST: &str = "PaymentRequest";

/// The currency every term of the offer this emulator serves is denominated in,
/// which is the currency of the charge an approval creates.
const CURRENCY: &str = "USD";

/// The term id every row's `VariablePaymentTerm` carries. A term whose acceptor
/// configured nothing resolves its kind from its id, so it is that kind's own
/// name.
const TERM_ID: &str = "VariablePaymentTerm";

/// One day and one hour, the units the rows measure a duration in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// See [`DAY`].
const HOUR: Duration = Duration::from_secs(60 * 60);

/// How closely two wire instants have to agree to be the same instant: they are
/// epoch seconds carried as a JSON number, so they are compared to the
/// millisecond rather than bit for bit.
const INSTANT_TOLERANCE: f64 = 0.001;

/// `ChargeId`'s `^ch-[a-zA-Z0-9]+$`, checked here rather than through the
/// crate's own code so a wrong minter cannot make this test pass.
fn is_charge_id(value: &str) -> bool {
    value
        .strip_prefix("ch-")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// A requested `VariablePaymentTerm` carrying the acceptor's approval strategy
/// and, where the row names one, the expiration they configured it with.
///
/// `agreement_lifecycle::variable_payment_term` is this same shape fixed at
/// `WAIT_FOR_APPROVAL` with no expiration; the rows here vary both members, so
/// the body is built from them.
fn configured_term(configuration: Option<(&str, Option<&str>)>) -> Value {
    let Some((strategy, expiration)) = configuration else {
        // A term whose acceptor configured nothing at all: its kind comes from
        // its id.
        return term(TERM_ID);
    };
    let mut configured = json!({"paymentRequestApprovalStrategy": strategy});
    if let Some(expiration) = expiration {
        configured["expirationDuration"] = json!(expiration);
    }
    json!({
        "id": TERM_ID,
        "configuration": {"variablePaymentTermConfiguration": configured},
    })
}

/// Move every instant the payment request `payment_request_id` stored `ago`
/// earlier, so it reads as one sent that long before now.
///
/// This is the only in-process step in this suite: the handlers read the wall
/// clock, which a test cannot wind forward past a ten-day expiration, and a
/// record whose own instants all sit `ago` earlier is the same thing seen from
/// the handler's side. Nothing about the request's status, or the charge an
/// approval would create, is touched — those are what the rows read back over
/// HTTP.
fn sent_ago(engine: &engine::InMemoryEngine, payment_request_id: &str, ago: Duration) {
    if ago.is_zero() {
        return;
    }
    let state = service_state(engine).expect("the emulator's own engine carries this store");
    let mut store = state.lock();
    let request = store
        .payment_requests
        .get_mut(payment_request_id)
        .unwrap_or_else(|| panic!("the sent payment request {payment_request_id} must be stored"));
    request.created_at -= ago;
    request.updated_at -= ago;
    request.auto_approves_at = request.auto_approves_at.map(|due| due - ago);
}

/// Send one payment request under `agreement_id` and answer its id together
/// with the `createdAt` the send recorded.
async fn send_request(
    router: &MultiServiceRouter,
    agreement_id: &str,
    case: &str,
    charge_amount: &str,
) -> (String, f64) {
    let answer = send(
        router,
        json!({
            "agreementId": agreement_id,
            "termId": TERM_ID,
            "name": "Metered overage",
            "chargeAmount": charge_amount,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: sending the payment request must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    // "The payment request is created in `PENDING_APPROVAL` status": whatever
    // the term configures, the send reports the status it created.
    assert_eq!(
        answer.str_at("/status"),
        PENDING_APPROVAL,
        "case {case:?}: a send reports the status it created, got {:?}",
        answer.body
    );
    let created_at = answer.body["createdAt"].as_f64().unwrap_or_else(|| {
        panic!(
            "case {case:?}: a sent payment request carries createdAt, got {:?}",
            answer.body
        )
    });
    (answer.str_at("/paymentRequestId"), created_at)
}

/// The `chargeId` a reported payment request carries, absent for one no approval
/// has moved ("this field is only present for approved payment requests").
fn charge_id_of(body: &Value) -> Option<String> {
    body.get("chargeId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every charge `ListAgreementCharges` reports for `agreement_id`, keyed by
/// charge id.
async fn reported_charges(router: &MultiServiceRouter, agreement_id: &str) -> Vec<(String, Value)> {
    let reported = charges(router, json!({"agreementId": agreement_id})).await;
    assert_eq!(
        reported.status, 200,
        "listing the charges of {agreement_id} must succeed, got {} with {:?}",
        reported.status, reported.body
    );
    charge_items(&reported)
        .into_iter()
        .map(|charge| (charge["id"].as_str().unwrap_or_default().to_string(), charge))
        .collect()
}

/// The ids of those charges, which the rows compare against the ones the
/// agreement's own quote minted.
async fn reported_charge_ids(router: &MultiServiceRouter, agreement_id: &str) -> Vec<String> {
    reported_charges(router, agreement_id)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// One row: the configuration the acceptor accepted the term with, how long
/// before the read the request was sent, and what the clock therefore owes it.
struct Row {
    case: &'static str,
    /// `(paymentRequestApprovalStrategy, expirationDuration)` as the acceptor
    /// configured them, or `None` for a term they configured nothing on.
    configuration: Option<(&'static str, Option<&'static str>)>,
    /// How long before the read the request was sent.
    sent_ago: Duration,
    /// The span after the send the automatic approval is due at, for a row whose
    /// configuration owes one that has elapsed; `None` for a row whose request
    /// is still the buyer's to decide.
    due_after: Option<Duration>,
    /// The `PaymentRequestStatus` the clock has therefore left the request in.
    status: &'static str,
}

/// The table. The duration forms are the fields `ISO8601Duration`'s own pattern
/// admits — `^([-+]?)P(?:([-+]?[0-9]+)D)?(T(?:([-+]?[0-9]+)H)?(?:([-+]?[0-9]+)M)?(?:([-+]?[0-9]+)(?:[.,]([0-9]{0,9}))?S)?)?$`
/// (`smithy_query(query="shape", shape="VariablePaymentTermConfiguration")`) —
/// including the `P10D` that member's own documentation gives as its example,
/// and the rows place the read well past the expiration, exactly at it, a second
/// short of it, and at the send itself.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a one-second expiration, read two seconds after the send",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("PT1S"))),
            sent_ago: Duration::from_secs(2),
            due_after: Some(Duration::from_secs(1)),
            status: APPROVED,
        },
        Row {
            case: "a one-minute expiration, read at the send",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("PT1M"))),
            sent_ago: Duration::ZERO,
            due_after: None,
            status: PENDING_APPROVAL,
        },
        Row {
            case: "the documented ten days, read a day past them",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("P10D"))),
            sent_ago: DAY * 11,
            due_after: Some(DAY * 10),
            status: APPROVED,
        },
        Row {
            case: "the documented ten days, read at the instant they are due",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("P10D"))),
            sent_ago: DAY * 10,
            due_after: Some(DAY * 10),
            status: APPROVED,
        },
        Row {
            case: "the documented ten days, read a second short of them",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("P10D"))),
            sent_ago: DAY * 10 - Duration::from_secs(1),
            due_after: None,
            status: PENDING_APPROVAL,
        },
        Row {
            case: "a fractional-second expiration, read a second after the send",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("PT0.5S"))),
            sent_ago: Duration::from_secs(1),
            due_after: Some(Duration::from_millis(500)),
            status: APPROVED,
        },
        Row {
            case: "an expiration of no length at all has elapsed as soon as the request is sent",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("P0D"))),
            sent_ago: Duration::ZERO,
            due_after: Some(Duration::ZERO),
            status: APPROVED,
        },
        Row {
            case: "an hours-and-minutes expiration, read at the instant it is due",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("PT12H30M"))),
            sent_ago: HOUR * 12 + Duration::from_secs(30 * 60),
            due_after: Some(HOUR * 12 + Duration::from_secs(30 * 60)),
            status: APPROVED,
        },
        Row {
            case: "every field at once, read a second past the expiration",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("P1DT2H3M4S"))),
            sent_ago: DAY + HOUR * 2 + Duration::from_secs(3 * 60 + 4 + 1),
            due_after: Some(DAY + HOUR * 2 + Duration::from_secs(3 * 60 + 4)),
            status: APPROVED,
        },
        Row {
            case: "an hour's expiration, read a second short of it",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, Some("PT1H"))),
            sent_ago: HOUR - Duration::from_secs(1),
            due_after: None,
            status: PENDING_APPROVAL,
        },
        Row {
            case: "approval on expiration with no expiration configured: nothing is ever due",
            configuration: Some((AUTO_APPROVE_ON_EXPIRATION, None)),
            sent_ago: DAY * 30,
            due_after: None,
            status: PENDING_APPROVAL,
        },
        Row {
            case: "waiting for the buyer: an expiration applies to the other strategy only",
            configuration: Some((WAIT_FOR_APPROVAL, Some("PT1S"))),
            sent_ago: DAY * 30,
            due_after: None,
            status: PENDING_APPROVAL,
        },
        Row {
            case: "waiting for the buyer, with no expiration configured",
            configuration: Some((WAIT_FOR_APPROVAL, None)),
            sent_ago: DAY * 30,
            due_after: None,
            status: PENDING_APPROVAL,
        },
        Row {
            case: "a term the acceptor configured nothing on: only the buyer decides",
            configuration: None,
            sent_ago: DAY * 30,
            due_after: None,
            status: PENDING_APPROVAL,
        },
    ]
}

/// The status a payment request reports is the one its term's own configuration
/// and the clock have left it in: `APPROVED`, with the charge the approval
/// created and dated at the instant that approval was due, once the configured
/// expiration has elapsed from the send under the strategy that approves on it —
/// and `PENDING_APPROVAL`, with no charge, for every other row. Every reader of
/// the request and the charge report of its agreement report that same record,
/// and reading it twice reports the same thing.
#[tokio::test]
async fn a_payment_request_is_approved_once_its_terms_expiration_has_elapsed() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        // One emulator per row, so the reports below carry this row's agreement
        // only.
        let (router, engine) = emulator_over_engine();
        let agreement_id = agreement_over(
            &router,
            &format!("ap-prauto{index}"),
            vec![configured_term(row.configuration)],
        )
        .await;

        // The acceptor's configuration is stored on the accepted term, which is
        // where the send reads it from: `GetAgreementTerms` reports it back.
        let accepted = variable_payment_term_of(&router, &agreement_id, TERM_ID).await;
        let configured = accepted["configuration"].clone();
        match row.configuration {
            Some((strategy, expiration)) => {
                assert_eq!(
                    configured["paymentRequestApprovalStrategy"].as_str(),
                    Some(strategy),
                    "case {case:?}: the accepted term must report the configured strategy, got \
                     {configured:?}"
                );
                assert_eq!(
                    configured["expirationDuration"].as_str(),
                    expiration,
                    "case {case:?}: the accepted term must report the configured expiration, got \
                     {configured:?}"
                );
            }
            None => assert!(
                configured.is_null(),
                "case {case:?}: a term configured with nothing reports no configuration, got \
                 {configured:?}"
            ),
        }

        // The charges the agreement's own quote minted, before any payment
        // request: whatever the clock then owes is a charge BESIDE these.
        let quoted = reported_charge_ids(&router, &agreement_id).await;

        let charge_amount = "125.50";
        let (payment_request_id, sent_at) =
            send_request(&router, &agreement_id, case, charge_amount).await;
        sent_ago(&engine, &payment_request_id, row.sent_ago);
        // The send's own instant, aged by the row: what a handler sees for a
        // request sent that long ago.
        let created_at = sent_at - row.sent_ago.as_secs_f64();

        let read = get(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the payment request must succeed, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.str_at("/status"),
            row.status,
            "case {case:?}: expected status {:?}, got {:?}",
            row.status,
            read.body
        );

        // The approval the clock owes does not re-date the send.
        let read_created_at = read.body["createdAt"].as_f64().unwrap_or_default();
        assert!(
            (read_created_at - created_at).abs() < INSTANT_TOLERANCE,
            "case {case:?}: expected createdAt {created_at}, got {read_created_at} in {:?}",
            read.body
        );

        let charge = charge_id_of(&read.body);
        let updated_at = read.body["updatedAt"].as_f64().unwrap_or_default();
        match row.due_after {
            Some(due_after) => {
                // "The unique identifier of the charge created after the payment
                // request is approved": the approval the clock owed created one.
                let minted = charge.clone().unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: an approved request reports its charge, got {:?}",
                        read.body
                    )
                });
                assert!(
                    is_charge_id(&minted),
                    "case {case:?}: {minted:?} must satisfy ChargeId's pattern"
                );
                assert!(
                    !quoted.contains(&minted),
                    "case {case:?}: the approval mints a charge of its own, got {minted} among \
                     {quoted:?}"
                );
                // The transition is dated at the instant it was DUE — the
                // configured expiration after the send — and not at the instant
                // this read was taken.
                let due = created_at + due_after.as_secs_f64();
                assert!(
                    (updated_at - due).abs() < INSTANT_TOLERANCE,
                    "case {case:?}: expected the approval dated at {due}, got {updated_at} in \
                     {:?}",
                    read.body
                );
            }
            None => {
                assert!(
                    charge.is_none(),
                    "case {case:?}: a request no approval has moved carries no charge, got {:?}",
                    read.body
                );
                assert!(
                    (updated_at - created_at).abs() < INSTANT_TOLERANCE,
                    "case {case:?}: an undecided request keeps the send's own instants, got \
                     {updated_at} in {:?}",
                    read.body
                );
            }
        }

        // The listing projects the same record.
        let listed = list(&router, json!({"partyType": "Proposer"})).await;
        let summary = items(&listed)
            .into_iter()
            .find(|item| item["paymentRequestId"].as_str() == Some(payment_request_id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the request must be listed, got {:?}",
                    listed.body
                )
            });
        assert_eq!(
            (
                summary["status"].as_str().unwrap_or_default().to_string(),
                charge_id_of(&summary),
            ),
            (row.status.to_string(), charge.clone()),
            "case {case:?}: the summary must agree with the detail, got {summary:?}"
        );

        // The charge an approval created is a charge of the agreement the
        // request was sent under, reported with the amount and the currency the
        // request carried and dated at the approval; a request nothing approved
        // leaves the agreement with the charges its quote minted and no more.
        let reported = reported_charges(&router, &agreement_id).await;
        match charge.as_deref() {
            Some(minted) => {
                let (_, charged) = reported
                    .iter()
                    .find(|(id, _)| id == minted)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: the charge {minted} the approval created must be \
                             reported, got {reported:?}"
                        )
                    });
                assert_eq!(
                    (
                        charged["amount"].as_str(),
                        charged["currencyCode"].as_str(),
                        charged["agreementId"].as_str(),
                    ),
                    (
                        Some(charge_amount),
                        Some(CURRENCY),
                        Some(agreement_id.as_str()),
                    ),
                    "case {case:?}: the charge must report what the request carried, got \
                     {charged:?}"
                );
                let charged_at = charged["time"].as_f64().unwrap_or_default();
                assert!(
                    (charged_at - updated_at).abs() < INSTANT_TOLERANCE,
                    "case {case:?}: the charge is dated at the approval ({updated_at}), got \
                     {charged_at} in {charged:?}"
                );
            }
            None => assert_eq!(
                reported.iter().map(|(id, _)| id.clone()).collect::<Vec<String>>(),
                quoted,
                "case {case:?}: a request nothing approved creates no charge"
            ),
        }

        // Reading does not move the record on: a second read reports the first
        // one's values, and mints no second charge.
        let again = get(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            (
                again.str_at("/status"),
                charge_id_of(&again.body),
                again.body["updatedAt"].as_f64(),
            ),
            (
                row.status.to_string(),
                charge.clone(),
                read.body["updatedAt"].as_f64(),
            ),
            "case {case:?}: a second read must report the same record, got {:?}",
            again.body
        );
        assert_eq!(
            reported_charge_ids(&router, &agreement_id).await,
            reported
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<String>>(),
            "case {case:?}: reading again must create no second charge"
        );
    }
}

/// A request the clock has already approved is terminal: every decision on it —
/// the buyer's accept and reject and the seller's cancel — earns the conflict a
/// second decision on one request earns, and leaves the record as the clock left
/// it. A request whose term waits for the buyer is still open to that same
/// decision however long ago it was sent.
#[tokio::test]
async fn no_decision_moves_a_request_the_clock_has_approved() {
    for decision in [
        "AcceptAgreementPaymentRequest",
        "RejectAgreementPaymentRequest",
        "CancelAgreementPaymentRequest",
    ] {
        let (router, engine) = emulator_over_engine();
        let expiring = agreement_over(
            &router,
            "ap-prautodecide",
            vec![configured_term(Some((
                AUTO_APPROVE_ON_EXPIRATION,
                Some("PT1S"),
            )))],
        )
        .await;
        let waiting = agreement_over(
            &router,
            "ap-prautowait",
            vec![configured_term(Some((WAIT_FOR_APPROVAL, Some("PT1S"))))],
        )
        .await;
        let (approved, _) = send_request(&router, &expiring, decision, "10").await;
        let (open, _) = send_request(&router, &waiting, decision, "10").await;
        sent_ago(&engine, &approved, Duration::from_secs(2));
        sent_ago(&engine, &open, Duration::from_secs(2));

        let before = get(&router, &expiring, &approved).await;
        assert_eq!(
            before.str_at("/status"),
            APPROVED,
            "the clock must have approved the request before a decision is tried, got {:?}",
            before.body
        );
        let refused = call(
            &router,
            decision,
            json!({"agreementId": expiring, "paymentRequestId": approved}),
        )
        .await;
        let case = format!("{decision} on a request the clock approved");
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            refused.body
        );
        assert_eq!(
            (
                refused.str_at("/resourceType"),
                refused.str_at("/resourceId"),
            ),
            (PAYMENT_REQUEST.to_string(), approved.clone()),
            "case {case:?}: the conflict names the request it refused, got {:?}",
            refused.body
        );
        assert!(
            !refused.str_at("/message").is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {:?}",
            refused.body
        );
        let after = get(&router, &expiring, &approved).await;
        assert_eq!(
            (
                after.str_at("/status"),
                charge_id_of(&after.body),
                after.body["updatedAt"].as_f64(),
            ),
            (
                APPROVED.to_string(),
                charge_id_of(&before.body),
                before.body["updatedAt"].as_f64(),
            ),
            "case {case:?}: the refused decision must leave the record as the clock left it, got \
             {:?}",
            after.body
        );

        // The same decision on a request whose term waits for the buyer is
        // served, however long ago it was sent.
        let served = call(
            &router,
            decision,
            json!({"agreementId": waiting, "paymentRequestId": open}),
        )
        .await;
        let case = format!("{decision} on a request whose term waits for the buyer");
        assert_eq!(
            served.status, 200,
            "case {case:?}: an open request must still be decidable, got {} with {:?}",
            served.status, served.body
        );
    }
}

/// The approval is the CLOCK's, not the reader's: it is materialised by a report
/// that was never handed the request's id, and the charge it created carries the
/// instant it was due rather than the instant of the call that first observed it.
#[tokio::test]
async fn the_automatic_approval_is_owed_whether_or_not_anybody_reads_the_request() {
    let (router, engine) = emulator_over_engine();
    let agreement_id = agreement_over(
        &router,
        "ap-prautoclock",
        vec![configured_term(Some((
            AUTO_APPROVE_ON_EXPIRATION,
            Some("PT1S"),
        )))],
    )
    .await;
    let case = "approved while nobody was looking";
    let quoted = reported_charge_ids(&router, &agreement_id).await;
    let (payment_request_id, sent_at) = send_request(&router, &agreement_id, case, "42").await;
    let aged = Duration::from_secs(5);
    sent_ago(&engine, &payment_request_id, aged);
    // The configured expiration is one second after the send, and the send sits
    // `aged` in the past.
    let due = sent_at - aged.as_secs_f64() + 1.0;

    // Read FIRST through the charge report, which is never handed the payment
    // request's id: the charge the approval created is already there.
    let reported = reported_charges(&router, &agreement_id).await;
    let (charge, charged) = reported
        .iter()
        .find(|(id, _)| !quoted.contains(id))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the approval the clock owed created a charge beside the quote's \
                 {quoted:?}, got {reported:?}"
            )
        });
    assert!(
        is_charge_id(&charge),
        "case {case:?}: {charge:?} must satisfy ChargeId's pattern"
    );
    let charged_at = charged["time"].as_f64().unwrap_or_default();
    assert!(
        (charged_at - due).abs() < INSTANT_TOLERANCE,
        "case {case:?}: the charge is dated at the instant the approval was due ({due}), got \
         {charged_at} in {charged:?}"
    );

    // The request itself then reports that same charge and that same instant.
    let read = get(&router, &agreement_id, &payment_request_id).await;
    assert_eq!(
        (read.str_at("/status"), charge_id_of(&read.body)),
        (APPROVED.to_string(), Some(charge.clone())),
        "case {case:?}: the request must report the charge the report carried, got {:?}",
        read.body
    );
    let updated_at = read.body["updatedAt"].as_f64().unwrap_or_default();
    assert!(
        (updated_at - due).abs() < INSTANT_TOLERANCE,
        "case {case:?}: the approval is dated at the instant it was due ({due}), got {updated_at} \
         in {:?}",
        read.body
    );
}
