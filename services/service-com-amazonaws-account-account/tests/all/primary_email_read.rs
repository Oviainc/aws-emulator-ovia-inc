// SPDX-License-Identifier: CC0-1.0

//! `GetPrimaryEmail` end to end over HTTP, with the operations it shares its
//! resource with (`StartPrimaryEmailUpdate` records an update,
//! `AcceptPrimaryEmailUpdate` confirms it).
//!
//! The rules under test, all stated by this service's own model:
//!
//! 1. The read reports the primary email address the named account HAS: every
//!    account record carries one (the root user email address, captured when the
//!    account is signed up for), so the read answers 200 with an address inside
//!    `PrimaryEmailAddress`'s modelled `@length(min: 5, max: 64)`, one address
//!    per account, and reports the same address again on a later read.
//! 2. An update changes what the read reports only once it has COMPLETED: a
//!    `PENDING` update is a request nobody has confirmed, an `ACCEPTED` one has
//!    a completion the clock still owes, and from the instant that completion
//!    comes due the address the update was started for is what the read
//!    reports.
//! 3. `AccountId` is required and names a MEMBER account of the caller's
//!    organization: the caller's own account is refused with HTTP 403
//!    `AccessDeniedException` ("the management account can't specify its own
//!    `AccountId`"), an account this service holds no record of is the same 403,
//!    and an id outside `^\d{12}$` — or no id at all — is the 400
//!    `ValidationException` the boundary answers.
//!
//! Each table spans the class rather than one request: the read is exercised
//! over several member accounts, over addresses at both published `@length`
//! bounds and in between, and over account ids that are well formed, malformed,
//! unknown, absent and the caller's own.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::default_account_id;
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `PrimaryEmailAddress`'s modelled `@length`, from
/// `smithy_query(query="operation", operation="GetPrimaryEmail")`.
const PRIMARY_EMAIL_MIN: usize = 5;
const PRIMARY_EMAIL_MAX: usize = 64;

/// The terminal `PrimaryEmailUpdateStatus` of an accepted update, from
/// `smithy_query(query="shape", shape="PrimaryEmailUpdateStatus")`.
const COMPLETED: &str = "COMPLETED";

/// The status an update carries while it waits for its one-time passcode.
const PENDING: &str = "PENDING";

/// How long a test waits for a change the clock owes before giving up. Far above
/// the emulator's own transition span, so a slow machine is not a failure.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Member account ids, each satisfying `AccountId`'s `^\d{12}$` and none of them
/// the caller's own account. One row of a table gets one id, so no row can see
/// what another row recorded.
const MEMBER_ACCOUNTS: [&str; 6] = [
    "111111111111",
    "222222222222",
    "333333333333",
    "444444444444",
    "987654321098",
    "123456789012",
];

/// Passcodes spanning the forms `Otp`'s `^[a-zA-Z0-9]{6}$` admits, from
/// `smithy_query(query="operation", operation="AcceptPrimaryEmailUpdate")`.
const WELL_FORMED_PASSCODES: [&str; 4] = ["123456", "abcdef", "ABCDEF", "a1B2c3"];

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

    /// The `PrimaryEmail` member of a success body, or the empty string when the
    /// answer carries none.
    fn primary_email(&self) -> &str {
        self.body["PrimaryEmail"].as_str().unwrap_or_default()
    }

    /// The `Status` member of a success body.
    fn reported_status(&self) -> &str {
        self.body["Status"].as_str().unwrap_or_default()
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

async fn read(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/getPrimaryEmail", request).await
}

/// Bring a member account's record into existence through a write that is not
/// part of the primary-email flow, so the read is exercised on an account no
/// email update has ever touched.
async fn create_record(router: &MultiServiceRouter, case: &str, account_id: &str) {
    let created = call(
        router,
        "/putAccountName",
        json!({"AccountId": account_id, "AccountName": "Member"}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the member account's record must be created, got {} with {:?}",
        created.status, created.body
    );
}

async fn start(router: &MultiServiceRouter, account_id: &str, address: &str) -> Answer {
    call(
        router,
        "/startPrimaryEmailUpdate",
        json!({"AccountId": account_id, "PrimaryEmail": address}),
    )
    .await
}

async fn accept(
    router: &MultiServiceRouter,
    account_id: &str,
    address: &str,
    passcode: &str,
) -> Answer {
    call(
        router,
        "/acceptPrimaryEmailUpdate",
        json!({"AccountId": account_id, "PrimaryEmail": address, "Otp": passcode}),
    )
    .await
}

/// An address of exactly `length` characters, in the shape of an email address.
fn address_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// The address the emulator reports for `account_id`, asserting the read
/// succeeded and answered an address the modelled `@length` admits.
async fn reported_email(router: &MultiServiceRouter, case: &str, account_id: &str) -> String {
    let answer = read(router, json!({"AccountId": account_id})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the read must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    let address = answer.primary_email().to_string();
    let length = address.chars().count();
    assert!(
        (PRIMARY_EMAIL_MIN..=PRIMARY_EMAIL_MAX).contains(&length),
        "case {case:?}: the reported address must be {PRIMARY_EMAIL_MIN}..={PRIMARY_EMAIL_MAX} \
         characters, got {length} in {address:?}"
    );
    address
}

/// Assert `answer` is a refusal with `status` and `code`, and that it is the
/// modelled refusal rather than the scaffold's "not implemented" signal.
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
    assert_eq!(
        answer.primary_email(),
        "",
        "case {case:?}: a refusal must report no address, got {:?}",
        answer.body
    );
}

/// Every member account the store holds reports a primary email address, one per
/// account, and reports the same one again on a later read — the read never
/// changes what the next caller sees.
#[tokio::test]
async fn every_account_the_store_holds_reports_one_stable_primary_email() {
    let router = emulator();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for account_id in MEMBER_ACCOUNTS {
        let case = format!("the member account {account_id}");
        create_record(&router, &case, account_id).await;

        let first = reported_email(&router, &case, account_id).await;
        let again = reported_email(&router, &case, account_id).await;
        assert_eq!(
            first, again,
            "case {case:?}: a second read must report the same address, got {again:?} after \
             {first:?}"
        );
        assert!(
            seen.insert(first.clone()),
            "case {case:?}: each account must report its own address, but {first:?} was already \
             reported for another account"
        );
    }

    assert_eq!(
        seen.len(),
        MEMBER_ACCOUNTS.len(),
        "every account must report an address of its own, got {seen:?}"
    );
}

/// The rows the update tables walk: an account of its own, the address the
/// update asks for, and the passcode that confirms it. The rows differ in the
/// address's length and form, both published `@length` bounds included.
fn update_rows() -> Vec<(&'static str, &'static str, String, &'static str)> {
    let rows = vec![
        (
            "the shortest address the bound admits",
            MEMBER_ACCOUNTS[0],
            "a@b.c".to_string(),
            WELL_FORMED_PASSCODES[0],
        ),
        (
            "the longest address the bound admits",
            MEMBER_ACCOUNTS[1],
            address_of_length(PRIMARY_EMAIL_MAX),
            WELL_FORMED_PASSCODES[1],
        ),
        (
            "an ordinary address",
            MEMBER_ACCOUNTS[2],
            "ada@example.com".to_string(),
            WELL_FORMED_PASSCODES[2],
        ),
        (
            "a tagged address on a two-label domain",
            MEMBER_ACCOUNTS[3],
            "ada+aws@example.co.uk".to_string(),
            WELL_FORMED_PASSCODES[3],
        ),
    ];
    assert_eq!(
        rows[0].2.chars().count(),
        PRIMARY_EMAIL_MIN,
        "the shortest row must sit on the published minimum"
    );
    assert_eq!(
        rows[1].2.chars().count(),
        PRIMARY_EMAIL_MAX,
        "the longest row must sit on the published maximum"
    );
    rows
}

/// A started update is not yet the account's primary email: it has to be
/// confirmed, and the read keeps reporting the address the account already had.
#[tokio::test]
async fn a_pending_update_does_not_change_the_reported_primary_email() {
    let router = emulator();

    for (case, account_id, address, _) in update_rows() {
        create_record(&router, case, account_id).await;
        let before = reported_email(&router, case, account_id).await;
        assert_ne!(
            before, address,
            "case {case:?}: the test's requested address must differ from the one the account \
             already reports, got {before:?}"
        );

        let started = start(&router, account_id, &address).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartPrimaryEmailUpdate must answer 200, got {} with {:?}",
            started.status, started.body
        );
        assert_eq!(
            started.reported_status(),
            PENDING,
            "case {case:?}: a started update must be {PENDING}, got {:?}",
            started.body
        );

        let after = reported_email(&router, case, account_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a {PENDING} update must not change the reported address, got \
             {after:?} after {before:?}"
        );
    }
}

/// Once an accepted update has completed, the address it was started for is what
/// the read reports — and keeps reporting.
#[tokio::test]
async fn a_completed_update_is_the_reported_primary_email() {
    let router = emulator();

    for (case, account_id, address, passcode) in update_rows() {
        create_record(&router, case, account_id).await;
        let before = reported_email(&router, case, account_id).await;

        let started = start(&router, account_id, &address).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartPrimaryEmailUpdate must answer 200, got {} with {:?}",
            started.status, started.body
        );
        let accepted = accept(&router, account_id, &address, passcode).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: AcceptPrimaryEmailUpdate must answer 200, got {} with {:?}",
            accepted.status, accepted.body
        );

        // The completion is owed by the CLOCK, so nothing but reading is asked
        // of the emulator: until it comes due the read reports the address the
        // account still has, and never a third address.
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            let reported = reported_email(&router, case, account_id).await;
            if reported == address {
                break;
            }
            assert_eq!(
                reported, before,
                "case {case:?}: before its completion comes due the read must report the address \
                 the account still has, got {reported:?}"
            );
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the accepted update must complete on its own within \
                 {SETTLE_TIMEOUT:?}, and the read still reports {before:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let status = call(
            &router,
            "/getPrimaryEmailUpdateStatus",
            json!({"AccountId": account_id}),
        )
        .await;
        assert_eq!(
            status.reported_status(),
            COMPLETED,
            "case {case:?}: the update whose address is reported must be {COMPLETED}, got {:?}",
            status.body
        );

        let again = reported_email(&router, case, account_id).await;
        assert_eq!(
            again, address,
            "case {case:?}: the completed address must keep being reported, got {again:?}"
        );
    }
}

/// A second update replaces the first: after both have completed, the read
/// reports the address of the LAST one.
#[tokio::test]
async fn the_last_completed_update_is_what_the_read_reports() {
    let router = emulator();
    let account_id = MEMBER_ACCOUNTS[4];
    // Three addresses in a row, differing in form and in length: the last one
    // sits on the published minimum.
    let addresses = ["first@example.com", "second@example.org", "a@b.c"];
    assert_eq!(
        addresses[addresses.len() - 1].chars().count(),
        PRIMARY_EMAIL_MIN,
        "the last address must sit on the published minimum"
    );
    create_record(&router, "three updates in a row", account_id).await;

    for address in addresses {
        let case = format!("the update to {address}");
        let started = start(&router, account_id, address).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartPrimaryEmailUpdate must answer 200, got {} with {:?}",
            started.status, started.body
        );
        let accepted = accept(&router, account_id, address, WELL_FORMED_PASSCODES[0]).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: AcceptPrimaryEmailUpdate must answer 200, got {} with {:?}",
            accepted.status, accepted.body
        );

        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            if reported_email(&router, &case, account_id).await == address {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the accepted update must complete on its own within \
                 {SETTLE_TIMEOUT:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    let final_address = reported_email(&router, "after three updates", account_id).await;
    assert_eq!(
        final_address,
        addresses[addresses.len() - 1],
        "the address of the LAST completed update must be the one reported, got \
         {final_address:?}"
    );
}

/// The read is about a MEMBER account of the caller's organization: the caller's
/// own account and an account the store holds no record of are both the 403
/// authorization refusal, and an `AccountId` the modelled pattern does not admit
/// — or none at all — is the 400 the boundary answers.
#[tokio::test]
async fn a_read_outside_a_member_account_of_the_organization_is_refused() {
    // (case, request, expected HTTP status, expected error code, an account id
    // the request named that must still be unknown afterwards)
    let cases: Vec<(&str, Value, u16, &str, Option<&str>)> = vec![
        (
            "the caller's own account",
            json!({"AccountId": default_account_id()}),
            403,
            "AccessDeniedException",
            None,
        ),
        (
            "an account the store does not know",
            json!({"AccountId": "210987654321"}),
            403,
            "AccessDeniedException",
            Some("210987654321"),
        ),
        (
            "another account the store does not know",
            json!({"AccountId": "000000000001"}),
            403,
            "AccessDeniedException",
            Some("000000000001"),
        ),
        (
            "the largest account id the pattern admits, unknown",
            json!({"AccountId": "999999999999"}),
            403,
            "AccessDeniedException",
            Some("999999999999"),
        ),
        (
            "no AccountId at all, though the member is required",
            json!({}),
            400,
            "ValidationException",
            None,
        ),
        (
            "an account id of eleven digits",
            json!({"AccountId": "11111111111"}),
            400,
            "ValidationException",
            None,
        ),
        (
            "an account id of thirteen digits",
            json!({"AccountId": "1111111111111"}),
            400,
            "ValidationException",
            None,
        ),
        (
            "an account id that is not digits",
            json!({"AccountId": "abcdefghijkl"}),
            400,
            "ValidationException",
            None,
        ),
    ];

    let router = emulator();
    for (case, request, status, code, probe) in cases {
        let answer = read(&router, request).await;
        assert_refused(case, &answer, status, code);

        // A refused read writes nothing: the account it named is still unknown,
        // so a later caller sees exactly what this one saw.
        if let Some(account_id) = probe {
            let after = call(
                &router,
                "/getAccountInformation",
                json!({"AccountId": account_id}),
            )
            .await;
            assert_eq!(
                after.status, 403,
                "case {case:?}: a refused read must leave the account unknown, got {} with {:?}",
                after.status, after.body
            );
        }
    }
}
