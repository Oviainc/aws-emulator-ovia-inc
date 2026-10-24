// SPDX-License-Identifier: CC0-1.0

//! The one-time-passcode flow over the phone number in an account's primary
//! contact information, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted store and serialization: the same
//! path a client takes.
//!
//! The rule under test: a phone-number verification is a state machine over the
//! phone number currently in the resolved account's primary contact information
//! — `UNVERIFIED` until `SendPhoneNumberVerification` sends a passcode to that
//! number, `PENDING` while exactly one passcode (the most recent) is
//! outstanding, and `VERIFIED` only once `VerifyPhoneNumber` was handed that
//! passcode back, which consumes it; every other submission is refused and
//! advances nothing (a well-formed passcode that is not the outstanding one with
//! `ValidationException`, a submission with nothing outstanding with
//! `ConflictException`), and both operations refuse an account this emulator
//! holds no record of (`AccessDeniedException`). `GetContactInformation` reports
//! whatever that machine last reached as `VerificationStatus`.
//!
//! The tables below span that class: rows differ along the form and the length
//! of the phone number (parenthesised, spaced, each published length bound),
//! which account the request resolves to (the caller's own implicitly and
//! explicitly, another member account), and the shape of a refused passcode
//! (one character changed, the characters reordered, the case flipped, a value
//! outside the modelled pattern on either side of its length).
//!
//! The passcode itself never appears on the wire: AWS sends it to the phone
//! number, and `Otp` is `@sensitive` with the responses carrying only `Status`.
//! So the test reads the outstanding passcode out of the emulator's own store,
//! the way the phone's owner reads it off a text message, and asserts it is
//! absent from every response body.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::{default_account_id, service_state};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The values `PhoneNumberVerificationStatus` admits, from
/// `smithy_query(query="operation", operation="VerifyPhoneNumber")` —
/// `NOT_SUPPORTED | PENDING | UNVERIFIED | VERIFIED`. Spelled out here, not
/// imported, so a wrong set in the implementation cannot make this test pass.
const PHONE_NUMBER_VERIFICATION_STATUSES: [&str; 4] =
    ["NOT_SUPPORTED", "PENDING", "UNVERIFIED", "VERIFIED"];

const UNVERIFIED: &str = "UNVERIFIED";
const PENDING: &str = "PENDING";
const VERIFIED: &str = "VERIFIED";

/// `Otp` is modelled `^[a-zA-Z0-9]{6}$`
/// (`smithy_query(query="operation", operation="VerifyPhoneNumber")`), so a
/// passcode is exactly six ASCII alphanumerics.
const OTP_LENGTH: usize = 6;

/// `ContactInformationPhoneNumber`'s modelled `@length(min: 1, max: 20)`
/// (`smithy_query(query="shape", shape="ContactInformation")`).
const PHONE_NUMBER_MAX: usize = 20;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// One emulator: a router carrying only this service over one shared engine,
/// plus that engine, so a test can read the store the handlers write.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The passcode outstanding for `account_id`, read out of the store the way
    /// the phone's owner reads it off the text message AWS sends. `None` once it
    /// has been consumed, or while none was ever sent.
    fn outstanding_passcode(&self, account_id: &str) -> Option<String> {
        let state = service_state(&*self.engine).expect("the account store is mounted");
        let store = state.lock();
        store
            .account(account_id)
            .and_then(|account| account.phone_verification.as_ref())
            .and_then(|verification| verification.outstanding_otp().map(str::to_string))
    }

    async fn call(&self, uri: &str, body: Value) -> Answer {
        let payload = body.to_string();
        let request = ::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "account.us-east-1.amazonaws.com")
            .header("authorization", SIGNED_FOR_US_EAST_1)
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request is well formed");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let text = String::from_utf8_lossy(&bytes).to_string();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!("POST {uri} answered a body that is not JSON ({error}): {text:?}")
            })
        };
        Answer { status, body, text }
    }

    async fn put_contact_information(&self, request: Value) -> Answer {
        self.call("/putContactInformation", request).await
    }

    async fn get_contact_information(&self, request: Value) -> Answer {
        self.call("/getContactInformation", request).await
    }

    async fn send_phone_number_verification(&self, request: Value) -> Answer {
        self.call("/sendPhoneNumberVerification", request).await
    }

    async fn verify_phone_number(&self, request: Value) -> Answer {
        self.call("/verifyPhoneNumber", request).await
    }
}

/// What the emulator answered: the HTTP status, the JSON body and the body as it
/// went over the wire (so a test can assert a secret is absent from ALL of it).
struct Answer {
    status: u16,
    body: Value,
    text: String,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The reported `Status` member (empty when absent).
    fn status_member(&self) -> &str {
        self.body["Status"].as_str().unwrap_or_default()
    }

    /// The reported `VerificationStatus` member (empty when absent).
    fn verification_status(&self) -> &str {
        self.body["VerificationStatus"].as_str().unwrap_or_default()
    }
}

/// A `ContactInformation` carrying the modelled required members, with
/// `PhoneNumber` set to `phone_number`.
fn contact_with(phone_number: &str) -> Value {
    json!({
        "FullName": "Ada Lovelace",
        "AddressLine1": "410 Terry Ave N",
        "City": "Seattle",
        "PostalCode": "98109",
        "CountryCode": "US",
        "PhoneNumber": phone_number,
    })
}

/// A phone number exactly `len` characters long and inside
/// `ContactInformationPhoneNumber`'s `^[+][\s0-9()-]+$`.
fn sized_phone_number(len: usize) -> String {
    format!("+{}", "1".repeat(len - 1))
}

/// Assert `answer` is a 200 reporting `expected` in `Status`, and that it is one
/// of the modelled `PhoneNumberVerificationStatus` values.
fn assert_status(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        PHONE_NUMBER_VERIFICATION_STATUSES.contains(&answer.status_member()),
        "case {case:?}: expected a PhoneNumberVerificationStatus among \
         {PHONE_NUMBER_VERIFICATION_STATUSES:?}, got {:?}",
        answer.status_member()
    );
    assert_eq!(
        answer.status_member(),
        expected,
        "case {case:?}: expected Status {expected}, got {:?}",
        answer.body
    );
}

/// Assert `answer` is a refusal with HTTP `status` and wire error code `code`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected {code}, got {:?}",
        answer.body
    );
}

/// Assert `GetContactInformation` reports `expected` for the resolved account.
async fn assert_reported_status(case: &str, emulator: &Emulator, request: Value, expected: &str) {
    let answer = emulator.get_contact_information(request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetContactInformation must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.verification_status(),
        expected,
        "case {case:?}: expected VerificationStatus {expected}, got {:?}",
        answer.body
    );
}

/// Assert a passcode is well formed per `Otp`'s `^[a-zA-Z0-9]{6}$`.
fn assert_well_formed_passcode(case: &str, passcode: &str) {
    assert_eq!(
        passcode.chars().count(),
        OTP_LENGTH,
        "case {case:?}: a passcode is {OTP_LENGTH} characters, got {:?}",
        passcode.chars().count()
    );
    assert!(
        passcode.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: a passcode is ASCII alphanumeric so the caller can submit it, \
         got a value with {} non-alphanumeric characters",
        passcode.chars().filter(|c| !c.is_ascii_alphanumeric()).count()
    );
}

/// Well-formed passcodes that are NOT `outstanding`: one character changed, the
/// characters reversed, and the case flipped. Each row keeps `Otp`'s modelled
/// pattern, so the boundary admits it and the handler is what refuses it.
fn wrong_passcodes(outstanding: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    let mut changed: Vec<char> = outstanding.chars().collect();
    changed[0] = if changed[0] == 'z' { 'a' } else { 'z' };
    rows.push(("one character changed".to_string(), changed.iter().collect()));

    rows.push((
        "the characters reversed".to_string(),
        outstanding.chars().rev().collect(),
    ));
    rows.push((
        "the case flipped".to_string(),
        outstanding
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect(),
    ));
    rows.push(("an unrelated passcode".to_string(), "A1b2C3".to_string()));

    // A row that happens to equal the outstanding passcode (a palindrome, an
    // all-digit code whose case cannot flip) is not a wrong passcode at all.
    rows.retain(|(_, candidate)| candidate != outstanding);
    rows
}

/// The flow carries a phone number UNVERIFIED -> PENDING -> VERIFIED, for every
/// form of phone number the model admits, and `GetContactInformation` reports
/// the status the flow reached at each step.
#[tokio::test]
async fn the_passcode_flow_verifies_the_primary_contact_phone_number() {
    // (case, the PhoneNumber in the primary contact information)
    let cases: [(&str, String); 6] = [
        ("an E.164 number", "+12065550100".to_string()),
        ("a parenthesised number", "+1 (425) 555-0101".to_string()),
        ("a spaced international number", "+44 20 7123 4567".to_string()),
        ("a hyphenated number", "+1-757-555-0199".to_string()),
        ("the shortest number the model admits", sized_phone_number(2)),
        (
            "the longest number the model admits",
            sized_phone_number(PHONE_NUMBER_MAX),
        ),
    ];

    for (case, phone_number) in cases {
        let emulator = Emulator::new();
        let own = default_account_id().to_string();

        let put = emulator
            .put_contact_information(json!({"ContactInformation": contact_with(&phone_number)}))
            .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutContactInformation must answer 200, got {} with {:?}",
            put.status, put.body
        );

        // Before any passcode is sent, the stored number has not been verified.
        assert_reported_status(case, &emulator, json!({}), UNVERIFIED).await;
        assert_eq!(
            emulator.outstanding_passcode(&own),
            None,
            "case {case:?}: no passcode can be outstanding before one is sent"
        );

        // Sending one moves the number to PENDING and mints a passcode the
        // response does not carry.
        let sent = emulator.send_phone_number_verification(json!({})).await;
        assert_status(case, &sent, PENDING);
        let passcode = emulator
            .outstanding_passcode(&own)
            .unwrap_or_else(|| panic!("case {case:?}: sending a passcode must store one"));
        assert_well_formed_passcode(case, &passcode);
        assert!(
            !sent.text.contains(&passcode),
            "case {case:?}: the passcode must not appear in the SendPhoneNumberVerification \
             response, got {:?}",
            sent.text
        );
        assert_reported_status(case, &emulator, json!({}), PENDING).await;

        // Every well-formed passcode that is not the outstanding one is refused,
        // and leaves the number PENDING with that passcode still outstanding.
        for (why, wrong) in wrong_passcodes(&passcode) {
            let refused = emulator.verify_phone_number(json!({"Otp": wrong})).await;
            let row = format!("{case}, {why}");
            assert_refused(&row, &refused, 400, "ValidationException");
            assert!(
                !refused.text.contains(&passcode),
                "case {row:?}: the outstanding passcode must not appear in a refusal, got {:?}",
                refused.text
            );
            assert_reported_status(&row, &emulator, json!({}), PENDING).await;
            assert_eq!(
                emulator.outstanding_passcode(&own).as_deref(),
                Some(passcode.as_str()),
                "case {row:?}: a refused submission must not consume the outstanding passcode"
            );
        }

        // The outstanding passcode verifies the number, and the report follows.
        let verified = emulator
            .verify_phone_number(json!({"Otp": passcode.clone()}))
            .await;
        assert_status(case, &verified, VERIFIED);
        assert_reported_status(case, &emulator, json!({}), VERIFIED).await;

        // The passcode is ONE-TIME: it is consumed, so submitting it again finds
        // nothing outstanding.
        assert_eq!(
            emulator.outstanding_passcode(&own),
            None,
            "case {case:?}: verifying must consume the one-time passcode"
        );
        let replayed = emulator
            .verify_phone_number(json!({"Otp": passcode.clone()}))
            .await;
        assert_refused(
            &format!("{case}, the passcode submitted twice"),
            &replayed,
            409,
            "ConflictException",
        );
        assert_reported_status(case, &emulator, json!({}), VERIFIED).await;
    }
}

/// Each send mints its OWN passcode and makes it the only outstanding one: the
/// passcode from an earlier send no longer verifies, and the latest one does.
#[tokio::test]
async fn a_resend_replaces_the_outstanding_passcode() {
    let emulator = Emulator::new();
    let own = default_account_id().to_string();
    let put = emulator
        .put_contact_information(json!({"ContactInformation": contact_with("+12065550100")}))
        .await;
    assert_eq!(
        put.status, 200,
        "case \"the stored contact\": expected 200, got {} with {:?}",
        put.status, put.body
    );

    // Several sends in a row: every passcode is well formed, and distinct from
    // the ones before it (each is drawn from a fresh v4 UUID, never from the
    // request or the account).
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut previous: Option<String> = None;
    for round in 1..=8u32 {
        let case = format!("send number {round}");
        let sent = emulator.send_phone_number_verification(json!({})).await;
        assert_status(&case, &sent, PENDING);
        let passcode = emulator
            .outstanding_passcode(&own)
            .unwrap_or_else(|| panic!("case {case:?}: sending a passcode must store one"));
        assert_well_formed_passcode(&case, &passcode);
        assert!(
            seen.insert(passcode.clone()),
            "case {case:?}: each send mints a fresh passcode, but {:?} was minted twice",
            passcode
        );

        // The passcode the PREVIOUS send minted is no longer the outstanding
        // one, so submitting it is refused and the number stays PENDING.
        if let Some(stale) = previous.replace(passcode.clone()) {
            if stale != passcode {
                let refused = emulator.verify_phone_number(json!({"Otp": stale.clone()})).await;
                assert_refused(
                    &format!("{case}, the passcode of the send before it"),
                    &refused,
                    400,
                    "ValidationException",
                );
                assert_reported_status(&case, &emulator, json!({}), PENDING).await;
            }
        }
    }

    // The latest passcode is the one that verifies.
    let latest = previous.expect("eight sends minted a passcode");
    let verified = emulator.verify_phone_number(json!({"Otp": latest})).await;
    assert_status("the latest passcode", &verified, VERIFIED);
    assert_reported_status("the latest passcode", &emulator, json!({}), VERIFIED).await;
}

/// A verification speaks for the NUMBER it was sent to: storing a different
/// primary-contact phone number puts the reported status back to UNVERIFIED, and
/// the passcode sent to the old number verifies nothing.
#[tokio::test]
async fn a_verification_does_not_carry_over_to_another_phone_number() {
    let emulator = Emulator::new();
    let own = default_account_id().to_string();

    let first = "+12065550100";
    let put = emulator
        .put_contact_information(json!({"ContactInformation": contact_with(first)}))
        .await;
    assert_eq!(
        put.status, 200,
        "case \"the first number\": expected 200, got {} with {:?}",
        put.status, put.body
    );
    let sent = emulator.send_phone_number_verification(json!({})).await;
    assert_status("the first number", &sent, PENDING);
    let passcode = emulator
        .outstanding_passcode(&own)
        .expect("sending a passcode stores one");
    let verified = emulator
        .verify_phone_number(json!({"Otp": passcode.clone()}))
        .await;
    assert_status("the first number", &verified, VERIFIED);

    // A put that changes the phone number: the number now in the primary
    // contact has never been verified.
    let second = "+1 (425) 555-0101";
    let put = emulator
        .put_contact_information(json!({"ContactInformation": contact_with(second)}))
        .await;
    assert_eq!(
        put.status, 200,
        "case \"the second number\": expected 200, got {} with {:?}",
        put.status, put.body
    );
    assert_reported_status("the second number", &emulator, json!({}), UNVERIFIED).await;
    let stale = emulator
        .verify_phone_number(json!({"Otp": passcode.clone()}))
        .await;
    assert_refused(
        "the second number, the first number's passcode",
        &stale,
        409,
        "ConflictException",
    );

    // A put that stores the SAME number again changes nothing about it: the
    // contact is replaced, the number is not.
    let put = emulator
        .put_contact_information(json!({"ContactInformation": contact_with(second)}))
        .await;
    assert_eq!(
        put.status, 200,
        "case \"the same number rewritten\": expected 200, got {} with {:?}",
        put.status, put.body
    );
    let sent = emulator.send_phone_number_verification(json!({})).await;
    assert_status("the second number", &sent, PENDING);
    let passcode = emulator
        .outstanding_passcode(&own)
        .expect("sending a passcode stores one");
    let put = emulator
        .put_contact_information(json!({"ContactInformation": contact_with(second)}))
        .await;
    assert_eq!(
        put.status, 200,
        "case \"the same number rewritten while PENDING\": expected 200, got {} with {:?}",
        put.status, put.body
    );
    assert_reported_status("the same number rewritten while PENDING", &emulator, json!({}), PENDING)
        .await;
    let verified = emulator.verify_phone_number(json!({"Otp": passcode})).await;
    assert_status("the same number rewritten while PENDING", &verified, VERIFIED);
}

/// The flow is per RESOLVED account: a request naming no `AccountId` and one
/// naming the caller's own id land on the same record, another 12-digit id is a
/// record of its own, and an account this emulator holds no record of is
/// refused.
#[tokio::test]
async fn the_flow_is_keyed_by_the_resolved_account() {
    let emulator = Emulator::new();
    let own = default_account_id().to_string();
    let other = "210987654321";
    let unknown = "999988887777";

    for (case, request) in [
        ("the caller's own account", json!({})),
        (
            "another account in the organization",
            json!({"AccountId": other}),
        ),
    ] {
        let mut body = request.clone();
        body["ContactInformation"] = contact_with("+12065550100");
        let put = emulator.put_contact_information(body).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutContactInformation must answer 200, got {} with {:?}",
            put.status, put.body
        );
    }

    // Sending for the OTHER account leaves the caller's own number alone.
    let sent = emulator
        .send_phone_number_verification(json!({"AccountId": other}))
        .await;
    assert_status("another account in the organization", &sent, PENDING);
    assert_reported_status(
        "the caller's own account while another account is PENDING",
        &emulator,
        json!({}),
        UNVERIFIED,
    )
    .await;
    assert_reported_status(
        "another account in the organization",
        &emulator,
        json!({"AccountId": other}),
        PENDING,
    )
    .await;
    assert_eq!(
        emulator.outstanding_passcode(&own),
        None,
        "case \"the caller's own account\": a send for another account must not mint a \
         passcode for it"
    );

    // The caller's own account, reached both ways, is ONE record: the passcode
    // sent without an `AccountId` verifies through the explicit id.
    let sent = emulator.send_phone_number_verification(json!({})).await;
    assert_status("the caller's own account, unnamed", &sent, PENDING);
    let passcode = emulator
        .outstanding_passcode(&own)
        .expect("sending a passcode stores one");
    let verified = emulator
        .verify_phone_number(json!({"AccountId": own, "Otp": passcode}))
        .await;
    assert_status("the caller's own account, named explicitly", &verified, VERIFIED);
    assert_reported_status(
        "the caller's own account, unnamed",
        &emulator,
        json!({}),
        VERIFIED,
    )
    .await;
    // And the other account is still only PENDING: its passcode was not touched.
    assert_reported_status(
        "another account in the organization",
        &emulator,
        json!({"AccountId": other}),
        PENDING,
    )
    .await;
    let other_passcode = emulator
        .outstanding_passcode(other)
        .expect("the other account's passcode is still outstanding");
    let verified = emulator
        .verify_phone_number(json!({"AccountId": other, "Otp": other_passcode}))
        .await;
    assert_status("another account in the organization", &verified, VERIFIED);

    // An account this emulator holds no record of: the same refusal a read of an
    // account outside the caller's organization earns, from both operations, and
    // no record is created by either.
    for (case, answer) in [
        (
            "SendPhoneNumberVerification for an unknown account",
            emulator
                .send_phone_number_verification(json!({"AccountId": unknown}))
                .await,
        ),
        (
            "VerifyPhoneNumber for an unknown account",
            emulator
                .verify_phone_number(json!({"AccountId": unknown, "Otp": "A1b2C3"}))
                .await,
        ),
    ] {
        assert_refused(case, &answer, 403, "AccessDeniedException");
    }
    let still_unknown = emulator
        .get_contact_information(json!({"AccountId": unknown}))
        .await;
    assert_refused(
        "the unknown account after both calls",
        &still_unknown,
        403,
        "AccessDeniedException",
    );
}

/// A submission with nothing outstanding answers `ConflictException` rather than
/// inventing a status — before any contact was put (the account's primary
/// contact is the one its record was created with) and after one was.
#[tokio::test]
async fn a_flow_with_nothing_to_act_on_is_refused() {
    let emulator = Emulator::new();

    // Nothing has been sent to the phone number the record carries, so there is
    // nothing to evaluate a submission against.
    let refused = emulator.verify_phone_number(json!({"Otp": "A1b2C3"})).await;
    assert_refused(
        "VerifyPhoneNumber before any passcode was sent",
        &refused,
        409,
        "ConflictException",
    );
    assert_reported_status(
        "VerifyPhoneNumber before any passcode was sent",
        &emulator,
        json!({}),
        UNVERIFIED,
    )
    .await;

    let put = emulator
        .put_contact_information(json!({"ContactInformation": contact_with("+12065550100")}))
        .await;
    assert_eq!(
        put.status, 200,
        "case \"the stored contact\": expected 200, got {} with {:?}",
        put.status, put.body
    );

    // A contact is stored but no passcode was sent: there is nothing to evaluate
    // the submission against.
    let refused = emulator.verify_phone_number(json!({"Otp": "A1b2C3"})).await;
    assert_refused(
        "VerifyPhoneNumber with no passcode outstanding",
        &refused,
        409,
        "ConflictException",
    );
    assert_reported_status(
        "VerifyPhoneNumber with no passcode outstanding",
        &emulator,
        json!({}),
        UNVERIFIED,
    )
    .await;
}

/// The phone number an account record is CREATED with is a real number in the
/// store: the passcode flow runs over it before any `PutContactInformation`, for
/// the caller's own account and for a member account, and the flow verifies a
/// number without rewriting the contact that number belongs to.
#[tokio::test]
async fn the_created_with_phone_number_runs_the_passcode_flow() {
    let member = "210987654321";

    // (case, the account the request names, the read)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the caller's own account, unnamed",
            json!({}),
            json!({}),
        ),
        (
            "a member account, named",
            json!({"AccountId": member}),
            json!({"AccountId": member}),
        ),
    ];

    for (case, request, read) in cases {
        let emulator = Emulator::new();
        let account_id = request["AccountId"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| default_account_id().to_string());
        if account_id != default_account_id() {
            // The record comes into existence here, with the primary contact it
            // is created with — no contact was ever put for it.
            let created = emulator
                .call(
                    "/putAccountName",
                    json!({"AccountId": account_id, "AccountName": "Member"}),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: PutAccountName must answer 200, got {} with {:?}",
                created.status, created.body
            );
        }

        let before = emulator.get_contact_information(read.clone()).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: GetContactInformation must answer 200 before any put, got {} \
             with {:?}",
            before.status, before.body
        );
        assert_eq!(
            before.verification_status(),
            UNVERIFIED,
            "case {case:?}: a number no passcode was sent to must be {UNVERIFIED}, got {:?}",
            before.body
        );

        let sent = emulator
            .send_phone_number_verification(request.clone())
            .await;
        assert_status(case, &sent, PENDING);
        assert_reported_status(case, &emulator, read.clone(), PENDING).await;

        let passcode = emulator
            .outstanding_passcode(&account_id)
            .expect("sending a passcode stores one");
        assert_well_formed_passcode(case, &passcode);
        let mut submission = request.clone();
        submission["Otp"] = json!(passcode);
        let verified = emulator.verify_phone_number(submission).await;
        assert_status(case, &verified, VERIFIED);
        assert_reported_status(case, &emulator, read.clone(), VERIFIED).await;

        // The flow moved a status, not the contact: the structure the record was
        // created with is the one still reported.
        let after = emulator.get_contact_information(read).await;
        assert_eq!(
            after.body["ContactInformation"], before.body["ContactInformation"],
            "case {case:?}: the passcode flow must leave the primary contact untouched, \
             got {:?} after {:?}",
            after.body, before.body
        );
    }
}

/// A value outside `Otp`'s modelled `^[a-zA-Z0-9]{6}$` is refused at the
/// boundary with HTTP 400 `ValidationException` — on each side of its length and
/// for a character the pattern excludes — and never reaches the flow: the
/// outstanding passcode survives and still verifies.
#[tokio::test]
async fn a_passcode_outside_the_modelled_pattern_is_refused() {
    let emulator = Emulator::new();
    let own = default_account_id().to_string();
    let put = emulator
        .put_contact_information(json!({"ContactInformation": contact_with("+12065550100")}))
        .await;
    assert_eq!(
        put.status, 200,
        "case \"the stored contact\": expected 200, got {} with {:?}",
        put.status, put.body
    );
    let sent = emulator.send_phone_number_verification(json!({})).await;
    assert_status("the stored contact", &sent, PENDING);
    let passcode = emulator
        .outstanding_passcode(&own)
        .expect("sending a passcode stores one");

    // (case, the Otp value) — each row is outside the modelled pattern.
    let cases: Vec<(&str, Value)> = vec![
        ("one character short of the length", json!("A1b2C")),
        ("one character past the length", json!("A1b2C3d")),
        ("empty", json!("")),
        ("a hyphen the pattern excludes", json!("A1-2C3")),
        ("a space the pattern excludes", json!("A1 2C3")),
        ("a non-ASCII character", json!("A1b2Cé")),
        ("the member absent", Value::Null),
    ];

    for (case, otp) in cases {
        let request = if otp.is_null() {
            json!({})
        } else {
            json!({"Otp": otp})
        };
        let refused = emulator.verify_phone_number(request).await;
        assert_refused(case, &refused, 400, "ValidationException");
        assert_reported_status(case, &emulator, json!({}), PENDING).await;
        assert_eq!(
            emulator.outstanding_passcode(&own).as_deref(),
            Some(passcode.as_str()),
            "case {case:?}: a refusal at the boundary must not consume the passcode"
        );
    }

    let verified = emulator.verify_phone_number(json!({"Otp": passcode})).await;
    assert_status("the surviving passcode", &verified, VERIFIED);
}

/// A send reports `PENDING` — "a one-time passcode has been sent and is waiting
/// to be submitted" — whatever the number last reached, and the passcode it
/// mints becomes the only outstanding one: the rows differ along the status the
/// number is in when the send arrives (never sent, already `PENDING`, already
/// `VERIFIED`), and in each the passcode outstanding before the send stops
/// verifying and the one minted by it does.
#[tokio::test]
async fn a_send_restarts_the_flow_whatever_the_number_last_reached() {
    // (case, how many sends and verifications run before the send under test,
    //  the status reported before it)
    let cases: [(&str, u32, bool, &str); 3] = [
        ("no passcode ever sent", 0, false, UNVERIFIED),
        ("a passcode already outstanding", 1, false, PENDING),
        ("the number already verified", 1, true, VERIFIED),
    ];

    for (case, sends_before, verify_before, status_before) in cases {
        let emulator = Emulator::new();
        let own = default_account_id().to_string();
        let put = emulator
            .put_contact_information(json!({"ContactInformation": contact_with("+12065550100")}))
            .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutContactInformation must answer 200, got {} with {:?}",
            put.status, put.body
        );

        for round in 0..sends_before {
            let sent = emulator.send_phone_number_verification(json!({})).await;
            assert_status(&format!("{case}, the send before it #{round}"), &sent, PENDING);
        }
        let before = emulator.outstanding_passcode(&own);
        if verify_before {
            let passcode = before
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: a send stores a passcode to submit"));
            let verified = emulator.verify_phone_number(json!({"Otp": passcode})).await;
            assert_status(&format!("{case}, the verification before it"), &verified, VERIFIED);
        }
        assert_reported_status(case, &emulator, json!({}), status_before).await;

        // The send under test: PENDING again, from whatever the number was in.
        let sent = emulator.send_phone_number_verification(json!({})).await;
        assert_status(case, &sent, PENDING);
        assert_eq!(
            sent.body.as_object().map(|body| body.keys().cloned().collect::<Vec<String>>()),
            Some(vec!["Status".to_string()]),
            "case {case:?}: the output carries only Status, got {:?}",
            sent.body
        );
        assert_reported_status(&format!("{case}, after the send"), &emulator, json!({}), PENDING)
            .await;

        let passcode = emulator
            .outstanding_passcode(&own)
            .unwrap_or_else(|| panic!("case {case:?}: sending a passcode must store one"));
        assert_well_formed_passcode(case, &passcode);
        assert!(
            !sent.text.contains(&passcode),
            "case {case:?}: the passcode must not appear in the response, got {:?}",
            sent.text
        );
        assert_ne!(
            Some(passcode.as_str()),
            before.as_deref(),
            "case {case:?}: a send mints a passcode of its own, not the one outstanding before it"
        );

        // Whatever was outstanding before the send no longer verifies, and the
        // number stays PENDING with the new passcode still outstanding.
        if let Some(stale) = before {
            let refused = emulator.verify_phone_number(json!({"Otp": stale})).await;
            assert_refused(
                &format!("{case}, the passcode outstanding before the send"),
                &refused,
                400,
                "ValidationException",
            );
            assert_reported_status(
                &format!("{case}, after the stale submission"),
                &emulator,
                json!({}),
                PENDING,
            )
            .await;
        }

        let verified = emulator
            .verify_phone_number(json!({"Otp": passcode}))
            .await;
        assert_status(&format!("{case}, the passcode the send minted"), &verified, VERIFIED);
        assert_reported_status(&format!("{case}, verified"), &emulator, json!({}), VERIFIED).await;
    }
}

/// An `AccountId` outside its modelled `^\d{12}$`
/// (`smithy_query(query="operation", operation="SendPhoneNumberVerification")`)
/// is refused at the boundary with HTTP 400 `ValidationException` and never
/// reaches the flow: no passcode is minted, for the named id or for the caller's
/// own account, and the reported status does not move.
#[tokio::test]
async fn a_send_naming_a_malformed_account_id_is_refused() {
    let emulator = Emulator::new();
    let own = default_account_id().to_string();
    let put = emulator
        .put_contact_information(json!({"ContactInformation": contact_with("+12065550100")}))
        .await;
    assert_eq!(
        put.status, 200,
        "case \"the stored contact\": expected 200, got {} with {:?}",
        put.status, put.body
    );

    // (case, the AccountId value) — each row is outside `^\d{12}$`: one digit
    // short of the length, one digit past it, and forms the pattern excludes.
    let cases: [(&str, &str); 7] = [
        ("one digit short of the length", "12345678901"),
        ("one digit past the length", "1234567890123"),
        ("empty", ""),
        ("letters among the digits", "12345678901a"),
        ("hyphens the pattern excludes", "1234-5678-901"),
        ("spaces around a well-formed id", " 123456789012 "),
        ("an account arn rather than an id", "arn:aws:::123456789012"),
    ];

    for (case, account_id) in cases {
        let refused = emulator
            .send_phone_number_verification(json!({"AccountId": account_id}))
            .await;
        assert_refused(case, &refused, 400, "ValidationException");
        assert_eq!(
            emulator.outstanding_passcode(account_id),
            None,
            "case {case:?}: a refusal at the boundary must mint no passcode"
        );
        assert_eq!(
            emulator.outstanding_passcode(&own),
            None,
            "case {case:?}: a refused send must not touch the caller's own account"
        );
        assert_reported_status(case, &emulator, json!({}), UNVERIFIED).await;
    }

    // The flow still runs for a well-formed id after every refusal.
    let sent = emulator.send_phone_number_verification(json!({})).await;
    assert_status("a well-formed request after the refusals", &sent, PENDING);
}
