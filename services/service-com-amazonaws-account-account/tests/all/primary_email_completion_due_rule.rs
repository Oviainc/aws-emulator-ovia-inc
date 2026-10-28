// SPDX-License-Identifier: CC0-1.0

//! One rule about one stored instant, driven end to end through every operation
//! that decides by it.
//!
//! A primary email update records a single clock fact: the instant its
//! `COMPLETED` status is due. Every answer the service gives about that update
//! is derived from that one fact — "the completion is owed once the due instant
//! is at or before now" — and four answers are derived from it:
//!
//! * `GetPrimaryEmailUpdateStatus` answers `Status` (`ACCEPTED` while nothing is
//!   owed, `COMPLETED` from the due instant on, per
//!   `smithy_query(query="shape", shape="PrimaryEmailUpdateStatus")`),
//! * the same read answers `UpdatedAt` (the due instant for a completion that is
//!   owed, else the instant the stored status was written),
//! * `GetPrimaryEmail` answers `PrimaryEmail` (the address the update was
//!   started for once its completion is owed, else the address the account
//!   already had),
//! * and `StartPrimaryEmailUpdate` / `AcceptPrimaryEmailUpdate` materialise that
//!   completion in the store before they decide anything, so what they persist —
//!   and which of them conflicts — follows the same rule.
//!
//! These suites assert the four answers AGREE for every relation the stored
//! instant can have to the clock: due long ago, due a moment ago, due at the
//! instant the call is served, due later, and no completion stored at all. A
//! disagreement between any two of them is the defect under test; the rows span
//! the class by varying the stored instant and the address (both published
//! `@length` bounds included).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::service_state;
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `PrimaryEmailUpdateStatus` values these suites speak about, from
/// `smithy_query(query="shape", shape="PrimaryEmailUpdateStatus")` — spelled out
/// here, not imported, so a status outside the modelled set cannot pass.
const PENDING: &str = "PENDING";
const ACCEPTED: &str = "ACCEPTED";
const COMPLETED: &str = "COMPLETED";

/// `PrimaryEmailAddress`'s modelled `@length`, from
/// `smithy_query(query="operation", operation="StartPrimaryEmailUpdate")`.
const PRIMARY_EMAIL_MIN: usize = 5;
const PRIMARY_EMAIL_MAX: usize = 64;

/// A passcode inside `Otp`'s modelled `^[a-zA-Z0-9]{6}$` that nothing sent, from
/// `smithy_query(query="operation", operation="AcceptPrimaryEmailUpdate")` — for
/// a case with no outstanding passcode left to present.
const WELL_FORMED_PASSCODE: &str = "a1B2c3";

/// How long a case waits for a status the clock owes before giving up. Far above
/// the emulator's own transition span, so a slow machine is not a failure.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// The wire renders a timestamp as epoch seconds, so a reported instant is
/// compared to the millisecond. Every instant a row writes into the store sits on
/// a whole second, so no rendering of the fraction can blur this.
const INSTANT_TOLERANCE: f64 = 1e-3;

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

/// One emulator: a router carrying only this service over one shared engine,
/// plus that engine, so a case can read the passcode a start sent out of the
/// store — the way the owner of the mailbox reads it off the message — and write
/// the one clock fact a row is about.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The passcode outstanding for the update of `account_id`, or `None` once
    /// it has been consumed (or while no update was ever started).
    fn passcode(&self, account_id: &str) -> Option<String> {
        let state = service_state(&*self.engine).expect("the account store is mounted");
        let store = state.lock();
        store
            .account(account_id)
            .and_then(|account| account.primary_email_update.as_ref())
            .and_then(|update| update.outstanding_otp().map(str::to_string))
    }

    /// Put the row's clock fact on the stored update of `account_id`: the
    /// instant its stored status was written, and the instant its completion is
    /// due (`None` for an update the clock does not move on). Nothing else is
    /// touched, so what every operation then answers is decided by this one
    /// fact and the clock alone — which is what a case can otherwise only
    /// observe by waiting.
    fn set_due_instant(
        &self,
        case: &str,
        account_id: &str,
        updated_at: SystemTime,
        completes_at: Option<SystemTime>,
    ) {
        let state = service_state(&*self.engine).expect("the account store is mounted");
        let mut store = state.lock();
        let update = store
            .existing_account_mut(account_id)
            .and_then(|account| account.primary_email_update.as_mut())
            .unwrap_or_else(|| {
                panic!("case {case:?}: an accepted update must be in the store to re-time")
            });
        update.updated_at = updated_at;
        update.completes_at = completes_at;
    }
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

    /// The `PrimaryEmail` member of a success body.
    fn primary_email(&self) -> &str {
        self.body["PrimaryEmail"].as_str().unwrap_or_default()
    }

    /// The `UpdatedAt` member of a success body, as the epoch seconds the
    /// restJson1 protocol renders a timestamp as.
    fn updated_at(&self) -> Option<f64> {
        self.body["UpdatedAt"].as_f64()
    }
}

async fn call(router: &Emulator, uri: &str, body: Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.router.dispatch(request).await;
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

async fn start(router: &Emulator, account_id: &str, address: &str) -> Answer {
    call(
        router,
        "/startPrimaryEmailUpdate",
        json!({"AccountId": account_id, "PrimaryEmail": address}),
    )
    .await
}

async fn accept(router: &Emulator, account_id: &str, address: &str, passcode: &str) -> Answer {
    call(
        router,
        "/acceptPrimaryEmailUpdate",
        json!({"AccountId": account_id, "PrimaryEmail": address, "Otp": passcode}),
    )
    .await
}

async fn status_read(router: &Emulator, account_id: &str) -> Answer {
    call(
        router,
        "/getPrimaryEmailUpdateStatus",
        json!({"AccountId": account_id}),
    )
    .await
}

async fn email_read(router: &Emulator, account_id: &str) -> Answer {
    call(router, "/getPrimaryEmail", json!({"AccountId": account_id})).await
}

/// An address of exactly `length` characters, in the shape of an email address.
fn address_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// `instant` as the epoch seconds the wire renders it as.
fn epoch_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("an instant at or after the epoch")
        .as_secs_f64()
}

/// The start of the current second: an instant with no fractional part, so every
/// instant a row derives from it renders exactly.
fn whole_second_now() -> SystemTime {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is at or after the epoch");
    UNIX_EPOCH + Duration::from_secs(since_epoch.as_secs())
}

/// Assert the status read reports `expected_status`, and answer the whole body.
async fn assert_reported_status(router: &Emulator, case: &str, account_id: &str, expected: &str) -> Answer {
    let answer = status_read(router, account_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the status read must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.reported_status(),
        expected,
        "case {case:?}: expected Status {expected}, got {:?}",
        answer.body
    );
    answer
}

/// Assert the reported `UpdatedAt` is the instant `expected`.
fn assert_reported_instant(case: &str, answer: &Answer, expected: SystemTime) {
    let expected = epoch_seconds(expected);
    let actual = answer
        .updated_at()
        .unwrap_or_else(|| panic!("case {case:?}: UpdatedAt must be reported, got {:?}", answer.body));
    assert!(
        (actual - expected).abs() < INSTANT_TOLERANCE,
        "case {case:?}: expected UpdatedAt {expected}, got {actual}"
    );
}

/// Assert the address read reports `expected`.
async fn assert_reported_email(router: &Emulator, case: &str, account_id: &str, expected: &str) {
    let answer = email_read(router, account_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the address read must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.primary_email(),
        expected,
        "case {case:?}: expected PrimaryEmail {expected:?}, got {:?}",
        answer.primary_email()
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

/// Start an update, assert it was recorded as `PENDING`, and answer the passcode
/// it was started with.
async fn start_pending(router: &Emulator, case: &str, account_id: &str, address: &str) -> String {
    let started = start(router, account_id, address).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: the start must answer 200, got {} with {:?}",
        started.status, started.body
    );
    assert_eq!(
        started.reported_status(),
        PENDING,
        "case {case:?}: expected a started update to be {PENDING}, got {:?}",
        started.body
    );
    router
        .passcode(account_id)
        .unwrap_or_else(|| panic!("case {case:?}: a started update must have a passcode"))
}

/// Accept the update of `account_id` with the passcode it was started with and
/// assert it answers `ACCEPTED`.
async fn accept_update(router: &Emulator, case: &str, account_id: &str, address: &str, passcode: &str) {
    let accepted = accept(router, account_id, address, passcode).await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: the acceptance must answer 200, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.reported_status(),
        ACCEPTED,
        "case {case:?}: expected {ACCEPTED}, got {:?}",
        accepted.body
    );
}

/// The addresses a lifecycle row walks: the address the update is started for,
/// and a second one for the start that follows it. The rows differ in the
/// address's length and form, both published `@length` bounds included.
fn lifecycle_rows() -> Vec<(&'static str, &'static str, String, &'static str)> {
    let rows = vec![
        (
            "the shortest address the bound admits",
            MEMBER_ACCOUNTS[0],
            "a@b.c".to_string(),
            "next-one@example.com",
        ),
        (
            "the longest address the bound admits",
            MEMBER_ACCOUNTS[1],
            address_of_length(PRIMARY_EMAIL_MAX),
            "next-two@example.com",
        ),
        (
            "a tagged address",
            MEMBER_ACCOUNTS[2],
            "ada+aws@example.co.uk".to_string(),
            "next-three@example.org",
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

/// The whole lifecycle, with no instant written by the case: the four answers
/// about the update's one clock fact agree at every stage — while nothing is
/// owed, from the instant the completion comes due, and across the write that
/// materialises it.
#[tokio::test]
async fn every_answer_about_an_updates_completion_agrees_through_its_lifecycle() {
    let router = Emulator::new();

    for (case, account_id, address, next_address) in lifecycle_rows() {
        let passcode = start_pending(&router, case, account_id, &address).await;
        // The account's own address, recorded when the account came into
        // existence: what every read must keep reporting until a completion is
        // owed.
        let original = email_read(&router, account_id).await.primary_email().to_string();
        assert_ne!(
            original, address,
            "case {case:?}: the row must ask for an address the account does not already hold"
        );

        // A PENDING update owes nothing: no completion instant is stored, so
        // the status read reports the stored status and the address read the
        // stored address.
        let pending = assert_reported_status(&router, case, account_id, PENDING).await;
        assert_reported_email(&router, case, account_id, &original).await;

        accept_update(&router, case, account_id, &address, &passcode).await;
        let accepted = assert_reported_status(&router, case, account_id, ACCEPTED).await;
        let accepted_updated_at = accepted
            .updated_at()
            .unwrap_or_else(|| panic!("case {case:?}: an accepted update must report UpdatedAt"));
        assert!(
            accepted_updated_at
                >= pending.updated_at().unwrap_or_else(|| {
                    panic!("case {case:?}: a pending update must report UpdatedAt")
                }),
            "case {case:?}: the acceptance must not move UpdatedAt backwards, got \
             {accepted_updated_at} after {:?}",
            pending.updated_at()
        );

        // The completion is owed by the clock. Until it comes due the two reads
        // must say the same thing about it, and they are read in an order that
        // makes the agreement checkable without stopping the clock: the address
        // read that BRACKETS the status read on the early side must still be the
        // old address whenever the status is still ACCEPTED, and the one on the
        // late side must be the new address whenever the status is COMPLETED.
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            let before = email_read(&router, account_id).await.primary_email().to_string();
            let status = status_read(&router, account_id).await;
            let after = email_read(&router, account_id).await.primary_email().to_string();
            match status.reported_status() {
                ACCEPTED => assert_eq!(
                    before, original,
                    "case {case:?}: the address read must still report the old address while \
                     the status read reports {ACCEPTED}, got {before:?}"
                ),
                COMPLETED => {
                    assert_eq!(
                        after, address,
                        "case {case:?}: the address read must report the update's address once \
                         the status read reports {COMPLETED}, got {after:?}"
                    );
                    break;
                }
                other => panic!(
                    "case {case:?}: an accepted update must be {ACCEPTED} or {COMPLETED}, got \
                     {other:?}"
                ),
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the completion is still not owed after {SETTLE_TIMEOUT:?} — a \
                 transition the clock owes must come due on its own"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // The completion is owed: UpdatedAt is the instant it came due, so it is
        // at or after the acceptance and at or before this read, and a read does
        // not move it.
        let completed = assert_reported_status(&router, case, account_id, COMPLETED).await;
        let completed_updated_at = completed
            .updated_at()
            .unwrap_or_else(|| panic!("case {case:?}: a completed update must report UpdatedAt"));
        assert!(
            completed_updated_at >= accepted_updated_at
                && completed_updated_at <= epoch_seconds(SystemTime::now()),
            "case {case:?}: expected UpdatedAt in {accepted_updated_at}..=now, got \
             {completed_updated_at}"
        );

        // The write path decides by the same rule: `AcceptPrimaryEmailUpdate`
        // materialises the completion before it looks at the update, so it
        // conflicts with the status the reads already report, and what it
        // persists is exactly what they reported — same status, same instant,
        // same address.
        let again = accept(&router, account_id, &address, &passcode).await;
        assert_refused(case, &again, 409, "ConflictException");
        assert!(
            again.message().contains(COMPLETED),
            "case {case:?}: the conflict must name the settled status {COMPLETED}, got {:?}",
            again.message()
        );
        let after_write = assert_reported_status(&router, case, account_id, COMPLETED).await;
        assert_eq!(
            after_write.updated_at(),
            completed.updated_at(),
            "case {case:?}: materialising the completion must persist the instant the reads \
             reported, got {:?} after {:?}",
            after_write.updated_at(),
            completed.updated_at()
        );
        assert_reported_email(&router, case, account_id, &address).await;

        // And the other write path agrees too: a completed update owes nothing,
        // so a new update replaces it and the address the completion settled
        // stays the account's until that new one completes in its turn.
        let restarted = start(&router, account_id, next_address).await;
        assert_eq!(
            restarted.status, 200,
            "case {case:?}: a start after a completed update must answer 200, got {} with {:?}",
            restarted.status, restarted.body
        );
        assert_reported_status(&router, case, account_id, PENDING).await;
        assert_reported_email(&router, case, account_id, &address).await;
    }
}

/// Whether the row's stored instant makes the completion owed as of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owed {
    Yes,
    No,
}

/// The rows: every relation the ONE stored instant can have to the clock, as an
/// offset in whole seconds from the start of the current second (`None` for an
/// update that stores no completion instant at all). Offset `0` is the boundary
/// case — a completion due at or just before the instant the call is served.
fn due_instant_rows() -> Vec<(&'static str, &'static str, &'static str, Option<i64>, Owed)> {
    vec![
        (
            "a completion that came due an hour ago",
            MEMBER_ACCOUNTS[3],
            "due-long-ago@example.com",
            Some(-3600),
            Owed::Yes,
        ),
        (
            "a completion that came due a second ago",
            MEMBER_ACCOUNTS[4],
            "due-a-moment-ago@example.com",
            Some(-1),
            Owed::Yes,
        ),
        (
            "a completion due at the instant the call is served",
            MEMBER_ACCOUNTS[5],
            "due-now@example.com",
            Some(0),
            Owed::Yes,
        ),
        (
            "a completion not due for another hour",
            MEMBER_ACCOUNTS[6],
            "due-later@example.com",
            Some(3600),
            Owed::No,
        ),
        (
            "an update that stores no completion instant",
            MEMBER_ACCOUNTS[7],
            "never-due@example.com",
            None,
            Owed::No,
        ),
    ]
}

/// The same four answers, over every relation the stored instant can have to the
/// clock: the status, the instant, the address and what the write path persists
/// all follow from that one instant, so they can never disagree about it.
#[tokio::test]
async fn every_answer_about_a_stored_due_instant_agrees() {
    let router = Emulator::new();

    for (case, account_id, address, due_offset, owed) in due_instant_rows() {
        let passcode = start_pending(&router, case, account_id, address).await;
        let original = email_read(&router, account_id).await.primary_email().to_string();
        accept_update(&router, case, account_id, address, &passcode).await;

        // The row's clock fact, written over the one the acceptance recorded:
        // the stored status was written two hours ago, and its completion is due
        // at the row's offset.
        let base = whole_second_now();
        let stored_updated_at = base - Duration::from_secs(7200);
        let due = due_offset.map(|offset| {
            if offset < 0 {
                base - Duration::from_secs(offset.unsigned_abs())
            } else {
                base + Duration::from_secs(offset.unsigned_abs())
            }
        });
        router.set_due_instant(case, account_id, stored_updated_at, due);

        let (expected_status, expected_address, expected_instant) = match owed {
            // Owed: the status is the terminal one, the instant reported is the
            // one the completion came due at, and the address is the update's.
            Owed::Yes => (
                COMPLETED,
                address.to_string(),
                due.expect("an owed completion has a due instant"),
            ),
            // Not owed: every answer is the stored one.
            Owed::No => (ACCEPTED, original.clone(), stored_updated_at),
        };

        let reported = assert_reported_status(&router, case, account_id, expected_status).await;
        assert_reported_instant(case, &reported, expected_instant);
        assert_reported_email(&router, case, account_id, &expected_address).await;

        // Reads leave the stored instant alone, so asking twice answers twice
        // the same.
        let again = assert_reported_status(&router, case, account_id, expected_status).await;
        assert_eq!(
            again.updated_at(),
            reported.updated_at(),
            "case {case:?}: a read must not move UpdatedAt, got {:?} then {:?}",
            reported.updated_at(),
            again.updated_at()
        );

        // The confirming write path decides by the same instant: it
        // materialises an owed completion before it looks at the update, and
        // the update is no longer waiting for a passcode either way — so the
        // conflict it answers names the status the reads report.
        let confirmed = accept(&router, account_id, address, WELL_FORMED_PASSCODE).await;
        assert_refused(case, &confirmed, 409, "ConflictException");
        assert!(
            confirmed.message().contains(expected_status),
            "case {case:?}: the conflict must name the status the reads report \
             ({expected_status}), got {:?}",
            confirmed.message()
        );
        // And what that write persisted is exactly what the reads reported:
        // same status, same instant, same address.
        let persisted = assert_reported_status(&router, case, account_id, expected_status).await;
        assert_reported_instant(case, &persisted, expected_instant);
        assert_reported_email(&router, case, account_id, &expected_address).await;

        // The other write path reads the same instant: `StartPrimaryEmailUpdate`
        // replaces an update whose completion is already owed, and refuses to
        // discard one that is still owed later.
        let restart_address = format!("restart-{account_id}@example.net");
        let before_restart = SystemTime::now();
        let restarted = start(&router, account_id, &restart_address).await;
        match owed {
            Owed::Yes => {
                assert_eq!(
                    restarted.status, 200,
                    "case {case:?}: a start over a completion already owed must answer 200, \
                     got {} with {:?}",
                    restarted.status, restarted.body
                );
                let after = assert_reported_status(&router, case, account_id, PENDING).await;
                let reported = after.updated_at().unwrap_or_else(|| {
                    panic!("case {case:?}: a restarted update must report UpdatedAt")
                });
                assert!(
                    reported >= epoch_seconds(before_restart)
                        && reported <= epoch_seconds(SystemTime::now()),
                    "case {case:?}: expected UpdatedAt in {}..={}, got {reported}",
                    epoch_seconds(before_restart),
                    epoch_seconds(SystemTime::now())
                );
                // The address the completion settled is the account's, and the
                // update just started has not completed, so the read still
                // reports the settled one.
                assert_reported_email(&router, case, account_id, address).await;
            }
            Owed::No => {
                assert_refused(case, &restarted, 409, "ConflictException");
                assert!(
                    restarted.message().contains(ACCEPTED),
                    "case {case:?}: the conflict must name the status the update is in \
                     ({ACCEPTED}), got {:?}",
                    restarted.message()
                );
                // Nothing was materialised, so every answer is still the stored
                // one.
                let after = assert_reported_status(&router, case, account_id, ACCEPTED).await;
                assert_reported_instant(case, &after, stored_updated_at);
                assert_reported_email(&router, case, account_id, &original).await;
            }
        }
    }
}
