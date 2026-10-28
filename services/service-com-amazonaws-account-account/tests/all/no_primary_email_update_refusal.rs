// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a primary-email call made when NO primary email
//! update is in flight for the account the call resolved to, end to end over
//! HTTP.
//!
//! RULE under test: an in-flight primary email update is not a stored
//! sub-resource a request addresses — it is the request the call's own
//! parameters assume — so a call that needs one when the resolved account has
//! none is refused as an invalid input parameter: the modelled HTTP 400
//! `ValidationException` carrying the one fixed sentence "No email update found
//! for account", never the HTTP 404 `ResourceNotFoundException` a client reads
//! as a benign retry-or-create signal. The sentence interpolates nothing, so it
//! is the same whichever account the call resolved to, however that account was
//! named, and whichever operation asked.
//!
//! The rows span what the rule generalises over rather than one request: the
//! operation that needs the update (the status read and the acceptance), how the
//! target account was resolved (left out, so the call lands on the caller's own,
//! or named as a member account, over several ids inside `AccountId`'s modelled
//! `^\d{12}$`), the acceptance's own well-formed members (addresses at both
//! published `@length` bounds, passcodes of differing form), and the history the
//! account has when the call arrives (never started; after a start this service
//! refused; after a start recorded on a DIFFERENT account). Each table is paired
//! with the positive control — the same call once a start HAS recorded an update
//! — so the refusal is shown to come from the store's contents, and with the
//! refusals the rule keeps distinct from it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::default_account_id;
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name). Signatures are not verified
/// here.
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The modelled status and error code of an invalid input parameter, from
/// `smithy_query(query="operation",
/// operation="GetPrimaryEmailUpdateStatus,AcceptPrimaryEmailUpdate")`.
const INVALID: (u16, &str) = (400, "ValidationException");
/// The modelled status and error code of a resource that can't be found — the
/// refusal this condition is NOT.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
/// The modelled status and error code of an account the caller may not read.
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The one sentence this service states for a primary email update that is not
/// there to be reported or accepted. Spelled out here rather than imported, so a
/// changed constant cannot make this test pass.
const NO_UPDATE_MESSAGE: &str = "No email update found for account";

/// Member account ids inside `AccountId`'s modelled `^\d{12}$`, none of them the
/// caller's own account, differing in which digits carry the value. Every row
/// that writes gets its own id, so no row can see what another recorded.
const MEMBER_ACCOUNTS: [&str; 6] = [
    "111111111111",
    "222222222222",
    "987654321098",
    "123456789012",
    "400000000004",
    "999999999999",
];

/// `PrimaryEmailAddress`'s modelled `@length`, from
/// `smithy_query(query="operation", operation="AcceptPrimaryEmailUpdate")`.
const PRIMARY_EMAIL_MIN: usize = 5;
const PRIMARY_EMAIL_MAX: usize = 64;

/// A passcode inside `Otp`'s modelled `^[a-zA-Z0-9]{6}$`, so an acceptance that
/// carries it is never refused for a malformed member.
const WELL_FORMED_OTP: &str = "a1B2c3";

/// An address of the syntax this service holds a primary email to, inside the
/// published `@length` bounds.
const WELL_FORMED_ADDRESS: &str = "ada@example.com";

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

    /// The status and error code together, which is what each row asserts.
    fn refusal(&self) -> (u16, &str) {
        (self.status, self.error_code())
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

/// An address of the requested total length, built from a local part that grows:
/// the domain keeps the address an address, so only the length varies.
fn address_of_length(length: usize) -> String {
    let domain = "@example.com";
    assert!(
        length > domain.len(),
        "case length {length}: an address needs a local part before {domain}"
    );
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// Make `account_id` an account the store holds, with no primary email update on
/// it: naming it on a `PutAccountName` is what creates the record.
async fn make_readable(router: &MultiServiceRouter, case: &str, account_id: &str) {
    let named = call(
        router,
        "/putAccountName",
        json!({"AccountId": account_id, "AccountName": "A member account"}),
    )
    .await;
    assert_eq!(
        named.status, 200,
        "case {case:?}: PutAccountName must answer 200 so the account is readable, \
         got {} with {:?}",
        named.status, named.body
    );
}

/// Assert `answer` is the modelled 400 carrying the one fixed sentence, and is
/// not the 404 this condition is commonly mistaken for.
fn assert_no_update_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.refusal(),
        INVALID,
        "case {case:?}: expected {INVALID:?}, got {:?} with {:?}",
        answer.refusal(),
        answer.body
    );
    assert_ne!(
        answer.error_code(),
        NOT_FOUND_CODE,
        "case {case:?}: the absent update must not be reported as a missing resource, \
         got {:?}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        NO_UPDATE_MESSAGE,
        "case {case:?}: expected message {NO_UPDATE_MESSAGE:?}, got {:?}",
        answer.message()
    );
}

/// Every call that needs an in-flight update, on an account that has none,
/// answers the modelled 400 with the one fixed sentence — whichever operation
/// asked, however the account was resolved, and whatever well-formed members the
/// request carried.
#[tokio::test]
async fn a_call_needing_an_absent_update_is_an_invalid_input_parameter() {
    let router = emulator();

    // Readable member accounts carrying nothing: the rows below write nothing,
    // so none of them can see what another recorded.
    for account_id in MEMBER_ACCOUNTS {
        make_readable(&router, "a readable member account", account_id).await;
    }

    // (case, uri, request) — the rows differ in the operation, in how the target
    // account is resolved, and in the acceptance's own well-formed members,
    // including both published address `@length` bounds.
    let cases: Vec<(String, &str, Value)> = vec![
        (
            "the status read with no AccountId, so it lands on the caller's own account"
                .to_string(),
            "/getPrimaryEmailUpdateStatus",
            json!({}),
        ),
        (
            format!("the status read naming member account {}", MEMBER_ACCOUNTS[0]),
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": MEMBER_ACCOUNTS[0]}),
        ),
        (
            format!("the status read naming member account {}", MEMBER_ACCOUNTS[2]),
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": MEMBER_ACCOUNTS[2]}),
        ),
        (
            format!("the status read naming member account {}", MEMBER_ACCOUNTS[5]),
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": MEMBER_ACCOUNTS[5]}),
        ),
        (
            format!("the acceptance naming member account {}", MEMBER_ACCOUNTS[1]),
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": MEMBER_ACCOUNTS[1],
                "PrimaryEmail": WELL_FORMED_ADDRESS,
                "Otp": WELL_FORMED_OTP,
            }),
        ),
        (
            format!(
                "the acceptance naming member account {} with an address at the {PRIMARY_EMAIL_MIN}-character bound",
                MEMBER_ACCOUNTS[3]
            ),
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": MEMBER_ACCOUNTS[3],
                "PrimaryEmail": "a@b.c",
                "Otp": "ZZZ999",
            }),
        ),
        (
            format!(
                "the acceptance naming member account {} with an address at the {PRIMARY_EMAIL_MAX}-character bound",
                MEMBER_ACCOUNTS[4]
            ),
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": MEMBER_ACCOUNTS[4],
                "PrimaryEmail": address_of_length(PRIMARY_EMAIL_MAX),
                "Otp": "abcdef",
            }),
        ),
    ];

    // The sentence interpolates nothing: every row answers the identical
    // message, whichever account it resolved to and whichever operation asked.
    let mut messages: Vec<String> = Vec::new();
    for (case, uri, request) in cases {
        let answer = call(&router, uri, request).await;
        assert_no_update_refusal(&case, &answer);
        messages.push(answer.message().to_string());
    }
    assert!(
        messages.windows(2).all(|pair| pair[0] == pair[1]),
        "the refusal's message must be the same fixed sentence on every call, got {messages:?}"
    );
}

/// The refusal comes from the store's contents, not from the call: the account
/// whose start was recorded reports its update, while an account whose start
/// this service REFUSED, and every account no start named, still answers the
/// modelled 400.
#[tokio::test]
async fn the_refusal_follows_the_account_no_start_recorded() {
    let router = emulator();
    let recorded = MEMBER_ACCOUNTS[0];
    let untouched = MEMBER_ACCOUNTS[1];
    for account_id in [recorded, untouched] {
        make_readable(&router, "a readable member account", account_id).await;
    }

    // A start this service refuses records nothing, so the probe that proves it
    // is the same 400: the caller's own account cannot be named as the target.
    let self_targeted = call(
        &router,
        "/startPrimaryEmailUpdate",
        json!({"AccountId": default_account_id(), "PrimaryEmail": WELL_FORMED_ADDRESS}),
    )
    .await;
    assert_eq!(
        self_targeted.refusal(),
        DENIED,
        "a start naming the caller's own account must be the modelled {DENIED:?}, \
         got {:?} with {:?}",
        self_targeted.refusal(),
        self_targeted.body
    );
    let own_after_refused_start = call(&router, "/getPrimaryEmailUpdateStatus", json!({})).await;
    assert_no_update_refusal(
        "the status read of the caller's own account after its start was refused",
        &own_after_refused_start,
    );

    // A start that IS recorded makes the SAME call answer: the positive control
    // for every refusal above.
    let started = call(
        &router,
        "/startPrimaryEmailUpdate",
        json!({"AccountId": recorded, "PrimaryEmail": WELL_FORMED_ADDRESS}),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartPrimaryEmailUpdate must answer 200, got {} with {:?}",
        started.status, started.body
    );
    let reported = call(
        &router,
        "/getPrimaryEmailUpdateStatus",
        json!({"AccountId": recorded}),
    )
    .await;
    assert_eq!(
        (reported.status, reported.body["Status"].as_str()),
        (200, Some("PENDING")),
        "the recorded update must be reported, got {} with {:?}",
        reported.status,
        reported.body
    );

    // The record is per account: another account's update is not this one's.
    for (case, uri, request) in [
        (
            "the status read of an account no start named",
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": untouched}),
        ),
        (
            "the acceptance of an account no start named",
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": untouched,
                "PrimaryEmail": WELL_FORMED_ADDRESS,
                "Otp": WELL_FORMED_OTP,
            }),
        ),
        (
            "the status read of the caller's own account, which no start named",
            "/getPrimaryEmailUpdateStatus",
            json!({}),
        ),
    ] {
        let answer = call(&router, uri, request).await;
        assert_no_update_refusal(case, &answer);
    }
}

/// The absent update stays distinguishable from the conditions it is not: an
/// account the caller may not read is the 403, and a request member outside its
/// modelled constraint is the same 400 code but a DIFFERENT sentence, so a
/// caller can still tell the two 400s apart.
#[tokio::test]
async fn the_absent_update_is_distinguishable_from_the_conditions_it_is_not() {
    let router = emulator();
    let known = MEMBER_ACCOUNTS[2];
    make_readable(&router, "the readable member account", known).await;
    let unknown = MEMBER_ACCOUNTS[3];

    // An account the store holds no record of is outside the caller's
    // organization: the 403, not the absent-update 400.
    for (case, uri, request) in [
        (
            "the status read of an account the store holds no record of",
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": unknown}),
        ),
        (
            "the acceptance on an account the store holds no record of",
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": unknown,
                "PrimaryEmail": WELL_FORMED_ADDRESS,
                "Otp": WELL_FORMED_OTP,
            }),
        ),
    ] {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.refusal(),
            DENIED,
            "case {case:?}: expected {DENIED:?}, got {:?} with {:?}",
            answer.refusal(),
            answer.body
        );
    }

    // A member outside its modelled constraint, on a readable account with no
    // update: the same 400 code, reported with its own sentence.
    for (case, uri, request) in [
        (
            "an AccountId outside the modelled pattern, on the status read",
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": "12345"}),
        ),
        (
            "an Otp outside the modelled pattern, on the acceptance",
            "/acceptPrimaryEmailUpdate",
            json!({"AccountId": known, "PrimaryEmail": WELL_FORMED_ADDRESS, "Otp": "12345"}),
        ),
        (
            "a PrimaryEmail one character below the modelled minimum length, on the acceptance",
            "/acceptPrimaryEmailUpdate",
            json!({
                // Four characters, one under `@length(min: 5)`, and an address
                // by the model's own expression otherwise.
                "PrimaryEmail": "a@b.",
                "AccountId": known,
                "Otp": WELL_FORMED_OTP,
            }),
        ),
    ] {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.refusal(),
            INVALID,
            "case {case:?}: expected {INVALID:?}, got {:?} with {:?}",
            answer.refusal(),
            answer.body
        );
        assert_ne!(
            answer.message(),
            NO_UPDATE_MESSAGE,
            "case {case:?}: a refused input member must not be reported as an absent \
             update, got {:?}",
            answer.body
        );
    }
}
