// SPDX-License-Identifier: CC0-1.0

//! Accepting a primary email update for the address the TARGET account itself
//! already holds, end to end over HTTP.
//!
//! The rule under test: an address is "already in use" — the conflict this
//! service's `ConflictException` documents by example ("you try to change an
//! account's root user email to an email address which is already in use") —
//! only when an account OTHER than the one being updated holds it, or is owed
//! it by an accepted update. Giving an account the address it already holds
//! takes that address from nobody, so it is no change and no conflict: the
//! acceptance completes.
//!
//! The table spans the class rather than one request: its rows differ in where
//! the address the account already holds came from (the seeded root address of
//! a record, or an address a previous completed flow put there), in the
//! address's length and form (both published `@length` bounds included) and in
//! the passcode's form. Each row also carries the other side of the rule: a
//! DIFFERENT account presented the same address is still refused with HTTP 409.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::initial_primary_email;
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

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

/// How long a test waits for a status the clock owes before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Member account ids, each satisfying `AccountId`'s `^\d{12}$` and none of them
/// the caller's own account. One row gets its own pair, so no row can see what
/// another row recorded.
const MEMBER_ACCOUNTS: [&str; 8] = [
    "811111111111",
    "822222222222",
    "833333333333",
    "844444444444",
    "855555555555",
    "866666666666",
    "877777777777",
    "888888888888",
];

/// Passcodes spanning the forms `Otp`'s `^[a-zA-Z0-9]{6}$` admits.
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

    /// The `Status` member of a success body.
    fn reported_status(&self) -> &str {
        self.body["Status"].as_str().unwrap_or_default()
    }

    /// The `PrimaryEmail` member of a success body.
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

async fn update_status(router: &MultiServiceRouter, case: &str, account_id: &str) -> String {
    let answer = call(
        router,
        "/getPrimaryEmailUpdateStatus",
        json!({"AccountId": account_id}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the status read must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.reported_status().to_string()
}

/// The primary email the emulator reports for `account_id`, read back through
/// `GetPrimaryEmail`.
async fn primary_email(router: &MultiServiceRouter, case: &str, account_id: &str) -> String {
    let answer = call(router, "/getPrimaryEmail", json!({"AccountId": account_id})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetPrimaryEmail must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.primary_email().to_string()
}

/// Poll the update status of `account_id` until it leaves `transient`, and
/// answer what it settled to.
async fn settled_status(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    transient: &str,
) -> String {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let status = update_status(router, case, account_id).await;
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

/// Run the whole flow — start, accept, settle — and assert the account holds
/// `address` afterwards.
async fn complete_update(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    address: &str,
    passcode: &str,
) {
    start_pending(router, case, account_id, address).await;
    let accepted = accept(router, account_id, address, passcode).await;
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
    assert_eq!(
        settled_status(router, case, account_id, ACCEPTED).await,
        COMPLETED,
        "case {case:?}: an accepted update must settle to {COMPLETED}"
    );
    assert_eq!(
        primary_email(router, case, account_id).await,
        address,
        "case {case:?}: a completed update must leave the account holding the address"
    );
}

/// An address of exactly `length` characters, in the shape of an email address.
fn address_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// One row: the account under test, the address it ALREADY holds when the
/// acceptance under test runs, whether that address has to be put there by a
/// completed flow first, the passcode, and a second account for the other side
/// of the rule.
struct Row {
    case: &'static str,
    account_id: &'static str,
    address: String,
    /// `true` when the account holds the address from the moment its record
    /// exists (its seeded root address), so no flow has to put it there.
    held_from_the_start: bool,
    passcode: &'static str,
    other_account_id: &'static str,
}

fn rows() -> Vec<Row> {
    let rows = vec![
        Row {
            case: "the seeded root address of the account's own record",
            account_id: MEMBER_ACCOUNTS[0],
            address: initial_primary_email(MEMBER_ACCOUNTS[0]),
            held_from_the_start: true,
            passcode: WELL_FORMED_PASSCODES[0],
            other_account_id: MEMBER_ACCOUNTS[1],
        },
        Row {
            case: "an ordinary address a previous completed flow put there",
            account_id: MEMBER_ACCOUNTS[2],
            address: "ada@example.com".to_string(),
            held_from_the_start: false,
            passcode: WELL_FORMED_PASSCODES[1],
            other_account_id: MEMBER_ACCOUNTS[3],
        },
        Row {
            case: "the shortest address the bound admits",
            account_id: MEMBER_ACCOUNTS[4],
            address: "a@b.c".to_string(),
            held_from_the_start: false,
            passcode: WELL_FORMED_PASSCODES[2],
            other_account_id: MEMBER_ACCOUNTS[5],
        },
        Row {
            case: "the longest address the bound admits",
            account_id: MEMBER_ACCOUNTS[6],
            address: address_of_length(PRIMARY_EMAIL_MAX),
            held_from_the_start: false,
            passcode: WELL_FORMED_PASSCODES[3],
            other_account_id: MEMBER_ACCOUNTS[7],
        },
    ];
    assert_eq!(
        rows[2].address.chars().count(),
        PRIMARY_EMAIL_MIN,
        "the shortest row must sit on the published minimum"
    );
    assert_eq!(
        rows[3].address.chars().count(),
        PRIMARY_EMAIL_MAX,
        "the longest row must sit on the published maximum"
    );
    rows
}

/// Accepting an update for the address the target account ALREADY holds
/// completes: it is no change, so it is not the "already in use" conflict. The
/// same address presented for a DIFFERENT account still conflicts.
#[tokio::test]
async fn accepting_the_address_the_target_account_already_holds_completes() {
    let router = emulator();

    for row in rows() {
        let Row {
            case,
            account_id,
            address,
            held_from_the_start,
            passcode,
            other_account_id,
        } = row;

        if held_from_the_start {
            // The record comes into existence on the first write to the
            // account, carrying its seeded root address — and starting the
            // update under test is that write, so the address the acceptance
            // presents is the one the record already holds.
            start_pending(&router, case, account_id, &address).await;
            assert_eq!(
                primary_email(&router, case, account_id).await,
                address,
                "case {case:?}: the account must hold its seeded address before any update \
                 has completed"
            );
        } else {
            // A completed flow puts the address on the account first; the
            // update under test then re-affirms it.
            complete_update(&router, case, account_id, &address, passcode).await;
            start_pending(&router, case, account_id, &address).await;
        }

        // The acceptance under test: the account re-affirms the address it
        // already holds, and the flow completes rather than conflicting.
        let accepted = accept(&router, account_id, &address, passcode).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: re-affirming the address the account holds must answer 200, \
             got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.reported_status(),
            ACCEPTED,
            "case {case:?}: expected {ACCEPTED}, got {:?}",
            accepted.body
        );
        assert_eq!(
            settled_status(&router, case, account_id, ACCEPTED).await,
            COMPLETED,
            "case {case:?}: the re-affirmed update must settle to {COMPLETED}"
        );
        assert_eq!(
            primary_email(&router, case, account_id).await,
            address,
            "case {case:?}: the account must still hold the address afterwards"
        );

        // The other side of the same rule: the address IS in use against every
        // other account, so one of those presented it is refused — and the
        // refusal leaves its own update waiting.
        start_pending(&router, case, other_account_id, &address).await;
        let conflict = accept(&router, other_account_id, &address, passcode).await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: another account taking the address must answer 409, got {} with {:?}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            conflict.body
        );
        assert_eq!(
            update_status(&router, case, other_account_id).await,
            PENDING,
            "case {case:?}: a refused acceptance must leave the update {PENDING}"
        );
    }
}

/// An account whose own update is accepted and not yet complete re-presents the
/// address it is already owed: the second acceptance conflicts on the update's
/// STATUS (only a `PENDING` update can be accepted), never on the address — and
/// the change the clock owes still lands.
#[tokio::test]
async fn an_accounts_own_in_flight_address_is_not_held_against_it() {
    let router = emulator();
    let case = "the account's own accepted update, still in flight";
    let account_id = MEMBER_ACCOUNTS[0];
    let address = "grace@example.com";

    start_pending(&router, case, account_id, address).await;
    let accepted = accept(&router, account_id, address, WELL_FORMED_PASSCODES[0]).await;
    assert_eq!(
        accepted.reported_status(),
        ACCEPTED,
        "case {case:?}: the acceptance must answer {ACCEPTED}, got {:?}",
        accepted.body
    );

    let again = accept(&router, account_id, address, WELL_FORMED_PASSCODES[1]).await;
    assert_eq!(
        again.status, 409,
        "case {case:?}: accepting an already accepted update must answer 409, got {} with {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {:?}",
        again.body
    );

    assert_eq!(
        settled_status(&router, case, account_id, ACCEPTED).await,
        COMPLETED,
        "case {case:?}: the accepted update must settle to {COMPLETED}"
    );
    assert_eq!(
        primary_email(&router, case, account_id).await,
        address,
        "case {case:?}: the settled update must leave the account holding the address"
    );
}
