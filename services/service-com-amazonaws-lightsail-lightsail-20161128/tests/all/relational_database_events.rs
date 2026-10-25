// SPDX-License-Identifier: CC0-1.0

//! `GetRelationalDatabaseEvents` — the event log of one managed relational
//! database, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetRelationalDatabaseEvents` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over a SET of databases — both engines this
//! service publishes blueprints for, several engine versions, standard and
//! high-availability bundles of different sizes, in four Regions — rather than
//! over one recorded call:
//!
//! * a database carries an event log that the SERVICE appends to as it changes
//!   that database — its engine going down and coming back up, a snapshot of it
//!   being taken, a modification or a parameter value reaching it — each entry
//!   timestamped at the instant the change happened, naming the database it
//!   relates to and carrying the categories that change falls in;
//! * `GetRelationalDatabaseEvents` REPORTS that log: the entries inside the
//!   window `durationInMinutes` opens, oldest first, a page at a time, with
//!   `nextPageToken` naming the first entry of the page after this one only
//!   while entries remain — and reading it changes nothing a later caller sees;
//! * the window is the published one: 60 minutes by default, accepted between 1
//!   and 20160 minutes, and a value outside those bounds is refused with the
//!   modelled 400 `InvalidInputException` rather than answered another window's
//!   events;
//! * a `pageToken` that names no entry of the requested window is refused the
//!   same way; and
//! * a name the account holds no database for is refused with the service's
//!   `DoesNotExist` envelope, never a hollow 200.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    self, MAX_RELATIONAL_DATABASE_EVENTS, RELATIONAL_DATABASE_EVENT_BACKUP_FINISHED,
    RELATIONAL_DATABASE_EVENT_BACKUP_STARTED, RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY,
    RELATIONAL_DATABASE_EVENT_CATEGORY_BACKUP,
    RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE,
    RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION, RELATIONAL_DATABASE_EVENT_CREATED,
    RELATIONAL_DATABASE_EVENT_MAX_DURATION_MINUTES,
    RELATIONAL_DATABASE_EVENT_MIN_DURATION_MINUTES,
    RELATIONAL_DATABASE_EVENT_MODIFICATION_APPLIED, RELATIONAL_DATABASE_EVENT_PAGE_SIZE,
    RELATIONAL_DATABASE_EVENT_PARAMETER_GROUP_CHANGED, RELATIONAL_DATABASE_EVENT_RESTARTED,
    RELATIONAL_DATABASE_EVENT_SHUTDOWN, RELATIONAL_DATABASE_EVENT_STARTED,
    RELATIONAL_DATABASE_EVENT_STOPPED,
};

use crate::instance_lifecycle::{error_code, Emulator};

/// How long to wait for a clock-owed change before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// One minute, in milliseconds — the unit `durationInMinutes` is in.
const MINUTE_MS: i64 = 60_000;

/// One database to drive, and the parameter value an update sets on it.
///
/// The rows differ along what the rules generalise over: the engine, the engine
/// version, the bundle's size and whether it is a high-availability plan, the
/// Region and Availability Zone the database lives in, and which parameter of
/// that engine's published set an update writes.
#[derive(Debug)]
struct Row {
    case: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    availability_zone: &'static str,
    /// A parameter of THIS row's engine, from the set
    /// `GetRelationalDatabaseParameters` publishes for it, and a value inside the
    /// range published beside it.
    parameter_name: &'static str,
    parameter_value: &'static str,
    /// The `applyMethod` the update names, or `None` to let the parameter's own
    /// apply type decide.
    apply_method: Option<&'static str>,
}

/// The databases every case here drives.
const ROWS: &[Row] = &[
    Row {
        case: "mysql, smallest standard bundle",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        availability_zone: "us-east-1a",
        parameter_name: "wait_timeout",
        parameter_value: "120",
        apply_method: Some("pending-reboot"),
    },
    Row {
        case: "postgres 17, large standard bundle",
        blueprint_id: "postgres_17",
        bundle_id: "large_1_0",
        availability_zone: "us-west-2b",
        parameter_name: "shared_buffers",
        parameter_value: "16384",
        apply_method: None,
    },
    Row {
        case: "postgres 13, micro high-availability bundle",
        blueprint_id: "postgres_13",
        bundle_id: "micro_ha_1_0",
        availability_zone: "eu-west-3a",
        parameter_name: "max_connections",
        parameter_value: "80",
        apply_method: Some("pending-reboot"),
    },
    Row {
        case: "mysql 8.4, medium high-availability bundle, another parameter of the set",
        blueprint_id: "mysql_8_4",
        bundle_id: "medium_ha_1_0",
        availability_zone: "ap-northeast-1c",
        parameter_name: "max_allowed_packet",
        parameter_value: "2048",
        apply_method: Some("pending-reboot"),
    },
];

/// Names the account holds no database for, differing along the length, the case,
/// the digits and the separators. Every one satisfies the modelled `ResourceName`
/// pattern `^\w[\w\-]*\w$`, so the centralized walk passes it through to the
/// handler.
const MISSING_NAMES: &[&str] = &[
    "ab",
    "never-created",
    "Probe_7",
    "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
];

/// The `CreateRelationalDatabase` body for `row`, naming one database.
fn create_body(row: &Row, name: &str) -> Value {
    json!({
        "relationalDatabaseName": name,
        "relationalDatabaseBlueprintId": row.blueprint_id,
        "relationalDatabaseBundleId": row.bundle_id,
        "availabilityZone": row.availability_zone,
        "masterDatabaseName": "appdb",
        "masterUsername": "dbadmin",
        "masterUserPassword": "Pw9#x!ZqPw9#x!Zq",
    })
}

/// The stored database `name`, as `GetRelationalDatabase` reports it.
async fn database(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok(
            "GetRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await["relationalDatabase"]
        .clone()
}

/// The stored database `name`, once the clock has settled it into `state`.
async fn wait_for_state(emulator: &Emulator, name: &str, state: &str) -> Value {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let read = database(emulator, name).await;
        if read["state"].as_str() == Some(state) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: the clock never settled the database into {state}, last read {read:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Create the database `name` for `row` and answer the emulator it lives in, once
/// the clock has settled it into `available`.
async fn with_available_database(row: &Row, name: &str) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok("CreateRelationalDatabase", create_body(row, name))
        .await;
    wait_for_state(&emulator, name, "available").await;
    emulator
}

/// One `GetRelationalDatabaseEvents` page of the database `name`, asking for
/// `duration` when one is given and continuing from `page_token` when one is.
async fn events_page(
    emulator: &Emulator,
    name: &str,
    duration: Option<i32>,
    page_token: Option<&str>,
) -> Value {
    let mut body = json!({"relationalDatabaseName": name});
    if let Some(duration) = duration {
        body["durationInMinutes"] = json!(duration);
    }
    if let Some(token) = page_token {
        body["pageToken"] = json!(token);
    }
    emulator.ok("GetRelationalDatabaseEvents", body).await
}

/// Every event of the database `name` inside the window `duration` opens, walking
/// every page.
///
/// Answers the events in page order together with the size of each page, so a
/// caller can assert both the contents and the paging.
async fn all_events(
    emulator: &Emulator,
    name: &str,
    duration: Option<i32>,
) -> (Vec<Value>, Vec<usize>) {
    let mut events = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = events_page(emulator, name, duration, token.as_deref()).await;
        let page = body["relationalDatabaseEvents"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case reading the events of {name}: expected a list, got {body:?}")
            })
            .clone();
        page_sizes.push(page.len());
        events.extend(page);
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            page_sizes.len() < 100,
            "case reading the events of {name}: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (events, page_sizes)
}

/// The `message` of each of `events`, in the order they were reported.
fn messages(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            event["message"]
                .as_str()
                .unwrap_or("<no message>")
                .to_string()
        })
        .collect()
}

/// Wait until the database `name` reports at least `count` events, answering
/// them.
///
/// The entries a CHANGE leaves in the log are written when the change happens,
/// and the ones the clock owes when the clock makes them due, so a case that
/// drives a clock-owed change waits for its entry here.
async fn wait_for_events(emulator: &Emulator, name: &str, count: usize) -> Vec<Value> {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let (events, _) = all_events(emulator, name, None).await;
        if events.len() >= count {
            return events;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: the log never reached {count} events, last read {:?}",
            messages(&events)
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Assert `event` is the modelled `RelationalDatabaseEvent` of `name` carrying
/// `message` and exactly `categories`, timestamped no earlier than `after_ms`.
fn assert_event(
    case: &str,
    event: &Value,
    name: &str,
    message: &str,
    categories: &[&str],
    after_ms: i64,
) {
    assert_eq!(
        event["message"].as_str(),
        Some(message),
        "case {case}: expected the event message {message:?}, got {event:?}"
    );
    assert_eq!(
        event["resource"].as_str(),
        Some(name),
        "case {case}: an event names the database it relates to, got {event:?}"
    );
    let reported = event["eventCategories"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an eventCategories list, got {event:?}"))
        .iter()
        .map(|category| category.as_str().unwrap_or("<not a string>").to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        reported, categories,
        "case {case}: expected the categories {categories:?}, got {reported:?}"
    );
    // awsJson1_1 renders a timestamp as epoch seconds, so the instant is a
    // number; it must be the instant the change happened, which is at or after
    // the call that started the sequence.
    let created_at = event["createdAt"].as_f64().unwrap_or_else(|| {
        panic!("case {case}: expected an epoch-seconds createdAt, got {event:?}")
    });
    let created_at_ms = (created_at * 1_000.0) as i64;
    assert!(
        created_at_ms >= after_ms,
        "case {case}: the event is timestamped {created_at_ms} ms, before the {after_ms} ms the \
         sequence started at, got {event:?}"
    );
}

/// Move every event of the database `name` `by_minutes` further into the past, so
/// a windowed read can be asserted against a log the API itself cannot age.
fn age_events(emulator: &Emulator, name: &str, by_minutes: i64) {
    let aged = state::with_state(emulator.engine(), |store| {
        let database = store
            .relational_database_mut(name)
            .unwrap_or_else(|| panic!("case ageing the events of {name}: the database is stored"));
        for event in &mut database.events {
            event.created_at_ms -= by_minutes * MINUTE_MS;
        }
        database.events.len()
    });
    assert!(
        aged.is_some_and(|count| count > 0),
        "case ageing the events of {name}: expected a log with entries to age, got {aged:?}"
    );
}

// ---------------------------------------------------------------------------
// The log itself
// ---------------------------------------------------------------------------

/// RULE: the service appends one entry to a database's event log for each change
/// it makes to that database, and `GetRelationalDatabaseEvents` reports those
/// entries oldest first — so a database driven through creation, a parameter
/// change, a modification, a backup, a restart, a stop and a start reports
/// exactly those changes, in that order, each in its own categories.
#[tokio::test]
async fn the_log_reports_every_change_the_service_made_to_the_database() {
    for row in ROWS {
        let name = "events-db";
        let snapshot_name = "events-db-snapshot";
        let started_ms = state::now_ms();
        let emulator = with_available_database(row, name).await;

        // Creation: the entry is the clock's, written when the database finished
        // being provisioned rather than when anyone read it.
        let mut expected: Vec<(String, Vec<&str>)> = vec![(
            RELATIONAL_DATABASE_EVENT_CREATED.to_string(),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION],
        )];
        let (events, _) = all_events(&emulator, name, None).await;
        assert_eq!(
            messages(&events),
            vec![RELATIONAL_DATABASE_EVENT_CREATED.to_string()],
            "case {}: a freshly provisioned database reports exactly its creation, got {:?}",
            row.case,
            messages(&events)
        );

        // A parameter value reaching the database's parameter group.
        let mut parameter = json!({
            "parameterName": row.parameter_name,
            "parameterValue": row.parameter_value,
        });
        if let Some(apply_method) = row.apply_method {
            parameter["applyMethod"] = json!(apply_method);
        }
        emulator
            .ok(
                "UpdateRelationalDatabaseParameters",
                json!({"relationalDatabaseName": name, "parameters": [parameter.clone()]}),
            )
            .await;
        expected.push((
            format!(
                "{RELATIONAL_DATABASE_EVENT_PARAMETER_GROUP_CHANGED}: {}",
                row.parameter_name
            ),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE],
        ));

        // A modification applied at once: one entry for the members that take
        // effect immediately, and one for the member the model can report
        // pending, which this call asked to apply now.
        emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "publiclyAccessible": true,
                    "disableBackupRetention": true,
                    "applyImmediately": true,
                }),
            )
            .await;
        expected.push((
            format!("{RELATIONAL_DATABASE_EVENT_MODIFICATION_APPLIED}: publiclyAccessible"),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE],
        ));
        expected.push((
            format!("{RELATIONAL_DATABASE_EVENT_MODIFICATION_APPLIED}: backupRetentionEnabled"),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE],
        ));

        // The same modification and the same parameter value again ask for what
        // the database already carries, so they change nothing and log nothing:
        // an entry says a change happened.
        emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "publiclyAccessible": true,
                    "disableBackupRetention": true,
                    "applyImmediately": true,
                }),
            )
            .await;
        emulator
            .ok(
                "UpdateRelationalDatabaseParameters",
                json!({"relationalDatabaseName": name, "parameters": [parameter.clone()]}),
            )
            .await;
        let (unchanged, _) = all_events(&emulator, name, None).await;
        assert_eq!(
            messages(&unchanged),
            expected
                .iter()
                .map(|(message, _)| message.clone())
                .collect::<Vec<_>>(),
            "case {}: a modification that changes nothing must log nothing, got {:?}",
            row.case,
            messages(&unchanged)
        );

        // A backup of the database: begun when the snapshot is taken, finished
        // when the copy can be restored from.
        emulator
            .ok(
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": name,
                    "relationalDatabaseSnapshotName": snapshot_name,
                }),
            )
            .await;
        expected.push((
            format!("{RELATIONAL_DATABASE_EVENT_BACKUP_STARTED}: {snapshot_name}"),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_BACKUP],
        ));
        expected.push((
            format!("{RELATIONAL_DATABASE_EVENT_BACKUP_FINISHED}: {snapshot_name}"),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_BACKUP],
        ));
        wait_for_events(&emulator, name, expected.len()).await;

        // A restart: the engine goes down when the call is made and comes back
        // up when the clock brings it up.
        emulator
            .ok(
                "RebootRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        expected.push((
            RELATIONAL_DATABASE_EVENT_SHUTDOWN.to_string(),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY],
        ));
        expected.push((
            RELATIONAL_DATABASE_EVENT_RESTARTED.to_string(),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY],
        ));
        wait_for_state(&emulator, name, "available").await;
        wait_for_events(&emulator, name, expected.len()).await;

        // A stop, then a start: the same two halves each.
        emulator
            .ok(
                "StopRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        expected.push((
            RELATIONAL_DATABASE_EVENT_SHUTDOWN.to_string(),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY],
        ));
        expected.push((
            RELATIONAL_DATABASE_EVENT_STOPPED.to_string(),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY],
        ));
        wait_for_state(&emulator, name, "stopped").await;
        wait_for_events(&emulator, name, expected.len()).await;

        emulator
            .ok(
                "StartRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        expected.push((
            RELATIONAL_DATABASE_EVENT_STARTED.to_string(),
            vec![RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY],
        ));
        wait_for_state(&emulator, name, "available").await;
        let (events, page_sizes) = {
            wait_for_events(&emulator, name, expected.len()).await;
            all_events(&emulator, name, None).await
        };

        // Every change, in the order it happened, and nothing else.
        assert_eq!(
            messages(&events),
            expected
                .iter()
                .map(|(message, _)| message.clone())
                .collect::<Vec<_>>(),
            "case {}: expected the log to report exactly the changes the service made, got {:?}",
            row.case,
            messages(&events)
        );
        for (event, (message, categories)) in events.iter().zip(&expected) {
            assert_event(row.case, event, name, message, categories, started_ms);
        }
        // Oldest first: the instants never go backwards.
        let instants = events
            .iter()
            .map(|event| event["createdAt"].as_f64().unwrap_or_default())
            .collect::<Vec<_>>();
        assert!(
            instants.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {}: the log is reported oldest first, got {instants:?}",
            row.case
        );
        // The log is longer than one page, so the walk above really paged: every
        // page but the last carries a full page of entries.
        assert!(
            events.len() > RELATIONAL_DATABASE_EVENT_PAGE_SIZE,
            "case {}: this sequence must log more than one page of events, got {}",
            row.case,
            events.len()
        );
        let (full, last) = page_sizes.split_at(page_sizes.len().saturating_sub(1));
        assert!(
            full.iter()
                .all(|size| *size == RELATIONAL_DATABASE_EVENT_PAGE_SIZE)
                && last
                    .first()
                    .is_some_and(|size| *size <= RELATIONAL_DATABASE_EVENT_PAGE_SIZE),
            "case {}: expected pages of at most {RELATIONAL_DATABASE_EVENT_PAGE_SIZE} entries, \
             got {page_sizes:?}",
            row.case
        );
        assert!(
            events.len() <= MAX_RELATIONAL_DATABASE_EVENTS,
            "case {}: the log is bounded at {MAX_RELATIONAL_DATABASE_EVENTS} entries, got {}",
            row.case,
            events.len()
        );

        // Reading the log changes nothing a later caller sees: the same read
        // answers the same log, and the database is where it was.
        let before = database(&emulator, name).await;
        let (again, _) = all_events(&emulator, name, None).await;
        assert_eq!(
            again, events,
            "case {}: a second read must report the same log, got {:?}",
            row.case,
            messages(&again)
        );
        assert_eq!(
            database(&emulator, name).await,
            before,
            "case {}: reading the events must not change the database",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// One window case: how far in the past the database's events are, the
/// `durationInMinutes` the request asks for, and whether the window reaches them.
#[derive(Debug)]
struct WindowCase {
    case: &'static str,
    age_minutes: i64,
    /// `None` leaves the member out, which asks for the published default of 60
    /// minutes.
    duration: Option<i32>,
    reported: bool,
}

/// The window cases: the default, a window named either side of an event, and
/// each published bound with an event inside it and one past it.
const WINDOW_CASES: &[WindowCase] = &[
    WindowCase {
        case: "no duration named, an event inside the default window",
        age_minutes: 5,
        duration: None,
        reported: true,
    },
    WindowCase {
        case: "no duration named, an event older than the default window",
        age_minutes: 90,
        duration: None,
        reported: false,
    },
    WindowCase {
        case: "a window long enough to reach the event",
        age_minutes: 90,
        duration: Some(120),
        reported: true,
    },
    WindowCase {
        case: "the shortest window the service accepts, an event inside it",
        age_minutes: 0,
        duration: Some(RELATIONAL_DATABASE_EVENT_MIN_DURATION_MINUTES),
        reported: true,
    },
    WindowCase {
        case: "the shortest window the service accepts, an event past it",
        age_minutes: 5,
        duration: Some(RELATIONAL_DATABASE_EVENT_MIN_DURATION_MINUTES),
        reported: false,
    },
    WindowCase {
        case: "the longest window the service accepts, an event inside it",
        age_minutes: 20_000,
        duration: Some(RELATIONAL_DATABASE_EVENT_MAX_DURATION_MINUTES),
        reported: true,
    },
    WindowCase {
        case: "the longest window the service accepts, an event past it",
        age_minutes: 20_300,
        duration: Some(RELATIONAL_DATABASE_EVENT_MAX_DURATION_MINUTES),
        reported: false,
    },
];

/// RULE: `durationInMinutes` is the number of minutes in the past from which to
/// retrieve events, defaulting to 60 — so an entry older than the window the
/// request opens is not reported, and one inside it is.
#[tokio::test]
async fn the_window_reports_the_events_inside_it_and_no_others() {
    let row = &ROWS[0];
    for window in WINDOW_CASES {
        let name = "windowed-db";
        let emulator = with_available_database(row, name).await;
        let (before, _) = all_events(&emulator, name, None).await;
        assert_eq!(
            before.len(),
            1,
            "case {}: the fixture is one creation event, got {:?}",
            window.case,
            messages(&before)
        );

        age_events(&emulator, name, window.age_minutes);
        let (reported, _) = all_events(&emulator, name, window.duration).await;
        let expected = usize::from(window.reported);
        assert_eq!(
            reported.len(),
            expected,
            "case {}: an event {} minutes old and a window of {:?} minutes must report {expected} \
             event(s), got {:?}",
            window.case,
            window.age_minutes,
            window.duration,
            messages(&reported)
        );
        if window.reported {
            assert_eq!(
                messages(&reported),
                messages(&before),
                "case {}: the window reports the entry the log holds, got {:?}",
                window.case,
                messages(&reported)
            );
        }
    }
}

/// `durationInMinutes` values outside the published bounds: each bound's
/// neighbour, zero, a negative value and the extremes of the modelled integer.
const REFUSED_DURATIONS: &[i32] = &[
    RELATIONAL_DATABASE_EVENT_MIN_DURATION_MINUTES - 1,
    RELATIONAL_DATABASE_EVENT_MAX_DURATION_MINUTES + 1,
    -1,
    -20_160,
    i32::MIN,
    i32::MAX,
];

/// RULE: a `durationInMinutes` outside the window the service serves — the
/// published minimum of 1 and maximum of 20160 minutes — is refused with the
/// modelled 400 `InvalidInputException` naming the member, rather than answered
/// another window's events.
#[tokio::test]
async fn a_duration_outside_the_published_bounds_is_refused() {
    let row = &ROWS[1];
    let name = "bounded-db";
    let emulator = with_available_database(row, name).await;

    for duration in REFUSED_DURATIONS {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseEvents",
                json!({"relationalDatabaseName": name, "durationInMinutes": duration}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case durationInMinutes {duration}: expected a 400 InvalidInputException, got \
             {status} {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("durationInMinutes")),
            "case durationInMinutes {duration}: the refusal names the member, got {body:?}"
        );
    }

    // Both published bounds are ACCEPTED, so the refusal is of what is outside
    // them rather than of the bounds themselves.
    for duration in [
        RELATIONAL_DATABASE_EVENT_MIN_DURATION_MINUTES,
        RELATIONAL_DATABASE_EVENT_MAX_DURATION_MINUTES,
    ] {
        let body = events_page(&emulator, name, Some(duration), None).await;
        assert!(
            body["relationalDatabaseEvents"].is_array(),
            "case durationInMinutes {duration}: a published bound must be accepted, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging and refusals
// ---------------------------------------------------------------------------

/// Tokens no page of a log ever reported, differing along the form: a token
/// shaped like a position no entry holds, a name, an empty-ish token and a very
/// long one.
const BOGUS_PAGE_TOKENS: &[&str] = &["9999", "-1", "not-a-token", "0000000000000000000000000000"];

/// RULE: a `pageToken` that names no entry of the requested window is refused
/// with the modelled 400 `InvalidInputException`, because there is no page to
/// answer — while the token a page reported continues the walk from that entry.
#[tokio::test]
async fn a_page_token_naming_no_event_of_the_window_is_refused() {
    let row = &ROWS[2];
    let name = "paged-db";
    let emulator = with_available_database(row, name).await;
    // Enough entries for more than one page: each restart logs two.
    let mut logged = 1;
    for _ in 0..RELATIONAL_DATABASE_EVENT_PAGE_SIZE {
        emulator
            .ok(
                "RebootRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        logged += 2;
        wait_for_state(&emulator, name, "available").await;
        wait_for_events(&emulator, name, logged).await;
    }

    let first = events_page(&emulator, name, None, None).await;
    let token = first["nextPageToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case paging {name}: expected a next page token, got {first:?}"))
        .to_string();
    let second = events_page(&emulator, name, None, Some(&token)).await;
    let first_page = first["relationalDatabaseEvents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let second_page = second["relationalDatabaseEvents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        first_page.len(),
        RELATIONAL_DATABASE_EVENT_PAGE_SIZE,
        "case paging {name}: a page that reports a token is full, got {:?}",
        messages(&first_page)
    );
    assert!(
        !second_page.is_empty()
            && second_page
                .iter()
                .all(|event| !first_page.contains(event)),
        "case paging {name}: the next page continues after the first, got {:?} then {:?}",
        messages(&first_page),
        messages(&second_page)
    );

    for token in BOGUS_PAGE_TOKENS {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseEvents",
                json!({"relationalDatabaseName": name, "pageToken": token}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case pageToken {token:?}: expected a 400 InvalidInputException, got {status} {body:?}"
        );
    }

    // A token naming a real entry the requested window no longer reaches is
    // refused the same way: the page it would continue is not in the window
    // asked for.
    age_events(&emulator, name, 120);
    let (status, body) = emulator
        .call(
            "GetRelationalDatabaseEvents",
            json!({"relationalDatabaseName": name, "pageToken": token}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "case a token outside the window: expected a 400 InvalidInputException, got {status} \
         {body:?}"
    );
}

/// RULE: a name the account holds no managed relational database for is refused
/// with the service's `DoesNotExist` envelope — HTTP 400, the modelled
/// `NotFoundException` and the `DoesNotExist` code — never a hollow 200 with an
/// empty log.
#[tokio::test]
async fn events_of_a_database_the_account_does_not_hold_are_refused() {
    let emulator = Emulator::new();
    for name in MISSING_NAMES {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseEvents",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), body["code"].as_str()),
            (400, "NotFoundException", Some("DoesNotExist")),
            "case {name}: expected the DoesNotExist envelope, got {status} {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(name)),
            "case {name}: the refusal names the database asked for, got {body:?}"
        );
    }
}
