// SPDX-License-Identifier: CC0-1.0

//! What `GetAccountInformation` puts on the wire, over the real request path.
//!
//! Every case goes through `MultiServiceRouter::dispatch` with `POST
//! /getAccountInformation` and a restJson1 body, so routing, the boundary validator, the
//! handler, the persisted store and the response codec all take part — the same path an
//! SDK client takes.
//!
//! Two rules are under test, each over a TABLE of inputs rather than the one request the
//! issue recorded:
//!
//! 1. A timestamp member whose model declares `@timestampFormat("date-time")` is
//!    serialized as an RFC 3339 date-time STRING naming the stored instant, never as the
//!    protocol's default epoch-seconds number. `AccountCreatedDate` targets
//!    `com.amazonaws.account#AccountCreatedDate`, which carries that trait
//!    (`smithy_query(query="shape", shape="com.amazonaws.account#AccountCreatedDate")`).
//! 2. Every account record this emulator holds has a name from the moment it exists, so
//!    `GetAccountInformation` reports `AccountName` on every 200: before any rename, and
//!    the value `PutAccountName` last stored after one. The reported name always
//!    satisfies the member's modelled `@length(min: 1, max: 50)` and `^[ -;=?-~]+$`.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::{default_account_id, service_state, Account};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// `account` service. Signatures are not verified here (authentication is off).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `AccountName`'s modelled `@length(min: 1, max: 50)`, from
/// `smithy_query(query="shape", shape="com.amazonaws.account#AccountName")`.
const ACCOUNT_NAME_MIN: usize = 1;
const ACCOUNT_NAME_MAX: usize = 50;

/// `AccountState` for a live account, from
/// `smithy_query(query="shape", shape="com.amazonaws.account#AccountState")`
/// (`PENDING_ACTIVATION | ACTIVE | SUSPENDED | CLOSED`).
const ACTIVE: &str = "ACTIVE";

/// One `AccountCreatedDate` case: a label, the instant the record was created (whole
/// seconds since the epoch plus nanoseconds), and the RFC 3339 rendering the wire must
/// carry when that rendering is exactly computable by hand.
///
/// The expected strings are the UTC renderings of those epoch offsets, computed from the
/// Unix epoch rather than from the implementation: 1_000_000 s is 11 d 13 h 46 m 40 s
/// after 1970-01-01 (the same value the generator-gap issue quotes as
/// `"1970-01-12T13:46:40Z"`), 951_782_400 is the 2000 leap day, 1_000_000_000 and
/// 1_700_000_000 are the well-known billion/1.7-billion instants, and 4_102_444_800 is
/// 2100-01-01 (36_525 days after 2000-01-01: 100 years with the 25 leap days of
/// 2000..2096, 2100 itself not being one).
const CREATED_DATE_CASES: &[(&str, u64, u32, Option<&str>)] = &[
    ("the epoch itself", 0, 0, Some("1970-01-01T00:00:00Z")),
    (
        "a million seconds after the epoch",
        1_000_000,
        0,
        Some("1970-01-12T13:46:40Z"),
    ),
    (
        "a leap day",
        951_782_400,
        0,
        Some("2000-02-29T00:00:00Z"),
    ),
    (
        "the turn of the billion",
        1_000_000_000,
        0,
        Some("2001-09-09T01:46:40Z"),
    ),
    (
        "a recent whole second",
        1_700_000_000,
        0,
        Some("2023-11-14T22:13:20Z"),
    ),
    // A sub-second instant, which is what a store created from the wall clock holds —
    // the fraction's rendering is the codec's, so only the round trip is asserted.
    ("an instant inside a second", 1_700_000_000, 645_000_000, None),
    (
        "a far-future instant",
        4_102_444_800,
        0,
        Some("2100-01-01T00:00:00Z"),
    ),
];

/// Names spanning what `AccountName` admits: both published length bounds, and values
/// differing in length and in the punctuation `^[ -;=?-~]+$` allows.
fn admitted_names() -> Vec<(&'static str, String)> {
    vec![
        (
            "the shortest name the bound admits",
            "A".repeat(ACCOUNT_NAME_MIN),
        ),
        (
            "the longest name the bound admits",
            "N".repeat(ACCOUNT_NAME_MAX),
        ),
        ("a name with spaces", "Demo Account".to_string()),
        (
            "a name with admitted punctuation",
            "Acct #1 (prod) ?+-:;@~".to_string(),
        ),
        ("a name of digits", "1234567890".to_string()),
    ]
}

/// Whether `value` satisfies `AccountName`'s modelled length bound and pattern. Spelled
/// out here, not imported, so a wrong set in the implementation cannot make this pass:
/// `^[ -;=?-~]+$` admits every ASCII byte from space to `;`, plus `=`, plus `?` to `~`.
fn is_account_name(value: &str) -> bool {
    (ACCOUNT_NAME_MIN..=ACCOUNT_NAME_MAX).contains(&value.len())
        && value
            .bytes()
            .all(|b| (b' '..=b';').contains(&b) || b == b'=' || (b'?'..=b'~').contains(&b))
}

/// One emulator, plus the shared engine behind it so a case can seed a record whose
/// creation instant it chooses instead of reading the wall clock.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Put an account record in the store, created at `created`. This is the store the
/// handler reads — the same one a `PutAccountName` would have written.
fn seed_account(engine: &engine::InMemoryEngine, account_id: &str, created: SystemTime) {
    let state = service_state(engine).expect("the account store is reachable");
    state.lock().accounts.insert(
        account_id.to_string(),
        Account::new(account_id.to_string(), created),
    );
}

/// What the emulator answered: the HTTP status, the parsed body and the raw bytes (so a
/// case can assert the JSON TYPE a member went out as, not just its value).
struct Answer {
    status: u16,
    body: Value,
    raw: String,
}

impl Answer {
    /// One member of this answer's body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
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
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("POST {uri} answered a body that is not JSON ({error}): {raw:?}"))
    };
    Answer { status, body, raw }
}

async fn get_account_information(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/getAccountInformation", request).await
}

async fn put_account_name(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/putAccountName", request).await
}

async fn put_alternate_contact(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/putAlternateContact", request).await
}

/// A 12-digit account id (`^\d{12}$`) keyed by `index`, so each case owns a record.
fn account_id(index: usize) -> String {
    format!("{:012}", 700_000_000_000u64 + index as u64)
}

// ---------------------------------------------------------------------------
// Rule 1: a `date-time` timestamp goes out as an RFC 3339 string.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_created_date_is_an_rfc_3339_string_for_every_stored_instant() {
    let (router, engine) = emulator();

    for (index, (case, secs, nanos, expected)) in CREATED_DATE_CASES.iter().enumerate() {
        let id = account_id(index);
        let created = UNIX_EPOCH + Duration::new(*secs, *nanos);
        seed_account(&engine, &id, created);

        let answer = get_account_information(&router, json!({"AccountId": id})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: GetAccountInformation must answer 200, got {} with {:?}",
            answer.status, answer.body
        );

        let value = &answer.body["AccountCreatedDate"];
        assert!(
            value.is_string(),
            "case {case:?}: AccountCreatedDate is modelled @timestampFormat(\"date-time\"), \
             so it must be a JSON string, got {value} in {:?}",
            answer.raw
        );
        let text = value.as_str().expect("checked to be a string");

        if let Some(expected) = expected {
            assert_eq!(
                text, *expected,
                "case {case:?}: expected {expected:?}, got {text:?}"
            );
        }

        let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
            panic!("case {case:?}: {text:?} is not an RFC 3339 date-time: {error}")
        });
        let stored = DateTime::from(created);
        assert_eq!(
            (parsed.secs(), parsed.subsec_nanos()),
            (stored.secs(), stored.subsec_nanos()),
            "case {case:?}: the reported instant must be the stored one, got {text:?}"
        );
    }
}

#[tokio::test]
async fn the_created_date_keeps_its_wire_type_across_reads_and_renames() {
    let (router, _engine) = emulator();
    let own = default_account_id().to_string();

    // The caller's own record exists before any call, with the instant the store came up:
    // a wall-clock creation date, not one a case chose.
    let first = get_account_information(&router, json!({})).await;
    assert_eq!(
        first.status, 200,
        "the caller's own account must be reportable before any write, got {} with {:?}",
        first.status, first.body
    );
    let created = first.body["AccountCreatedDate"].clone();
    assert!(
        created.is_string(),
        "AccountCreatedDate must be a date-time string, got {created} in {:?}",
        first.raw
    );
    let created_text = created.as_str().expect("checked to be a string").to_string();
    DateTime::from_str(&created_text, Format::DateTime).unwrap_or_else(|error| {
        panic!("{created_text:?} is not an RFC 3339 date-time: {error}")
    });

    // Rows differ along how the read names the account and along what has been written to
    // it since: the created date is set once, so every row reports the same string. The
    // caller's own record is reached by the standalone call only — naming its own id is
    // the authorization refusal, asserted on its own below.
    for (case, request) in [
        ("a second read, AccountId absent", json!({})),
        ("a third read, AccountId absent", json!({})),
    ] {
        let answer = get_account_information(&router, request).await;
        assert_eq!(
            answer.body["AccountCreatedDate"], created,
            "case {case:?}: a report must not move the created date, got {:?}",
            answer.body["AccountCreatedDate"]
        );
    }

    let named = get_account_information(&router, json!({"AccountId": own})).await;
    assert_eq!(
        (named.status, named.error_code()),
        (403, "AccessDeniedException"),
        "case \"AccountId named explicitly\": naming the caller's own id must be refused, \
         got {} with {:?}",
        named.status,
        named.body
    );

    for (case, name) in admitted_names() {
        let put = put_account_name(&router, json!({"AccountName": name})).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutAccountName must answer 200 for {name:?}, got {} with {:?}",
            put.status, put.body
        );
        let answer = get_account_information(&router, json!({})).await;
        assert_eq!(
            answer.body["AccountCreatedDate"], created,
            "case {case:?}: a rename must not move the created date, got {:?}",
            answer.body["AccountCreatedDate"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an account always has a name, so every 200 reports one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_account_name_is_reported_before_any_rename() {
    let (router, _engine) = emulator();
    let own = default_account_id().to_string();

    // Rows differ along how the record came to exist and how the read names it: the
    // caller's own pre-existing account (reached by the standalone call, which is the
    // only way to reach it), and a member account this emulator learned about from a
    // write that never set a name.
    let contact = json!({
        "AccountId": account_id(0),
        "AlternateContactType": "BILLING",
        "Name": "Billing Contact",
        "Title": "Controller",
        "EmailAddress": "billing@example.com",
        "PhoneNumber": "+1-555-0100",
    });
    let put = put_alternate_contact(&router, contact).await;
    assert_eq!(
        put.status, 200,
        "PutAlternateContact must answer 200, got {} with {:?}",
        put.status, put.body
    );

    for (case, request) in [
        ("the caller's own account, AccountId absent", json!({})),
        (
            "a member account known only from an alternate-contact write",
            json!({"AccountId": account_id(0)}),
        ),
    ] {
        let answer = get_account_information(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: GetAccountInformation must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let name = answer.member("AccountName");
        assert!(
            is_account_name(name),
            "case {case:?}: AccountName must be reported and satisfy @length(1..50) and \
             ^[ -;=?-~]+$, got {:?}",
            answer.body["AccountName"]
        );
        assert_eq!(
            answer.member("AccountState"),
            ACTIVE,
            "case {case:?}: a known account must report {ACTIVE}, got {:?}",
            answer.body["AccountState"]
        );
    }

    let named = get_account_information(&router, json!({"AccountId": own})).await;
    assert_eq!(
        (named.status, named.error_code()),
        (403, "AccessDeniedException"),
        "case \"the caller's own account, AccountId named\": naming the caller's own id \
         must be refused, got {} with {:?}",
        named.status,
        named.body
    );
}

#[tokio::test]
async fn the_reported_name_is_the_one_last_stored_and_never_absent() {
    let (router, _engine) = emulator();

    // One record per row, each reachable only by its own account id, so a name written to
    // one cannot be what another reports.
    for (row, (case, name)) in admitted_names().into_iter().enumerate() {
        let id = account_id(10 + row);

        // Before the rename: the record does not exist yet, so a read is the modelled
        // refusal rather than a 200 with no name.
        let unknown = get_account_information(&router, json!({"AccountId": id})).await;
        assert_eq!(
            unknown.status, 403,
            "case {case:?}: an account with no record must be refused, got {} with {:?}",
            unknown.status, unknown.body
        );

        let put = put_account_name(&router, json!({"AccountName": name, "AccountId": id})).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutAccountName must answer 200 for {name:?}, got {} with {:?}",
            put.status, put.body
        );

        let answer = get_account_information(&router, json!({"AccountId": id})).await;
        assert_eq!(
            answer.member("AccountName"),
            name.as_str(),
            "case {case:?}: AccountName must be read back as stored, got {:?}",
            answer.body["AccountName"]
        );
        assert!(
            answer.body["AccountCreatedDate"].is_string(),
            "case {case:?}: AccountCreatedDate must be a date-time string, got {:?}",
            answer.raw
        );
    }
}

/// The invariant over a run of records: whatever an account's id and however it came to
/// exist, a 200 carries both an admitted `AccountName` and a date-time
/// `AccountCreatedDate`.
#[tokio::test]
async fn every_reported_account_carries_a_name_and_a_date_time_created_date() {
    let (router, engine) = emulator();

    // Twenty records with distinct ids and creation instants an hour apart, half seeded
    // into the store and half created by a write, so neither path can be the only one
    // that holds.
    for row in 0..20usize {
        let id = account_id(100 + row);
        if row % 2 == 0 {
            seed_account(
                &engine,
                &id,
                UNIX_EPOCH + Duration::from_secs(1_600_000_000 + row as u64 * 3_600),
            );
        } else {
            let put = put_account_name(
                &router,
                json!({"AccountName": format!("Row {row}"), "AccountId": id}),
            )
            .await;
            assert_eq!(
                put.status, 200,
                "row {row}: PutAccountName must answer 200, got {} with {:?}",
                put.status, put.body
            );
        }

        let answer = get_account_information(&router, json!({"AccountId": id})).await;
        assert_eq!(
            answer.status, 200,
            "row {row}: GetAccountInformation must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let name = answer.member("AccountName");
        assert!(
            is_account_name(name),
            "row {row}: AccountName must be reported and admitted by the model, got {:?}",
            answer.body["AccountName"]
        );
        let value = &answer.body["AccountCreatedDate"];
        let text = value.as_str().unwrap_or_else(|| {
            panic!("row {row}: AccountCreatedDate must be a JSON string, got {value}")
        });
        DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
            panic!("row {row}: {text:?} is not an RFC 3339 date-time: {error}")
        });
    }
}
