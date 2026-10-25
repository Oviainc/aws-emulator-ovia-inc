// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SMS sandbox VERIFICATION step:
//! `CreateSMSSandboxPhoneNumber` delivers a one-time password to the destination
//! number it adds, and `VerifySMSSandboxPhoneNumber` decides on it.
//!
//! The rule under test: a one-time password is a per-destination secret the create
//! call delivers out of band, and the verification call serves only the code that was
//! delivered to the number it names — moving that number's status to `Verified` on a
//! match, refusing any other code with `VerificationException` (HTTP 400) while the
//! status stays `Pending`, and refusing a number this account's sandbox does not hold
//! as a lookup miss (`ResourceNotFound`, HTTP 404) before the code is looked at at
//! all.
//!
//! The rows below differ in the phone-number FORM, the code's LENGTH (both published
//! bounds of the modelled `OTPCode` and one past each), the code's DIGITS, which
//! destination the code was delivered to, and how many times a code was delivered —
//! which is what the rule generalises over. Every call goes over the wire through the
//! router the emulator serves in production; the code itself is never on the wire in
//! either direction but the caller's, because AWS delivers it by SMS, so the suite
//! reads it from the record it was delivered to, exactly as the handler does.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    INVALID_OTP_MESSAGE, INVALID_OTP_STATUS, RESOURCE_DOES_NOT_EXIST, SANDBOX_NUMBER_PENDING,
    SANDBOX_NUMBER_VERIFIED, SANDBOX_OTP_DIGITS,
};

use super::sms_sandbox_phone_numbers::{accepted_number_forms, add, reported_numbers};
use super::topics_subscriptions_publish::{Answer, Sns};

/// The length bounds the modelled code carries: `smithy_query(query="shape",
/// shape="OTPCode")` reports `length {min: 5, max: 8}` and pattern `^[0-9]+$`.
const MIN_OTP_LEN: usize = 5;
const MAX_OTP_LEN: usize = 8;

/// The awsQuery `<Code>` of the modelled verification failure. The model declares no
/// `awsQueryError` override for it, so the shape's own name is what the envelope
/// carries.
const VERIFICATION_CODE: &str = "VerificationException";

/// One verification call.
async fn verify(sns: &Sns, number: &str, one_time_password: &str) -> Answer {
    sns.call(
        "VerifySMSSandboxPhoneNumber",
        &[
            ("PhoneNumber", number),
            ("OneTimePassword", one_time_password),
        ],
    )
    .await
}

/// The code the create call delivered to `number`, read from the record it was
/// delivered to — the emulator's stand-in for the SMS carrier.
fn delivered_code(sns: &Sns, number: &str) -> String {
    let state = sns.state();
    let state = state.lock();
    state
        .sandbox_phone_numbers
        .get(number)
        .unwrap_or_else(|| panic!("the sandbox holds no record for the added number"))
        .one_time_password
        .clone()
}

/// The status the listing reports for `number`, or `None` when the sandbox does not
/// hold it — read back over the wire, so only persisted state carries it.
async fn listed_status(sns: &Sns, number: &str) -> Option<String> {
    let answer = sns.call("ListSMSSandboxPhoneNumbers", &[]).await;
    assert_eq!(
        answer.status, 200,
        "reading the sandbox back: expected 200, got {} / {}",
        answer.status, answer.body
    );
    reported_numbers(&answer)
        .into_iter()
        .find(|(listed, _)| listed == number)
        .map(|(_, status)| status)
}

/// Codes that are NOT `delivered`, each labelled: the two published length bounds of
/// the modelled code, its digits rearranged, and its last digit bumped — the nearest
/// miss a caller can make.
fn wrong_codes(delivered: &str) -> Vec<(String, String)> {
    let bumped: String = delivered
        .chars()
        .enumerate()
        .map(|(at, digit)| {
            if at + 1 == delivered.len() {
                let next = (digit.to_digit(10).unwrap_or(0) + 1) % 10;
                char::from_digit(next, 10).unwrap_or('0')
            } else {
                digit
            }
        })
        .collect();
    let reversed: String = delivered.chars().rev().collect();

    [
        ("at the published minimum length", "1".repeat(MIN_OTP_LEN)),
        ("at the published maximum length", "9".repeat(MAX_OTP_LEN)),
        (
            "the delivered length, different digits",
            "0".repeat(SANDBOX_OTP_DIGITS),
        ),
        ("the delivered digits reversed", reversed),
        ("the delivered code's last digit bumped", bumped),
    ]
    .into_iter()
    // A row that happens to BE the delivered code is not a wrong code.
    .filter(|(_, code)| code != delivered)
    .map(|(case, code)| (case.to_string(), code))
    .collect()
}

/// The code delivered to a number verifies it, and no other code does — for every
/// phone-number form the model accepts.
#[tokio::test]
async fn only_the_delivered_code_verifies_the_number_it_was_delivered_to() {
    let sns = Sns::new();

    for (case, number) in accepted_number_forms() {
        add(&sns, case, &number).await;
        let delivered = assert_well_formed_code(case, delivered_code(&sns, &number));

        for (row, wrong) in wrong_codes(&delivered) {
            let answer = verify(&sns, &number, &wrong).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} / {row:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                VERIFICATION_CODE,
                "case {case:?} / {row:?}: expected the {VERIFICATION_CODE} code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(INVALID_OTP_MESSAGE),
                "case {case:?} / {row:?}: expected the fixed invalid-code sentence, got {}",
                answer.body
            );
            assert!(
                !answer.body.contains(&delivered),
                "case {case:?} / {row:?}: the refusal must not leak the delivered code, got {}",
                answer.body
            );
            assert!(
                !answer.body.contains(number.as_str()),
                "case {case:?} / {row:?}: the refusal must not echo the sensitive number, \
                 got {}",
                answer.body
            );
            assert_eq!(
                listed_status(&sns, &number).await.as_deref(),
                Some(SANDBOX_NUMBER_PENDING),
                "case {case:?} / {row:?}: a refused verification must leave the status alone"
            );
        }

        let verified = verify(&sns, &number, &delivered).await;
        assert_eq!(
            verified.status, 200,
            "case {case:?}: expected the delivered code to verify, got {} / {}",
            verified.status, verified.body
        );
        assert_eq!(
            listed_status(&sns, &number).await.as_deref(),
            Some(SANDBOX_NUMBER_VERIFIED),
            "case {case:?}: expected the status {SANDBOX_NUMBER_VERIFIED:?} after the \
             verification"
        );

        // The operation models no "already verified" error and its output carries no
        // state, so presenting the same code again leaves the number verified.
        let again = verify(&sns, &number, &delivered).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected a repeated verification to answer 200, got {} / {}",
            again.status, again.body
        );
        assert_eq!(
            listed_status(&sns, &number).await.as_deref(),
            Some(SANDBOX_NUMBER_VERIFIED),
            "case {case:?}: expected the number to stay verified"
        );
    }
}

/// A delivered code is `SANDBOX_OTP_DIGITS` decimal digits — inside the modelled
/// `OTPCode` bounds, so a caller can actually present it.
fn assert_well_formed_code(case: &str, delivered: String) -> String {
    assert_eq!(
        delivered.len(),
        SANDBOX_OTP_DIGITS,
        "case {case:?}: expected a {SANDBOX_OTP_DIGITS}-digit code, got {delivered:?}"
    );
    assert!(
        (MIN_OTP_LEN..=MAX_OTP_LEN).contains(&delivered.len())
            && delivered.chars().all(|digit| digit.is_ascii_digit()),
        "case {case:?}: expected a code the model accepts, got {delivered:?}"
    );
    delivered
}

/// A code is scoped to the destination number it was delivered to: presenting one
/// number's code for another is refused, and every number keeps its own.
#[tokio::test]
async fn a_code_delivered_to_one_number_does_not_verify_another() {
    let sns = Sns::new();
    let held: Vec<String> = (0..4).map(|at| format!("+1206555{at:04}")).collect();
    for number in &held {
        add(&sns, "a held number", number).await;
    }
    let codes: Vec<String> = held
        .iter()
        .map(|number| delivered_code(&sns, number))
        .collect();

    for (at, number) in held.iter().enumerate() {
        for (other, code) in codes.iter().enumerate() {
            if other == at {
                continue;
            }
            let case = format!("number {at} with the code delivered to number {other}");
            assert_ne!(
                code.as_str(),
                codes[at].as_str(),
                "case {case:?}: each delivery must mint its own code, got {code:?} twice"
            );
            let answer = verify(&sns, number, code).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                VERIFICATION_CODE,
                "case {case:?}: expected the {VERIFICATION_CODE} code, got {}",
                answer.body
            );
        }
        assert_eq!(
            listed_status(&sns, number).await.as_deref(),
            Some(SANDBOX_NUMBER_PENDING),
            "number {at}: the refused verifications must leave the status alone"
        );
    }

    for (at, number) in held.iter().enumerate() {
        let answer = verify(&sns, number, &codes[at]).await;
        assert_eq!(
            answer.status, 200,
            "number {at}: expected its own code to verify it, got {} / {}",
            answer.status, answer.body
        );
    }
}

/// Every create call "sends a one-time password (OTP) to that phone number", so a
/// repeat delivers a FRESH code: the superseded one stops verifying and the new one
/// works, whatever status the record had reached.
#[tokio::test]
async fn a_repeated_create_delivers_a_fresh_code_that_supersedes_the_last() {
    let sns = Sns::new();
    let number = "+12065550100";
    add(&sns, "the first delivery", number).await;
    let first = delivered_code(&sns, number);

    add(&sns, "the second delivery", number).await;
    let second = delivered_code(&sns, number);
    assert_ne!(
        first, second,
        "a repeated create must deliver a fresh code, got {first:?} twice"
    );

    let superseded = verify(&sns, number, &first).await;
    assert_eq!(
        superseded.status, 400,
        "the superseded code: expected 400, got {} / {}",
        superseded.status, superseded.body
    );
    assert_eq!(
        superseded.code(),
        VERIFICATION_CODE,
        "the superseded code: expected the {VERIFICATION_CODE} code, got {}",
        superseded.body
    );

    let answered = verify(&sns, number, &second).await;
    assert_eq!(
        answered.status, 200,
        "the fresh code: expected 200, got {} / {}",
        answered.status, answered.body
    );
    assert_eq!(
        listed_status(&sns, number).await.as_deref(),
        Some(SANDBOX_NUMBER_VERIFIED),
        "the fresh code: expected the number to be verified"
    );

    // Adding a number the sandbox already holds takes nothing away: the verification
    // status it reached stays, while a third code is delivered.
    add(&sns, "the third delivery", number).await;
    let third = delivered_code(&sns, number);
    assert_ne!(
        second, third,
        "the third delivery must mint its own code, got {second:?} twice"
    );
    assert_eq!(
        listed_status(&sns, number).await.as_deref(),
        Some(SANDBOX_NUMBER_VERIFIED),
        "a repeated create must leave the status it reached alone"
    );
}

/// Verifying a number this account's sandbox does not hold is a lookup miss, resolved
/// BEFORE the code is looked at — whatever code is presented, and whatever form the
/// number takes.
#[tokio::test]
async fn verifying_a_number_the_sandbox_does_not_hold_is_a_lookup_miss() {
    let sns = Sns::new();
    let held = "+12065550100";
    add(&sns, "the one number the sandbox holds", held).await;
    let delivered = delivered_code(&sns, held);

    for (case, number) in accepted_number_forms() {
        if number == held {
            continue;
        }
        // Both a code nothing delivered and the code delivered to the number the
        // sandbox DOES hold: neither turns a missing resource into a verification.
        for (row, code) in [
            ("a code nothing delivered", "123456".to_string()),
            ("the code delivered to another number", delivered.clone()),
        ] {
            let answer = verify(&sns, &number, &code).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?} / {row:?}: expected 404, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "ResourceNotFound",
                "case {case:?} / {row:?}: expected the ResourceNotFound code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(RESOURCE_DOES_NOT_EXIST),
                "case {case:?} / {row:?}: expected the fixed missing-resource sentence, \
                 got {}",
                answer.body
            );
            assert!(
                !answer.body.contains(number.as_str()),
                "case {case:?} / {row:?}: the refusal must not echo the sensitive number, \
                 got {}",
                answer.body
            );
        }
    }

    assert_eq!(
        listed_status(&sns, held).await.as_deref(),
        Some(SANDBOX_NUMBER_PENDING),
        "the refused verifications must leave the held number alone"
    );
    assert_eq!(
        sns.state().lock().sandbox_phone_numbers.len(),
        1,
        "a refused verification must not add a record"
    );
}

/// A code outside the modelled `OTPCode` constraints never reaches the verification
/// step: it is refused as bad input at both published bounds plus one past each, and
/// a refusal of either kind leaves the number pending.
#[tokio::test]
async fn a_code_outside_the_modelled_constraints_is_refused_as_bad_input() {
    let sns = Sns::new();
    let number = "+12065550100";
    add(&sns, "the held number", number).await;

    // `accepted` is whether the code satisfies the MODELLED constraints — not whether
    // it verifies: none of these is the delivered code, so every one is refused, the
    // well-formed ones by the verification step and the rest before it.
    let cases: Vec<(String, String, bool)> = vec![
        (
            "one digit below the published minimum length".to_string(),
            "1".repeat(MIN_OTP_LEN - 1),
            false,
        ),
        (
            "at the published minimum length".to_string(),
            "1".repeat(MIN_OTP_LEN),
            true,
        ),
        (
            "at the published maximum length".to_string(),
            "1".repeat(MAX_OTP_LEN),
            true,
        ),
        (
            "one digit past the published maximum length".to_string(),
            "1".repeat(MAX_OTP_LEN + 1),
            false,
        ),
        ("empty".to_string(), String::new(), false),
        ("not digits".to_string(), "12a456".to_string(), false),
        ("signed".to_string(), "-12345".to_string(), false),
        ("spaced".to_string(), "12 456".to_string(), false),
    ];

    for (case, code, accepted) in cases {
        let answer = verify(&sns, number, &code).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        if accepted {
            assert_eq!(
                answer.code(),
                VERIFICATION_CODE,
                "case {case:?}: a code the model accepts must reach the verification step, \
                 got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Status").as_deref(),
                Some(INVALID_OTP_STATUS),
                "case {case:?}: expected the modelled verification status, got {}",
                answer.body
            );
        } else {
            assert_ne!(
                answer.code(),
                VERIFICATION_CODE,
                "case {case:?}: a code the model refuses must not be reported as a failed \
                 verification, got {}",
                answer.body
            );
        }
        assert_eq!(
            listed_status(&sns, number).await.as_deref(),
            Some(SANDBOX_NUMBER_PENDING),
            "case {case:?}: a refused verification must leave the status alone"
        );
    }
}
