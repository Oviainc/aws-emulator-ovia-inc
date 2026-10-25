// SPDX-License-Identifier: CC0-1.0

//! An account-id member that names the CALLER'S OWN account, over every
//! operation of this service that takes one.
//!
//! The rule under test: an account-id member of an Account Management operation
//! names a member account of the caller's organization, so a request whose
//! member names the caller's own account is refused with HTTP 403
//! `AccessDeniedException` — the caller's own account is reached by the
//! standalone call, which omits the member entirely. The model states it on
//! every one of these members: "The management account can't specify its own
//! `AccountId`; it must call the operation in standalone context by not
//! including the `AccountId` parameter"
//! (`smithy_query(query="doc", shape="GetAccountInformationRequest",
//! member="AccountId")`).
//!
//! The rows span what the rule generalises over: the operation, whether it
//! reports or writes, whether its account-id member is optional or required,
//! what that member is called (`AccountId`, and `StandardAccountId` on
//! `GetGovCloudAccountInformation`) and the shape of the rest of the request (a
//! bare account id, an account id plus an enum, plus a Region, plus a passcode,
//! plus an address, plus a list filter). Every case goes through the router, so
//! it exercises routing, deserialization, the handler and the persisted store.

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

/// The modelled error code and HTTP status of the refusal, from
/// `smithy_query(query="service_info")`: `AccessDeniedException` is a common
/// error of every operation of this service, at HTTP 403.
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The values `AlternateContactType` admits, from
/// `smithy_query(query="shape", shape="AlternateContactType")` — spelled out
/// here, not imported, so a wrong set in the implementation cannot make this
/// test pass.
const ALTERNATE_CONTACT_TYPES: [&str; 3] = ["BILLING", "OPERATIONS", "SECURITY"];

/// Region codes this service's catalog holds, needed by the Region operations:
/// one AWS turns on by default and one that is opted into.
const DEFAULT_ON_REGION: &str = "eu-west-1";
const OPT_IN_REGION: &str = "ap-east-1";

/// A passcode inside `Otp`'s `^[a-zA-Z0-9]{6}$`, so a passcode operation is
/// refused for the ACCOUNT it names and never for a malformed member.
const WELL_FORMED_OTP: &str = "A1b2C3";

/// An address inside `PrimaryEmail`'s `@length(min: 5, max: 64)`.
const WELL_FORMED_EMAIL: &str = "ada@example.com";

/// Member accounts of the caller's organization this suite writes records for.
/// They sit one digit away from the caller's own id in either direction, so a
/// refusal that keyed on anything looser than exact equality would catch them.
const MEMBER_ACCOUNTS: [&str; 3] = ["000000000001", "100000000000", "210987654321"];

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

/// A full `ContactInformation`, valid under `PutContactInformation`'s required
/// members.
fn contact_information() -> Value {
    json!({
        "FullName": "Ada Lovelace",
        "AddressLine1": "1 Analytical Way",
        "City": "Seattle",
        "PostalCode": "98109",
        "CountryCode": "US",
        "PhoneNumber": "+12065550100",
    })
}

/// One row: a label, the operation's URI, and its request body for the account
/// the row targets — `None` meaning the member is left out altogether.
type OptionalRow = (&'static str, &'static str, fn(Option<&str>) -> Value);

/// The request `body` for `account`, with the account-id member `member` set
/// only when the row names an account.
fn with_account(mut body: Value, member: &str, account: Option<&str>) -> Value {
    if let Some(account_id) = account {
        body[member] = json!(account_id);
    }
    body
}

/// Every operation of this service whose account-id member is OPTIONAL, so the
/// same row can be driven both as the standalone call and as a call naming an
/// account.
fn optional_rows() -> Vec<OptionalRow> {
    let mut rows: Vec<OptionalRow> = vec![
        ("GetAccountInformation", "/getAccountInformation", |account| {
            with_account(json!({}), "AccountId", account)
        }),
        ("PutAccountName", "/putAccountName", |account| {
            with_account(json!({"AccountName": "Renamed"}), "AccountId", account)
        }),
        ("GetContactInformation", "/getContactInformation", |account| {
            with_account(json!({}), "AccountId", account)
        }),
        ("PutContactInformation", "/putContactInformation", |account| {
            with_account(
                json!({"ContactInformation": contact_information()}),
                "AccountId",
                account,
            )
        }),
        (
            "GetPrimaryEmailUpdateStatus",
            "/getPrimaryEmailUpdateStatus",
            |account| with_account(json!({}), "AccountId", account),
        ),
        (
            // The account this one names is its `StandardAccountId`: the same
            // "the account to operate on" member under another name, carrying
            // the same documented rule.
            "GetGovCloudAccountInformation",
            "/getGovCloudAccountInformation",
            |account| with_account(json!({}), "StandardAccountId", account),
        ),
        ("GetRegionOptStatus", "/getRegionOptStatus", |account| {
            with_account(
                json!({"RegionName": DEFAULT_ON_REGION}),
                "AccountId",
                account,
            )
        }),
        ("ListRegions", "/listRegions", |account| {
            with_account(json!({}), "AccountId", account)
        }),
        ("ListRegions filtered and paged", "/listRegions", |account| {
            with_account(
                json!({"MaxResults": 1, "RegionOptStatusContains": ["ENABLED_BY_DEFAULT"]}),
                "AccountId",
                account,
            )
        }),
        ("EnableRegion", "/enableRegion", |account| {
            with_account(json!({"RegionName": OPT_IN_REGION}), "AccountId", account)
        }),
        ("DisableRegion", "/disableRegion", |account| {
            with_account(json!({"RegionName": OPT_IN_REGION}), "AccountId", account)
        }),
        (
            "SendPhoneNumberVerification",
            "/sendPhoneNumberVerification",
            |account| with_account(json!({}), "AccountId", account),
        ),
        ("VerifyPhoneNumber", "/verifyPhoneNumber", |account| {
            with_account(json!({"Otp": WELL_FORMED_OTP}), "AccountId", account)
        }),
    ];

    // The alternate-contact rows, one per contact type: which contact type the
    // request names makes no difference to the rule.
    for contact_type in ALTERNATE_CONTACT_TYPES {
        let put: fn(Option<&str>) -> Value = match contact_type {
            "BILLING" => |account| alternate_contact_body("BILLING", account),
            "OPERATIONS" => |account| alternate_contact_body("OPERATIONS", account),
            _ => |account| alternate_contact_body("SECURITY", account),
        };
        let name: fn(Option<&str>) -> Value = match contact_type {
            "BILLING" => |account| {
                with_account(
                    json!({"AlternateContactType": "BILLING"}),
                    "AccountId",
                    account,
                )
            },
            "OPERATIONS" => |account| {
                with_account(
                    json!({"AlternateContactType": "OPERATIONS"}),
                    "AccountId",
                    account,
                )
            },
            _ => |account| {
                with_account(
                    json!({"AlternateContactType": "SECURITY"}),
                    "AccountId",
                    account,
                )
            },
        };
        rows.push(("PutAlternateContact", "/putAlternateContact", put));
        rows.push(("GetAlternateContact", "/getAlternateContact", name));
        rows.push(("DeleteAlternateContact", "/deleteAlternateContact", name));
    }

    rows
}

/// A full alternate contact of `contact_type`, valid under
/// `PutAlternateContact`'s required members.
fn alternate_contact_body(contact_type: &str, account: Option<&str>) -> Value {
    with_account(
        json!({
            "AlternateContactType": contact_type,
            "Name": "Grace Hopper",
            "Title": "Rear Admiral",
            "EmailAddress": "grace@example.com",
            "PhoneNumber": "+1 206-555-0199",
        }),
        "AccountId",
        account,
    )
}

/// One row whose account-id member is REQUIRED: a label, the operation's URI,
/// and its request body for the account it names.
type RequiredRow = (&'static str, &'static str, fn(&str) -> Value);

/// The operations whose `AccountId` is REQUIRED: the standalone call does not
/// exist for them, so only the self-naming row applies — and the rule is the
/// same one.
fn required_rows() -> Vec<RequiredRow> {
    vec![
        ("GetPrimaryEmail", "/getPrimaryEmail", |id| {
            json!({"AccountId": id})
        }),
        ("StartPrimaryEmailUpdate", "/startPrimaryEmailUpdate", |id| {
            json!({"AccountId": id, "PrimaryEmail": WELL_FORMED_EMAIL})
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

/// Every operation that takes an account-id member refuses a request naming the
/// caller's own account with the modelled 403 `AccessDeniedException`, and the
/// refusal is the same answer however often it is repeated.
#[tokio::test]
async fn naming_the_callers_own_account_is_refused_by_every_operation() {
    let router = emulator();
    let own = default_account_id().to_string();

    let mut cases: Vec<(String, &str, Value)> = Vec::new();
    for (label, uri, body) in optional_rows() {
        cases.push((
            format!("{label} naming the caller's own id"),
            uri,
            body(Some(&own)),
        ));
    }
    for (label, uri, body) in required_rows() {
        cases.push((
            format!("{label} naming the caller's own id"),
            uri,
            body(&own),
        ));
    }

    for (case, uri, body) in cases {
        // Twice: a refusal is not a one-shot that a repeat turns into a 200.
        for attempt in 1..=2 {
            let answer = call(&router, uri, body.clone()).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                DENIED,
                "case {case:?} (attempt {attempt}): expected {DENIED:?}, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert!(
                !answer.message().contains(&own),
                "case {case:?} (attempt {attempt}): the refusal must not echo the account id, \
                 got {:?}",
                answer.message()
            );
        }
    }
}

/// The standalone call — the account-id member left out — is served: whatever it
/// answers is about the account's own resources, never the authorization
/// refusal that naming the caller's own id earns.
#[tokio::test]
async fn the_standalone_call_is_served_for_every_optional_member() {
    let router = emulator();

    for (label, uri, body) in optional_rows() {
        let case = format!("{label} with no account id at all");
        let answer = call(&router, uri, body(None)).await;
        assert_ne!(
            (answer.status, answer.error_code()),
            DENIED,
            "case {case:?}: the standalone call must not be the self-target refusal, got {} \
             with {:?}",
            answer.status,
            answer.body
        );
    }
}

/// A MEMBER account of the organization is served, including ids one digit away
/// from the caller's own: the refusal keys on being the caller's own account,
/// not on resembling it.
#[tokio::test]
async fn a_member_account_the_store_holds_is_not_refused() {
    let router = emulator();

    for member in MEMBER_ACCOUNTS {
        // The record comes into existence here, so a later refusal cannot be the
        // "no record of that account" one.
        let created = call(
            &router,
            "/putAccountName",
            json!({"AccountName": "Member", "AccountId": member}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {member:?}: the member record must be created, got {} with {:?}",
            created.status, created.body
        );

        for (label, uri, body) in optional_rows() {
            let case = format!("{label} naming the member account {member}");
            let answer = call(&router, uri, body(Some(member))).await;
            assert_ne!(
                (answer.status, answer.error_code()),
                DENIED,
                "case {case:?}: a member account must not be refused, got {} with {:?}",
                answer.status,
                answer.body
            );
        }
    }
}

/// A refused self-naming call changes nothing: every standalone read of the
/// caller's own account reports exactly what it reported before, so no record,
/// contact, Region opt status or passcode was written by the refusal.
#[tokio::test]
async fn a_refused_self_naming_call_writes_nothing() {
    let router = emulator();
    let own = default_account_id().to_string();

    // The standalone reads that would show any write the refusals might have
    // made: the account record, its primary contact, each alternate contact and
    // the whole Region opt list.
    let reads: Vec<(String, &str, Value)> = {
        let mut reads: Vec<(String, &str, Value)> = vec![
            (
                "GetAccountInformation".to_string(),
                "/getAccountInformation",
                json!({}),
            ),
            (
                "GetContactInformation".to_string(),
                "/getContactInformation",
                json!({}),
            ),
            ("ListRegions".to_string(), "/listRegions", json!({})),
        ];
        for contact_type in ALTERNATE_CONTACT_TYPES {
            reads.push((
                format!("GetAlternateContact {contact_type}"),
                "/getAlternateContact",
                json!({"AlternateContactType": contact_type}),
            ));
        }
        reads
    };

    let mut before: Vec<(String, u16, Value)> = Vec::new();
    for (label, uri, body) in &reads {
        let answer = call(&router, uri, body.clone()).await;
        before.push((label.clone(), answer.status, answer.body));
    }

    for (label, uri, body) in optional_rows() {
        let answer = call(&router, uri, body(Some(&own))).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            DENIED,
            "case {label:?}: expected {DENIED:?} before checking what it wrote, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
    for (label, uri, body) in required_rows() {
        let answer = call(&router, uri, body(&own)).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            DENIED,
            "case {label:?}: expected {DENIED:?} before checking what it wrote, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    for (case, status, body) in before {
        let (_, uri, request) = reads
            .iter()
            .find(|(label, _, _)| *label == case)
            .expect("every recorded read came from the table");
        let answer = call(&router, uri, request.clone()).await;
        assert_eq!(
            (answer.status, &answer.body),
            (status, &body),
            "case {case:?}: the refused calls must have written nothing, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}
