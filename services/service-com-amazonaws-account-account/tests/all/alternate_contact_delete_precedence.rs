// SPDX-License-Identifier: CC0-1.0

//! `DeleteAlternateContact` resolves the ACCOUNT before it looks at that
//! account's alternate contacts.
//!
//! The rule under test: for an operation that acts on a sub-resource of an
//! account, naming an account this service holds no record of is an
//! authorization refusal — HTTP 403 `AccessDeniedException` — decided before
//! the sub-resource is looked up, so the answer never depends on what any
//! OTHER account has stored under the same sub-resource name, and never
//! reports a not-found about an account the caller has no standing on.
//!
//! The dimension this suite adds over `unknown_account_refusal`: the same
//! contact type is deliberately STORED on the caller's own account and on a
//! known member account first, so a handler that looked the contact up before
//! resolving the account (or that leaked another account's record) would answer
//! 200 or 404 here instead of 403. Every case goes through the router, so it
//! exercises routing, deserialization, the handler and the persisted store.

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

/// The one sentence Account Management states for an alternate contact that is
/// not set, spelled out rather than imported so a changed constant cannot make
/// this test pass.
const ALTERNATE_CONTACT_NOT_FOUND: &str = "No contact of the inputted alternate contact type found.";

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

/// A full alternate contact body for `owner` / `contact_type`, valid under
/// `PutAlternateContact`'s required members. `None` is the caller's own account,
/// which a request reaches by OMITTING `AccountId`.
fn contact_body(owner: Option<&str>, contact_type: &str) -> Value {
    let label = owner.unwrap_or("the standalone account");
    let mut body = json!({
        "AlternateContactType": contact_type,
        "Name": format!("{contact_type} on {label}"),
        "Title": format!("{contact_type} Lead"),
        "EmailAddress": "grace@example.com",
        "PhoneNumber": "+1 206-555-0199",
    });
    if let Some(account_id) = owner {
        body["AccountId"] = json!(account_id);
    }
    body
}

/// Account ids inside `AccountId`'s `^\d{12}$` that no request ever writes:
/// ids differing in their digits, including the ends of the 12-digit space, and
/// none of them the ids the issue or the sibling suite cite.
fn unknown_account_ids() -> Vec<String> {
    let ids = vec![
        "000000000002".to_string(),
        "999999999998".to_string(),
        "314159265358".to_string(),
        "500000000005".to_string(),
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

/// The member account this suite populates, so a contact of every type really
/// is stored on an account OTHER than the ones under test.
const POPULATED_MEMBER: &str = "876543210987";

/// The accounts this suite populates: the caller's own (named by omitting
/// `AccountId`) and one member account.
const POPULATED_ACCOUNTS: [Option<&str>; 2] = [None, Some(POPULATED_MEMBER)];

/// Store every contact type on the caller's own account and on one known member
/// account, so the sub-resource the refused calls name exists elsewhere.
async fn populate_contacts_elsewhere(router: &MultiServiceRouter) {
    for owner in POPULATED_ACCOUNTS {
        for contact_type in ALTERNATE_CONTACT_TYPES {
            let put = call(router, "/putAlternateContact", contact_body(owner, contact_type)).await;
            assert_eq!(
                put.status, 200,
                "case {contact_type:?} on {owner:?}: the setup put must answer 200, got {} with {:?}",
                put.status, put.body
            );
        }
    }
}

/// A request naming `owner`'s alternate contact of `contact_type`.
fn contact_request(owner: Option<&str>, contact_type: &str) -> Value {
    let mut request = json!({"AlternateContactType": contact_type});
    if let Some(account_id) = owner {
        request["AccountId"] = json!(account_id);
    }
    request
}

/// Deleting a contact on an account with no record is the 403 authorization
/// refusal even though the very same contact type is stored on other accounts —
/// and the refusal says nothing about contacts.
#[tokio::test]
async fn delete_on_an_account_with_no_record_is_refused_before_the_contact_lookup() {
    let router = emulator();
    populate_contacts_elsewhere(&router).await;

    for account_id in unknown_account_ids() {
        for contact_type in ALTERNATE_CONTACT_TYPES {
            // Two attempts: the refusal is the same answer every time, so it
            // cannot be a one-shot that a repeat turns into a 404 or a 200.
            for attempt in 1..=2 {
                let case =
                    format!("DeleteAlternateContact {contact_type} on unknown {account_id}, attempt {attempt}");
                let answer = call(
                    &router,
                    "/deleteAlternateContact",
                    json!({"AccountId": account_id, "AlternateContactType": contact_type}),
                )
                .await;
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
                    "case {case:?}: the refusal must not report on the account's contacts, got {:?}",
                    answer.message()
                );
            }
        }
    }
}

/// The refused delete wrote nothing and consumed nothing: the contacts stored on
/// the other accounts are all still readable afterwards, and those accounts'
/// deletes still succeed.
#[tokio::test]
async fn a_refused_delete_leaves_every_other_account_untouched() {
    let router = emulator();
    populate_contacts_elsewhere(&router).await;

    for account_id in unknown_account_ids() {
        for contact_type in ALTERNATE_CONTACT_TYPES {
            call(
                &router,
                "/deleteAlternateContact",
                json!({"AccountId": account_id, "AlternateContactType": contact_type}),
            )
            .await;
        }
    }

    // Nothing the refusals touched: every stored contact still reads back, and
    // the unknown accounts are still unknown.
    for owner in POPULATED_ACCOUNTS {
        for contact_type in ALTERNATE_CONTACT_TYPES {
            let case = format!("{contact_type} on {owner:?} after the refusals");
            let read = call(
                &router,
                "/getAlternateContact",
                contact_request(owner, contact_type),
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the stored contact must still read back 200, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body["AlternateContact"]["Name"],
                contact_body(owner, contact_type)["Name"],
                "case {case:?}: the stored Name must be unchanged, got {:?}",
                read.body["AlternateContact"]["Name"]
            );
        }
    }

    for account_id in unknown_account_ids() {
        let case = format!("account {account_id} after the refusals");
        let after = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": account_id}),
        )
        .await;
        assert_eq!(
            after.status, 403,
            "case {case:?}: the refused delete must not have created a record, got {} with {:?}",
            after.status, after.body
        );
    }
}

/// On an account the store DOES hold, the same delete reaches the contact
/// lookup: it answers 200 while the contact is stored, then the modelled 404
/// once it is gone — the two outcomes a single refusal for both causes erases.
#[tokio::test]
async fn delete_on_a_known_account_reaches_the_contact_lookup() {
    let router = emulator();
    populate_contacts_elsewhere(&router).await;

    for owner in POPULATED_ACCOUNTS {
        for contact_type in ALTERNATE_CONTACT_TYPES {
            let request = contact_request(owner, contact_type);

            let case = format!("first delete of {contact_type} on {owner:?}");
            let deleted = call(&router, "/deleteAlternateContact", request.clone()).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                deleted.status, deleted.body
            );

            let case = format!("second delete of {contact_type} on {owner:?}");
            let again = call(&router, "/deleteAlternateContact", request.clone()).await;
            assert_eq!(
                again.status, 404,
                "case {case:?}: expected HTTP 404, got {} with {:?}",
                again.status, again.body
            );
            assert_eq!(
                again.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                again.body
            );
            assert_eq!(
                again.message(),
                ALTERNATE_CONTACT_NOT_FOUND,
                "case {case:?}: expected the constant not-found message, got {:?}",
                again.message()
            );

            let case = format!("read back {contact_type} on {owner:?} after the delete");
            let read = call(&router, "/getAlternateContact", request).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: the deleted contact must be gone, got {} with {:?}",
                read.status, read.body
            );
        }
    }
}
