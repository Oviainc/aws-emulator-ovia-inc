// SPDX-License-Identifier: CC0-1.0

//! The one-time-passcode rule this service's two confirmation operations share,
//! end to end over HTTP.
//!
//! An operation that confirms a one-time passcode this service sent out of band
//! admits exactly the passcode that was sent for that resource: every other
//! value of the modelled form is refused with HTTP 400 `ValidationException`
//! naming `Otp` and consumes nothing, and the passcode that WAS sent is
//! consumed by the call that confirms it, so it cannot be presented twice. The
//! two operations under that one rule are `AcceptPrimaryEmailUpdate`, whose
//! `Otp` is "the OTP code sent to the PrimaryEmail specified on the
//! StartPrimaryEmailUpdate API call", and `VerifyPhoneNumber`, whose `Otp` is
//! the passcode `SendPhoneNumberVerification` sent to the primary contact's
//! phone number (both from
//! `smithy_query(query="operation", operation="AcceptPrimaryEmailUpdate")` and
//! `smithy_query(query="operation", operation="VerifyPhoneNumber")`).
//!
//! The passcode goes to a mailbox or a handset the caller owns and NOT into the
//! response, so each case reads the outstanding passcode out of the emulator's
//! own store — the way the owner of that mailbox reads it off the message — and
//! asserts the response never carries it.
//!
//! Every table spans the class rather than one request: the rows differ in the
//! account, in the address's length and form (both published `@length` bounds
//! included) and in HOW the presented passcode differs from the one that was
//! sent (one character, the order, the case, an unrelated value, another
//! account's passcode).

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::{default_account_id, service_state};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The statuses of a primary email update, from
/// `smithy_query(query="shape", shape="PrimaryEmailUpdateStatus")` — spelled out
/// here, not imported, so a status outside the modelled set cannot pass.
const PENDING: &str = "PENDING";
const ACCEPTED: &str = "ACCEPTED";
const COMPLETED: &str = "COMPLETED";

/// The `PhoneNumberVerificationStatus` values this suite drives, from
/// `smithy_query(query="shape", shape="PhoneNumberVerificationStatus")`.
const PENDING_VERIFICATION: &str = "PENDING";
const VERIFIED: &str = "VERIFIED";

/// `Otp` is modelled `^[a-zA-Z0-9]{6}$`
/// (`smithy_query(query="operation", operation="AcceptPrimaryEmailUpdate")`), so
/// a passcode a caller can submit is exactly six ASCII alphanumerics.
const OTP_LENGTH: usize = 6;

/// `PrimaryEmailAddress`'s modelled `@length`, from
/// `smithy_query(query="operation", operation="AcceptPrimaryEmailUpdate")`.
const PRIMARY_EMAIL_MIN: usize = 5;
const PRIMARY_EMAIL_MAX: usize = 64;

/// How long a case waits for a status the clock owes before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Member account ids, each satisfying `AccountId`'s `^\d{12}$` and none of them
/// the caller's own account. One row gets one id, so no row can see what another
/// row recorded.
const MEMBER_ACCOUNTS: [&str; 6] = [
    "711111111111",
    "722222222222",
    "733333333333",
    "744444444444",
    "755555555555",
    "766666666666",
];

/// One emulator: a router carrying only this service over one shared engine,
/// plus that engine, so a case can read an outstanding passcode out of the
/// store.
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

    /// The passcode outstanding for the primary email update of `account_id`,
    /// or `None` once it has been consumed (or while no update was started).
    fn email_passcode(&self, account_id: &str) -> Option<String> {
        let state = service_state(&*self.engine).expect("the account store is mounted");
        let store = state.lock();
        store
            .account(account_id)
            .and_then(|account| account.primary_email_update.as_ref())
            .and_then(|update| update.outstanding_otp().map(str::to_string))
    }

    /// The passcode outstanding for the phone-number verification of
    /// `account_id`, or `None` once it has been consumed.
    fn phone_passcode(&self, account_id: &str) -> Option<String> {
        let state = service_state(&*self.engine).expect("the account store is mounted");
        let store = state.lock();
        store
            .account(account_id)
            .and_then(|account| account.phone_verification.as_ref())
            .and_then(|verification| verification.outstanding_otp().map(str::to_string))
    }

    async fn call(&self, uri: &str, body: Value) -> Answer {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "account.us-east-1.amazonaws.com")
            .header("authorization", SIGNED_FOR_US_EAST_1)
            .header("content-type", "application/json")
            .body(Bytes::from(body.to_string()))
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

    async fn start(&self, account_id: &str, address: &str) -> Answer {
        self.call(
            "/startPrimaryEmailUpdate",
            json!({"AccountId": account_id, "PrimaryEmail": address}),
        )
        .await
    }

    async fn accept(&self, account_id: &str, address: &str, passcode: &str) -> Answer {
        self.call(
            "/acceptPrimaryEmailUpdate",
            json!({"AccountId": account_id, "PrimaryEmail": address, "Otp": passcode}),
        )
        .await
    }

    async fn update_status(&self, account_id: &str) -> Answer {
        self.call(
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": account_id}),
        )
        .await
    }

    async fn primary_email(&self, account_id: &str) -> Answer {
        self.call("/getPrimaryEmail", json!({"AccountId": account_id}))
            .await
    }
}

/// What the emulator answered: the HTTP status, the JSON body and the body as it
/// went over the wire, so a case can assert a secret is absent from ALL of it.
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

    /// The error `message`, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `Status` member of a success body.
    fn reported_status(&self) -> &str {
        self.body["Status"].as_str().unwrap_or_default()
    }

    /// The `PrimaryEmail` member of a `GetPrimaryEmail` success body.
    fn primary_email(&self) -> &str {
        self.body["PrimaryEmail"].as_str().unwrap_or_default()
    }

    /// The `VerificationStatus` of the phone number in a
    /// `GetContactInformation` success body.
    fn verification_status(&self) -> &str {
        self.body["VerificationStatus"].as_str().unwrap_or_default()
    }

    /// The `name` of the first `fieldList` entry of a refusal.
    fn refused_member(&self) -> &str {
        self.body["fieldList"][0]["name"].as_str().unwrap_or_default()
    }

    /// The `reason` of a refusal.
    fn reason(&self) -> &str {
        self.body["reason"].as_str().unwrap_or_default()
    }
}

/// An address of exactly `length` characters, in the shape of an email address.
fn address_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// Assert a passcode this service minted is one a caller could submit at all:
/// `Otp`'s modelled `^[a-zA-Z0-9]{6}$`.
fn assert_well_formed_passcode(case: &str, passcode: &str) {
    assert_eq!(
        passcode.chars().count(),
        OTP_LENGTH,
        "case {case:?}: a passcode is {OTP_LENGTH} characters, got {passcode:?}"
    );
    assert!(
        passcode.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: a passcode is ASCII alphanumeric so the caller can submit it, got {} \
         characters outside that class",
        passcode.chars().filter(|c| !c.is_ascii_alphanumeric()).count()
    );
}

/// Assert `answer` is the modelled refusal with HTTP `status` and wire error
/// code `code`, never the scaffold's "not implemented" signal.
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
    assert!(
        !answer.message().contains("not implemented"),
        "case {case:?}: the refusal must be the modelled one, got {:?}",
        answer.message()
    );
}

/// Assert `answer` is the refusal of a passcode that is not the one that was
/// sent: the modelled 400 `ValidationException`, naming `Otp` as the member
/// that was invalid and echoing neither passcode.
fn assert_passcode_refused(case: &str, answer: &Answer, outstanding: &str, presented: &str) {
    assert_refused(case, answer, 400, "ValidationException");
    assert_eq!(
        answer.reason(),
        "fieldValidationFailed",
        "case {case:?}: expected reason fieldValidationFailed, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.refused_member(),
        "Otp",
        "case {case:?}: the refused member must be named, got {:?}",
        answer.body
    );
    for secret in [outstanding, presented] {
        assert!(
            !answer.text.contains(secret),
            "case {case:?}: a sensitive passcode must not be echoed, got {:?}",
            answer.text
        );
    }
}

/// Well-formed passcodes that are NOT `outstanding`, each differing from it in
/// its own way: one character, the order, the case, an unrelated value, and a
/// passcode `other` that was minted for a DIFFERENT resource. Every row keeps
/// `Otp`'s modelled pattern, so the boundary admits it and the handler is what
/// refuses it.
fn wrong_passcodes(outstanding: &str, other: Option<&str>) -> Vec<(String, String)> {
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
    rows.push(("all digits".to_string(), "000000".to_string()));
    if let Some(other) = other {
        rows.push((
            "the passcode of another resource".to_string(),
            other.to_string(),
        ));
    }

    // A row that happens to equal the outstanding passcode (a palindrome, an
    // all-digit code whose case cannot flip) is not a wrong passcode at all.
    rows.retain(|(_, candidate)| candidate != outstanding);
    rows
}

/// Poll the update status of `account_id` until it leaves `transient`, and
/// answer what it settled to.
async fn settled_status(
    emulator: &Emulator,
    case: &str,
    account_id: &str,
    transient: &str,
) -> String {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let answer = emulator.update_status(account_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the status read must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        if answer.reported_status() != transient {
            return answer.reported_status().to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: status is still {transient} after {SETTLE_TIMEOUT:?} — a transition \
             the clock owes must come due on its own"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Only the passcode `StartPrimaryEmailUpdate` sent confirms the update it
/// started: every other well-formed value is refused and consumes nothing, and
/// the passcode that was sent is consumed by the acceptance it confirms.
#[tokio::test]
async fn only_the_passcode_the_start_sent_confirms_a_primary_email_update() {
    // (case, the account, the address the update asks for) — the rows differ in
    // the address's length and form, both published bounds included.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "the shortest address the bound admits",
            MEMBER_ACCOUNTS[0],
            "a@b.c".to_string(),
        ),
        (
            "the longest address the bound admits",
            MEMBER_ACCOUNTS[1],
            address_of_length(PRIMARY_EMAIL_MAX),
        ),
        (
            "an ordinary address",
            MEMBER_ACCOUNTS[2],
            "ada@example.com".to_string(),
        ),
        (
            "a tagged address on a two-label domain",
            MEMBER_ACCOUNTS[3],
            "ada+aws@example.co.uk".to_string(),
        ),
    ];
    assert_eq!(
        rows[0].2.chars().count(),
        PRIMARY_EMAIL_MIN,
        "the shortest row must sit on the published minimum"
    );
    assert_eq!(
        rows[1].2.chars().count(),
        PRIMARY_EMAIL_MAX,
        "the longest row must sit on the published maximum"
    );

    let emulator = Emulator::new();

    // A second account with an update of its own, so a case can present a
    // passcode that WAS minted — just not for this update.
    let other_account = MEMBER_ACCOUNTS[4];
    let started_elsewhere = emulator.start(other_account, "grace@example.net").await;
    assert_eq!(
        started_elsewhere.status, 200,
        "the second account's update must be recorded, got {} with {:?}",
        started_elsewhere.status, started_elsewhere.body
    );
    let other_passcode = emulator
        .email_passcode(other_account)
        .expect("the second account's start must mint a passcode");

    for (case, account_id, address) in rows {
        assert_eq!(
            emulator.email_passcode(account_id),
            None,
            "case {case:?}: no passcode can be outstanding before an update is started"
        );

        let started = emulator.start(account_id, &address).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartPrimaryEmailUpdate must answer 200, got {} with {:?}",
            started.status, started.body
        );
        assert_eq!(
            started.reported_status(),
            PENDING,
            "case {case:?}: a started update must be {PENDING}, got {:?}",
            started.body
        );

        let sent = emulator
            .email_passcode(account_id)
            .unwrap_or_else(|| panic!("case {case:?}: starting an update must mint a passcode"));
        assert_well_formed_passcode(case, &sent);
        assert!(
            !started.text.contains(&sent),
            "case {case:?}: the passcode must not appear in the StartPrimaryEmailUpdate \
             response, got {:?}",
            started.text
        );
        assert_ne!(
            sent, other_passcode,
            "case {case:?}: two updates must not share a passcode"
        );

        for (why, wrong) in wrong_passcodes(&sent, Some(&other_passcode)) {
            let row = format!("{case}, {why}");
            let refused = emulator.accept(account_id, &address, &wrong).await;
            assert_passcode_refused(&row, &refused, &sent, &wrong);
            assert_eq!(
                emulator.email_passcode(account_id).as_deref(),
                Some(sent.as_str()),
                "case {row:?}: a refused acceptance must not consume the outstanding passcode"
            );
            let status = emulator.update_status(account_id).await;
            assert_eq!(
                status.reported_status(),
                PENDING,
                "case {row:?}: a refused acceptance must leave the update {PENDING}, got {:?}",
                status.body
            );
        }

        // The passcode that WAS sent confirms the update, and is consumed with
        // it: the change the clock then owes lands, and the address reads back.
        let accepted = emulator.accept(account_id, &address, &sent).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the passcode that was sent must be admitted, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.reported_status(),
            ACCEPTED,
            "case {case:?}: expected {ACCEPTED}, got {:?}",
            accepted.body
        );
        assert_eq!(
            emulator.email_passcode(account_id),
            None,
            "case {case:?}: the acceptance must consume the passcode"
        );

        assert_eq!(
            settled_status(&emulator, case, account_id, ACCEPTED).await,
            COMPLETED,
            "case {case:?}: an accepted update must settle to {COMPLETED}"
        );
        let read = emulator.primary_email(account_id).await;
        assert_eq!(
            read.primary_email(),
            address,
            "case {case:?}: the completed address must be what the read reports, got {:?}",
            read.body
        );
    }
}

/// A restart replaces the update and the passcode with it: the passcode the
/// first start sent confirms nothing afterwards, and the one the restart sent
/// does.
#[tokio::test]
async fn a_restarted_update_retires_the_passcode_the_first_start_sent() {
    // (case, the account, the first address, the address the restart asks for)
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a restart for a different address",
            MEMBER_ACCOUNTS[0],
            "first@example.com",
            "second@example.org",
        ),
        (
            "a restart for the SAME address",
            MEMBER_ACCOUNTS[1],
            "ada@example.com",
            "ada@example.com",
        ),
    ];

    let emulator = Emulator::new();
    for (case, account_id, first_address, second_address) in rows {
        let first = emulator.start(account_id, first_address).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first start must answer 200, got {} with {:?}",
            first.status, first.body
        );
        let first_passcode = emulator
            .email_passcode(account_id)
            .unwrap_or_else(|| panic!("case {case:?}: the first start must mint a passcode"));

        let second = emulator.start(account_id, second_address).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: the restart must answer 200, got {} with {:?}",
            second.status, second.body
        );
        let second_passcode = emulator
            .email_passcode(account_id)
            .unwrap_or_else(|| panic!("case {case:?}: the restart must mint a passcode"));
        assert_well_formed_passcode(case, &second_passcode);
        assert_ne!(
            first_passcode, second_passcode,
            "case {case:?}: a restart must mint a passcode of its own"
        );

        let stale = emulator
            .accept(account_id, second_address, &first_passcode)
            .await;
        assert_passcode_refused(case, &stale, &second_passcode, &first_passcode);
        assert_eq!(
            emulator.email_passcode(account_id).as_deref(),
            Some(second_passcode.as_str()),
            "case {case:?}: a refused acceptance must not consume the outstanding passcode"
        );

        let accepted = emulator
            .accept(account_id, second_address, &second_passcode)
            .await;
        assert_eq!(
            accepted.reported_status(),
            ACCEPTED,
            "case {case:?}: the passcode the restart sent must confirm the update, got {:?}",
            accepted.body
        );
    }
}

/// The SAME rule on the other operation that takes an `Otp`: the passcode
/// `SendPhoneNumberVerification` sent is the only value `VerifyPhoneNumber`
/// admits, a wrong one consumes nothing, and the right one is consumed.
#[tokio::test]
async fn only_the_passcode_that_was_sent_verifies_a_phone_number() {
    let emulator = Emulator::new();
    let case = "the primary contact's phone number";
    let own = default_account_id().to_string();

    let sent = emulator.call("/sendPhoneNumberVerification", json!({})).await;
    assert_eq!(
        sent.status, 200,
        "case {case:?}: SendPhoneNumberVerification must answer 200, got {} with {:?}",
        sent.status, sent.body
    );
    assert_eq!(
        sent.reported_status(),
        PENDING_VERIFICATION,
        "case {case:?}: a sent passcode must leave the number {PENDING_VERIFICATION}, got {:?}",
        sent.body
    );
    let passcode = emulator
        .phone_passcode(&own)
        .unwrap_or_else(|| panic!("case {case:?}: sending a passcode must store one"));
    assert_well_formed_passcode(case, &passcode);
    assert!(
        !sent.text.contains(&passcode),
        "case {case:?}: the passcode must not appear in the response, got {:?}",
        sent.text
    );

    for (why, wrong) in wrong_passcodes(&passcode, None) {
        let row = format!("{case}, {why}");
        let refused = emulator
            .call("/verifyPhoneNumber", json!({"Otp": wrong}))
            .await;
        assert_passcode_refused(&row, &refused, &passcode, &wrong);
        assert_eq!(
            emulator.phone_passcode(&own).as_deref(),
            Some(passcode.as_str()),
            "case {row:?}: a refused submission must not consume the outstanding passcode"
        );
    }

    let verified = emulator
        .call("/verifyPhoneNumber", json!({"Otp": passcode}))
        .await;
    assert_eq!(
        verified.status, 200,
        "case {case:?}: the passcode that was sent must be admitted, got {} with {:?}",
        verified.status, verified.body
    );
    assert_eq!(
        verified.reported_status(),
        VERIFIED,
        "case {case:?}: expected {VERIFIED}, got {:?}",
        verified.body
    );
    assert_eq!(
        emulator.phone_passcode(&own),
        None,
        "case {case:?}: the verification must consume the passcode"
    );

    let contact = emulator.call("/getContactInformation", json!({})).await;
    assert_eq!(
        contact.verification_status(),
        VERIFIED,
        "case {case:?}: the read must report the number as {VERIFIED}, got {:?}",
        contact.body
    );
}
