// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` makes idempotent: the IDENTITY of the purchase, not the
//! spelling the request carried it in.
//!
//! `CreateSavingsPlan.clientToken` is "a unique, case-sensitive identifier that you
//! provide to ensure the idempotency of the request"
//! (`smithy_query(query="shape",shape="CreateSavingsPlanRequest")`, member
//! `clientToken`), so a replayed token answers with the plan the first call created
//! and buys nothing more, while the same token carrying a DIFFERENT purchase is
//! refused rather than answered with the first plan.
//!
//! The rule these rows pin: two requests under one token are the SAME request exactly
//! when the plan they would store is the same, so every member is compared as the
//! service HOLDS it — an amount at the scale it parses to, an instant at the second it
//! truncates to — and never as the caller spelled it. The rows therefore differ along
//! what a spelling can vary without changing the stored plan (an amount's scale, its
//! leading zeros, its surrounding space, the sub-second part of an instant) and, in the
//! second suite, along what genuinely changes the stored plan (a different amount, a
//! different second, a member present in one request and absent in the other).
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, reusing
//! the harness in [`crate::savings_plans_lifecycle`].

use serde_json::{json, Value};

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// The members of a stored plan the purchase REQUEST decides, as the reader reports
/// them: what the stored resource depends on, and so what an idempotent replay must
/// leave exactly as it was.
fn stored_identity(plan: &Value) -> Vec<(&'static str, String)> {
    [
        "savingsPlanId",
        "offeringId",
        "commitment",
        "upfrontPaymentAmount",
        "recurringPaymentAmount",
        "start",
        "end",
    ]
    .into_iter()
    .map(|name| (name, text(plan, name)))
    .collect()
}

/// How many plans the account holds. `maxResults` is absent, so every plan comes back
/// in one page.
async fn plan_count(emulator: &Emulator) -> usize {
    let answer = emulator.call("DescribeSavingsPlans", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "describing every plan: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.list("savingsPlans").len()
}

/// `request` with `member` set to `value`, or with `member` removed when `value` is
/// JSON null — "the request did not carry this member" is one of the spellings under
/// test.
fn with_member(request: &Value, member: &str, value: &Value) -> Value {
    let mut request = request.clone();
    let object = request
        .as_object_mut()
        .expect("the request body is a JSON object");
    if value.is_null() {
        object.remove(member);
    } else {
        object.insert(member.to_string(), value.clone());
    }
    request
}

/// A whole second safely inside the window `purchaseTime` is accepted in, as epoch
/// seconds — the unit this protocol puts a `DateTime` on the wire in.
fn purchase_second() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_secs(),
    )
    .expect("an epoch that fits in i64")
}

/// A purchase of a Partial Upfront offering: the one payment option that takes an
/// `upfrontPaymentAmount`, so one base request can vary every member of the
/// fingerprint. A one-year term at 0.1 an hour is a total value of 876, so 438 is the
/// 50 percent the up-front amount's documented window opens at.
async fn base_request(emulator: &Emulator, client_token: &str) -> Value {
    let offering_id = text(
        &emulator
            .offering("Compute", "Partial Upfront", ONE_YEAR)
            .await,
        "offeringId",
    );
    json!({
        "savingsPlanOfferingId": offering_id,
        "commitment": "0.1",
        "upfrontPaymentAmount": "438",
        "clientToken": client_token,
    })
}

/// A replay whose members the service reads to the values the first call stored is the
/// SAME request: it answers with the first plan, buys nothing more and leaves the
/// stored plan untouched.
#[tokio::test]
async fn a_replay_spelled_differently_is_still_the_same_purchase() {
    let emulator = Emulator::new();
    let second = purchase_second();

    // (case, member, the first call's spelling, the replay's spelling)
    let rows: [(&str, &str, Value, Value); 9] = [
        (
            "the same up-front amount spelled identically",
            "upfrontPaymentAmount",
            json!("438"),
            json!("438"),
        ),
        (
            "an up-front amount with a zero fractional part",
            "upfrontPaymentAmount",
            json!("438"),
            json!("438.00"),
        ),
        (
            "an up-front amount at the full rendered scale",
            "upfrontPaymentAmount",
            json!("438"),
            json!("438.00000"),
        ),
        (
            "an up-front amount with a leading zero",
            "upfrontPaymentAmount",
            json!("438"),
            json!("0438"),
        ),
        (
            "an up-front amount surrounded by space",
            "upfrontPaymentAmount",
            json!("438.0"),
            json!(" 438 "),
        ),
        (
            "a commitment at a different scale",
            "commitment",
            json!("0.1"),
            json!("0.10000"),
        ),
        (
            "an instant and the whole second it falls in",
            "purchaseTime",
            json!(second as f64 + 0.25),
            json!(second),
        ),
        (
            "two instants inside one second",
            "purchaseTime",
            json!(second as f64 + 0.001),
            json!(second as f64 + 0.999),
        ),
        (
            "the same instant spelled identically",
            "purchaseTime",
            json!(second),
            json!(second),
        ),
    ];

    for (index, (case, member, first_spelling, replay_spelling)) in rows.into_iter().enumerate() {
        let token = format!("token-same-{index}");
        let base = base_request(&emulator, &token).await;
        let first_id = emulator
            .purchase(with_member(&base, member, &first_spelling))
            .await;
        let stored = stored_identity(&emulator.plan(&first_id).await);
        let count = plan_count(&emulator).await;

        let answer = emulator
            .call(
                "CreateSavingsPlan",
                with_member(&base, member, &replay_spelling),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} ({member} {first_spelling} then {replay_spelling}): expected 200, \
             got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            text(&answer.body, "savingsPlanId"),
            first_id,
            "case {case:?} ({member} {first_spelling} then {replay_spelling}): expected the \
             first plan {first_id:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            plan_count(&emulator).await,
            count,
            "case {case:?} ({member} {first_spelling} then {replay_spelling}): a replay must \
             buy nothing more than the {count} plans already held"
        );
        assert_eq!(
            stored_identity(&emulator.plan(&first_id).await),
            stored,
            "case {case:?} ({member} {first_spelling} then {replay_spelling}): a replay must \
             leave the stored plan exactly as it was"
        );
    }
}

/// The premise of the rule above, asserted on the stored plan itself: whatever scale an
/// amount is spelled at, and wherever inside a second an instant falls, the plan the
/// purchase stores is the same one.
#[tokio::test]
async fn one_purchase_spelled_two_ways_stores_one_plan() {
    let emulator = Emulator::new();
    let second = purchase_second();

    // (case, member, the two spellings)
    let rows: [(&str, &str, Value, Value); 4] = [
        (
            "an up-front amount at two scales",
            "upfrontPaymentAmount",
            json!("438"),
            json!("438.00000"),
        ),
        (
            "an up-front amount with a leading zero",
            "upfrontPaymentAmount",
            json!("438"),
            json!("0438"),
        ),
        (
            "a commitment at two scales",
            "commitment",
            json!("0.1"),
            json!("0.10000"),
        ),
        (
            "an instant and the whole second it falls in",
            "purchaseTime",
            json!(second as f64 + 0.25),
            json!(second),
        ),
    ];

    for (index, (case, member, left, right)) in rows.into_iter().enumerate() {
        // Separate tokens: this pins what the two purchases STORE, with idempotency
        // out of the way.
        let base_left = base_request(&emulator, &format!("token-store-{index}-left")).await;
        let base_right = with_member(
            &base_left,
            "clientToken",
            &json!(format!("token-store-{index}-right")),
        );
        let left_id = emulator
            .purchase(with_member(&base_left, member, &left))
            .await;
        let right_id = emulator
            .purchase(with_member(&base_right, member, &right))
            .await;
        assert_ne!(
            left_id, right_id,
            "case {case:?}: two tokens must buy two plans"
        );

        let drop_id = |identity: Vec<(&'static str, String)>| -> Vec<(&'static str, String)> {
            identity
                .into_iter()
                .filter(|(name, _)| *name != "savingsPlanId")
                .collect()
        };
        let left_plan = drop_id(stored_identity(&emulator.plan(&left_id).await));
        let right_plan = drop_id(stored_identity(&emulator.plan(&right_id).await));
        assert_eq!(
            left_plan, right_plan,
            "case {case:?} ({member} {left} vs {right}): the two spellings must store one plan"
        );
    }
}

/// A replay the service reads to a DIFFERENT purchase is refused with the modelled
/// `ValidationException` (HTTP 400) naming the token, and buys nothing: answering it
/// with the first plan would report success for a purchase that never happened.
#[tokio::test]
async fn the_same_token_carrying_a_different_purchase_is_refused() {
    let emulator = Emulator::new();
    let second = purchase_second();

    // (case, member, the first call's value, the second call's value; null is "absent")
    let rows: [(&str, &str, Value, Value); 6] = [
        (
            "a different commitment",
            "commitment",
            json!("0.1"),
            json!("0.2"),
        ),
        (
            "a commitment one step apart at the finest scale a request may spell",
            "commitment",
            json!("0.1"),
            json!("0.10001"),
        ),
        (
            "a different up-front amount",
            "upfrontPaymentAmount",
            json!("438"),
            json!("500"),
        ),
        (
            "an up-front amount only one of the two carries",
            "upfrontPaymentAmount",
            json!("438"),
            Value::Null,
        ),
        (
            "a different second",
            "purchaseTime",
            json!(second),
            json!(second - 1),
        ),
        (
            "a purchase time only one of the two carries",
            "purchaseTime",
            json!(second),
            Value::Null,
        ),
    ];

    for (index, (case, member, first_value, second_value)) in rows.into_iter().enumerate() {
        let token = format!("token-different-{index}");
        let base = base_request(&emulator, &token).await;
        let first_id = emulator
            .purchase(with_member(&base, member, &first_value))
            .await;
        let stored = stored_identity(&emulator.plan(&first_id).await);
        let count = plan_count(&emulator).await;

        let answer = emulator
            .call(
                "CreateSavingsPlan",
                with_member(&base, member, &second_value),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case {case:?} ({member} {first_value} then {second_value}): expected a 400 \
             ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
        let expected = format!(
            "clientToken '{token}' was already used for a different CreateSavingsPlan request"
        );
        let message = text(&answer.body, "message");
        assert_eq!(
            message, expected,
            "case {case:?} ({member} {first_value} then {second_value}): expected \
             {expected:?}, got {message:?}"
        );
        assert_eq!(
            plan_count(&emulator).await,
            count,
            "case {case:?} ({member} {first_value} then {second_value}): a refused replay must \
             buy nothing"
        );
        assert_eq!(
            stored_identity(&emulator.plan(&first_id).await),
            stored,
            "case {case:?} ({member} {first_value} then {second_value}): a refused replay must \
             leave the stored plan exactly as it was"
        );
    }
}
