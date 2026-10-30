// SPDX-License-Identifier: CC0-1.0

//! The COMPOSITION of this service's account-target rule — resolve the
//! account-id member, then refuse an id this emulator holds no record of —
//! over every operation that acts on an account which must already exist.
//!
//! The rule under test: one composition answers every such operation, so for
//! one account id the refusal is ONE payload — status, error code and message
//! byte-identical — whichever of those operations the request names and
//! whatever else the request carries, the two causes (the caller's own account,
//! an account with no record) keep their own distinct payloads, and the only
//! part of the unknown-account message that varies between ids is the account
//! id itself. The resolving arm is tested too: a request naming an account the
//! store holds lands on THAT record, and one naming no account lands on the
//! caller's own.
//!
//! Every case goes through the router, so it exercises routing,
//! deserialization, the handler and the persisted store: the path a client
//! takes.

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

/// The modelled error code and HTTP status of this service's account-target
/// refusal: `AccessDeniedException` at HTTP 403 (`smithy_query(query="operation",
/// operation="GetAccountInformation")`).
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// A Region code this service's catalog holds, needed by the Region reads.
/// `eu-north-1` is not the Region the other suites drive, so the case is not a
/// cited one.
const CATALOGUED_REGION: &str = "eu-north-1";

/// A passcode inside `Otp`'s modelled `^[a-zA-Z0-9]{6}$`, so a passcode-bearing
/// row is refused for its ACCOUNT and never for a malformed member.
const WELL_FORMED_OTP: &str = "Q7r4Tz";

/// An address inside `PrimaryEmailAddress`'s modelled `@length(min: 5, max:
/// 64)` and this model's email expression, so the acceptance row is refused for
/// its account and never for a malformed member.
const WELL_FORMED_EMAIL: &str = "grace@example.com";

/// The placeholder the account id is folded into before two messages are
/// compared: what is left is the FIXED text of the refusal, which the
/// composition states once.
const ACCOUNT_ID_PLACEHOLDER: &str = "{AccountId}";

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

    /// The whole refusal as one comparable value: status, error code and
    /// message with `account_id` folded into a placeholder.
    fn refusal(&self, account_id: &str) -> (u16, String, String) {
        (
            self.status,
            self.error_code().to_string(),
            self.message().replace(account_id, ACCOUNT_ID_PLACEHOLDER),
        )
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

/// One row: a label, the operation's URI, and its request body for the account
/// the row names.
type Row = (&'static str, &'static str, fn(&str) -> Value);

/// Every operation that goes through the composition, one row each, with the
/// account id under test in its account-id member. The rows differ in which
/// operation runs, in whether it reports or writes, in which sub-resource it
/// names and in the shape of the request (a bare account id, an account id plus
/// an enum, plus a Region, plus a passcode, plus an address and a passcode, plus
/// a page size and a list filter) — which is what the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        // The reporting operations: the composition hands back the record to
        // read.
        ("GetAccountInformation", "/getAccountInformation", |id| {
            json!({"AccountId": id})
        }),
        ("GetContactInformation", "/getContactInformation", |id| {
            json!({"AccountId": id})
        }),
        ("GetAlternateContact", "/getAlternateContact", |id| {
            json!({"AccountId": id, "AlternateContactType": "SECURITY"})
        }),
        ("GetPrimaryEmail", "/getPrimaryEmail", |id| {
            json!({"AccountId": id})
        }),
        (
            "GetPrimaryEmailUpdateStatus",
            "/getPrimaryEmailUpdateStatus",
            |id| json!({"AccountId": id}),
        ),
        ("GetRegionOptStatus", "/getRegionOptStatus", |id| {
            json!({"AccountId": id, "RegionName": CATALOGUED_REGION})
        }),
        ("ListRegions", "/listRegions", |id| {
            json!({"AccountId": id})
        }),
        ("ListRegions filtered and paged", "/listRegions", |id| {
            json!({
                "AccountId": id,
                "MaxResults": 2,
                "RegionOptStatusContains": ["ENABLED_BY_DEFAULT", "ENABLED"],
            })
        }),
        (
            // The account this one names is its `StandardAccountId`: the same
            // "the account to operate on" member under another name, resolved
            // by the same composition.
            "GetGovCloudAccountInformation",
            "/getGovCloudAccountInformation",
            |id| json!({"StandardAccountId": id}),
        ),
        // The writing operations that must not bring a record into existence:
        // the composition hands back the record to change.
        ("DeleteAlternateContact", "/deleteAlternateContact", |id| {
            json!({"AccountId": id, "AlternateContactType": "OPERATIONS"})
        }),
        (
            "SendPhoneNumberVerification",
            "/sendPhoneNumberVerification",
            |id| json!({"AccountId": id}),
        ),
        ("VerifyPhoneNumber", "/verifyPhoneNumber", |id| {
            json!({"AccountId": id, "Otp": WELL_FORMED_OTP})
        }),
        (
            "AcceptPrimaryEmailUpdate",
            "/acceptPrimaryEmailUpdate",
            |id| {
                json!({
                    "AccountId": id,
                    "PrimaryEmail": WELL_FORMED_EMAIL,
                    "Otp": WELL_FORMED_OTP,
                })
            },
        ),
    ]
}

/// Account ids inside `AccountId`'s modelled `^\d{12}$` that no request has
/// ever written: they differ in which digits carry the value, so a refusal
/// keyed on anything looser than "the store holds no such record" would miss
/// one.
const UNKNOWN_ACCOUNTS: [&str; 4] = [
    "400000000002",
    "888888888888",
    "000000000003",
    "314159265358",
];

/// Every operation sharing the composition answers an account with no record
/// with the IDENTICAL refusal: one status, one error code and one message whose
/// only varying part is the account id asked about.
#[tokio::test]
async fn an_unknown_account_is_one_refusal_for_every_operation() {
    let router = emulator();

    // The refusal the first case answers, with the account id folded out: every
    // later case — other operation, other account id — must answer it too.
    let mut reference: Option<(String, (u16, String, String))> = None;
    for account_id in UNKNOWN_ACCOUNTS {
        assert_ne!(
            account_id,
            default_account_id(),
            "case {account_id:?}: the test's account id must not be the caller's own account"
        );
        for (label, uri, body) in rows() {
            let case = format!("{label} on the unknown account {account_id}");
            let answer = call(&router, uri, body(account_id)).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                DENIED,
                "case {case:?}: expected {DENIED:?}, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert!(
                answer.message().contains(account_id),
                "case {case:?}: the refusal must name the account asked about, got {:?}",
                answer.message()
            );
            let observed = answer.refusal(account_id);
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
    assert!(
        reference.is_some(),
        "the table must drive at least one operation, got none"
    );
}

/// The OTHER arm of the same composition — the member naming the caller's own
/// account — is one payload across the same operations as well, and it is NOT
/// the unknown-account payload: the two causes stay distinguishable.
#[tokio::test]
async fn the_two_causes_keep_their_own_one_payload_each() {
    let router = emulator();
    let own = default_account_id().to_string();
    let unknown = UNKNOWN_ACCOUNTS[0];

    let unknown_refusal = {
        let answer = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": unknown}),
        )
        .await;
        answer.refusal(unknown)
    };

    let mut reference: Option<(String, (u16, String, String))> = None;
    for (label, uri, body) in rows() {
        let case = format!("{label} naming the caller's own account");
        let answer = call(&router, uri, body(&own)).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            DENIED,
            "case {case:?}: expected {DENIED:?}, got {} with {:?}",
            answer.status,
            answer.body
        );
        let observed = answer.refusal(&own);
        assert_ne!(
            observed, unknown_refusal,
            "case {case:?}: the self-target refusal must stay distinct from the \
             unknown-account one, got {observed:?}"
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

/// The resolving arm: once the store holds the account, the same composition
/// hands every one of those operations that record — so none of them is an
/// authorization refusal any more, and each answers about the sub-resource it
/// names.
#[tokio::test]
async fn an_account_the_store_holds_is_resolved_by_every_operation() {
    let router = emulator();
    let member = UNKNOWN_ACCOUNTS[1];
    let member_name = "Composition Member";

    let created = call(
        &router,
        "/putAccountName",
        json!({"AccountName": member_name, "AccountId": member}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the member account's record must be created, got {} with {:?}",
        created.status, created.body
    );

    for (label, uri, body) in rows() {
        let case = format!("{label} on the known account {member}");
        let answer = call(&router, uri, body(member)).await;
        assert_ne!(
            (answer.status, answer.error_code()),
            DENIED,
            "case {case:?}: an account the store holds must not be an authorization \
             refusal, got {:?}",
            answer.body
        );
    }
}

/// The resolution the composition starts from is unchanged by it: a named
/// account lands on THAT record, and an absent member lands on the caller's own
/// — read back through `GetAccountInformation`.
#[tokio::test]
async fn the_composition_lands_on_the_account_the_member_names() {
    let router = emulator();
    let own = default_account_id().to_string();

    for (index, member) in UNKNOWN_ACCOUNTS.iter().enumerate() {
        let member_name = format!("Member {index}");
        let created = call(
            &router,
            "/putAccountName",
            json!({"AccountName": member_name, "AccountId": member}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {member:?}: the member account's record must be created, got {} with {:?}",
            created.status, created.body
        );

        let read = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": member}),
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.body["AccountId"].as_str().unwrap_or_default(),
                read.body["AccountName"].as_str().unwrap_or_default(),
            ),
            (200, *member, member_name.as_str()),
            "case {member:?}: the read must land on the account the member names, got {} \
             with {:?}",
            read.status,
            read.body
        );
    }

    // No member: the standalone call on the caller's own account, which the
    // member accounts above left untouched.
    let standalone = call(&router, "/getAccountInformation", json!({})).await;
    assert_eq!(
        (
            standalone.status,
            standalone.body["AccountId"].as_str().unwrap_or_default(),
        ),
        (200, own.as_str()),
        "case \"standalone\": an absent member must land on the caller's own account, got {} \
         with {:?}",
        standalone.status,
        standalone.body
    );
}
