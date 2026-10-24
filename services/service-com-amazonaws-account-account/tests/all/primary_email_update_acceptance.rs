// SPDX-License-Identifier: CC0-1.0

//! `AcceptPrimaryEmailUpdate` end to end over HTTP, with the two operations it
//! shares its resource with (`StartPrimaryEmailUpdate` records the update,
//! `GetPrimaryEmailUpdateStatus` reports it).
//!
//! The rules under test, each from this service's own model:
//!
//! 1. Accepting the update of a member account moves it from `PENDING` to
//!    `ACCEPTED`, which the status read reports; `ACCEPTED` is not terminal, so
//!    the CLOCK carries the update to `COMPLETED` on its own.
//! 2. Only an update that is still waiting for its passcode can be accepted: an
//!    update already accepted (or completed) conflicts — HTTP 409
//!    `ConflictException` — and an account with no update at all has nothing to
//!    accept (HTTP 400 `ValidationException`, or HTTP 403
//!    `AccessDeniedException` for an account outside the caller's
//!    organization, and for the caller's OWN account, which this operation
//!    cannot target).
//! 3. The presented `PrimaryEmail` "must match the PrimaryEmail from the
//!    StartPrimaryEmailUpdate API call" (the model's own member documentation):
//!    any other address is refused with HTTP 400 `ValidationException` and
//!    leaves the update waiting.
//! 4. A primary email cannot be changed to an address already in use as an
//!    account's primary email — the conflict the service's `ConflictException`
//!    documents by example — so such an acceptance is HTTP 409.
//! 5. `Otp` is constrained `^[a-zA-Z0-9]{6}$`: anything outside that form is
//!    refused with HTTP 400 and consumes nothing, and every passcode of that
//!    form is admitted.
//!
//! Every table spans the class rather than one request: the rows differ in the
//! account, in the address's length and form (both published `@length` bounds
//! included) and in the passcode's form.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::default_account_id;
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
/// `smithy_query(query="operation", operation="AcceptPrimaryEmailUpdate")`.
const PRIMARY_EMAIL_MIN: usize = 5;
const PRIMARY_EMAIL_MAX: usize = 64;

/// How long a test waits for a status the clock owes before giving up. Far
/// above the emulator's own transition span, so a slow machine is not a
/// failure.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Member account ids, each satisfying `AccountId`'s `^\d{12}$` and none of them
/// the caller's own account. One row of a table gets one id, so no row can see
/// what another row recorded.
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

async fn status_read(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/getPrimaryEmailUpdateStatus", request).await
}

/// An address of exactly `length` characters, in the shape of an email address.
fn address_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// Start an update and assert it was recorded as `PENDING`.
async fn start_pending(router: &MultiServiceRouter, case: &str, account_id: &str, address: &str) {
    let started = start(router, account_id, address).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartPrimaryEmailUpdate must answer 200, got {} with {:?}",
        started.status, started.body
    );
    assert_eq!(
        started.reported_status(),
        PENDING,
        "case {case:?}: expected a started update to be {PENDING}, got {:?}",
        started.body
    );
}

/// The status the emulator reports for `account_id`, asserting the read
/// succeeded with a modelled status.
async fn reported_status(router: &MultiServiceRouter, case: &str, account_id: &str) -> String {
    let answer = status_read(router, json!({"AccountId": account_id})).await;
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

/// The rows every acceptance table walks: an account of its own, an address and
/// a passcode. They differ in the address's length and form — both published
/// `@length` bounds included — and in the passcode's form.
fn acceptance_rows() -> Vec<(&'static str, &'static str, String, &'static str)> {
    let rows = vec![
        (
            "the shortest address the bound admits, a digits-only passcode",
            MEMBER_ACCOUNTS[0],
            "a@b.c".to_string(),
            WELL_FORMED_PASSCODES[0],
        ),
        (
            "the longest address the bound admits, a lower-case passcode",
            MEMBER_ACCOUNTS[1],
            address_of_length(PRIMARY_EMAIL_MAX),
            WELL_FORMED_PASSCODES[1],
        ),
        (
            "an ordinary address, an upper-case passcode",
            MEMBER_ACCOUNTS[2],
            "ada@example.com".to_string(),
            WELL_FORMED_PASSCODES[2],
        ),
        (
            "a tagged address, a mixed-case passcode",
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

/// Accepting a pending update answers `ACCEPTED`, which the status read reports
/// too; the update is not terminal there, so the clock carries it to
/// `COMPLETED` without anything but a read being asked of it, and a second
/// acceptance of the same update conflicts.
#[tokio::test]
async fn accepting_a_pending_update_moves_it_to_accepted_and_then_to_completed() {
    let router = emulator();

    for (case, account_id, address, passcode) in acceptance_rows() {
        start_pending(&router, case, account_id, &address).await;
        assert_eq!(
            reported_status(&router, case, account_id).await,
            PENDING,
            "case {case:?}: an update nobody accepted must stay {PENDING}"
        );

        let accepted = accept(&router, account_id, &address, passcode).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: AcceptPrimaryEmailUpdate must answer 200, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.reported_status(),
            ACCEPTED,
            "case {case:?}: expected {ACCEPTED}, got {:?}",
            accepted.body
        );

        // The change is in flight: the transient status is observable, so the
        // completion is never instantaneous.
        assert_eq!(
            reported_status(&router, case, account_id).await,
            ACCEPTED,
            "case {case:?}: the status straight after the acceptance must be {ACCEPTED}"
        );

        // Accepting the same update again conflicts with its current status.
        let again = accept(&router, account_id, &address, passcode).await;
        assert_refused(case, &again, 409, "ConflictException");

        // The clock owes the terminal status and pays it on its own.
        assert_eq!(
            settled_status(&router, case, account_id, ACCEPTED).await,
            COMPLETED,
            "case {case:?}: an accepted update must settle to {COMPLETED}"
        );

        // A completed update is terminal: reads do not move it, and its
        // `UpdatedAt` is stable.
        let first = status_read(&router, json!({"AccountId": account_id})).await;
        let second = status_read(&router, json!({"AccountId": account_id})).await;
        assert_eq!(
            second.reported_status(),
            COMPLETED,
            "case {case:?}: a completed update must stay {COMPLETED}, got {:?}",
            second.body
        );
        assert_eq!(
            second.updated_at(),
            first.updated_at(),
            "case {case:?}: a read must not move UpdatedAt, got {:?} then {:?}",
            first.body,
            second.body
        );

        // And a completed update cannot be accepted again either.
        let terminal = accept(&router, account_id, &address, passcode).await;
        assert_refused(case, &terminal, 409, "ConflictException");
    }
}

/// The presented `PrimaryEmail` must be the address the update was started for:
/// any other address is refused with HTTP 400 `ValidationException` naming the
/// member, and the update is left waiting — so the right address still accepts
/// it afterwards.
#[tokio::test]
async fn accepting_an_address_other_than_the_requested_one_is_refused() {
    // (case, account, the address the update was started for, the address
    // presented instead) — the rows differ in how the two addresses differ:
    // domain, local part, length, and the published bounds.
    let cases: Vec<(&str, &str, String, String)> = vec![
        (
            "a different domain",
            MEMBER_ACCOUNTS[0],
            "ada@example.com".to_string(),
            "ada@example.org".to_string(),
        ),
        (
            "a different local part",
            MEMBER_ACCOUNTS[1],
            "grace@example.com".to_string(),
            "ada@example.com".to_string(),
        ),
        (
            "the shortest address the bound admits against another of the same length",
            MEMBER_ACCOUNTS[2],
            "a@b.c".to_string(),
            "x@y.z".to_string(),
        ),
        (
            "the longest address the bound admits against a short one",
            MEMBER_ACCOUNTS[3],
            address_of_length(PRIMARY_EMAIL_MAX),
            "ada@example.com".to_string(),
        ),
        (
            "a tagged address against the same address without its tag",
            MEMBER_ACCOUNTS[4],
            "ada+aws@example.com".to_string(),
            "ada@example.com".to_string(),
        ),
    ];

    let router = emulator();
    for (case, account_id, requested, presented) in cases {
        start_pending(&router, case, account_id, &requested).await;

        let refused = accept(&router, account_id, &presented, WELL_FORMED_PASSCODES[0]).await;
        assert_refused(case, &refused, 400, "ValidationException");
        assert_eq!(
            refused.body["reason"].as_str().unwrap_or_default(),
            "fieldValidationFailed",
            "case {case:?}: expected reason fieldValidationFailed, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["fieldList"][0]["name"]
                .as_str()
                .unwrap_or_default(),
            "PrimaryEmail",
            "case {case:?}: the refused member must be named, got {:?}",
            refused.body
        );
        // The addresses are `@sensitive`: neither is echoed back.
        let body = refused.body.to_string();
        for address in [&requested, &presented] {
            assert!(
                !body.contains(address.as_str()),
                "case {case:?}: a sensitive address must not be echoed, got {body:?}"
            );
        }

        // Nothing was consumed: the update is still waiting, and the address it
        // was started for still accepts it.
        assert_eq!(
            reported_status(&router, case, account_id).await,
            PENDING,
            "case {case:?}: a refused acceptance must leave the update {PENDING}"
        );
        let accepted = accept(&router, account_id, &requested, WELL_FORMED_PASSCODES[0]).await;
        assert_eq!(
            accepted.reported_status(),
            ACCEPTED,
            "case {case:?}: the requested address must still accept the update, got {:?}",
            accepted.body
        );
    }
}

/// A passcode outside `Otp`'s modelled `^[a-zA-Z0-9]{6}$` is refused with HTTP
/// 400 `ValidationException` and consumes nothing; every passcode of that form
/// is admitted.
#[tokio::test]
async fn a_passcode_outside_the_modelled_form_is_refused() {
    // Rows one character on either side of the modelled length, and rows whose
    // characters are outside the admitted class.
    let malformed = [
        ("one character short of the modelled length", "12345"),
        ("one character past the modelled length", "1234567"),
        ("a punctuation character in the middle", "12-456"),
        ("a space in the middle", "12 456"),
        ("no passcode at all", ""),
        ("a non-ASCII character", "12345é"),
    ];

    let router = emulator();
    let address = "ada@example.com";
    for (case, passcode) in malformed {
        let account_id = MEMBER_ACCOUNTS[5];
        start_pending(&router, case, account_id, address).await;

        let refused = accept(&router, account_id, address, passcode).await;
        assert_refused(case, &refused, 400, "ValidationException");
        assert_eq!(
            reported_status(&router, case, account_id).await,
            PENDING,
            "case {case:?}: a malformed passcode must leave the update {PENDING}"
        );
    }

    // Every well-formed passcode is admitted, on an account of its own.
    for (index, passcode) in WELL_FORMED_PASSCODES.iter().enumerate() {
        let case = format!("the well-formed passcode {passcode}");
        let account_id = MEMBER_ACCOUNTS[index];
        let address = format!("ada{index}@example.com");
        start_pending(&router, &case, account_id, &address).await;
        let accepted = accept(&router, account_id, &address, passcode).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: a well-formed passcode must be admitted, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.reported_status(),
            ACCEPTED,
            "case {case:?}: expected {ACCEPTED}, got {:?}",
            accepted.body
        );
    }
}

/// An acceptance with no update to accept is refused: an account the emulator
/// has never been told about is outside the caller's organization (HTTP 403),
/// the caller's OWN account is not a target of this operation at all (HTTP
/// 403), and an account that exists with no update started has nothing to
/// accept (HTTP 400).
#[tokio::test]
async fn accepting_without_a_pending_update_is_refused() {
    let router = emulator();

    // An account the store knows, with no primary email update: naming it on a
    // PutAccountName is what creates the record.
    let known = MEMBER_ACCOUNTS[6];
    let named = call(
        &router,
        "/putAccountName",
        json!({"AccountId": known, "AccountName": "A member account"}),
    )
    .await;
    assert_eq!(
        named.status, 200,
        "PutAccountName must answer 200 so the account exists, got {} with {:?}",
        named.status, named.body
    );

    // (case, the account, expected status, expected code)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "an account the store does not know",
            MEMBER_ACCOUNTS[7].to_string(),
            403,
            "AccessDeniedException",
        ),
        (
            "another account the store does not know",
            MEMBER_ACCOUNTS[5].to_string(),
            403,
            "AccessDeniedException",
        ),
        (
            "the caller's own account, which this operation cannot target",
            default_account_id().to_string(),
            403,
            "AccessDeniedException",
        ),
        (
            "an account the store knows with no update started",
            known.to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (case, account_id, status, code) in cases {
        let refused = accept(
            &router,
            &account_id,
            "ada@example.com",
            WELL_FORMED_PASSCODES[0],
        )
        .await;
        assert_refused(case, &refused, status, code);
    }
}

/// An address another account's ACCEPTED update is about to take is in use
/// already: its completion is owed by the clock, so the second acceptance
/// conflicts even while the first change is still in flight.
#[tokio::test]
async fn accepting_an_address_an_accepted_update_is_about_to_take_conflicts() {
    // (case, the account that accepts first, the account that accepts next, the
    // address) — the rows differ in the address's length and form.
    let cases: Vec<(&str, &str, &str, String)> = vec![
        (
            "an ordinary address",
            MEMBER_ACCOUNTS[0],
            MEMBER_ACCOUNTS[1],
            "ada@example.com".to_string(),
        ),
        (
            "the shortest address the bound admits",
            MEMBER_ACCOUNTS[2],
            MEMBER_ACCOUNTS[3],
            "a@b.c".to_string(),
        ),
    ];

    let router = emulator();
    for (case, first_account, second_account, address) in cases {
        // Both accounts may ASK for the address: a pending request changes
        // nothing, so neither start is refused.
        start_pending(&router, case, first_account, &address).await;
        start_pending(&router, case, second_account, &address).await;

        let accepted = accept(&router, first_account, &address, WELL_FORMED_PASSCODES[0]).await;
        assert_eq!(
            accepted.reported_status(),
            ACCEPTED,
            "case {case:?}: the first acceptance must answer {ACCEPTED}, got {:?}",
            accepted.body
        );

        // The first change is still in flight — this is asserted, not assumed —
        // and the second acceptance of the same address is already a conflict.
        assert_eq!(
            reported_status(&router, case, first_account).await,
            ACCEPTED,
            "case {case:?}: the first update must still be in flight"
        );
        let conflict = accept(&router, second_account, &address, WELL_FORMED_PASSCODES[1]).await;
        assert_refused(case, &conflict, 409, "ConflictException");
        assert_eq!(
            reported_status(&router, case, second_account).await,
            PENDING,
            "case {case:?}: a refused acceptance must leave the update {PENDING}"
        );

        // And it stays a conflict once the first change has been applied.
        assert_eq!(
            settled_status(&router, case, first_account, ACCEPTED).await,
            COMPLETED,
            "case {case:?}: the first update must settle to {COMPLETED}"
        );
        let still = accept(&router, second_account, &address, WELL_FORMED_PASSCODES[1]).await;
        assert_refused(case, &still, 409, "ConflictException");
    }
}

/// A primary email cannot be changed to an address already in use as an
/// account's primary email: the acceptance that would do it is refused with
/// HTTP 409 `ConflictException`, and the update it refused is left waiting.
#[tokio::test]
async fn accepting_an_address_already_in_use_conflicts() {
    // (case, the account that gets the address first, the account that tries it
    // next, the address) — the rows differ in the address's length and form.
    let cases: Vec<(&str, &str, &str, String)> = vec![
        (
            "an ordinary address",
            MEMBER_ACCOUNTS[0],
            MEMBER_ACCOUNTS[1],
            "ada@example.com".to_string(),
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

    let router = emulator();
    for (case, first_account, second_account, address) in cases {
        // The first account takes the address, all the way to COMPLETED: the
        // address is in use from the instant that completion came due.
        start_pending(&router, case, first_account, &address).await;
        let accepted = accept(&router, first_account, &address, WELL_FORMED_PASSCODES[0]).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the first acceptance must answer 200, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            settled_status(&router, case, first_account, ACCEPTED).await,
            COMPLETED,
            "case {case:?}: the first update must settle to {COMPLETED}"
        );

        // The second account may ask for the same address — nothing has changed
        // yet at that point — but completing the change conflicts.
        start_pending(&router, case, second_account, &address).await;
        let conflict = accept(&router, second_account, &address, WELL_FORMED_PASSCODES[1]).await;
        assert_refused(case, &conflict, 409, "ConflictException");
        assert!(
            !conflict.body.to_string().contains(address.as_str()),
            "case {case:?}: a sensitive address must not be echoed, got {:?}",
            conflict.body
        );

        // The refused acceptance consumed nothing, and the account that holds
        // the address is unaffected.
        assert_eq!(
            reported_status(&router, case, second_account).await,
            PENDING,
            "case {case:?}: a refused acceptance must leave the update {PENDING}"
        );
        assert_eq!(
            reported_status(&router, case, first_account).await,
            COMPLETED,
            "case {case:?}: the account holding the address must stay {COMPLETED}"
        );
    }
}
