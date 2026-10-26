// SPDX-License-Identifier: CC0-1.0

//! The syntax an address member of this service admits, end to end over HTTP.
//!
//! One rule is under test: a request member documented to carry an email
//! address is refused with HTTP 400 `ValidationException` (reason
//! `fieldValidationFailed`, `fieldList` naming the member) unless its value
//! satisfies the email-address expression this service's model declares, and a
//! refused request records nothing. The expression is the `@pattern` the model
//! hangs on its `EmailAddress` shape
//! (`smithy_query(query="shape", shape="EmailAddress")`,
//! `^[\s]*[\w+=.#|!&-]+@[\w.-]+\.[\w]+[\s]*$` — a local part, `@`, a domain and
//! a top-level label); `PrimaryEmailAddress` carries no `@pattern` of its own
//! (`@length(min: 5, max: 64)` only), so the primary-email operations are where
//! the rule is the service's to enforce. The refusal is worded the way this
//! service words every value outside an expression — the bracketed ECMA-262
//! reason naming the expression and the input string, which
//! `tests/all/pattern_validation_messages.rs` pins for the member the expression
//! is declared on — so one expression produces one message on both members.
//!
//! The tables span the class rather than one request: the malformed rows differ
//! in WHICH part of an address is missing or malformed (the `@`, the local part,
//! the domain, the top-level label, an embedded space, a second `@`) and in
//! size, including a row on the published `@length` maximum; the well-formed
//! rows differ in length (both published bounds), in the form of the local part
//! (plain, tagged, dotted, digits, the punctuation the expression admits) and in
//! the number of domain labels. Every row is checked on BOTH email-address
//! members this service models — `StartPrimaryEmailUpdate.PrimaryEmail` and
//! `PutAlternateContact.EmailAddress` — so the two cannot be judged by two
//! different rules.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::{email_address_expression, service_state};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `PrimaryEmailAddress`'s modelled `@length`, from
/// `smithy_query(query="operation", operation="StartPrimaryEmailUpdate")`.
const PRIMARY_EMAIL_MIN: usize = 5;
const PRIMARY_EMAIL_MAX: usize = 64;

/// The Smithy member a refusal about the proposed address names.
const PRIMARY_EMAIL_MEMBER: &str = "PrimaryEmail";

/// The email-address `@pattern` this service's model declares, from
/// `smithy_query(query="shape", shape="EmailAddress")` — spelled out here, not
/// imported from the crate, so a service that enforced some OTHER expression
/// could not make this suite pass.
const EMAIL_ADDRESS_PATTERN: &str = r"^[\s]*[\w+=.#|!&-]+@[\w.-]+\.[\w]+[\s]*$";

/// The whole `ValidationException` message this service states for a value
/// outside an expression: the bracketed ECMA-262 reason, the violated
/// expression and the input string that failed it — the wording
/// `tests/all/pattern_validation_messages.rs` pins for every `@pattern`-
/// constrained member of this service, the boundary-validated
/// `PutAlternateContact.EmailAddress` among them.
fn pattern_refusal_message(value: &str) -> String {
    format!("[ECMA 262 regex \"{EMAIL_ADDRESS_PATTERN}\" does not match input string \"{value}\"]")
}

/// The `ValidationExceptionReason` a refused input member reports, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`
/// (`fieldValidationFailed | invalidRegionOptTarget`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The status an update carries the moment it is started, and the one it carries
/// once the one-time passcode has been presented — two of
/// `PrimaryEmailUpdateStatus`'s four values.
const PENDING: &str = "PENDING";
const ACCEPTED: &str = "ACCEPTED";

/// A passcode inside `Otp`'s modelled `^[a-zA-Z0-9]{6}$` that nothing sent,
/// for a case with no outstanding passcode to present.
const WELL_FORMED_PASSCODE: &str = "a1B2c3";

/// Member account ids this emulator has never been told about: each satisfies
/// `AccountId`'s `^\d{12}$` and none is the caller's own account.
const MEMBER_ACCOUNTS: [&str; 7] = [
    "111111111111",
    "222222222222",
    "333333333333",
    "444444444444",
    "555555555555",
    "666666666666",
    "123456789012",
];

/// One emulator: a router carrying only this service over one shared engine,
/// plus that engine, so a case can read the passcode a start sent out of the
/// store — the way the owner of the mailbox reads it off the message.
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

    /// The passcode outstanding for the update of `account_id`, or `None` once
    /// it has been consumed (or while no update was ever started).
    fn passcode(&self, account_id: &str) -> Option<String> {
        let state = service_state(&*self.engine).expect("the account store is mounted");
        let store = state.lock();
        store
            .account(account_id)
            .and_then(|account| account.primary_email_update.as_ref())
            .and_then(|update| update.outstanding_otp().map(str::to_string))
    }
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

    /// The `reason` member of a `ValidationException` body.
    fn reason(&self) -> &str {
        self.body["reason"].as_str().unwrap_or_default()
    }

    /// The name of the first member the `fieldList` of a `ValidationException`
    /// names, or the empty string when there is none.
    fn first_refused_member(&self) -> &str {
        self.body["fieldList"][0]["name"].as_str().unwrap_or_default()
    }

    /// The reason that `fieldList` entry carries.
    fn first_refused_message(&self) -> &str {
        self.body["fieldList"][0]["message"]
            .as_str()
            .unwrap_or_default()
    }

    /// The `Status` member of a success body.
    fn reported_status(&self) -> &str {
        self.body["Status"].as_str().unwrap_or_default()
    }
}

async fn call(router: &Emulator, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.router.dispatch(request).await;
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

async fn start(router: &Emulator, account_id: &str, address: &str) -> Answer {
    call(
        router,
        "/startPrimaryEmailUpdate",
        json!({"AccountId": account_id, "PrimaryEmail": address}),
    )
    .await
}

/// Accept the update of `account_id` with the passcode that update was started
/// with — the only value that confirms it — read out of the store. A case with
/// no outstanding passcode presents a well-formed value nothing sent, so the
/// request is still one the boundary admits.
async fn accept(router: &Emulator, account_id: &str, address: &str) -> Answer {
    let passcode = router
        .passcode(account_id)
        .unwrap_or_else(|| WELL_FORMED_PASSCODE.to_string());
    call(
        router,
        "/acceptPrimaryEmailUpdate",
        json!({"AccountId": account_id, "PrimaryEmail": address, "Otp": passcode}),
    )
    .await
}

async fn status_read(router: &Emulator, account_id: &str) -> Answer {
    call(
        router,
        "/getPrimaryEmailUpdateStatus",
        json!({"AccountId": account_id}),
    )
    .await
}

/// A `PutAlternateContact` whose `EmailAddress` is `address` and whose every
/// other member is inside its own modelled constraint
/// (`smithy_query(query="operation", operation="PutAlternateContact")`). It
/// names no `AccountId`, so it lands on the caller's own account.
async fn put_alternate_contact(router: &Emulator, address: &str) -> Answer {
    call(
        router,
        "/putAlternateContact",
        json!({
            "Name": "Ada Lovelace",
            "Title": "Operations",
            "EmailAddress": address,
            "PhoneNumber": "+1 (206) 555-0100",
            "AlternateContactType": "OPERATIONS",
        }),
    )
    .await
}

/// An address of exactly `length` characters, in the shape of an email address.
fn address_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// Values inside `PrimaryEmailAddress`'s `@length(min: 5, max: 64)` that are not
/// email addresses: each row breaks a different part of the modelled
/// expression's form.
fn malformed_rows() -> Vec<(&'static str, String)> {
    let rows = vec![
        ("no @ and no domain at all", "aaaaa".to_string()),
        ("a word with no @", "not-an-email-address".to_string()),
        ("nothing but @ characters", "@@@@@".to_string()),
        ("no local part", "@example.com".to_string()),
        ("a domain with no top-level label", "ada@example".to_string()),
        ("a domain whose first label is empty", "ada@.com".to_string()),
        ("a trailing dot and no top-level label", "ada@example.".to_string()),
        ("a space inside the local part", "ada lovelace@example.com".to_string()),
        ("a space inside the domain", "ada@exam ple.com".to_string()),
        ("two @ characters", "ada@example@com.org".to_string()),
        (
            "the published maximum length, with no @ at all",
            "n".repeat(PRIMARY_EMAIL_MAX),
        ),
    ];
    for (case, address) in &rows {
        let length = address.chars().count();
        assert!(
            (PRIMARY_EMAIL_MIN..=PRIMARY_EMAIL_MAX).contains(&length),
            "case {case:?}: a row must sit INSIDE the modelled bound so the refusal is \
             the syntax and not the length, got {length}"
        );
    }
    rows
}

/// Values that ARE email addresses by the modelled expression: the rows differ
/// in length (both published bounds), in the form of the local part and in the
/// number of domain labels.
fn well_formed_rows() -> Vec<(&'static str, String)> {
    let rows = vec![
        ("the shortest address the bound admits", "a@b.c".to_string()),
        (
            "the longest address the bound admits",
            address_of_length(PRIMARY_EMAIL_MAX),
        ),
        ("an ordinary address", "ada@example.com".to_string()),
        (
            "a tagged local part and three domain labels",
            "ada+aws@example.co.uk".to_string(),
        ),
        (
            "a dotted local part",
            "ada.lovelace@example.com".to_string(),
        ),
        ("a digits-only local part", "12345@example.com".to_string()),
        (
            // Every one of `=`, `#`, `|`, `!`, `&` and `-` is inside the
            // expression's local-part class, and the domain carries a hyphen.
            "the punctuation the expression admits",
            "ADA=1#2|3!4&5-6@mail-1.example.com".to_string(),
        ),
    ];
    assert_eq!(
        rows[0].1.chars().count(),
        PRIMARY_EMAIL_MIN,
        "the shortest row must sit on the published minimum"
    );
    assert_eq!(
        rows[1].1.chars().count(),
        PRIMARY_EMAIL_MAX,
        "the longest row must sit on the published maximum"
    );
    rows
}

/// Assert `answer` is the modelled refusal of one input member: HTTP 400
/// `ValidationException`, the message this service states for a value outside
/// the email-address expression, `reason` `fieldValidationFailed`, and
/// `fieldList` naming `member` with that same reason.
fn assert_member_refused(case: &str, answer: &Answer, member: &str, value: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
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
        pattern_refusal_message(value),
        "case {case:?}: expected the expression this service states a value outside it \
         with, got {:?}",
        answer.message()
    );
    assert_eq!(
        answer.reason(),
        FIELD_VALIDATION_FAILED,
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.first_refused_member(),
        member,
        "case {case:?}: the refused member must be named, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.first_refused_message(),
        pattern_refusal_message(value),
        "case {case:?}: the named member must carry the same reason, got {:?}",
        answer.body
    );
}

/// A proposed address that is not an email address is refused with the modelled
/// HTTP 400 `ValidationException` naming `PrimaryEmail`, and records nothing —
/// not even the account record a stored update would have created, so the status
/// read is still the refusal for an account outside the caller's organization.
#[tokio::test]
async fn an_address_that_is_not_an_email_address_is_refused_and_records_nothing() {
    let router = Emulator::new();
    for (case, address) in malformed_rows() {
        let account_id = MEMBER_ACCOUNTS[0];
        let refused = start(&router, account_id, &address).await;
        assert_member_refused(case, &refused, PRIMARY_EMAIL_MEMBER, &address);

        let read = status_read(&router, account_id).await;
        assert_eq!(
            read.status, 403,
            "case {case:?}: a refused start must record nothing, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {:?}",
            read.body
        );
    }
}

/// Every address OF the modelled form is admitted, so the refusals above are the
/// expression and not a blanket rejection: the update is recorded as `PENDING`,
/// the status read reports it back, and the one-time passcode carries it to
/// `ACCEPTED`.
#[tokio::test]
async fn every_address_of_the_modelled_form_is_admitted_and_round_trips() {
    let router = Emulator::new();
    for (index, (case, address)) in well_formed_rows().into_iter().enumerate() {
        let account_id = MEMBER_ACCOUNTS[index];
        let started = start(&router, account_id, &address).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartPrimaryEmailUpdate must answer 200, got {} with {:?}",
            started.status, started.body
        );
        assert_eq!(
            started.reported_status(),
            PENDING,
            "case {case:?}: expected {PENDING}, got {:?}",
            started.body
        );

        let read = status_read(&router, account_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the status read must answer 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.reported_status(),
            PENDING,
            "case {case:?}: the recorded update must read back as {PENDING}, got {:?}",
            read.body
        );

        let accepted = accept(&router, account_id, &address).await;
        assert_eq!(
            accepted.reported_status(),
            ACCEPTED,
            "case {case:?}: the recorded address must accept its own update, got {:?}",
            accepted.body
        );
    }
}

/// The presented address is held to the same syntax: an acceptance whose
/// `PrimaryEmail` is not an email address is refused with the same
/// `ValidationException` the start of such an update earns, and the update it
/// refused is left waiting.
#[tokio::test]
async fn an_acceptance_of_an_address_that_is_not_an_email_address_is_refused() {
    let router = Emulator::new();
    let requested = "ada@example.com";
    for (case, address) in malformed_rows() {
        let account_id = MEMBER_ACCOUNTS[1];
        let started = start(&router, account_id, requested).await;
        assert_eq!(
            started.reported_status(),
            PENDING,
            "case {case:?}: the update under test must be {PENDING}, got {:?}",
            started.body
        );

        let refused = accept(&router, account_id, &address).await;
        assert_member_refused(case, &refused, PRIMARY_EMAIL_MEMBER, &address);
        // The refusal is the one about the address's SYNTAX, which is the same
        // sentence the start of an update with that address earns — not the
        // refusal for an address that does not match the requested one.
        let on_start = start(&router, MEMBER_ACCOUNTS[2], &address).await;
        assert_eq!(
            refused.message(),
            on_start.message(),
            "case {case:?}: both address members must be refused by one rule, got {:?} \
             and {:?}",
            refused.message(),
            on_start.message()
        );

        let read = status_read(&router, account_id).await;
        assert_eq!(
            read.reported_status(),
            PENDING,
            "case {case:?}: a refused acceptance must leave the update {PENDING}, got {:?}",
            read.body
        );
    }
}

/// Both email-address members this service models are judged by the ONE
/// expression, and refused with the ONE sentence: a value
/// `PutAlternateContact.EmailAddress` refuses for its modelled `@pattern` is
/// refused for `StartPrimaryEmailUpdate.PrimaryEmail` with the same message the
/// boundary states for that member, and a value it admits is admitted there as
/// well.
#[tokio::test]
async fn the_two_address_members_admit_the_same_values() {
    let router = Emulator::new();

    for (case, address) in malformed_rows() {
        let on_contact = put_alternate_contact(&router, &address).await;
        assert_eq!(
            on_contact.status, 400,
            "case {case:?}: PutAlternateContact must refuse a value outside its modelled \
             email pattern, got {} with {:?}",
            on_contact.status, on_contact.body
        );
        let on_update = start(&router, MEMBER_ACCOUNTS[3], &address).await;
        assert_eq!(
            on_update.status, 400,
            "case {case:?}: the same value must be refused as a primary email, got {} \
             with {:?}",
            on_update.status, on_update.body
        );
        assert_eq!(
            on_update.message(),
            on_contact.message(),
            "case {case:?}: one expression must produce one message, got {:?} for the \
             primary email and {:?} for the alternate contact's",
            on_update.message(),
            on_contact.message()
        );
    }

    for (index, (case, address)) in well_formed_rows().into_iter().enumerate() {
        let on_contact = put_alternate_contact(&router, &address).await;
        assert_eq!(
            on_contact.status, 200,
            "case {case:?}: PutAlternateContact must admit a value inside its modelled \
             email pattern, got {} with {:?}",
            on_contact.status, on_contact.body
        );
        let on_update = start(&router, MEMBER_ACCOUNTS[index], &address).await;
        assert_eq!(
            on_update.status, 200,
            "case {case:?}: the same value must be admitted as a primary email, got {} \
             with {:?}",
            on_update.status, on_update.body
        );
    }
}

/// The expression itself is the model's, and it RESOLVES: a service that could
/// not read it off its own model would admit every value, so the expression this
/// crate enforces is asserted to be the one the model declares.
#[test]
fn the_enforced_expression_is_the_one_the_model_declares() {
    assert_eq!(
        email_address_expression(),
        Some(EMAIL_ADDRESS_PATTERN),
        "case \"the modelled email-address expression\": expected the model's own \
         expression, got {:?}",
        email_address_expression()
    );
}
