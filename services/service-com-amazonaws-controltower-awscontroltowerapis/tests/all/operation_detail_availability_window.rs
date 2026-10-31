// SPDX-License-Identifier: CC0-1.0

//! The window an operation's details stay available for, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies — not the handlers'
//! Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of ages rather than one case:
//!
//! * a record of an operation log whose reporting operations document an availability
//!   window is reported for as long as it is inside that window and not at all after it:
//!   past it the by-identifier read answers exactly as it does for an identifier no call
//!   ever returned, `ResourceNotFoundException` (404);
//! * the two readers of one log agree about one record: a record the by-identifier read no
//!   longer resolves is a record the listing no longer reports, and vice versa;
//! * the window is the CLOCK's: reading a record neither drops one that is still inside it
//!   nor brings back one past it, however many times a caller reads.
//!
//! Both logs this covers are driven: landing zone operations (`GetLandingZoneOperation` /
//! `ListLandingZoneOperations`) and control operations (`GetControlOperation` /
//! `ListControlOperations`).
//!
//! Every value written out by hand here names its source. The window itself comes from the
//! operations' own model docs — `smithy_query(query="doc",
//! shape="GetLandingZoneOperation")` and `shape="GetControlOperation"` both say "Details
//! for an operation are available for 90 days", and `shape="ListLandingZoneOperations"`
//! says the listing covers "the past 90 days" — and is read off the crate's own
//! `state::OPERATION_DETAILS_AVAILABLE_FOR` rather than retyped, with the documented
//! 90-day length pinned once below. `ResourceNotFoundException` (404) is among both read
//! operations' modelled errors (`smithy_query(query="operation",
//! operation="GetLandingZoneOperation,GetControlOperation")`), and the operation types the
//! rows cycle through come from `smithy_query(query="shape",
//! shape="LandingZoneOperationType,ControlOperationType")`.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    service_state, ControlOperationType, LandingZoneOperationType,
    OPERATION_DETAILS_AVAILABLE_FOR,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{control_operations, member};
use crate::landing_zone_lifecycle::{call, emulator_with_engine, Answer};

/// One day, the unit the documented window is expressed in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A landing zone ARN of the form this service mints, for the records a case appends to the
/// log directly.
const SEEDED_LANDING_ZONE_ARN: &str =
    "arn:aws:controltower:us-west-2:000000000000:landingzone/retention";

/// A control ARN of the form the catalog publishes, for the control records a case appends
/// directly.
const SEEDED_CONTROL: &str = "arn:aws:controltower:us-west-2::control/AWS-GR_LOG_RETENTION";

/// An organizational-unit target of the form `EnableControl` accepts.
const SEEDED_TARGET: &str = "arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-ret1";

/// The `EnabledControl` a seeded control operation acted on.
const SEEDED_ENABLED_CONTROL_ARN: &str =
    "arn:aws:controltower:us-west-2:000000000000:enabledcontrol/RETENTION";

/// Which log a row drives, and so which pair of reporting operations answers for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Log {
    LandingZoneOperations,
    ControlOperations,
}

impl Log {
    /// The `@http` path of the operation that reads one record of this log by identifier.
    fn read_path(self) -> &'static str {
        match self {
            Self::LandingZoneOperations => "/get-landingzone-operation",
            Self::ControlOperations => "/get-control-operation",
        }
    }
}

/// Every log an availability window is documented for.
const WINDOWED_LOGS: &[Log] = &[Log::LandingZoneOperations, Log::ControlOperations];

/// The landing zone operation types a row cycles through, so no row depends on one type.
const LANDING_ZONE_TYPES: &[LandingZoneOperationType] = &[
    LandingZoneOperationType::Create,
    LandingZoneOperationType::Update,
    LandingZoneOperationType::Reset,
    LandingZoneOperationType::Delete,
];

/// The control operation types a row cycles through, for the same reason.
const CONTROL_TYPES: &[ControlOperationType] = &[
    ControlOperationType::EnableControl,
    ControlOperationType::UpdateEnabledControl,
    ControlOperationType::ResetEnabledControl,
    ControlOperationType::DisableControl,
];

/// Append one record per start time straight to `log`, answering their identifiers in the
/// order given.
///
/// The cases here are about records whose start times lie months apart, which no sequence
/// of calls inside one test could produce; everything else goes through the wire path.
fn seed(
    engine: &engine::InMemoryEngine,
    log: Log,
    start_times: &[SystemTime],
) -> Vec<String> {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    start_times
        .iter()
        .enumerate()
        .map(|(index, at)| match log {
            Log::LandingZoneOperations => store.record_operation(
                LANDING_ZONE_TYPES[index % LANDING_ZONE_TYPES.len()],
                SEEDED_LANDING_ZONE_ARN,
                *at,
            ),
            Log::ControlOperations => store.record_control_operation(
                CONTROL_TYPES[index % CONTROL_TYPES.len()],
                SEEDED_CONTROL,
                SEEDED_TARGET,
                SEEDED_ENABLED_CONTROL_ARN,
                *at,
            ),
        })
        .collect()
}

/// Read one record of `log` by identifier.
async fn read(router: &MultiServiceRouter, log: Log, operation_identifier: &str) -> Answer {
    call(
        router,
        log.read_path(),
        serde_json::json!({ "operationIdentifier": operation_identifier }),
    )
    .await
}

/// The identifiers the listing over `log` reports.
async fn listed(router: &MultiServiceRouter, log: Log) -> Vec<String> {
    match log {
        Log::LandingZoneOperations => {
            let answer = call(
                router,
                "/list-landingzone-operations",
                serde_json::json!({ "maxResults": 100 }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "list-landingzone-operations: expected 200, got {} body {:?}",
                answer.status, answer.body
            );
            crate::enabled_control_lifecycle::entries(answer.get("landingZoneOperations"))
        }
        Log::ControlOperations => control_operations(router, serde_json::Value::Null).await,
    }
    .iter()
    .map(|record| {
        member(record, "operationIdentifier")
            .unwrap_or_default()
            .to_string()
    })
    .collect()
}

/// How many records each windowed log holds, straight off the store.
fn log_sizes(engine: &engine::InMemoryEngine) -> (usize, usize) {
    let state = service_state(engine).expect("the service's own store");
    let store = state.lock();
    (
        store.landing_zone_operations.len(),
        store.control_operations.len(),
    )
}

/// The ages a case spans: well inside the window at both ends, a minute inside its
/// published bound, and one second, a minute, a day and a year past it.
///
/// `(case, how far back the record started, whether its details are still available)`
fn ages() -> Vec<(&'static str, Duration, bool)> {
    vec![
        ("started moments ago", Duration::from_secs(1), true),
        ("a day old", DAY, true),
        ("a month old", 30 * DAY, true),
        (
            "an hour inside the window",
            OPERATION_DETAILS_AVAILABLE_FOR - Duration::from_secs(3600),
            true,
        ),
        (
            "a minute inside the window",
            OPERATION_DETAILS_AVAILABLE_FOR - Duration::from_secs(60),
            true,
        ),
        (
            "a second past the window",
            OPERATION_DETAILS_AVAILABLE_FOR + Duration::from_secs(1),
            false,
        ),
        (
            "a minute past the window",
            OPERATION_DETAILS_AVAILABLE_FOR + Duration::from_secs(60),
            false,
        ),
        (
            "a day past the window",
            OPERATION_DETAILS_AVAILABLE_FOR + DAY,
            false,
        ),
        (
            "a year past the window",
            OPERATION_DETAILS_AVAILABLE_FOR + 365 * DAY,
            false,
        ),
    ]
}

/// RULE: the window the reporting operations document is 90 days, measured from the instant
/// the operation started.
#[test]
fn the_documented_window_is_ninety_days() {
    assert_eq!(
        OPERATION_DETAILS_AVAILABLE_FOR,
        Duration::from_secs(90 * 24 * 60 * 60),
        "the window both reads document — \"Details for an operation are available for 90 \
         days\" — expected 90 days, got {OPERATION_DETAILS_AVAILABLE_FOR:?}"
    );
}

/// RULE: a record is read by identifier for as long as it is inside the documented window
/// and not at all after it, when it answers as it does for an identifier no call returned.
#[tokio::test]
async fn a_record_resolves_for_as_long_as_its_details_are_available() {
    for log in WINDOWED_LOGS {
        let (router, engine) = emulator_with_engine();
        let now = SystemTime::now();
        let rows = ages();
        let start_times: Vec<SystemTime> = rows.iter().map(|(_, age, _)| now - *age).collect();
        let seeded = seed(&engine, *log, &start_times);

        for ((case, age, available), operation) in rows.iter().zip(seeded.iter()) {
            let answer = read(&router, *log, operation).await;
            let answered = (answer.status, answer.error_code.as_deref());
            let expected = if *available {
                (200, None)
            } else {
                (404, Some("ResourceNotFoundException"))
            };
            assert_eq!(
                answered, expected,
                "case {case:?} ({age:?} old) on {log:?}: expected {expected:?}, got \
                 {answered:?} body {:?}",
                answer.body
            );
        }

        // An identifier the service never minted answers exactly as the record past the
        // window does, which is what "no longer available" means on the wire.
        let never_minted = read(&router, *log, "00000000-0000-4000-8000-000000000000").await;
        let answered = (never_minted.status, never_minted.error_code.as_deref());
        assert_eq!(
            answered,
            (404, Some("ResourceNotFoundException")),
            "an identifier no call returned on {log:?}: expected 404 \
             ResourceNotFoundException, got {answered:?} body {:?}",
            never_minted.body
        );
    }
}

/// RULE: the two readers of one log agree about one record — what the by-identifier read no
/// longer resolves the listing no longer reports, and what it resolves the listing reports.
#[tokio::test]
async fn the_listing_and_the_by_identifier_read_agree_on_the_window() {
    for log in WINDOWED_LOGS {
        let (router, engine) = emulator_with_engine();
        let now = SystemTime::now();
        let rows = ages();
        let start_times: Vec<SystemTime> = rows.iter().map(|(_, age, _)| now - *age).collect();
        let seeded = seed(&engine, *log, &start_times);

        let reported = listed(&router, *log).await;
        for ((case, age, available), operation) in rows.iter().zip(seeded.iter()) {
            assert_eq!(
                reported.contains(operation),
                *available,
                "case {case:?} ({age:?} old) on {log:?}: expected the listing to report \
                 {operation}: {available}, got {reported:?}"
            );
            let answer = read(&router, *log, operation).await;
            assert_eq!(
                answer.status == 200,
                reported.contains(operation),
                "case {case:?} ({age:?} old) on {log:?}: the two readers disagree — the \
                 listing reports it: {}, the read answered {} body {:?}",
                reported.contains(operation),
                answer.status,
                answer.body
            );
        }
    }
}

/// RULE: the window is the clock's, not a reader's — reading neither drops a record still
/// inside it nor brings back one past it, whatever a caller reads and however often.
#[tokio::test]
async fn reading_neither_expires_a_record_nor_revives_one() {
    let (router, engine) = emulator_with_engine();
    let now = SystemTime::now();

    // Two records per windowed log: one inside the window, one past it.
    let inside_and_past = [
        now - Duration::from_secs(1),
        now - (OPERATION_DETAILS_AVAILABLE_FOR + DAY),
    ];
    let landing_zone = seed(&engine, Log::LandingZoneOperations, &inside_and_past);
    let control = seed(&engine, Log::ControlOperations, &inside_and_past);

    // The first read is what materialises the expiry the clock already owed, so the store
    // settles to the record inside the window per log and stays there.
    let _ = read(&router, Log::LandingZoneOperations, &landing_zone[0]).await;
    let _ = read(&router, Log::ControlOperations, &control[0]).await;
    let settled = log_sizes(&engine);
    assert_eq!(
        settled,
        (1, 1),
        "after the clock's expiry: expected one record left in each windowed log, got \
         {settled:?}"
    );

    for round in 0..4u32 {
        for (log, identifiers) in [
            (Log::LandingZoneOperations, &landing_zone),
            (Log::ControlOperations, &control),
        ] {
            let still_there = read(&router, log, &identifiers[0]).await;
            assert_eq!(
                still_there.status, 200,
                "round {round} on {log:?}: expected the record inside the window to keep \
                 resolving, got {} body {:?}",
                still_there.status, still_there.body
            );
            let still_gone = read(&router, log, &identifiers[1]).await;
            assert_eq!(
                (still_gone.status, still_gone.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "round {round} on {log:?}: expected the record past the window to stay \
                 gone, got {:?} body {:?}",
                (still_gone.status, still_gone.error_code.as_deref()),
                still_gone.body
            );
            let reported = listed(&router, log).await;
            assert_eq!(
                reported,
                vec![identifiers[0].clone()],
                "round {round} on {log:?}: expected the listing to report only the record \
                 inside the window, got {reported:?}"
            );
        }
        let sizes = log_sizes(&engine);
        assert_eq!(
            sizes, settled,
            "round {round}: reading changed the logs — expected {settled:?}, got {sizes:?}"
        );
    }
}
