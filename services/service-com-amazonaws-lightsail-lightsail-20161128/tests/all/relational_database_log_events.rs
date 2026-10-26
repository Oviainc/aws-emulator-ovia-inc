// SPDX-License-Identifier: CC0-1.0

//! `GetRelationalDatabaseLogEvents` — one page of a managed relational
//! database's log stream — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetRelationalDatabaseLogEvents` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over a SET of databases — both engines this
//! service publishes blueprints for, several engine versions, standard and
//! high-availability bundles, in four Regions — rather than over one call:
//!
//! * a log stream is a log FILE the database's engine writes, so the lines it
//!   reports are what that engine did: its error log carries the engine coming
//!   up and going down, each line timestamped at the instant it happened, and a
//!   QUERY log carries a line per statement a client ran, of which this emulator
//!   has served none;
//! * the request's time interval SELECTS among those lines and is half-open — a
//!   line written at `startTime` is reported and one written at `endTime` is
//!   not;
//! * `startFromHead` chooses which END of the log the first page is taken from,
//!   the lines of a page are always reported in the order they were written, and
//!   `nextForwardToken` / `nextBackwardToken` walk the rest of the log from
//!   there without reporting a line twice or skipping one;
//! * a PostgreSQL database serves only the tail of its log, so asking it for the
//!   head is refused with the modelled 400 `InvalidInputException`, as is an
//!   interval whose end does not follow its start and a page token this
//!   operation never issued; and
//! * a name the account holds no database for, and a stream name the database
//!   does not publish, are refused with the service's `DoesNotExist` envelope,
//!   never a hollow 200.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    self, RELATIONAL_DATABASE_ERROR_LOG_STREAM_PREFIX, RELATIONAL_DATABASE_EVENT_CREATED,
    RELATIONAL_DATABASE_EVENT_RESTARTED, RELATIONAL_DATABASE_EVENT_SHUTDOWN,
    RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE,
};

use crate::instance_lifecycle::{error_code, Emulator};

/// How long to wait for a clock-owed change before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// A master user password every engine's limits admit (8..41 characters for
/// MySQL, 8..128 for PostgreSQL), drawn only from characters the modelled
/// `masterUserPassword` documentation allows.
const PASSWORD: &str = "Pw9#x!ZqPw9#x!Zq";

/// One database to drive.
///
/// The rows differ along what the rules generalise over: the engine, the engine
/// version, the bundle's size and whether it is a high-availability plan, and
/// the Region and Availability Zone the database lives in.
#[derive(Debug)]
struct Row {
    case: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    availability_zone: &'static str,
    /// The engine this row's blueprint installs, as the published catalog's own
    /// `RelationalDatabaseBlueprint$engine` member reports it — checked against
    /// the catalog by [`assert_catalog_engine`], so a row cannot drift from it.
    engine: &'static str,
}

/// The databases every case here drives.
const ROWS: &[Row] = &[
    Row {
        case: "mysql 8.4, smallest standard bundle",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        availability_zone: "us-east-1a",
        engine: "mysql",
    },
    Row {
        case: "postgres 17, large standard bundle",
        blueprint_id: "postgres_17",
        bundle_id: "large_1_0",
        availability_zone: "us-west-2b",
        engine: "postgres",
    },
    Row {
        case: "postgres 13, micro high-availability bundle",
        blueprint_id: "postgres_13",
        bundle_id: "micro_ha_1_0",
        availability_zone: "eu-west-3a",
        engine: "postgres",
    },
    Row {
        case: "mysql 8.4, medium high-availability bundle",
        blueprint_id: "mysql_8_4",
        bundle_id: "medium_ha_1_0",
        availability_zone: "ap-northeast-1c",
        engine: "mysql",
    },
];

/// The log streams a database of `engine` publishes, keyed on the catalog's own
/// `RelationalDatabaseBlueprint$engine` value.
///
/// A Lightsail managed database is an Amazon RDS DB instance and a log stream is
/// one of the log files its engine writes, so this is the engine's own log-file
/// set from the Amazon RDS User Guide's database-log-file pages: MySQL writes an
/// error log (`error/mysql-error.log`, with its hourly-rotated running copy
/// `error/mysql-error-running.log`), a general query log
/// (`general/mysql-general.log`) and a slow query log
/// (`slowquery/mysql-slowquery.log`); PostgreSQL writes one log,
/// `error/postgresql.log`.
fn want_streams(engine: &str) -> Vec<&'static str> {
    match engine {
        "postgres" => vec!["error/postgresql.log"],
        "mysql" => vec![
            "error/mysql-error-running.log",
            "error/mysql-error.log",
            "general/mysql-general.log",
            "slowquery/mysql-slowquery.log",
        ],
        other => panic!(
            "case {other}: this suite's table does not know the log files of the engine {other:?}"
        ),
    }
}

/// The `CreateRelationalDatabase` body for `row`, naming one database.
fn create_body(row: &Row, name: &str) -> Value {
    json!({
        "relationalDatabaseName": name,
        "relationalDatabaseBlueprintId": row.blueprint_id,
        "relationalDatabaseBundleId": row.bundle_id,
        "availabilityZone": row.availability_zone,
        "masterDatabaseName": "appdb",
        "masterUsername": "dbadmin",
        "masterUserPassword": PASSWORD,
    })
}

/// The state `GetRelationalDatabase` reports for `name`.
async fn read_state(emulator: &Emulator, name: &str) -> String {
    let body = emulator
        .ok(
            "GetRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    body["relationalDatabase"]["state"]
        .as_str()
        .unwrap_or_else(|| panic!("case reading {name}: expected a state, got {body:?}"))
        .to_string()
}

/// Wait until the clock has settled the database `name` into `want`.
async fn wait_for_state(emulator: &Emulator, name: &str, want: &str) {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let state = read_state(emulator, name).await;
        if state == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for {want} on {name}: the clock never settled the database, last read \
             {state}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Create the database `name` for `row` and answer the emulator it lives in,
/// once the clock has settled it into `available` — the state in which its
/// engine has run and written its log.
async fn create_available(row: &Row, name: &str) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok("CreateRelationalDatabase", create_body(row, name))
        .await;
    wait_for_state(&emulator, name, "available").await;
    emulator
}

/// Restart the database `name` and wait until it is serving again: the engine
/// goes down and comes back up, which is two more lines in its error log.
async fn reboot(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "RebootRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    wait_for_state(emulator, name, "available").await;
}

/// The streams `GetRelationalDatabaseLogStreams` says `name` publishes — the
/// names a caller hands to this operation.
async fn read_streams(emulator: &Emulator, name: &str) -> Vec<String> {
    let body = emulator
        .ok(
            "GetRelationalDatabaseLogStreams",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    body["logStreams"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the streams of {name}: expected a list, got {body:?}"))
        .iter()
        .map(|stream| {
            stream
                .as_str()
                .unwrap_or_else(|| panic!("case reading the streams of {name}: expected string streams, got {body:?}"))
                .to_string()
        })
        .collect()
}

/// The `GetRelationalDatabaseLogEvents` request for the stream `stream` of
/// `name`, carrying whatever `request` adds to the two required members.
fn events_body(name: &str, stream: &str, request: &[(&str, Value)]) -> Value {
    let mut body = json!({"relationalDatabaseName": name, "logStreamName": stream});
    for (member, value) in request {
        body[*member] = value.clone();
    }
    body
}

/// One page of the stream `stream` of `name`, requiring success.
async fn events_page(
    emulator: &Emulator,
    name: &str,
    stream: &str,
    request: &[(&str, Value)],
) -> Value {
    emulator
        .ok(
            "GetRelationalDatabaseLogEvents",
            events_body(name, stream, request),
        )
        .await
}

/// The same read, answering the status and body whatever they are.
async fn events_call(
    emulator: &Emulator,
    name: &str,
    stream: &str,
    request: &[(&str, Value)],
) -> (u16, Value) {
    emulator
        .call(
            "GetRelationalDatabaseLogEvents",
            events_body(name, stream, request),
        )
        .await
}

/// The `message` of each line of `body`, in the order they were reported.
fn messages(case: &str, body: &Value) -> Vec<String> {
    lines(case, body)
        .iter()
        .map(|line| {
            line["message"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a message on every line, got {line:?}")
                })
                .to_string()
        })
        .collect()
}

/// The instant each line of `body` was written, in epoch milliseconds.
///
/// awsJson1_1 renders a timestamp as epoch seconds, so the member is a number.
fn instants_ms(case: &str, body: &Value) -> Vec<i64> {
    lines(case, body)
        .iter()
        .map(|line| {
            let seconds = line["createdAt"].as_f64().unwrap_or_else(|| {
                panic!("case {case}: expected an epoch-seconds createdAt, got {line:?}")
            });
            (seconds * 1_000.0).round() as i64
        })
        .collect()
}

/// The `resourceLogEvents` list of `body` — always present, empty or not.
fn lines(case: &str, body: &Value) -> Vec<Value> {
    body["resourceLogEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a resourceLogEvents list, got {body:?}"))
        .clone()
}

/// The token `member` of `body`.
fn token(case: &str, body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a {member}, got {body:?}"))
        .to_string()
}

/// Assert the row's `engine` is the one the published catalog says its blueprint
/// installs, so every expectation keyed on the engine is keyed on the service's
/// own catalog rather than on how a blueprint id is spelled.
async fn assert_catalog_engine(emulator: &Emulator, row: &Row) {
    let body = emulator
        .ok("GetRelationalDatabaseBlueprints", json!({}))
        .await;
    let published = body["blueprints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {}: expected a blueprint catalog, got {body:?}", row.case))
        .iter()
        .find(|blueprint| blueprint["blueprintId"].as_str() == Some(row.blueprint_id))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {}: expected the catalog to publish {}, got {body:?}",
                row.case, row.blueprint_id
            )
        });
    assert_eq!(
        published["engine"].as_str(),
        Some(row.engine),
        "case {}: expected the catalog's engine for {}, got {published:?}",
        row.case,
        row.blueprint_id
    );
}

/// Place the lines of `name`'s error log at `count` instants two seconds apart,
/// answering the first of them in epoch milliseconds.
///
/// The rule under test here is about the interval's BOUNDS, and no request can
/// write a log line at an instant of the caller's choosing, so the fixture is
/// written into the store the handler reads — the same seam
/// `relational_database_events.rs` ages a log through. Whole seconds, so a bound
/// this suite sends round-trips through the wire's epoch-seconds rendering
/// exactly.
fn place_lines(emulator: &Emulator, name: &str, count: usize) -> i64 {
    let base_ms = (state::now_ms() / 1_000) * 1_000 - 60_000;
    let placed = state::with_state(emulator.engine(), |store| {
        let database = store
            .relational_database_mut(name)
            .unwrap_or_else(|| panic!("case placing the lines of {name}: the database is stored"));
        for (index, event) in database.events.iter_mut().enumerate() {
            event.created_at_ms = base_ms + 2_000 * i64::try_from(index).unwrap_or(0);
        }
        database.events.len()
    });
    assert_eq!(
        placed,
        Some(count),
        "case placing the lines of {name}: expected exactly {count} entries to place"
    );
    base_ms
}

// ---------------------------------------------------------------------------
// What a stream reports
// ---------------------------------------------------------------------------

/// RULE: a log stream is a log FILE the database's engine writes, so its lines
/// are what that engine did — the error log carries the engine coming up and
/// going down, at the instants those happened, and a query log carries a line
/// per statement a client ran, of which none has reached an engine here.
#[tokio::test]
async fn a_streams_lines_are_what_its_engine_wrote() {
    for row in ROWS {
        let name = format!("logevents-{}-{}", row.blueprint_id, row.bundle_id);
        let started_ms = state::now_ms();
        let emulator = create_available(row, &name).await;
        assert_catalog_engine(&emulator, row).await;

        let streams = read_streams(&emulator, &name).await;
        assert_eq!(
            streams,
            want_streams(row.engine),
            "case {}: expected the {} log streams, got {streams:?}",
            row.case,
            row.engine
        );

        // A database that has only been created has written one line: its engine
        // reached the point of serving connections.
        for stream in &streams {
            let case = format!("{} / {stream} after the create", row.case);
            let body = events_page(&emulator, &name, stream, &[]).await;
            let want: Vec<String> = if stream.starts_with(RELATIONAL_DATABASE_ERROR_LOG_STREAM_PREFIX)
            {
                vec![RELATIONAL_DATABASE_EVENT_CREATED.to_string()]
            } else {
                Vec::new()
            };
            let got = messages(&case, &body);
            assert_eq!(
                got, want,
                "case {case}: expected the lines {want:?}, got {got:?}"
            );
            for instant_ms in instants_ms(&case, &body) {
                assert!(
                    instant_ms >= started_ms,
                    "case {case}: a line is timestamped when it was written, so not before the \
                     {started_ms} ms this case started at, got {instant_ms}"
                );
            }
            // Both tokens are reported whether the page carried a line or not.
            for member in ["nextForwardToken", "nextBackwardToken"] {
                let reported = token(&case, &body, member);
                assert!(
                    !reported.is_empty(),
                    "case {case}: expected a non-empty {member}, got {reported:?}"
                );
            }
        }

        // The restart is two more lines in the error log, each at its own
        // instant — and nothing at all in a query log.
        reboot(&emulator, &name).await;
        for stream in &streams {
            let case = format!("{} / {stream} after the restart", row.case);
            let body = events_page(&emulator, &name, stream, &[]).await;
            let want: Vec<String> = if stream.starts_with(RELATIONAL_DATABASE_ERROR_LOG_STREAM_PREFIX)
            {
                vec![
                    RELATIONAL_DATABASE_EVENT_CREATED.to_string(),
                    RELATIONAL_DATABASE_EVENT_SHUTDOWN.to_string(),
                    RELATIONAL_DATABASE_EVENT_RESTARTED.to_string(),
                ]
            } else {
                Vec::new()
            };
            let got = messages(&case, &body);
            assert_eq!(
                got, want,
                "case {case}: expected the lines {want:?}, got {got:?}"
            );
            let instants = instants_ms(&case, &body);
            assert!(
                instants.windows(2).all(|pair| pair[0] <= pair[1]),
                "case {case}: a page reports its lines in the order they were written, got \
                 {instants:?}"
            );

            // A read changes nothing a later reader sees.
            let again = events_page(&emulator, &name, stream, &[]).await;
            assert_eq!(
                messages(&case, &again),
                got,
                "case {case}: two reads of one stream must answer the same lines, got {again:?}"
            );
            assert_eq!(
                token(&case, &again, "nextForwardToken"),
                token(&case, &body, "nextForwardToken"),
                "case {case}: two reads of one stream must answer the same forward token, got \
                 {again:?}"
            );
        }
        let state = read_state(&emulator, &name).await;
        assert_eq!(
            state, "available",
            "case {}: reading a log stream must not move the database's state, got {state}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// The interval
// ---------------------------------------------------------------------------

/// RULE: the request's time interval selects among the lines the stream holds
/// and is half-open — a line written at `startTime` is reported and one written
/// at `endTime` is not — so abutting intervals report every line exactly once.
#[tokio::test]
async fn the_interval_selects_the_lines_reported() {
    let row = &ROWS[0];
    let name = "logevents-window";
    let emulator = create_available(row, name).await;
    reboot(&emulator, name).await;
    let stream = "error/mysql-error.log";

    // Three lines, two seconds apart, at instants this case knows.
    let base_ms = place_lines(&emulator, name, 3);
    let all = [
        RELATIONAL_DATABASE_EVENT_CREATED,
        RELATIONAL_DATABASE_EVENT_SHUTDOWN,
        RELATIONAL_DATABASE_EVENT_RESTARTED,
    ];
    // The lines the case expects, by their place in the log.
    let want_lines = |places: &[usize]| -> Vec<String> {
        places
            .iter()
            .map(|place| all[*place].to_string())
            .collect()
    };

    /// One interval to ask for: the bounds in milliseconds from the first
    /// line's instant (the lines are at 0, 2000 and 4000), and the lines the
    /// interval holds, by their place in the log.
    struct Interval {
        case: &'static str,
        start_offset_ms: Option<i64>,
        end_offset_ms: Option<i64>,
        want: &'static [usize],
    }

    let cases = [
        Interval {
            case: "both bounds open",
            start_offset_ms: None,
            end_offset_ms: None,
            want: &[0, 1, 2],
        },
        Interval {
            case: "start at the first line's instant",
            start_offset_ms: Some(0),
            end_offset_ms: None,
            want: &[0, 1, 2],
        },
        Interval {
            case: "start between the first and second line",
            start_offset_ms: Some(1_000),
            end_offset_ms: None,
            want: &[1, 2],
        },
        Interval {
            case: "start at the second line's instant",
            start_offset_ms: Some(2_000),
            end_offset_ms: None,
            want: &[1, 2],
        },
        Interval {
            case: "end at the last line's instant",
            start_offset_ms: None,
            end_offset_ms: Some(4_000),
            want: &[0, 1],
        },
        Interval {
            case: "end at the first line's instant",
            start_offset_ms: None,
            end_offset_ms: Some(0),
            want: &[],
        },
        Interval {
            case: "end after the last line",
            start_offset_ms: None,
            end_offset_ms: Some(5_000),
            want: &[0, 1, 2],
        },
        Interval {
            case: "a window holding the middle line alone",
            start_offset_ms: Some(2_000),
            end_offset_ms: Some(4_000),
            want: &[1],
        },
        Interval {
            case: "a window after every line",
            start_offset_ms: Some(6_000),
            end_offset_ms: None,
            want: &[],
        },
        Interval {
            case: "a window before every line",
            start_offset_ms: Some(-10_000),
            end_offset_ms: Some(0),
            want: &[],
        },
    ];

    for interval in &cases {
        let case = interval.case;
        let mut request: Vec<(&str, Value)> = Vec::new();
        if let Some(offset_ms) = interval.start_offset_ms {
            request.push(("startTime", json!((base_ms + offset_ms) / 1_000)));
        }
        if let Some(offset_ms) = interval.end_offset_ms {
            request.push(("endTime", json!((base_ms + offset_ms) / 1_000)));
        }
        let body = events_page(&emulator, name, stream, &request).await;
        let got = messages(case, &body);
        let want = want_lines(interval.want);
        assert_eq!(
            got, want,
            "case {case}: expected the lines {want:?} inside the interval, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ends of the log, and the tokens between them
// ---------------------------------------------------------------------------

/// RULE: `startFromHead` chooses which END of the log the first page comes from,
/// a page's lines are always in the order they were written, and the two tokens
/// walk the rest of the log from there — reporting every line exactly once, and
/// answering no line, with the token unchanged, once the end is reached.
#[tokio::test]
async fn the_tokens_walk_the_whole_log_from_either_end() {
    let row = &ROWS[0];
    let name = "logevents-paging";
    let emulator = create_available(row, name).await;
    let stream = "error/mysql-error.log";

    // Enough lines for more than one page: the create wrote one, and each
    // restart writes two.
    let mut want: Vec<String> = vec![RELATIONAL_DATABASE_EVENT_CREATED.to_string()];
    while want.len() <= RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE {
        reboot(&emulator, name).await;
        want.push(RELATIONAL_DATABASE_EVENT_SHUTDOWN.to_string());
        want.push(RELATIONAL_DATABASE_EVENT_RESTARTED.to_string());
    }

    // The head: the oldest lines the stream holds.
    let head = events_page(&emulator, name, stream, &[("startFromHead", json!(true))]).await;
    let head_lines = messages("the head of the log", &head);
    assert_eq!(
        head_lines,
        want[..RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE],
        "case the head of the log: expected the oldest {RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE} \
         lines, got {head_lines:?}"
    );

    // The tail, which is the default: the newest lines.
    let tail = events_page(&emulator, name, stream, &[]).await;
    let tail_lines = messages("the tail of the log", &tail);
    assert_eq!(
        tail_lines,
        want[want.len() - RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE..],
        "case the tail of the log: expected the newest {RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE} \
         lines, got {tail_lines:?}"
    );
    assert_ne!(
        head_lines, tail_lines,
        "case the two ends: a log longer than one page has different ends, got {head_lines:?}"
    );
    assert_eq!(
        messages(
            "the tail of the log named by startFromHead false",
            &events_page(&emulator, name, stream, &[("startFromHead", json!(false))]).await
        ),
        tail_lines,
        "case startFromHead false: expected the same tail the default answers, got {tail_lines:?}"
    );

    // Forward from the head, page by page, to the end of the log.
    let mut walked = head_lines.clone();
    let mut forward = token("the head of the log", &head, "nextForwardToken");
    let mut pages = 1;
    loop {
        let case = format!("forward page {pages} of the log");
        let body = events_page(&emulator, name, stream, &[("pageToken", json!(forward))]).await;
        let page = messages(&case, &body);
        let next = token(&case, &body, "nextForwardToken");
        if page.is_empty() {
            assert_eq!(
                next, forward,
                "case {case}: a page at the end of the stream reports the same forward token it \
                 was asked with, got {next}"
            );
            break;
        }
        walked.extend(page);
        forward = next;
        pages += 1;
        assert!(
            pages < 100,
            "case walking the log forward: the walk did not terminate after {pages} pages"
        );
    }
    assert_eq!(
        walked, want,
        "case walking the log forward from its head: expected every line exactly once, got \
         {walked:?}"
    );

    // Backward from the tail: the page that ends where the tail page begins.
    let backward = token("the tail of the log", &tail, "nextBackwardToken");
    let before = events_page(&emulator, name, stream, &[("pageToken", json!(backward))]).await;
    let before_lines = messages("the page before the tail", &before);
    let mut rejoined = before_lines.clone();
    rejoined.extend(tail_lines.clone());
    assert_eq!(
        rejoined, want,
        "case walking the log backward from its tail: expected every line exactly once, got \
         {rejoined:?}"
    );

    // A position past everything the stream holds is a page with no line, not a
    // refusal: the caller is at the end of the log.
    let past = events_page(
        &emulator,
        name,
        stream,
        &[("pageToken", json!("f/100000"))],
    )
    .await;
    let past_lines = messages("a position past the end of the log", &past);
    assert!(
        past_lines.is_empty(),
        "case a position past the end of the log: expected no line, got {past_lines:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a PostgreSQL database serves only the tail of its log, so asking it for
/// the head is refused with the modelled 400 `InvalidInputException` — while a
/// MySQL database serves both ends.
#[tokio::test]
async fn postgres_serves_only_the_tail_of_its_log() {
    for row in ROWS {
        let name = format!("logevents-head-{}-{}", row.blueprint_id, row.bundle_id);
        let emulator = create_available(row, &name).await;
        let stream = want_streams(row.engine)[0];
        let (status, body) = events_call(
            &emulator,
            &name,
            stream,
            &[("startFromHead", json!(true))],
        )
        .await;
        if row.engine == "postgres" {
            assert_eq!(
                status, 400,
                "case {}: expected the head of a PostgreSQL log to be refused, got {status} \
                 {body:?}",
                row.case
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {}: expected InvalidInputException, got {body:?}",
                row.case
            );
            let message = body["message"].as_str().unwrap_or("<no message>");
            assert!(
                message.contains("startFromHead"),
                "case {}: expected the refusal to name the member, got {message:?}",
                row.case
            );
        } else {
            assert_eq!(
                status, 200,
                "case {}: expected a {} database to serve the head of its log, got {status} \
                 {body:?}",
                row.case, row.engine
            );
        }

        // Either engine serves the tail, which is the documented default.
        for request in [vec![], vec![("startFromHead", json!(false))]] {
            let (status, body) = events_call(&emulator, &name, stream, &request).await;
            assert_eq!(
                status, 200,
                "case {}: expected the tail of the log to be served for {request:?}, got {status} \
                 {body:?}",
                row.case
            );
        }
    }
}

/// RULE: a name the account holds no database for, and a stream name the
/// database does not publish, are refused with the service's `DoesNotExist`
/// envelope — HTTP 400, `NotFoundException`, body code `DoesNotExist` — never a
/// 501 and never an empty list of lines.
#[tokio::test]
async fn a_log_nothing_publishes_is_refused() {
    let row = &ROWS[0];
    let name = "logevents-present";
    let emulator = create_available(row, name).await;

    // Names that name nothing, each satisfying the modelled `ResourceName`
    // pattern so the centralized walk passes it to the handler.
    for missing in [
        "ab",
        "never-created",
        "Logevents-Present",
        "logevents_present",
        "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
    ] {
        let (status, body) =
            events_call(&emulator, missing, "error/mysql-error.log", &[]).await;
        assert_eq!(
            status, 400,
            "case reading the log of {missing}: expected the DoesNotExist refusal, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case reading the log of {missing}: expected NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case reading the log of {missing}: expected the DoesNotExist code, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The RelationalDatabase does not exist: {missing}").as_str()),
            "case reading the log of {missing}: expected the resource-type/name message, got \
             {body:?}"
        );
    }

    // Stream names this database publishes none of: another engine's log file, a
    // near-miss spelling, an absolute path, an empty name and one differing only
    // in case.
    for stream in [
        "error/postgresql.log",
        "error/mysql-error.log.0",
        "/error/mysql-error.log",
        "",
        "ERROR/MYSQL-ERROR.LOG",
        "general/mysql-general.log.1",
    ] {
        let (status, body) = events_call(&emulator, name, stream, &[]).await;
        assert_eq!(
            status, 400,
            "case reading the stream {stream:?}: expected the DoesNotExist refusal, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case reading the stream {stream:?}: expected NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case reading the stream {stream:?}: expected the DoesNotExist code, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or("<no message>");
        assert!(
            message.contains(stream) && message.contains(name),
            "case reading the stream {stream:?}: expected the refusal to name the stream and the \
             database, got {message:?}"
        );
    }
}

/// RULE: a database still being created has started no engine and written no
/// log, so every stream name is refused for it — the clock, not a reader, is
/// what gives it a log to report.
#[tokio::test]
async fn a_database_still_being_created_has_no_log_to_report() {
    for row in ROWS {
        let name = format!("logevents-creating-{}-{}", row.blueprint_id, row.bundle_id);
        let emulator = Emulator::new();
        emulator
            .ok("CreateRelationalDatabase", create_body(row, &name))
            .await;
        let state = read_state(&emulator, &name).await;
        assert_eq!(
            state, "creating",
            "case {}: expected the database to be created `creating`, got {state}",
            row.case
        );

        let stream = want_streams(row.engine)[0];
        let (status, body) = events_call(&emulator, &name, stream, &[]).await;
        assert_eq!(
            status, 400,
            "case {}: expected the stream {stream} to be refused while the database is being \
             created, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {}: expected the DoesNotExist code, got {body:?}",
            row.case
        );

        // Once the engine has run, the same request is served.
        wait_for_state(&emulator, &name, "available").await;
        let body = events_page(&emulator, &name, stream, &[]).await;
        let reported = messages(row.case, &body);
        assert_eq!(
            reported,
            vec![RELATIONAL_DATABASE_EVENT_CREATED.to_string()],
            "case {}: expected the engine's first line once the database is available, got \
             {reported:?}",
            row.case
        );
    }
}

/// RULE: a request the model admits but the service does not is refused with the
/// modelled 400 `InvalidInputException` — an interval whose end does not follow
/// its start, and a page token this operation never issued.
#[tokio::test]
async fn an_impossible_request_is_refused_as_invalid_input() {
    let row = &ROWS[0];
    let name = "logevents-invalid";
    let emulator = create_available(row, name).await;
    let stream = "error/mysql-error.log";
    let now_seconds = state::now_ms() / 1_000;

    // Intervals that are no interval: the end before the start, and the end at
    // the start.
    let intervals: &[(&str, i64, i64)] = &[
        ("the end before the start", now_seconds, now_seconds - 60),
        ("the end at the start", now_seconds, now_seconds),
        ("the end long before the start", now_seconds, 0),
    ];
    for (case, start_seconds, end_seconds) in intervals {
        let (status, body) = events_call(
            &emulator,
            name,
            stream,
            &[
                ("startTime", json!(start_seconds)),
                ("endTime", json!(end_seconds)),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the interval to be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {body:?}"
        );
    }

    // Values that are no page token of this operation: a word, an unknown
    // direction, a position that is not a number, a negative one, a bare
    // position and a token with no position at all.
    for bad in [
        "nonsense", "x/1", "f/abc", "f/-1", "7", "f/", "/7", "f/1/2", "F/1",
    ] {
        let (status, body) =
            events_call(&emulator, name, stream, &[("pageToken", json!(bad))]).await;
        assert_eq!(
            status, 400,
            "case the page token {bad:?}: expected it to be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case the page token {bad:?}: expected InvalidInputException, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or("<no message>");
        assert!(
            message.contains("pageToken"),
            "case the page token {bad:?}: expected the refusal to name the member, got {message:?}"
        );
    }

    // An empty token asks for the first page, as an absent one does.
    let empty = events_page(&emulator, name, stream, &[("pageToken", json!(""))]).await;
    assert_eq!(
        messages("an empty page token", &empty),
        messages(
            "no page token",
            &events_page(&emulator, name, stream, &[]).await
        ),
        "case an empty page token: expected the first page, got {empty:?}"
    );
}
