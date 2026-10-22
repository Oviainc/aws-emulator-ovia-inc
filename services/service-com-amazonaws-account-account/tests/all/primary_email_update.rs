// SPDX-License-Identifier: CC0-1.0

//! The primary-email-update slice — `StartPrimaryEmailUpdate` and
//! `GetPrimaryEmailUpdateStatus` — end to end over HTTP.
//!
//! Two rules are under test, both stated by this service's own model:
//!
//! 1. `StartPrimaryEmailUpdate` updates a MEMBER account of the organization
//!    from the management or delegated administrator account; a request whose
//!    `AccountId` is the caller's own account is refused with HTTP 403
//!    `AccessDeniedException` before the proposed address is acted on, and
//!    records nothing. (The model's `AccountId` documentation: "The management
//!    account can't specify its own `AccountId`.")
//! 2. `GetPrimaryEmailUpdateStatus` reports the status of the MOST RECENT update
//!    of the resolved account (the one the request names, else the caller's
//!    own); an account with no update has no status to report and is refused
//!    with HTTP 400 `ValidationException`, never a 501 and never an invented
//!    status.
//!
//! Each table spans the class rather than one recorded request: the status read
//! is exercised with the member absent, with the caller's own id and with an id
//! the store does not know, and the start is exercised over several member
//! accounts and several addresses the modelled `@length(min: 5, max: 64)`
//! admits, including both published bounds.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::default_account_id;
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The values `PrimaryEmailUpdateStatus` admits, from
/// `smithy_query(query="shape", shape="PrimaryEmailUpdateStatus")` — spelled out
/// here, not imported, so a status outside the modelled set cannot pass.
const PRIMARY_EMAIL_UPDATE_STATUSES: [&str; 4] = ["PENDING", "ACCEPTED", "COMPLETED", "FAILED"];

/// The status an update carries the moment it is started: the address still has
/// to be confirmed with the one-time passcode Account Management mails to it.
const PENDING: &str = "PENDING";

/// `PrimaryEmailAddress`'s modelled `@length`, from
/// `smithy_query(query="operation", operation="StartPrimaryEmailUpdate")`.
const PRIMARY_EMAIL_MIN: usize = 5;
const PRIMARY_EMAIL_MAX: usize = 64;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Member account ids this emulator has never been told about: each satisfies
/// `AccountId`'s `^\d{12}$` and none is the caller's own account.
const MEMBER_ACCOUNTS: [&str; 3] = ["111111111111", "222222222222", "123456789012"];

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

    /// The `Status` member of a success body.
    fn reported_status(&self) -> &str {
        self.body["Status"].as_str().unwrap_or_default()
    }

    /// The `UpdatedAt` member of a success body, as the epoch seconds the
    /// restJson1 protocol renders a timestamp as.
    fn updated_at(&self) -> Option<f64> {
        self.body["UpdatedAt"].as_f64()
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

async fn start_primary_email_update(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/startPrimaryEmailUpdate", request).await
}

async fn get_primary_email_update_status(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/getPrimaryEmailUpdateStatus", request).await
}

/// An address of exactly `length` characters, in the shape of an email address.
fn address_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// Assert `answer` is a refusal with `status` and `code`, and that its message
/// is not the scaffold's "not implemented" signal.
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

/// An account with no primary email update ever started has no status to
/// report: the read is refused with HTTP 400 `ValidationException`, whether the
/// request names no account, the caller's own, or one the store does not know
/// (which is a 403 instead — the caller is outside that account's
/// organization).
#[tokio::test]
async fn a_status_read_with_no_update_started_is_refused() {
    // (case, request, expected status, expected code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "no AccountId at all, so the caller's own account",
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "the caller's own account named explicitly",
            json!({"AccountId": default_account_id()}),
            400,
            "ValidationException",
        ),
        (
            "a member account the store does not know",
            json!({"AccountId": MEMBER_ACCOUNTS[0]}),
            403,
            "AccessDeniedException",
        ),
        (
            "another member account the store does not know",
            json!({"AccountId": MEMBER_ACCOUNTS[1]}),
            403,
            "AccessDeniedException",
        ),
    ];

    let router = emulator();
    for (case, request, status, code) in cases {
        let answer = get_primary_email_update_status(&router, request).await;
        assert_refused(case, &answer, status, code);
    }
}

/// A `StartPrimaryEmailUpdate` that names the caller's OWN account is refused
/// with HTTP 403 `AccessDeniedException`, whatever address it proposes — the
/// authorization rule is applied before the address is acted on — and records
/// nothing, so the status read still reports no update.
#[tokio::test]
async fn starting_an_update_on_the_callers_own_account_is_denied() {
    // Addresses spanning what `@length(min: 5, max: 64)` admits, including one
    // that is not a valid email address at all: the refusal does not depend on
    // the address, so every row is the same 403.
    let addresses = [
        ("the shortest address the bound admits", "a@b.c".to_string()),
        ("the longest address the bound admits", address_of_length(PRIMARY_EMAIL_MAX)),
        ("an ordinary address", "ada@example.com".to_string()),
        (
            "an address that is not an email address at all",
            "not-an-email-address".to_string(),
        ),
    ];
    assert_eq!(
        addresses[0].1.chars().count(),
        PRIMARY_EMAIL_MIN,
        "the shortest row must sit on the published minimum"
    );

    let router = emulator();
    for (case, address) in addresses {
        let answer = start_primary_email_update(
            &router,
            json!({"AccountId": default_account_id(), "PrimaryEmail": address}),
        )
        .await;
        assert_refused(case, &answer, 403, "AccessDeniedException");

        // Nothing was recorded: the caller's own account still has no update.
        let status = get_primary_email_update_status(&router, json!({})).await;
        assert_refused(
            case,
            &status,
            400,
            "ValidationException",
        );
    }
}

/// A `StartPrimaryEmailUpdate` on a member account records a PENDING update that
/// `GetPrimaryEmailUpdateStatus` reports back, for every address the modelled
/// bound admits; each account carries its own update, and an account nobody
/// started one for is unaffected.
#[tokio::test]
async fn an_update_started_on_a_member_account_round_trips() {
    // (case, the member account, the proposed address) — the rows differ in the
    // account, and in the address's length and form.
    let cases: Vec<(&str, &str, String)> = vec![
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
            "an ordinary address with a tag",
            MEMBER_ACCOUNTS[2],
            "ada+aws@example.com".to_string(),
        ),
    ];

    let router = emulator();
    for (case, account_id, address) in &cases {
        let started = start_primary_email_update(
            &router,
            json!({"AccountId": account_id, "PrimaryEmail": address}),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartPrimaryEmailUpdate must answer 200, got {} with {:?}",
            started.status, started.body
        );
        assert_eq!(
            started.reported_status(),
            PENDING,
            "case {case:?}: expected the started update to be {PENDING}, got {:?}",
            started.body
        );
        assert!(
            PRIMARY_EMAIL_UPDATE_STATUSES.contains(&started.reported_status()),
            "case {case:?}: {:?} is not a modelled PrimaryEmailUpdateStatus",
            started.reported_status()
        );
    }

    // Each account reports its own update, and a read does not change it.
    for (case, account_id, _) in &cases {
        let first = get_primary_email_update_status(&router, json!({"AccountId": account_id})).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the status read must answer 200, got {} with {:?}",
            first.status, first.body
        );
        assert_eq!(
            first.reported_status(),
            PENDING,
            "case {case:?}: expected {PENDING}, got {:?}",
            first.body
        );
        let updated_at = first
            .updated_at()
            .unwrap_or_else(|| panic!("case {case:?}: expected an UpdatedAt, got {:?}", first.body));
        assert!(
            updated_at > 0.0,
            "case {case:?}: expected an UpdatedAt after the epoch, got {updated_at}"
        );

        let again = get_primary_email_update_status(&router, json!({"AccountId": account_id})).await;
        assert_eq!(
            again.updated_at(),
            Some(updated_at),
            "case {case:?}: a read must not move UpdatedAt, got {:?}",
            again.body
        );
    }

    // The caller's own account never had one started, so it is still a refusal:
    // a write to a member account did not land on the caller's record.
    let own = get_primary_email_update_status(&router, json!({})).await;
    assert_refused(
        "the caller's own account after member updates",
        &own,
        400,
        "ValidationException",
    );
}

/// `GetPrimaryEmailUpdateStatus` reports the MOST RECENT update: a second start
/// on the same account replaces the first rather than adding a second record,
/// and the reported `UpdatedAt` does not go backwards.
#[tokio::test]
async fn a_restarted_update_is_the_one_reported() {
    let router = emulator();
    let account_id = MEMBER_ACCOUNTS[0];

    let addresses = [
        ("the first address", "first@example.com"),
        ("a second address", "second@example.com"),
        ("a third address", "third@example.com"),
    ];

    let mut previous: Option<f64> = None;
    for (case, address) in addresses {
        let started = start_primary_email_update(
            &router,
            json!({"AccountId": account_id, "PrimaryEmail": address}),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: a restart must answer 200, got {} with {:?}",
            started.status, started.body
        );

        let read = get_primary_email_update_status(&router, json!({"AccountId": account_id})).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the status read must answer 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.reported_status(),
            PENDING,
            "case {case:?}: expected {PENDING}, got {:?}",
            read.body
        );
        let updated_at = read
            .updated_at()
            .unwrap_or_else(|| panic!("case {case:?}: expected an UpdatedAt, got {:?}", read.body));
        if let Some(earlier) = previous {
            assert!(
                updated_at >= earlier,
                "case {case:?}: UpdatedAt must not go backwards, got {updated_at} after {earlier}"
            );
        }
        previous = Some(updated_at);
    }
}

/// An address outside `PrimaryEmailAddress`'s modelled bound is refused with
/// HTTP 400 `ValidationException` and records nothing, so the operation's own
/// refusals are the modelled ones on both sides of the authorization rule.
#[tokio::test]
async fn an_address_outside_the_modelled_bound_is_refused() {
    let cases: Vec<(&str, String)> = vec![
        (
            "one character short of the minimum",
            "a@b.".to_string(),
        ),
        (
            "one character past the maximum",
            address_of_length(PRIMARY_EMAIL_MAX + 1),
        ),
    ];

    let router = emulator();
    for (case, address) in cases {
        let answer = start_primary_email_update(
            &router,
            json!({"AccountId": MEMBER_ACCOUNTS[0], "PrimaryEmail": address}),
        )
        .await;
        assert_refused(case, &answer, 400, "ValidationException");

        // A refused start recorded nothing — not even the account record a
        // stored update would have created, so the read is still the refusal
        // for an account outside the caller's organization.
        let read =
            get_primary_email_update_status(&router, json!({"AccountId": MEMBER_ACCOUNTS[0]})).await;
        assert_refused(case, &read, 403, "AccessDeniedException");
    }
}
