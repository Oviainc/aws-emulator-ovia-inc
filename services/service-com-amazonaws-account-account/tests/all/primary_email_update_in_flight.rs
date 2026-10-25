// SPDX-License-Identifier: CC0-1.0

//! What `StartPrimaryEmailUpdate` does to an update whose address change is
//! still in flight, end to end over HTTP, with the operations that share its
//! resource (`AcceptPrimaryEmailUpdate` confirms the update,
//! `GetPrimaryEmailUpdateStatus` reports it and `GetPrimaryEmail` reports the
//! address it put on the account).
//!
//! The rule under test: a change this service has already confirmed and whose
//! application the clock still owes is never discarded to make room for a new
//! one — a call that would replace it is refused with the modelled HTTP 409
//! `ConflictException` ("a conflict in the current status of the resource"),
//! and the owed change still comes due on its own. A record that owes nothing
//! — one waiting for input that may never arrive, or one already in a terminal
//! status — is replaced as before.
//!
//! Each table spans the class rather than one request: the rows differ in the
//! account, in the status the stored update is in when the next call arrives,
//! and in the addresses' length and form, including both published `@length`
//! bounds.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The values `PrimaryEmailUpdateStatus` admits, from
/// `smithy_query(query="shape", shape="PrimaryEmailUpdateStatus")` — spelled out
/// here, not imported, so a status outside the modelled set cannot pass.
const PRIMARY_EMAIL_UPDATE_STATUSES: [&str; 4] = ["PENDING", "ACCEPTED", "COMPLETED", "FAILED"];

/// The status an update carries while it waits for the one-time passcode.
const PENDING: &str = "PENDING";
/// The status an accepted update carries until its change has been applied.
const ACCEPTED: &str = "ACCEPTED";
/// The terminal status of an accepted update.
const COMPLETED: &str = "COMPLETED";

/// `PrimaryEmailAddress`'s modelled `@length`, from
/// `smithy_query(query="operation", operation="StartPrimaryEmailUpdate")`.
const PRIMARY_EMAIL_MIN: usize = 5;
const PRIMARY_EMAIL_MAX: usize = 64;

/// How long a test waits for a status the clock owes before giving up. Far above
/// the emulator's own transition span, so a slow machine is not a failure.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Member account ids, each satisfying `AccountId`'s `^\d{12}$` and none of them
/// the caller's own account. One row of a table gets its own id, so no row can
/// see what another row recorded.
const MEMBER_ACCOUNTS: [&str; 8] = [
    "111111111111",
    "222222222222",
    "333333333333",
    "444444444444",
    "555555555555",
    "666666666666",
    "777777777777",
    "123456789012",
];

/// Passcodes spanning the forms `Otp`'s `^[a-zA-Z0-9]{6}$` admits (digits only,
/// lower case, upper case, mixed) — the pattern is from
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

    /// The `Status` member of a success body.
    fn reported_status(&self) -> &str {
        self.body["Status"].as_str().unwrap_or_default()
    }

    /// The `PrimaryEmail` member of a `GetPrimaryEmail` success body.
    fn primary_email(&self) -> &str {
        self.body["PrimaryEmail"].as_str().unwrap_or_default()
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

async fn status_read(router: &MultiServiceRouter, account_id: &str) -> Answer {
    call(
        router,
        "/getPrimaryEmailUpdateStatus",
        json!({"AccountId": account_id}),
    )
    .await
}

async fn primary_email_read(router: &MultiServiceRouter, account_id: &str) -> Answer {
    call(router, "/getPrimaryEmail", json!({"AccountId": account_id})).await
}

/// An address of exactly `length` characters, in the shape of an email address.
fn address_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// Assert `answer` is a success answering `expected` as its `Status`.
fn assert_started(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        PRIMARY_EMAIL_UPDATE_STATUSES.contains(&answer.reported_status()),
        "case {case:?}: expected a modelled PrimaryEmailUpdateStatus, got {:?}",
        answer.reported_status()
    );
    assert_eq!(
        answer.reported_status(),
        expected,
        "case {case:?}: expected {expected}, got {:?}",
        answer.body
    );
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
}

/// The status the emulator reports for `account_id`, asserting the read
/// succeeded with a modelled status.
async fn reported_status(router: &MultiServiceRouter, case: &str, account_id: &str) -> String {
    let answer = status_read(router, account_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the status read must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        PRIMARY_EMAIL_UPDATE_STATUSES.contains(&answer.reported_status()),
        "case {case:?}: {:?} is not a modelled PrimaryEmailUpdateStatus",
        answer.reported_status()
    );
    answer.reported_status().to_string()
}

/// Poll the status of `account_id` until it leaves `transient`, and answer what
/// it settled to. Fails the case if it is still transient after
/// [`SETTLE_TIMEOUT`].
async fn settled_status(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    transient: &str,
) -> String {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let status = reported_status(router, case, account_id).await;
        if status != transient {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: status is still {transient} after {SETTLE_TIMEOUT:?} — \
             a transition the clock owes must come due on its own"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The rows the in-flight tables walk: an account of its own, the address the
/// accepted update is for, the address a later call proposes instead, and a
/// passcode. They differ in the addresses' length and form — both published
/// `@length` bounds included — and in the passcode's form.
fn in_flight_rows() -> Vec<(&'static str, &'static str, String, String, &'static str)> {
    let rows = vec![
        (
            "the shortest address the bound admits, a digits-only passcode",
            MEMBER_ACCOUNTS[0],
            "a@b.c".to_string(),
            "x@y.z".to_string(),
            WELL_FORMED_PASSCODES[0],
        ),
        (
            "the longest address the bound admits, a lower-case passcode",
            MEMBER_ACCOUNTS[1],
            address_of_length(PRIMARY_EMAIL_MAX),
            "second@example.com".to_string(),
            WELL_FORMED_PASSCODES[1],
        ),
        (
            "an ordinary address, an upper-case passcode",
            MEMBER_ACCOUNTS[2],
            "ada@example.com".to_string(),
            address_of_length(PRIMARY_EMAIL_MAX),
            WELL_FORMED_PASSCODES[2],
        ),
        (
            "a tagged address, a mixed-case passcode",
            MEMBER_ACCOUNTS[3],
            "ada+aws@example.co.uk".to_string(),
            "grace@example.org".to_string(),
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

/// A start that lands while an accepted update's address change is still owed is
/// refused with HTTP 409 `ConflictException` and writes nothing: the accepted
/// update keeps its own address, the clock still carries it to `COMPLETED`, and
/// `GetPrimaryEmail` reports the address the service accepted.
#[tokio::test]
async fn starting_an_update_while_an_accepted_one_is_in_flight_is_refused() {
    let router = emulator();

    for (case, account_id, accepted_address, other_address, passcode) in in_flight_rows() {
        assert_started(
            case,
            &start(&router, account_id, &accepted_address).await,
            PENDING,
        );
        // The record exists from that start on, so its address can be read; an
        // unconfirmed request has changed nothing yet.
        let seeded = primary_email_read(&router, account_id).await;
        assert_eq!(
            seeded.status, 200,
            "case {case:?}: the primary email read must answer 200, got {} with {:?}",
            seeded.status, seeded.body
        );
        let address_before = seeded.primary_email().to_string();
        assert_ne!(
            address_before, accepted_address,
            "case {case:?}: the address under test must differ from the one the record came up \
             with, else the assertion below proves nothing"
        );
        assert_started(
            case,
            &accept(&router, account_id, &accepted_address, passcode).await,
            ACCEPTED,
        );

        // The change is in flight: still ACCEPTED, so its completion is owed.
        assert_eq!(
            reported_status(&router, case, account_id).await,
            ACCEPTED,
            "case {case:?}: the status straight after the acceptance must be {ACCEPTED}"
        );

        // A start inside that window is the conflict, for any address — the
        // same address the update is for as well as a different one.
        for (proposed_case, proposed) in [
            ("a different address", other_address.as_str()),
            ("the very address in flight", accepted_address.as_str()),
        ] {
            let refused = start(&router, account_id, proposed).await;
            assert_refused(
                &format!("{case} / {proposed_case}"),
                &refused,
                409,
                "ConflictException",
            );
        }

        // The refusal wrote nothing: the accepted update is still the one
        // reported, and the clock pays its completion on its own.
        assert_eq!(
            settled_status(&router, case, account_id, ACCEPTED).await,
            COMPLETED,
            "case {case:?}: an accepted update must still settle to {COMPLETED}"
        );

        // And the address the service accepted is the account's primary email.
        let after = primary_email_read(&router, account_id).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: the primary email read must answer 200, got {} with {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.primary_email(),
            accepted_address,
            "case {case:?}: expected the accepted address (was {address_before:?} before), \
             got {:?}",
            after.primary_email()
        );
    }
}

/// An update that owes nothing is replaced as before: a start on top of a
/// `PENDING` update (nothing but a passcode moves it) and on top of a
/// `COMPLETED` one (terminal) both answer 200 `PENDING`, and the account's
/// primary email is untouched by the new, unconfirmed request.
#[tokio::test]
async fn a_start_replaces_an_update_that_owes_nothing() {
    let router = emulator();

    // (case, account, whether the first update is accepted and settled before
    // the restart, the first address, the address the restart proposes)
    let rows: Vec<(&str, &str, bool, String, String)> = vec![
        (
            "on top of a PENDING update, two ordinary addresses",
            MEMBER_ACCOUNTS[4],
            false,
            "first@example.com".to_string(),
            "second@example.com".to_string(),
        ),
        (
            "on top of a PENDING update, the shortest and the longest address",
            MEMBER_ACCOUNTS[5],
            false,
            "a@b.c".to_string(),
            address_of_length(PRIMARY_EMAIL_MAX),
        ),
        (
            "on top of a COMPLETED update, two ordinary addresses",
            MEMBER_ACCOUNTS[6],
            true,
            "done@example.com".to_string(),
            "next@example.com".to_string(),
        ),
        (
            "on top of a COMPLETED update, a tagged address then a short one",
            MEMBER_ACCOUNTS[7],
            true,
            "ada+aws@example.com".to_string(),
            "x@y.z".to_string(),
        ),
    ];

    for (case, account_id, settle_first, first_address, next_address) in rows {
        assert_started(case, &start(&router, account_id, &first_address).await, PENDING);

        let expected_primary_email = if settle_first {
            assert_started(
                case,
                &accept(&router, account_id, &first_address, WELL_FORMED_PASSCODES[0]).await,
                ACCEPTED,
            );
            assert_eq!(
                settled_status(&router, case, account_id, ACCEPTED).await,
                COMPLETED,
                "case {case:?}: the first update must settle to {COMPLETED} before the restart"
            );
            first_address.clone()
        } else {
            // Nothing has been confirmed, so the account keeps the address it
            // came up with.
            primary_email_read(&router, account_id)
                .await
                .primary_email()
                .to_string()
        };

        // The restart is admitted and is the update now reported.
        assert_started(case, &start(&router, account_id, &next_address).await, PENDING);
        assert_eq!(
            reported_status(&router, case, account_id).await,
            PENDING,
            "case {case:?}: the restarted update must be the one reported"
        );

        // An unconfirmed request changes no address.
        let answer = primary_email_read(&router, account_id).await;
        assert_eq!(
            answer.primary_email(),
            expected_primary_email,
            "case {case:?}: expected the primary email to stay {expected_primary_email:?}, \
             got {:?}",
            answer.primary_email()
        );
    }
}

/// The address of an in-flight accepted update is in use: it survives a start on
/// its own account, so another account cannot take it — the acceptance there is
/// the modelled HTTP 409 `ConflictException`, before and after the first
/// account's completion comes due.
#[tokio::test]
async fn an_address_accepted_in_flight_stays_in_use_for_other_accounts() {
    // (case, the account that accepted the address, the account that tries to
    // take it, the address) — the rows differ in the address's length and form.
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "an ordinary address",
            MEMBER_ACCOUNTS[0],
            MEMBER_ACCOUNTS[1],
            "shared@example.com".to_string(),
        ),
        (
            "the shortest address the bound admits",
            MEMBER_ACCOUNTS[2],
            MEMBER_ACCOUNTS[3],
            "a@b.c".to_string(),
        ),
        (
            "the longest address the bound admits",
            MEMBER_ACCOUNTS[4],
            MEMBER_ACCOUNTS[5],
            address_of_length(PRIMARY_EMAIL_MAX),
        ),
    ];

    for (case, holder, rival, address) in rows {
        // Each row gets its own emulator, so the holder accounts are fresh.
        let router = emulator();

        assert_started(case, &start(&router, holder, &address).await, PENDING);
        assert_started(
            case,
            &accept(&router, holder, &address, WELL_FORMED_PASSCODES[3]).await,
            ACCEPTED,
        );

        // A start on the holder inside the window is refused, so the accepted
        // address is still owed to it.
        assert_refused(
            case,
            &start(&router, holder, "other@example.com").await,
            409,
            "ConflictException",
        );

        // The rival may ASK for the address (a pending request changes nothing)
        // but cannot have it confirmed, in flight or completed.
        assert_started(case, &start(&router, rival, &address).await, PENDING);
        assert_refused(
            &format!("{case} / while the holder's change is in flight"),
            &accept(&router, rival, &address, WELL_FORMED_PASSCODES[0]).await,
            409,
            "ConflictException",
        );

        assert_eq!(
            settled_status(&router, case, holder, ACCEPTED).await,
            COMPLETED,
            "case {case:?}: the holder's accepted update must settle to {COMPLETED}"
        );

        assert_refused(
            &format!("{case} / after the holder's change completed"),
            &accept(&router, rival, &address, WELL_FORMED_PASSCODES[0]).await,
            409,
            "ConflictException",
        );

        // The holder kept the address.
        let answer = primary_email_read(&router, holder).await;
        assert_eq!(
            answer.primary_email(),
            address,
            "case {case:?}: expected the holder to keep the accepted address, got {:?}",
            answer.primary_email()
        );
    }
}
