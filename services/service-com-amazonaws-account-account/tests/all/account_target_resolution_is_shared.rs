// SPDX-License-Identifier: CC0-1.0

//! The ONE resolution of an account-id member, over operations whose member is
//! optional and operations whose member is required.
//!
//! The rule under test: an account-id member is resolved the same way whatever
//! operation carries it and whether the model makes it optional or required —
//! an absent member is the standalone call on the caller's own account, a
//! member naming the caller's own account is refused with HTTP 403
//! `AccessDeniedException` and the identical payload, and any other named
//! account is the account the call lands on. The model states the refusal half
//! on every such member: "The management account can't specify its own
//! `AccountId`; it must call the operation in standalone context by not
//! including the `AccountId` parameter"
//! (`smithy_query(query="doc", shape="GetPrimaryEmailRequest",
//! member="AccountId")`).
//!
//! The rows span both sides of the optional/required split that used to decide
//! which code answered: `GetPrimaryEmail`, `StartPrimaryEmailUpdate` and
//! `AcceptPrimaryEmailUpdate` (required), against `GetPrimaryEmailUpdateStatus`
//! and `GetAccountInformation` (optional). Every case goes through the router,
//! so it exercises routing, deserialization, the handler and the persisted
//! store.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::default_account_id;
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The modelled error code and HTTP status of the self-target refusal:
/// `AccessDeniedException` at HTTP 403
/// (`smithy_query(query="service_info")`).
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The modelled error code and HTTP status of a refused input member:
/// `ValidationException` at HTTP 400.
const INVALID: (u16, &str) = (400, "ValidationException");

/// The modelled error code and HTTP status of a sub-resource an otherwise
/// readable account does not hold: `ResourceNotFoundException` at HTTP 404.
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// A passcode inside `Otp`'s modelled `^[a-zA-Z0-9]{6}$`, so an acceptance is
/// refused for its passcode and never for a malformed member. It is NOT the
/// passcode any update was started with: this service never answers that one
/// back.
const WRONG_OTP: &str = "Zz9Yy8";

/// Member accounts of the caller's organization, inside `AccountId`'s modelled
/// `^\d{12}$`: they differ in which digits carry the value and sit one digit
/// away from the caller's own id in either direction, so a resolution keying on
/// anything looser than exact equality would catch them.
const MEMBER_ACCOUNTS: [&str; 4] = [
    "000000000001",
    "100000000000",
    "210987654321",
    "999999999999",
];

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
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
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

/// An address inside `PrimaryEmailAddress`'s modelled `@length(min: 5, max:
/// 64)`, distinct per account so no two rows ask for an address another row
/// already holds.
fn proposed_email(account_id: &str) -> String {
    format!("new-{account_id}@example.com")
}

/// One row: a label, the operation's URI, and its request body for the account
/// the row names.
type Row = (&'static str, &'static str, fn(&str) -> Value);

/// Operations whose account-id member the model makes REQUIRED — the three that
/// used to decide the self-target rule in their own code.
fn required_member_rows() -> Vec<Row> {
    vec![
        ("GetPrimaryEmail", "/getPrimaryEmail", |id| {
            json!({"AccountId": id})
        }),
        ("StartPrimaryEmailUpdate", "/startPrimaryEmailUpdate", |id| {
            json!({"AccountId": id, "PrimaryEmail": proposed_email(id)})
        }),
        (
            "AcceptPrimaryEmailUpdate",
            "/acceptPrimaryEmailUpdate",
            |id| {
                json!({
                    "AccountId": id,
                    "PrimaryEmail": proposed_email(id),
                    "Otp": WRONG_OTP,
                })
            },
        ),
    ]
}

/// Operations whose account-id member the model makes OPTIONAL, which have
/// always resolved it through the shared path: the reference the required-member
/// rows are compared against.
fn optional_member_rows() -> Vec<Row> {
    vec![
        (
            "GetPrimaryEmailUpdateStatus",
            "/getPrimaryEmailUpdateStatus",
            |id| json!({"AccountId": id}),
        ),
        ("GetAccountInformation", "/getAccountInformation", |id| {
            json!({"AccountId": id})
        }),
    ]
}

/// Naming the caller's own account earns the SAME answer — status, error code
/// and message — from an operation whose member is required as from one whose
/// member is optional: one rule, stated once.
#[tokio::test]
async fn the_self_target_refusal_is_one_payload_for_required_and_optional_members() {
    let router = emulator();
    let own = default_account_id().to_string();

    let mut rows: Vec<(String, &str, Value)> = Vec::new();
    for (label, uri, body) in optional_member_rows() {
        rows.push((format!("{label} (optional member)"), uri, body(&own)));
    }
    for (label, uri, body) in required_member_rows() {
        rows.push((format!("{label} (required member)"), uri, body(&own)));
    }

    let mut reference: Option<(String, (u16, String, String))> = None;
    for (case, uri, body) in rows {
        let answer = call(&router, uri, body).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            DENIED,
            "case {case:?}: expected {DENIED:?}, got {} with {:?}",
            answer.status,
            answer.body
        );
        let observed = (
            answer.status,
            answer.error_code().to_string(),
            answer.message().to_string(),
        );
        match &reference {
            None => reference = Some((case, observed)),
            Some((first_case, expected)) => assert_eq!(
                &observed, expected,
                "case {case:?}: expected the same refusal as case {first_case:?} \
                 ({expected:?}), got {observed:?}"
            ),
        }
    }
}

/// The resolution lands a required-member operation on the account the member
/// NAMES, not on the caller's own: the update a start records is the named
/// account's, the acceptance finds that account's pending update, the read
/// reports that account's address, and the caller's own account is left with no
/// update at all.
#[tokio::test]
async fn a_required_member_lands_on_the_account_it_names() {
    let router = emulator();
    let own = default_account_id().to_string();

    for member in MEMBER_ACCOUNTS {
        // Before any record exists, the read resolves to the NAMED account:
        // were it resolving to the caller's own (which the store always holds)
        // it would answer that account's address instead of refusing.
        let unknown = call(&router, "/getPrimaryEmail", json!({"AccountId": member})).await;
        assert_eq!(
            (unknown.status, unknown.error_code()),
            DENIED,
            "case {member:?} unknown GetPrimaryEmail: expected {DENIED:?}, got {} with {:?}",
            unknown.status,
            unknown.body
        );
        assert!(
            unknown.message().contains(member),
            "case {member:?} unknown GetPrimaryEmail: the refusal must name the account \
             asked about, got {:?}",
            unknown.message()
        );

        let started = call(
            &router,
            "/startPrimaryEmailUpdate",
            json!({"AccountId": member, "PrimaryEmail": proposed_email(member)}),
        )
        .await;
        assert_eq!(
            (started.status, started.body["Status"].as_str()),
            (200, Some("PENDING")),
            "case {member:?} StartPrimaryEmailUpdate: expected 200 PENDING, got {} with {:?}",
            started.status,
            started.body
        );

        // The update is the NAMED account's: its status read reports it.
        let status = call(
            &router,
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": member}),
        )
        .await;
        assert_eq!(
            (status.status, status.body["Status"].as_str()),
            (200, Some("PENDING")),
            "case {member:?} GetPrimaryEmailUpdateStatus: expected 200 PENDING, got {} with {:?}",
            status.status,
            status.body
        );

        // And the caller's OWN account carries no update: the start wrote
        // nothing there.
        let own_status = call(&router, "/getPrimaryEmailUpdateStatus", json!({})).await;
        assert_eq!(
            (own_status.status, own_status.error_code()),
            NOT_FOUND,
            "case {member:?} standalone GetPrimaryEmailUpdateStatus: expected {NOT_FOUND:?} \
             because the caller's own account has no update, got {} with {:?}",
            own_status.status,
            own_status.body
        );

        // The acceptance resolves to the same named account: it finds that
        // account's PENDING update and refuses the passcode, which is not the
        // refusal an account with no update at all earns.
        let accepted = call(
            &router,
            "/acceptPrimaryEmailUpdate",
            json!({
                "AccountId": member,
                "PrimaryEmail": proposed_email(member),
                "Otp": WRONG_OTP,
            }),
        )
        .await;
        assert_eq!(
            (accepted.status, accepted.error_code()),
            INVALID,
            "case {member:?} AcceptPrimaryEmailUpdate: expected {INVALID:?}, got {} with {:?}",
            accepted.status,
            accepted.body
        );
        assert!(
            accepted.message().contains("passcode"),
            "case {member:?} AcceptPrimaryEmailUpdate: expected the passcode refusal of the \
             named account's pending update, got {:?}",
            accepted.message()
        );

        // The read resolves to the named account too: the record the start
        // created reports ITS OWN address, naming that account and not the
        // caller's.
        let read = call(&router, "/getPrimaryEmail", json!({"AccountId": member})).await;
        let reported = read.body["PrimaryEmail"].as_str().unwrap_or_default();
        assert_eq!(
            read.status, 200,
            "case {member:?} GetPrimaryEmail: expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert!(
            reported.contains(member) && !reported.contains(own.as_str()),
            "case {member:?} GetPrimaryEmail: expected the named account's own address, \
             got {reported:?}"
        );
    }
}
