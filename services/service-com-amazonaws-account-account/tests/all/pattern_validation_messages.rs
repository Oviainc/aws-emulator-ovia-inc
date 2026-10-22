// SPDX-License-Identifier: CC0-1.0

//! How Account words a `@pattern` violation, end to end over HTTP.
//!
//! The rule under test: a request member whose value violates its modelled
//! `@pattern` is refused with HTTP 400 `ValidationException` whose `message`
//! states the violated ECMA-262 expression and the input string that failed it,
//! as `[ECMA 262 regex "<pattern>" does not match input string "<value>"]` — for
//! every pattern-constrained member of every operation, at any depth in the
//! input, and naming neither the member nor its path.
//!
//! The table below is that class: one row per pattern-constrained member this
//! service models, so the rows differ along the operation, the member, the
//! expression, the member's depth (top level and nested) and the form of the
//! offending value (plain, quoted, non-ASCII). Each row states the expression
//! itself rather than importing it from the crate, so an implementation that
//! renders the wrong expression cannot make this test pass.
//!
//! Every clause is checked against the modelled `@pattern` from
//! `smithy_query(query="operation", operation="…")` on this service's model.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `AccountId`'s `@pattern`, from
/// `smithy_query(query="operation", operation="GetAlternateContact")`.
const ACCOUNT_ID_PATTERN: &str = r"^\d{12}$";

/// `AccountName`'s `@pattern`, from
/// `smithy_query(query="operation", operation="PutAccountName")`.
const ACCOUNT_NAME_PATTERN: &str = "^[ -;=?-~]+$";

/// `PutAlternateContact.EmailAddress`'s `@pattern`, from
/// `smithy_query(query="operation", operation="PutAlternateContact")`.
const EMAIL_ADDRESS_PATTERN: &str = r"^[\s]*[\w+=.#|!&-]+@[\w.-]+\.[\w]+[\s]*$";

/// `PutAlternateContact.PhoneNumber`'s `@pattern`, from
/// `smithy_query(query="operation", operation="PutAlternateContact")`.
const CONTACT_PHONE_PATTERN: &str = r"^[\s0-9()+-]+$";

/// `ContactInformation.PhoneNumber`'s `@pattern` (target shape
/// `ContactInformationPhoneNumber`), from
/// `smithy_query(query="closure", operation="PutContactInformation")`. A
/// different expression from the alternate contact's phone number, and a NESTED
/// member.
const CONTACT_INFORMATION_PHONE_PATTERN: &str = r"^[+][\s0-9()-]+$";

/// `Otp`'s `@pattern`, from
/// `smithy_query(query="operation", operation="VerifyPhoneNumber")`.
const OTP_PATTERN: &str = "^[a-zA-Z0-9]{6}$";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
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
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The reason Account states for one `@pattern` violation.
fn reason(pattern: &str, value: &str) -> String {
    format!("ECMA 262 regex \"{pattern}\" does not match input string \"{value}\"")
}

/// The whole `ValidationException.message` for a refusal of `clauses`.
fn bracketed(clauses: &[String]) -> String {
    format!("[{}]", clauses.join(", "))
}

/// A `PutAlternateContact` body with every member valid except the ones the
/// caller overrides, so a row isolates the member it is about.
fn alternate_contact(email: &str, phone: &str) -> Value {
    json!({
        "AlternateContactType": "SECURITY",
        "Name": "Ada Lovelace",
        "Title": "Security Lead",
        "EmailAddress": email,
        "PhoneNumber": phone,
    })
}

/// A `PutContactInformation` body with every member valid except
/// `ContactInformation.PhoneNumber`.
fn contact_information(phone: &str) -> Value {
    json!({
        "ContactInformation": {
            "FullName": "Ada Lovelace",
            "AddressLine1": "410 Terry Ave N",
            "City": "Seattle",
            "PostalCode": "98109",
            "CountryCode": "US",
            "PhoneNumber": phone,
        }
    })
}

/// Every pattern-constrained member of this service, refused one at a time: the
/// message is exactly the bracketed ECMA-262 reason for the violated expression
/// and the offending value.
#[tokio::test]
async fn a_pattern_violation_is_reported_as_the_ecma_262_reason() {
    // (case, uri, request, the violated @pattern, the offending input string)
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "the alternate contact's email address",
            "/putAlternateContact",
            alternate_contact("not-a-valid-email", "+1 206-555-0100"),
            EMAIL_ADDRESS_PATTERN,
            "not-a-valid-email",
        ),
        (
            "an email address whose offending value carries quotes",
            "/putAlternateContact",
            alternate_contact("\"ada\"@example.com", "+1 206-555-0100"),
            EMAIL_ADDRESS_PATTERN,
            "\"ada\"@example.com",
        ),
        (
            "the alternate contact's phone number",
            "/putAlternateContact",
            alternate_contact("ada@example.com", "call me"),
            CONTACT_PHONE_PATTERN,
            "call me",
        ),
        (
            "the primary contact's nested phone number",
            "/putContactInformation",
            contact_information("not-a-phone-number"),
            CONTACT_INFORMATION_PHONE_PATTERN,
            "not-a-phone-number",
        ),
        (
            "a nested phone number missing the leading plus",
            "/putContactInformation",
            contact_information("206-555-0100"),
            CONTACT_INFORMATION_PHONE_PATTERN,
            "206-555-0100",
        ),
        (
            "the account name",
            "/putAccountName",
            json!({"AccountName": "Demo<Account>"}),
            ACCOUNT_NAME_PATTERN,
            "Demo<Account>",
        ),
        (
            "an account name outside ASCII",
            "/putAccountName",
            json!({"AccountName": "Déjà vu"}),
            ACCOUNT_NAME_PATTERN,
            "Déjà vu",
        ),
        (
            "the account id on a read",
            "/getAccountInformation",
            json!({"AccountId": "12345"}),
            ACCOUNT_ID_PATTERN,
            "12345",
        ),
        (
            "the account id on a write",
            "/putAccountName",
            json!({"AccountName": "Demo", "AccountId": "abcdefghijkl"}),
            ACCOUNT_ID_PATTERN,
            "abcdefghijkl",
        ),
        (
            "the one-time passcode",
            "/verifyPhoneNumber",
            json!({"Otp": "12345"}),
            OTP_PATTERN,
            "12345",
        ),
        (
            "a one-time passcode with punctuation",
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": "123456789012",
                "PrimaryEmail": "ada@example.com",
                "Otp": "12-345",
            }),
            OTP_PATTERN,
            "12-345",
        ),
    ];

    let router = emulator();
    for (case, uri, request, pattern, value) in cases {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: {uri} must answer 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            bracketed(&[reason(pattern, value)]),
            "case {case:?}: expected the ECMA 262 reason, got {:?}",
            answer.message()
        );
    }
}

/// A value that SATISFIES the same expression is accepted, so the refusals above
/// are the pattern and not a blanket rejection of the member. A stub operation
/// answers 501 here, which is still an acceptance of the input: what matters is
/// that it is not the 400 the violating value earns.
#[tokio::test]
async fn a_value_inside_the_pattern_is_not_refused() {
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "an email address and phone number inside their patterns",
            "/putAlternateContact",
            alternate_contact("ada+aws@example.com", "+1 (206) 555-0100"),
        ),
        (
            "a nested phone number inside its pattern",
            "/putContactInformation",
            contact_information("+1 (206) 555-0100"),
        ),
        (
            "an account name inside its pattern",
            "/putAccountName",
            json!({"AccountName": "Demo Account"}),
        ),
        (
            "a twelve-digit account id",
            "/getAccountInformation",
            json!({"AccountId": "123456789012"}),
        ),
        (
            "a six-character alphanumeric passcode",
            "/verifyPhoneNumber",
            json!({"Otp": "a1B2c3"}),
        ),
    ];

    let router = emulator();
    for (case, uri, request) in cases {
        let answer = call(&router, uri, request).await;
        assert_ne!(
            answer.status, 400,
            "case {case:?}: {uri} must not refuse a value inside the pattern, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// Two members violating their patterns in one request: the message is the list
/// of both reasons inside the one pair of brackets. The order is the one the
/// validating walk finds them in, so each reason is checked for membership and
/// the whole message against the two orders that list admits.
#[tokio::test]
async fn two_pattern_violations_are_listed_in_one_message() {
    let router = emulator();
    let answer = call(
        &router,
        "/putAlternateContact",
        alternate_contact("not-a-valid-email", "call me"),
    )
    .await;

    let email = reason(EMAIL_ADDRESS_PATTERN, "not-a-valid-email");
    let phone = reason(CONTACT_PHONE_PATTERN, "call me");
    assert_eq!(
        answer.status, 400,
        "case \"two violated patterns\": expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case \"two violated patterns\": expected ValidationException, got {:?}",
        answer.body
    );
    let listed = [
        bracketed(&[email.clone(), phone.clone()]),
        bracketed(&[phone, email]),
    ];
    assert!(
        listed.contains(&answer.message().to_string()),
        "case \"two violated patterns\": expected one of {listed:?}, got {:?}",
        answer.message()
    );
}

/// A refusal that is NOT a pattern violation keeps the framework's wording: this
/// service has no recorded phrasing for an absent member, a length bound or a
/// value outside an enum, so none of them is dressed in the ECMA-262 form.
#[tokio::test]
async fn other_refusals_are_not_worded_as_pattern_violations() {
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "an absent required member",
            "/putAccountName",
            json!({}),
        ),
        (
            "a value below a length bound",
            "/putAccountName",
            json!({"AccountName": ""}),
        ),
        (
            "a value past a length bound",
            "/putAlternateContact",
            alternate_contact(
                &format!("{}@example.com", "a".repeat(255 - "@example.com".len())),
                "+1 206-555-0100",
            ),
        ),
        (
            "a value outside an enum",
            "/getAlternateContact",
            json!({"AlternateContactType": "MARKETING"}),
        ),
    ];

    let router = emulator();
    for (case, uri, request) in cases {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: {uri} must answer 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert!(
            !answer.message().starts_with('['),
            "case {case:?}: a non-pattern refusal must not be worded as a pattern \
             violation, got {:?}",
            answer.message()
        );
    }
}
