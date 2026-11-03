// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeAttackStatistics`.
//!
//! Every case goes over the wire — an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AWSShield_20160616.DescribeAttackStatistics` through the same
//! `MultiServiceRouter` the production router serves — so routing, deserialization, the
//! handler and serialization are all exercised.
//!
//! The rules under test, grounded in this operation's own Smithy documentation:
//!
//! * the reported period is fixed by the clock and not by the request: "The operation
//!   returns data for the time range of midnight UTC, one year ago, to midnight UTC,
//!   today", so `ToExclusive` is midnight UTC of the current UTC day and `FromInclusive`
//!   is midnight UTC of the same calendar day one year earlier;
//! * one data record summarises that period (`AttackStatisticsDataItem`: "A single
//!   attack statistics data record ... returned by DescribeAttackStatistics along with a
//!   time range indicating the time period that the attack statistics apply to"), whose
//!   `AttackCount` counts exactly the account's detected attacks that started at or
//!   after `FromInclusive` and before `ToExclusive`;
//! * `AttackVolume` reports, per unit, the maximum the counted attacks observed
//!   (`AttackVolumeStatistics$Max`: "The maximum attack volume observed for the given
//!   unit"); the block itself belongs to every data record, so it is always present, and
//!   it is its three unit members that say what was observed — a unit no counted attack
//!   reported is left unset, and a window with no reported unit at all is an empty
//!   `AttackVolume`: "If the accompanying `AttackCount` in the statistics object is
//!   zero, this setting might be empty";
//! * the operation reports, so repeating it answers the same thing and leaves the
//!   account's attack log as it was.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format as DateTimeFormat;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_shield_awsshield_20160616::state::{
    attack_statistics_window, service_state, AttackRecord,
};
use service_com_amazonaws_shield_awsshield_20160616::EmulatedService;

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSShield_20160616.";

/// Seconds in a UTC day, the grid every bound of the reported window sits on.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// One answer from the emulator: the HTTP status and the parsed JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// Assert this reply is a success, and hand back its body.
    fn expect_ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected HTTP 200, got {self:?}"
        );
        &self.body
    }
}

/// The emulator under test: one shared engine, one registered service, requests
/// dispatched through the production router.
struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's; the
        // routing key still comes from the `Authorization` header below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, operation: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/us-east-1/shield/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{operation}: response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Reply { status, body }
    }

    /// Record one attack Shield detected against the account. Reaches the store directly
    /// because Shield detects attacks rather than being asked to create them: no
    /// modelled operation writes the attack log.
    fn detect_attack(&self, seed: &Seed, from_inclusive: SystemTime, to_exclusive: SystemTime) {
        let state = service_state(&*self.engine).expect("the shield store");
        let mut guard = state.lock();
        let attack_id = format!("attack-{}", guard.attacks.len());
        guard.attacks.push(AttackRecord {
            resource_arn: format!("arn:aws:cloudfront::000000000000:distribution/{attack_id}"),
            attack_id,
            start_time: seed.at.instant(from_inclusive, to_exclusive),
            end_time: None,
            max_bits_per_second: seed.bits_per_second,
            max_packets_per_second: seed.packets_per_second,
            max_requests_per_second: seed.requests_per_second,
            attack_vectors: Vec::new(),
        });
    }

    /// How many attacks the account's log holds, so a reporting call can be shown to
    /// leave it alone.
    fn attacks_held(&self) -> usize {
        let state = service_state(&*self.engine).expect("the shield store");
        let guard = state.lock();
        guard.attacks.len()
    }
}

/// Where in the reported window an attack started. The window is `[from, to)`, so the
/// lower bound is in it and the upper bound is not.
#[derive(Debug, Clone, Copy)]
enum At {
    /// One second before the window opens — outside.
    BeforeWindow,
    /// Exactly the window's inclusive lower bound — inside.
    WindowOpens,
    /// Half way through the window — inside.
    MidWindow,
    /// One second before the window closes — inside.
    WindowClosing,
    /// Exactly the window's exclusive upper bound — outside.
    WindowCloses,
    /// The current instant, which is in today's UTC day and so past the upper bound —
    /// outside.
    Now,
}

impl At {
    fn instant(self, from_inclusive: SystemTime, to_exclusive: SystemTime) -> SystemTime {
        let one_second = Duration::from_secs(1);
        let span = to_exclusive
            .duration_since(from_inclusive)
            .expect("the window's upper bound is after its lower bound");
        match self {
            At::BeforeWindow => from_inclusive - one_second,
            At::WindowOpens => from_inclusive,
            At::MidWindow => from_inclusive + span / 2,
            At::WindowClosing => to_exclusive - one_second,
            At::WindowCloses => to_exclusive,
            At::Now => SystemTime::now(),
        }
    }
}

/// One attack to record before the call: where it started, and the volume it observed
/// per unit (`None` for a unit it did not report).
#[derive(Debug, Clone, Copy)]
struct Seed {
    at: At,
    bits_per_second: Option<f64>,
    packets_per_second: Option<f64>,
    requests_per_second: Option<f64>,
}

impl Seed {
    /// A network-level attack, which reports bits and packets per second.
    const fn network(at: At, bits_per_second: f64, packets_per_second: f64) -> Self {
        Self {
            at,
            bits_per_second: Some(bits_per_second),
            packets_per_second: Some(packets_per_second),
            requests_per_second: None,
        }
    }

    /// An application-level attack, which reports requests per second.
    const fn application(at: At, requests_per_second: f64) -> Self {
        Self {
            at,
            bits_per_second: None,
            packets_per_second: None,
            requests_per_second: Some(requests_per_second),
        }
    }
}

/// One row of the table: the attacks the account has seen, and the single data record
/// the operation must answer with.
#[derive(Debug)]
struct Case {
    name: &'static str,
    attacks: &'static [Seed],
    attack_count: i64,
    bits_per_second: Option<f64>,
    packets_per_second: Option<f64>,
    requests_per_second: Option<f64>,
}

/// The instant an RFC 3339 timestamp names.
fn instant_at(timestamp: &str) -> SystemTime {
    let parsed = DateTime::from_str(timestamp, DateTimeFormat::DateTime)
        .unwrap_or_else(|e| panic!("{timestamp} is an RFC 3339 timestamp ({e})"));
    SystemTime::try_from(parsed).unwrap_or_else(|_| panic!("{timestamp} is representable"))
}

/// An instant as RFC 3339, the form the window table states its expectations in.
fn rendered(instant: SystemTime) -> String {
    DateTime::from(instant)
        .fmt(DateTimeFormat::DateTime)
        .expect("an instant RFC 3339 can render")
}

/// Epoch seconds, as the `awsJson1_1` wire form of a timestamp carries them.
fn epoch_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// The `Max` of one `AttackVolume` unit, or `None` when the unit (or the whole volume)
/// is unset.
fn unit_max(item: &Value, unit: &str) -> Option<f64> {
    item.get("AttackVolume")?.get(unit)?.get("Max")?.as_f64()
}

/// The three units an `AttackVolume` reports, from the modelled shape
/// (`com.amazonaws.shield#AttackVolume`: `BitsPerSecond`, `PacketsPerSecond`,
/// `RequestsPerSecond`, each an optional `AttackVolumeStatistics`).
const VOLUME_UNITS: [&str; 3] = ["BitsPerSecond", "PacketsPerSecond", "RequestsPerSecond"];

#[tokio::test]
async fn attack_statistics_aggregate_the_account_attacks_of_the_reported_window() {
    // Every row differs in how many attacks the account saw, where in the window they
    // started, and which units they reported.
    const CASES: &[Case] = &[
        Case {
            name: "no attacks at all: a zero count carries an empty volume",
            attacks: &[],
            attack_count: 0,
            bits_per_second: None,
            packets_per_second: None,
            requests_per_second: None,
        },
        Case {
            name: "a counted attack that reported no unit at all carries an empty volume",
            attacks: &[Seed {
                at: At::MidWindow,
                bits_per_second: None,
                packets_per_second: None,
                requests_per_second: None,
            }],
            attack_count: 1,
            bits_per_second: None,
            packets_per_second: None,
            requests_per_second: None,
        },
        Case {
            name: "one network-level attack reports bits and packets, not requests",
            attacks: &[Seed::network(At::MidWindow, 1_456_815.4, 1_155.0)],
            attack_count: 1,
            bits_per_second: Some(1_456_815.4),
            packets_per_second: Some(1_155.0),
            requests_per_second: None,
        },
        Case {
            name: "one application-level attack reports requests only",
            attacks: &[Seed::application(At::MidWindow, 24.5)],
            attack_count: 1,
            bits_per_second: None,
            packets_per_second: None,
            requests_per_second: Some(24.5),
        },
        Case {
            name: "each unit reports the maximum over the counted attacks",
            attacks: &[
                Seed::network(At::WindowOpens, 2_000.0, 30.0),
                Seed::network(At::MidWindow, 900.0, 75.5),
                Seed::application(At::WindowClosing, 12.0),
                Seed::application(At::MidWindow, 7.0),
            ],
            attack_count: 4,
            bits_per_second: Some(2_000.0),
            packets_per_second: Some(75.5),
            requests_per_second: Some(12.0),
        },
        Case {
            name: "the window includes its lower bound and excludes its upper bound",
            attacks: &[
                Seed::network(At::BeforeWindow, 9_999.0, 9_999.0),
                Seed::network(At::WindowOpens, 10.0, 11.0),
                Seed::network(At::WindowClosing, 12.0, 13.0),
                Seed::network(At::WindowCloses, 8_888.0, 8_888.0),
                Seed::network(At::Now, 7_777.0, 7_777.0),
            ],
            attack_count: 2,
            bits_per_second: Some(12.0),
            packets_per_second: Some(13.0),
            requests_per_second: None,
        },
        Case {
            name: "attacks only outside the window leave the count at zero",
            attacks: &[
                Seed::application(At::BeforeWindow, 500.0),
                Seed::network(At::Now, 600.0, 700.0),
            ],
            attack_count: 0,
            bits_per_second: None,
            packets_per_second: None,
            requests_per_second: None,
        },
    ];

    for case in CASES {
        // A UTC day boundary crossed between the test's clock read and the handler's
        // would move the seeded boundary attacks to the other side of the reported
        // window, so such an attempt asserts nothing and is simply retried.
        let mut attempts = 0;
        while !run_case(case).await {
            attempts += 1;
            assert!(
                attempts < 3,
                "case {}: the reported window kept disagreeing with the test's own \
                 clock read over {attempts} attempts",
                case.name
            );
        }
    }
}

/// Run one row: record its attacks, call the operation and assert the data record and
/// the reported period. Answers `false` when the UTC day turned between the test's
/// clock read and the handler's, which makes the row's seeded instants meaningless and
/// is the caller's cue to run it again.
async fn run_case(case: &Case) -> bool {
    let name = case.name;
    let emulator = Emulator::new();

    // The window the handler will report over at this instant. The rule the bounds
    // themselves follow is pinned, over seeded instants, by the table in
    // `the_reported_window_is_a_calendar_year_of_utc_days`; here they only place the
    // attacks relative to the period being reported.
    let (from_inclusive, to_exclusive) = attack_statistics_window(SystemTime::now());
    for seed in case.attacks {
        emulator.detect_attack(seed, from_inclusive, to_exclusive);
    }

    let reply = emulator.call("DescribeAttackStatistics", &json!({})).await;
    let body = reply.expect_ok(name);

    // The reported period is the documented one, whatever the attacks were.
    let time_range = body
        .get("TimeRange")
        .unwrap_or_else(|| panic!("case {name}: no TimeRange in {body}"));
    let reported_to = time_range
        .get("ToExclusive")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {name}: no ToExclusive in {time_range}"));
    let reported_from = time_range
        .get("FromInclusive")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {name}: no FromInclusive in {time_range}"));
    let now = epoch_seconds(SystemTime::now());
    assert_eq!(
        reported_to % SECONDS_PER_DAY,
        0.0,
        "case {name}: expected ToExclusive at midnight UTC, got {reported_to}"
    );
    assert!(
        reported_to <= now && now - reported_to < SECONDS_PER_DAY,
        "case {name}: expected ToExclusive at midnight UTC of the current UTC day, \
         got {reported_to} with now {now}"
    );
    // One calendar year is 365 or 366 days of the same month and day, so the lower bound
    // is a whole number of days earlier and falls on the same date of the year.
    let span_in_days = (reported_to - reported_from) / SECONDS_PER_DAY;
    assert!(
        (365.0..=366.0).contains(&span_in_days),
        "case {name}: expected a calendar year between FromInclusive {reported_from} \
         and ToExclusive {reported_to}, got {span_in_days} days"
    );
    let month_day = |epoch: f64| {
        rendered(SystemTime::UNIX_EPOCH + Duration::from_secs(epoch as u64))
            .split_once('-')
            .map(|(_year, rest)| rest.to_string())
            .expect("an RFC 3339 rendering carries a month and day")
    };
    assert_eq!(
        month_day(reported_from),
        month_day(reported_to),
        "case {name}: expected FromInclusive on the same date of the year as \
         ToExclusive {reported_to}, got {reported_from}"
    );
    if reported_to != epoch_seconds(to_exclusive) {
        return false;
    }

    let items = body
        .get("DataItems")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {name}: no DataItems in {body}"));
    assert_eq!(
        items.len(),
        1,
        "case {name}: expected a single statistics data record, got {items:?}"
    );
    let item = &items[0];

    assert_eq!(
        item.get("AttackCount").and_then(Value::as_i64),
        Some(case.attack_count),
        "case {name}: expected AttackCount {}, got {item:?}",
        case.attack_count
    );
    let expected_units = [
        case.bits_per_second,
        case.packets_per_second,
        case.requests_per_second,
    ];
    for (unit, expected) in VOLUME_UNITS.into_iter().zip(expected_units) {
        assert_eq!(
            unit_max(item, unit),
            expected,
            "case {name}: expected {unit} max {expected:?}, got {item:?}"
        );
    }

    // The volume block belongs to the data record whatever the attacks were, so it is
    // present — as an object — even when not one of its units was reported, and it then
    // carries none of them.
    let volume = item
        .get("AttackVolume")
        .unwrap_or_else(|| panic!("case {name}: no AttackVolume in {item:?}"));
    let reported = volume
        .as_object()
        .unwrap_or_else(|| panic!("case {name}: expected AttackVolume to be an object, got {volume:?}"));
    let mut expected_members: Vec<&str> = VOLUME_UNITS
        .into_iter()
        .zip(expected_units)
        .filter_map(|(unit, expected)| expected.map(|_| unit))
        .collect();
    expected_members.sort_unstable();
    let mut present_members: Vec<&str> = reported.keys().map(String::as_str).collect();
    present_members.sort_unstable();
    assert_eq!(
        present_members, expected_members,
        "case {name}: expected AttackVolume to carry exactly the reported units \
         {expected_members:?}, got {volume:?}"
    );
    true
}

#[tokio::test]
async fn describing_attack_statistics_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let (from_inclusive, to_exclusive) = attack_statistics_window(SystemTime::now());
    for seed in [
        Seed::network(At::MidWindow, 3_000.0, 40.0),
        Seed::application(At::BeforeWindow, 9.0),
    ] {
        emulator.detect_attack(&seed, from_inclusive, to_exclusive);
    }

    let first = emulator
        .call("DescribeAttackStatistics", &json!({}))
        .await;
    let first_body = first.expect_ok("first read").clone();
    let held = emulator.attacks_held();
    let second = emulator
        .call("DescribeAttackStatistics", &json!({}))
        .await;
    let second_body = second.expect_ok("second read");

    assert_eq!(
        &first_body, second_body,
        "case repeated read: a reporting operation must answer the same thing twice, \
         got {first_body:?} then {second_body:?}"
    );
    assert_eq!(
        emulator.attacks_held(),
        held,
        "case repeated read: the attack log must be untouched, it held {held} records"
    );
}

/// One row of the window table: the instant the operation is called at, and the period
/// it must then report.
#[derive(Debug)]
struct WindowCase {
    now: &'static str,
    from_inclusive: &'static str,
    to_exclusive: &'static str,
}

#[test]
fn the_reported_window_is_a_calendar_year_of_utc_days() {
    // The rule, from this operation's own Smithy documentation: "The operation returns
    // data for the time range of midnight UTC, one year ago, to midnight UTC, today. For
    // example, if the current time is 2020-10-26 15:39:32 PDT, equal to 2020-10-26
    // 22:39:32 UTC, then the time range for the attack data returned is from 2019-10-26
    // 00:00:00 UTC to 2020-10-26 00:00:00 UTC." The first row is that documented
    // example; the rest vary the instant within the day, the day within the year, and
    // the two dates a calendar year cannot step between naively.
    const CASES: &[WindowCase] = &[
        WindowCase {
            now: "2020-10-26T22:39:32Z",
            from_inclusive: "2019-10-26T00:00:00Z",
            to_exclusive: "2020-10-26T00:00:00Z",
        },
        WindowCase {
            now: "2026-03-01T00:00:00Z",
            from_inclusive: "2025-03-01T00:00:00Z",
            to_exclusive: "2026-03-01T00:00:00Z",
        },
        WindowCase {
            // The last second of the same UTC day reports the same period: the bounds
            // are days, not the instant of the call.
            now: "2026-03-01T23:59:59Z",
            from_inclusive: "2025-03-01T00:00:00Z",
            to_exclusive: "2026-03-01T00:00:00Z",
        },
        WindowCase {
            // The next UTC day moves both bounds by one day.
            now: "2026-03-02T00:00:00Z",
            from_inclusive: "2025-03-02T00:00:00Z",
            to_exclusive: "2026-03-02T00:00:00Z",
        },
        WindowCase {
            // A 29 February has no counterpart one year earlier, so the lower bound is
            // that year's last day of February.
            now: "2024-02-29T12:00:00Z",
            from_inclusive: "2023-02-28T00:00:00Z",
            to_exclusive: "2024-02-29T00:00:00Z",
        },
        WindowCase {
            // A window that spans a 29 February is 366 days long, not 365.
            now: "2025-01-15T06:30:00Z",
            from_inclusive: "2024-01-15T00:00:00Z",
            to_exclusive: "2025-01-15T00:00:00Z",
        },
        WindowCase {
            // The day after a leap day steps back to the same date of the short year.
            now: "2024-03-01T08:00:00Z",
            from_inclusive: "2023-03-01T00:00:00Z",
            to_exclusive: "2024-03-01T00:00:00Z",
        },
    ];

    for case in CASES {
        let (from_inclusive, to_exclusive) = attack_statistics_window(instant_at(case.now));
        assert_eq!(
            (rendered(from_inclusive).as_str(), rendered(to_exclusive).as_str()),
            (case.from_inclusive, case.to_exclusive),
            "case {}: expected the window [{}, {}), got [{}, {})",
            case.now,
            case.from_inclusive,
            case.to_exclusive,
            rendered(from_inclusive),
            rendered(to_exclusive)
        );
    }
}
