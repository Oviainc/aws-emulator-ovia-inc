// SPDX-License-Identifier: CC0-1.0

//! An `AccountId` this service holds no record of, over every operation that
//! does not bring an account record into existence.
//!
//! The rule under test: naming another account requires the caller to be an
//! identity in the organization's management (or delegated administrator)
//! account with the named account a member of the same organization, so an
//! account this service holds no record of is an AUTHORIZATION refusal — HTTP
//! 403 `AccessDeniedException` — decided before anything about that account's
//! sub-resources (its alternate contacts, its primary contact, its email
//! update, its Region opt statuses) is looked at or reported.
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

/// The values `AlternateContactType` admits, from
/// `smithy_query(query="shape", shape="AlternateContactType")` — spelled out
/// here, not imported, so a wrong set in the implementation cannot make this
/// test pass.
const ALTERNATE_CONTACT_TYPES: [&str; 3] = ["BILLING", "OPERATIONS", "SECURITY"];

/// A Region code this service's catalog holds, needed by the Region reads. Not
/// the `us-east-1` the other suites use, so the case is not the cited one.
const CATALOGUED_REGION: &str = "af-south-1";

/// The one sentence Account Management states for an alternate contact that is
/// not set, spelled out rather than imported so a per-request message cannot
/// make this test pass.
const ALTERNATE_CONTACT_NOT_FOUND: &str = "No contact of the inputted alternate contact type found.";

/// A passcode inside `Otp`'s `^[a-zA-Z0-9]{6}$`, so the phone-number and email
/// operations are refused for the ACCOUNT, never for a malformed member.
const WELL_FORMED_OTP: &str = "A1b2C3";

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

/// One row of the class: a label, the operation's URI, and its body for the
/// account id under test. Every one of these operations acts on a resource of an
/// account that already exists, so none of them may create a record.
type Row = (String, &'static str, fn(&str) -> Value);

/// Every non-creating operation of this service, with a valid body for
/// `account_id`. The rows differ in operation, in which sub-resource the call
/// names and in the shape of the request (a bare account id, an account id plus
/// an enum, plus a Region, plus a passcode, plus a list filter), which is what
/// the rule generalises over.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = vec![
        (
            "GetAccountInformation".to_string(),
            "/getAccountInformation",
            |id| json!({"AccountId": id}),
        ),
        (
            "GetContactInformation".to_string(),
            "/getContactInformation",
            |id| json!({"AccountId": id}),
        ),
        (
            "GetPrimaryEmail".to_string(),
            "/getPrimaryEmail",
            |id| json!({"AccountId": id}),
        ),
        (
            "GetPrimaryEmailUpdateStatus".to_string(),
            "/getPrimaryEmailUpdateStatus",
            |id| json!({"AccountId": id}),
        ),
        (
            "GetRegionOptStatus".to_string(),
            "/getRegionOptStatus",
            |id| json!({"AccountId": id, "RegionName": CATALOGUED_REGION}),
        ),
        (
            "ListRegions".to_string(),
            "/listRegions",
            |id| json!({"AccountId": id}),
        ),
        (
            "ListRegions filtered and paged".to_string(),
            "/listRegions",
            |id| {
                json!({
                    "AccountId": id,
                    "MaxResults": 1,
                    "RegionOptStatusContains": ["ENABLED_BY_DEFAULT"],
                })
            },
        ),
        (
            "SendPhoneNumberVerification".to_string(),
            "/sendPhoneNumberVerification",
            |id| json!({"AccountId": id}),
        ),
        (
            "VerifyPhoneNumber".to_string(),
            "/verifyPhoneNumber",
            |id| json!({"AccountId": id, "Otp": WELL_FORMED_OTP}),
        ),
        (
            "AcceptPrimaryEmailUpdate".to_string(),
            "/acceptPrimaryEmailUpdate",
            |id| {
                json!({
                    "AccountId": id,
                    "PrimaryEmail": "ada@example.com",
                    "Otp": WELL_FORMED_OTP,
                })
            },
        ),
    ];

    // The alternate-contact reads and deletes, one row per contact type: the
    // contact type must make no difference to the refusal.
    for contact_type in ALTERNATE_CONTACT_TYPES {
        let get: fn(&str) -> Value = match contact_type {
            "BILLING" => |id| json!({"AccountId": id, "AlternateContactType": "BILLING"}),
            "OPERATIONS" => |id| json!({"AccountId": id, "AlternateContactType": "OPERATIONS"}),
            _ => |id| json!({"AccountId": id, "AlternateContactType": "SECURITY"}),
        };
        rows.push((
            format!("GetAlternateContact {contact_type}"),
            "/getAlternateContact",
            get,
        ));
        rows.push((
            format!("DeleteAlternateContact {contact_type}"),
            "/deleteAlternateContact",
            get,
        ));
    }

    rows
}

/// Account ids inside `AccountId`'s `^\d{12}$` that no request has ever written:
/// ids differing in their digits, including the bounds of the 12-digit space.
fn unknown_account_ids() -> Vec<String> {
    let ids = vec![
        "210987654321".to_string(),
        "999999999999".to_string(),
        "000000000001".to_string(),
        "123123123123".to_string(),
    ];
    for id in &ids {
        assert!(
            id.len() == 12 && id.bytes().all(|b| b.is_ascii_digit()),
            "case {id:?}: the test's account id must satisfy ^\\d{{12}}$"
        );
        assert_ne!(
            id,
            default_account_id(),
            "case {id:?}: the test's account id must not be the caller's own account"
        );
    }
    ids
}

/// Every non-creating operation refuses an account the store holds no record of
/// with HTTP 403 `AccessDeniedException`, whatever sub-resource the call names —
/// and says nothing about that account's sub-resources.
#[tokio::test]
async fn an_account_with_no_record_is_an_authorization_refusal() {
    let router = emulator();

    for account_id in unknown_account_ids() {
        for (label, uri, body) in rows() {
            let case = format!("{label} on the unknown account {account_id}");
            let answer = call(&router, uri, body(&account_id)).await;
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected HTTP 403, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {:?}",
                answer.body
            );
            assert_ne!(
                answer.message(),
                ALTERNATE_CONTACT_NOT_FOUND,
                "case {case:?}: the refusal must not report on the account's sub-resources, got {:?}",
                answer.message()
            );
        }
    }
}

/// A refused call writes nothing: the account is still unknown afterwards, so a
/// later caller sees exactly what this one saw.
#[tokio::test]
async fn a_refused_call_leaves_the_account_unknown() {
    let router = emulator();

    for account_id in unknown_account_ids() {
        for (label, uri, body) in rows() {
            let case = format!("{label} on the unknown account {account_id}");
            call(&router, uri, body(&account_id)).await;

            let after = call(&router, "/getAccountInformation", json!({"AccountId": account_id}))
                .await;
            assert_eq!(
                after.status, 403,
                "case {case:?}: the refused call must leave the account unknown, got {} with {:?}",
                after.status, after.body
            );
            assert_eq!(
                after.error_code(),
                "AccessDeniedException",
                "case {case:?}: the refused call must leave the account unknown, got {:?}",
                after.body
            );
        }
    }
}

/// Once the store DOES hold the account, the same call is no longer an
/// authorization refusal: whatever it answers, it is about the sub-resource the
/// call names, which is the distinction a single 404 for both causes erases.
#[tokio::test]
async fn an_account_with_a_record_is_not_an_authorization_refusal() {
    let router = emulator();
    let account_id = "210987654321";

    let created = call(
        &router,
        "/putAccountName",
        json!({"AccountName": "Member", "AccountId": account_id}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the member account's record must be created, got {} with {:?}",
        created.status, created.body
    );

    for (label, uri, body) in rows() {
        let case = format!("{label} on the known account {account_id}");
        let answer = call(&router, uri, body(account_id)).await;
        assert_ne!(
            answer.status, 403,
            "case {case:?}: an account the store holds must not be an authorization refusal, got {:?}",
            answer.body
        );
        assert_ne!(
            answer.error_code(),
            "AccessDeniedException",
            "case {case:?}: an account the store holds must not be an authorization refusal, got {:?}",
            answer.body
        );
    }
}

/// On an account the store holds, an alternate contact type with nothing stored
/// is the 404 `ResourceNotFoundException` — for both the caller's own account
/// and a member account, and for every contact type — while the SAME call on an
/// account with no record is the 403 above.
#[tokio::test]
async fn an_unset_contact_on_a_known_account_is_still_not_found() {
    let router = emulator();
    let member = "210987654321";
    let created = call(
        &router,
        "/putAccountName",
        json!({"AccountName": "Member", "AccountId": member}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the member account's record must be created, got {} with {:?}",
        created.status, created.body
    );

    for account in [default_account_id(), member] {
        for contact_type in ALTERNATE_CONTACT_TYPES {
            for uri in ["/getAlternateContact", "/deleteAlternateContact"] {
                let case = format!("{uri} {contact_type} on the known account {account}");
                let answer = call(
                    &router,
                    uri,
                    json!({"AccountId": account, "AlternateContactType": contact_type}),
                )
                .await;
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}: expected HTTP 404, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.message(),
                    ALTERNATE_CONTACT_NOT_FOUND,
                    "case {case:?}: expected the constant not-found message, got {:?}",
                    answer.message()
                );
            }
        }
    }
}

/// The refusal is about the account, not about the operation: with the contact
/// stored on an account the store holds, the read round-trips, and deleting it
/// answers 200 — while the same pair on an unknown account stays a 403.
#[tokio::test]
async fn a_stored_contact_round_trips_on_a_known_account() {
    let router = emulator();
    let member = "210987654321";

    for contact_type in ALTERNATE_CONTACT_TYPES {
        let stored = json!({
            "AccountId": member,
            "AlternateContactType": contact_type,
            "Name": format!("{contact_type} Name"),
            "Title": format!("{contact_type} Title"),
            "EmailAddress": "ada@example.com",
            "PhoneNumber": "+1 206-555-0100",
        });
        // The put is the operation that may bring the record into existence.
        let put = call(&router, "/putAlternateContact", stored.clone()).await;
        assert_eq!(
            put.status, 200,
            "case {contact_type:?}: the put must answer 200, got {} with {:?}",
            put.status, put.body
        );

        let read = call(
            &router,
            "/getAlternateContact",
            json!({"AccountId": member, "AlternateContactType": contact_type}),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {contact_type:?}: the stored contact must read back 200, got {} with {:?}",
            read.status, read.body
        );
        for member_name in ["Name", "Title", "EmailAddress", "PhoneNumber", "AlternateContactType"] {
            assert_eq!(
                read.body["AlternateContact"][member_name], stored[member_name],
                "case {contact_type:?}: {member_name} must be read back as stored, got {:?}",
                read.body["AlternateContact"][member_name]
            );
        }

        let deleted = call(
            &router,
            "/deleteAlternateContact",
            json!({"AccountId": member, "AlternateContactType": contact_type}),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {contact_type:?}: the delete must answer 200, got {} with {:?}",
            deleted.status, deleted.body
        );

        // The same pair on an account with no record is the authorization
        // refusal, not this 200 and not a 404.
        for uri in ["/getAlternateContact", "/deleteAlternateContact"] {
            let unknown = call(
                &router,
                uri,
                json!({"AccountId": "999999999999", "AlternateContactType": contact_type}),
            )
            .await;
            assert_eq!(
                unknown.status, 403,
                "case {uri} {contact_type:?}: an unknown account must be refused 403, got {} with {:?}",
                unknown.status, unknown.body
            );
        }
    }
}
