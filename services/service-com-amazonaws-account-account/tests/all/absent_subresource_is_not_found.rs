// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a call whose TARGET ACCOUNT is readable but whose
//! named sub-resource is not stored, over the real request path.
//!
//! RULE under test: when every input member of the request is valid and the
//! account the call resolves to is one the caller may read, a STORED
//! sub-resource that account does not hold — one the request addresses by name,
//! an alternate contact or a GovCloud link — is the modelled HTTP 404
//! `ResourceNotFoundException` ("the operation failed because it specified a
//! resource that can't be found"), while an absent IN-FLIGHT primary-email
//! update, which no request addresses and which only `StartPrimaryEmailUpdate`
//! can bring into being, is the modelled HTTP 400 `ValidationException`: the
//! call's own parameters assume a request that is not there. HTTP 403
//! `AccessDeniedException` stays reserved for an account the caller may not
//! read, so the conditions remain distinguishable by status code.
//!
//! The rows span what the rule generalises over rather than one request: the
//! operation (the primary-email update status read and its acceptance, the
//! alternate-contact read and delete, the GovCloud report), the sub-resource
//! (an email update, each of the three alternate-contact types, a GovCloud
//! link), and how the account was resolved (named explicitly, or left out so
//! the call resolves to the caller's own). Each absence row is paired with the
//! write that CREATES the sub-resource, so the refusal is shown to come from the
//! store's contents and not from the operation being unimplemented, and with
//! the invalid-input and unreadable-account rows the rule separates it from.

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

/// The modelled status and error code of a sub-resource that can't be found.
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");
/// The modelled status and error code of an invalid input parameter.
const INVALID: (u16, &str) = (400, "ValidationException");
/// The modelled status and error code of an account the caller may not read.
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The values `AlternateContactType` admits, from `smithy_query(query="shape",
/// shape="com.amazonaws.account#AlternateContactType")` — spelled out here, not
/// imported, so a wrong set in the implementation cannot make this test pass.
const ALTERNATE_CONTACT_TYPES: [&str; 3] = ["BILLING", "OPERATIONS", "SECURITY"];

/// Member account ids inside `AccountId`'s modelled `^\d{12}$`, none of them the
/// caller's own account, differing in which digits carry the value.
const MEMBER_ACCOUNTS: [&str; 4] = [
    "111111111111",
    "222222222222",
    "987654321098",
    "123456789012",
];

/// A passcode inside `Otp`'s modelled `^[a-zA-Z0-9]{6}$`, so an acceptance that
/// carries it is never refused for a malformed member.
const WELL_FORMED_OTP: &str = "a1B2c3";

/// An address inside `PrimaryEmailAddress`'s modelled `@length(min: 5, max: 64)`
/// and of the syntax this service holds an address to.
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

/// Assert `answer` is the refusal `expected`, and that it is a modelled refusal
/// rather than the scaffold's "not implemented" signal.
fn assert_refused(case: &str, answer: &Answer, expected: (u16, &str)) {
    assert_eq!(
        answer.refusal(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?} with {:?}",
        answer.refusal(),
        answer.body
    );
    assert!(
        !answer.message().is_empty() && !answer.message().contains("not implemented"),
        "case {case:?}: expected the modelled refusal's own message, got {:?}",
        answer.message()
    );
}

/// Make `account_id` an account the store holds, with no sub-resource of any
/// kind on it: naming it on a `PutAccountName` is what creates the record.
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

/// Every operation whose target account is readable and whose named STORED
/// sub-resource is absent answers the modelled 404, whichever sub-resource it
/// is and however the account was resolved; the two primary-email calls, whose
/// absent sub-resource is an in-flight request rather than a stored one, answer
/// the modelled 400.
#[tokio::test]
async fn an_absent_subresource_of_a_readable_account_is_not_found() {
    let router = emulator();

    // A member account the store holds, carrying nothing: one account per row
    // that writes nothing, so no row can see what another recorded.
    let known = MEMBER_ACCOUNTS[0];
    make_readable(&router, "the shared readable member account", known).await;

    // (case, uri, request, expected) — the rows differ in the operation, in the
    // sub-resource named and in how the target account is resolved.
    let mut cases: Vec<(String, &str, Value, (u16, &str))> = vec![
        (
            "the status read of the caller's own account, which has no update".to_string(),
            "/getPrimaryEmailUpdateStatus",
            json!({}),
            INVALID,
        ),
        (
            "the status read of a member account the store holds with no update".to_string(),
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": known}),
            INVALID,
        ),
        (
            "the acceptance of a member account the store holds with no update".to_string(),
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": known,
                "PrimaryEmail": WELL_FORMED_ADDRESS,
                "Otp": WELL_FORMED_OTP,
            }),
            INVALID,
        ),
        (
            "the GovCloud report of the caller's own account, which has no link".to_string(),
            "/getGovCloudAccountInformation",
            json!({}),
            NOT_FOUND,
        ),
        (
            "the GovCloud report of a member account the store holds with no link".to_string(),
            "/getGovCloudAccountInformation",
            json!({"StandardAccountId": known}),
            NOT_FOUND,
        ),
    ];
    // One row per alternate-contact type the model admits, on both the operation
    // that reads one and the one that deletes it, resolved both ways.
    for contact_type in ALTERNATE_CONTACT_TYPES {
        cases.push((
            format!("the read of an unset {contact_type} contact of the caller's own account"),
            "/getAlternateContact",
            json!({"AlternateContactType": contact_type}),
            NOT_FOUND,
        ));
        cases.push((
            format!("the delete of an unset {contact_type} contact of a member account"),
            "/deleteAlternateContact",
            json!({"AccountId": known, "AlternateContactType": contact_type}),
            NOT_FOUND,
        ));
    }

    for (case, uri, request, expected) in cases {
        let answer = call(&router, uri, request).await;
        assert_refused(&case, &answer, expected);
    }
}

/// The 404 comes from the store's contents: once the write that creates the
/// sub-resource has run, the same call reports it, and the account's OTHER
/// sub-resources are still absent.
#[tokio::test]
async fn the_same_call_reports_the_subresource_once_it_is_stored() {
    let router = emulator();

    // The primary-email update: absent, then recorded by the start, then
    // reported by the status read.
    let account_id = MEMBER_ACCOUNTS[1];
    make_readable(&router, "the account whose update is recorded", account_id).await;
    let before = call(
        &router,
        "/getPrimaryEmailUpdateStatus",
        json!({"AccountId": account_id}),
    )
    .await;
    assert_refused("the update before it is started", &before, INVALID);

    let started = call(
        &router,
        "/startPrimaryEmailUpdate",
        json!({"AccountId": account_id, "PrimaryEmail": WELL_FORMED_ADDRESS}),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartPrimaryEmailUpdate must answer 200, got {} with {:?}",
        started.status, started.body
    );

    let after = call(
        &router,
        "/getPrimaryEmailUpdateStatus",
        json!({"AccountId": account_id}),
    )
    .await;
    assert_eq!(
        (after.status, after.body["Status"].as_str()),
        (200, Some("PENDING")),
        "the recorded update must be reported, got {} with {:?}",
        after.status,
        after.body
    );

    // One alternate-contact type at a time: the type just written is reported
    // and the other two are still the 404, so the absence is per sub-resource
    // and not per account.
    for stored in ALTERNATE_CONTACT_TYPES {
        let put = call(
            &router,
            "/putAlternateContact",
            json!({
                "AccountId": account_id,
                "AlternateContactType": stored,
                "Name": "Ada Lovelace",
                "Title": "Analyst",
                "EmailAddress": WELL_FORMED_ADDRESS,
                "PhoneNumber": "+1 206-555-0100",
            }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {stored:?}: PutAlternateContact must answer 200, got {} with {:?}",
            put.status, put.body
        );

        let read = call(
            &router,
            "/getAlternateContact",
            json!({"AccountId": account_id, "AlternateContactType": stored}),
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.body["AlternateContact"]["AlternateContactType"].as_str()
            ),
            (200, Some(stored)),
            "case {stored:?}: the stored contact must be reported, got {} with {:?}",
            read.status,
            read.body
        );

        for other in ALTERNATE_CONTACT_TYPES {
            if other == stored {
                continue;
            }
            let absent = call(
                &router,
                "/getAlternateContact",
                json!({"AccountId": account_id, "AlternateContactType": other}),
            )
            .await;
            assert_refused(
                &format!("the {other} contact, with only {stored} stored"),
                &absent,
                NOT_FOUND,
            );
        }

        let deleted = call(
            &router,
            "/deleteAlternateContact",
            json!({"AccountId": account_id, "AlternateContactType": stored}),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {stored:?}: DeleteAlternateContact must answer 200, got {} with {:?}",
            deleted.status, deleted.body
        );
        // Deleted is absent again: the same 404 the type had before the write.
        let after_delete = call(
            &router,
            "/getAlternateContact",
            json!({"AccountId": account_id, "AlternateContactType": stored}),
        )
        .await;
        assert_refused(
            &format!("the {stored} contact after it is deleted"),
            &after_delete,
            NOT_FOUND,
        );
    }
}

/// The three conditions stay distinguishable by status code: an invalid input
/// parameter is the 400 and an account the caller may not read is the 403, on
/// the very calls whose missing sub-resource is the 404 above.
#[tokio::test]
async fn an_invalid_input_and_an_unreadable_account_keep_their_own_refusals() {
    let router = emulator();
    let known = MEMBER_ACCOUNTS[2];
    make_readable(&router, "the readable member account", known).await;

    // (case, uri, request, expected) — every row names a readable account for
    // the 400 rows, so the only difference from the 404 rows is the input
    // member, and an account the store holds no record of for the 403 rows.
    let cases: Vec<(&str, &str, Value, (u16, &str))> = vec![
        (
            "an AlternateContactType outside the modelled enum, on a readable account",
            "/getAlternateContact",
            json!({"AccountId": known, "AlternateContactType": "BILLING_CONTACT"}),
            INVALID,
        ),
        (
            "an AccountId outside the modelled pattern, on the status read",
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": "12345"}),
            INVALID,
        ),
        (
            "an Otp outside the modelled pattern, on a readable account with no update",
            "/acceptPrimaryEmailUpdate",
            json!({"AccountId": known, "PrimaryEmail": WELL_FORMED_ADDRESS, "Otp": "12345"}),
            INVALID,
        ),
        (
            "a PrimaryEmail that is not an email address, on a readable account with no update",
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": known,
                "PrimaryEmail": "not-an-address",
                "Otp": WELL_FORMED_OTP,
            }),
            INVALID,
        ),
        (
            "the status read of an account the store holds no record of",
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": MEMBER_ACCOUNTS[3]}),
            DENIED,
        ),
        (
            "the acceptance on an account the store holds no record of",
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": MEMBER_ACCOUNTS[3],
                "PrimaryEmail": WELL_FORMED_ADDRESS,
                "Otp": WELL_FORMED_OTP,
            }),
            DENIED,
        ),
        (
            "the status read of the caller's own account, which this member cannot name",
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": default_account_id()}),
            DENIED,
        ),
        (
            "the GovCloud report of an account the store holds no record of",
            "/getGovCloudAccountInformation",
            json!({"StandardAccountId": MEMBER_ACCOUNTS[3]}),
            DENIED,
        ),
    ];

    for (case, uri, request, expected) in cases {
        let answer = call(&router, uri, request).await;
        assert_refused(case, &answer, expected);
    }
}
