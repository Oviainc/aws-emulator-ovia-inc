// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListHealthEvents` and its single-event reader `GetHealthEvent`.
//!
//! The rule under test: both operations report exactly the health events this service's
//! store holds for the monitor the request names — `ListHealthEvents` in `EventId` order,
//! narrowed by `EventStatus` and by the intersection of the requested window with each
//! event's own interval, and paged by `MaxResults`/`NextToken`; `GetHealthEvent` one event
//! at a time — so a monitor nothing has recorded an event for reports an empty list, a
//! monitor that has events reports those events and no others, and an `EventId` the store
//! holds no event for is refused rather than invented.
//!
//! Internet Monitor raises health events itself, from the internet-measurement pipeline it
//! runs behind a monitor, and its API has no operation that creates one. So these cases
//! put events in the one place an event can live — the service's own store, reached
//! exactly as a handler reaches it — and then read them back over real HTTP, through the
//! same `MultiServiceRouter` the emulator binary mounts.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::ops::types::{
    ImpactedLocation, HEALTH_EVENT_IMPACT_TYPE_VALUES, HEALTH_EVENT_STATUS_VALUES,
};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::state::{
    health_event_arn, service_state, HealthEventRecord, HEALTH_EVENT_STATUS_ACTIVE,
    HEALTH_EVENT_STATUS_RESOLVED, MAX_HEALTH_EVENTS_PER_MONITOR,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::monitor_lifecycle::{
    call, create_monitor, emulator, emulator_on, label, member, timestamp_secs, REGION,
};

/// The account id every emulated ARN carries, and the only `LinkedAccountId` whose
/// monitors are reachable here.
const ACCOUNT: &str = "000000000000";

/// The monitor the seeded cases hang their events off.
const MONITOR: &str = "health-events-monitor";

/// One seeded health event: what the store holds, and so what the listing must report.
#[derive(Debug, Clone)]
struct Seed {
    /// `EventId` — also the key the listing orders and pages by.
    event_id: &'static str,
    /// When the event started, in whole seconds before [`anchor`].
    started_secs_ago: u64,
    /// When the event ended, in whole seconds before [`anchor`]; `None` for an event that
    /// is still ongoing, which is what makes it report `ACTIVE`.
    ended_secs_ago: Option<u64>,
    /// `ImpactType`, taken from the model's catalog.
    impact_type: &'static str,
    /// `PercentOfTotalTrafficImpacted`.
    percent: Option<f64>,
    /// `HealthScoreThreshold`.
    threshold: f64,
    /// How many `ImpactedLocations` the event carries.
    locations: usize,
}

/// The instant the seeded cases are relative to: now, truncated to a whole second so a
/// seeded timestamp round-trips through the wire's epoch-seconds form exactly.
fn anchor() -> SystemTime {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// The seeded events, one per `HealthEventImpactType` the model publishes (see
/// `smithy_query(query="shape",shape="HealthEventImpactType")`, mirrored in
/// `HEALTH_EVENT_IMPACT_TYPE_VALUES`), spanning both statuses, both
/// `PercentOfTotalTrafficImpacted` forms and 0, 1 and 2 impacted locations.
fn seeds() -> Vec<Seed> {
    let impact_types = HEALTH_EVENT_IMPACT_TYPE_VALUES;
    assert!(
        impact_types.len() >= 4,
        "the model's impact-type catalog must cover every seeded row, got {impact_types:?}"
    );
    vec![
        Seed {
            event_id: "AWS-event-0001",
            started_secs_ago: 3_600,
            ended_secs_ago: Some(3_000),
            impact_type: impact_types[0],
            percent: Some(12.5),
            threshold: 95.0,
            locations: 1,
        },
        Seed {
            event_id: "AWS-event-0002",
            started_secs_ago: 2_000,
            ended_secs_ago: None,
            impact_type: impact_types[1],
            percent: None,
            threshold: 0.0,
            locations: 0,
        },
        Seed {
            event_id: "AWS-event-0003",
            started_secs_ago: 1_000,
            ended_secs_ago: Some(500),
            impact_type: impact_types[2],
            percent: Some(100.0),
            threshold: 50.5,
            locations: 2,
        },
        Seed {
            event_id: "AWS-event-0004",
            started_secs_ago: 120,
            ended_secs_ago: None,
            impact_type: impact_types[3],
            percent: Some(0.0),
            threshold: 100.0,
            locations: 1,
        },
    ]
}

/// The `Status` a seed must report: an event that has not ended is still `ACTIVE`.
fn seed_status(seed: &Seed) -> &'static str {
    if seed.ended_secs_ago.is_some() {
        HEALTH_EVENT_STATUS_RESOLVED
    } else {
        HEALTH_EVENT_STATUS_ACTIVE
    }
}

/// `count` impacted locations, distinct so a case can tell them apart.
fn impacted_locations(count: usize) -> Vec<ImpactedLocation> {
    (0..count)
        .map(|index| ImpactedLocation {
            as_name: format!("AS-{index}"),
            as_number: 64_512 + i64::try_from(index).unwrap_or(0),
            country: "United States".to_string(),
            city: Some(format!("City-{index}")),
            status: HEALTH_EVENT_STATUS_ACTIVE.to_string(),
            ..ImpactedLocation::default()
        })
        .collect()
}

/// The record the store holds for `seed`, relative to `at`.
fn record(seed: &Seed, at: SystemTime) -> HealthEventRecord {
    let started_at = at - Duration::from_secs(seed.started_secs_ago);
    let mut held = HealthEventRecord::new(REGION, MONITOR, seed.event_id, seed.impact_type, started_at);
    held.ended_at = seed
        .ended_secs_ago
        .map(|ago| at - Duration::from_secs(ago));
    // A health event is created when the pipeline raises it and updated as it is
    // recalculated, so the two instants are distinct from the start time here.
    held.created_at = started_at + Duration::from_secs(1);
    held.last_updated_at = held.ended_at.unwrap_or(at);
    held.impacted_locations = impacted_locations(seed.locations);
    held.percent_of_total_traffic_impacted = seed.percent;
    held.health_score_threshold = seed.threshold;
    held
}

/// An emulator whose store already holds [`seeds`] for [`MONITOR`], plus the anchor the
/// seeded instants are relative to.
async fn emulator_with_seeded_events() -> (MultiServiceRouter, SystemTime) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_monitor(&router, json!({ "MonitorName": MONITOR })).await;

    let at = anchor();
    let state = service_state(&*engine).expect("the service's own store");
    for seed in seeds() {
        let stored = state.lock().record_health_event(record(&seed, at));
        assert!(stored, "seeding {}: the store refused the record", seed.event_id);
    }
    (router, at)
}

/// A query timestamp, in the `date-time` form the boundary parses an `@httpQuery`
/// timestamp as when the schema carries no `@timestampFormat`.
fn query_time(at: SystemTime) -> String {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_secs();
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(secs).expect("an in-range instant"))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable timestamp")
}

/// `GET /HealthEvents` for `monitor` with `query` appended, as (status, body).
async fn list(
    router: &MultiServiceRouter,
    monitor: &str,
    query: &str,
) -> (u16, Value) {
    let path = if query.is_empty() {
        format!("/v20210603/Monitors/{monitor}/HealthEvents")
    } else {
        format!("/v20210603/Monitors/{monitor}/HealthEvents?{query}")
    };
    call(router, "GET", &path, None).await
}

/// The `EventId`s a listing reported, in the order it reported them.
fn event_ids(listed: &Value) -> Vec<String> {
    listed["HealthEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("HealthEvents must be a list, got {listed}"))
        .iter()
        .map(|event| {
            event["EventId"]
                .as_str()
                .unwrap_or_else(|| panic!("EventId must be a string, got {event}"))
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// A monitor with no health events
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn list_health_events_is_empty_for_a_monitor_that_has_had_no_events() {
    let router = emulator();

    // The query shape must not change the answer: a monitor nothing has recorded an event
    // for has none to report, filtered or paged however the caller likes.
    let at = anchor();
    let cases: Vec<(&str, String, String)> = vec![
        ("plain name", "new-monitor".to_string(), String::new()),
        (
            "name with every allowed punctuation",
            "new.monitor_1-2".to_string(),
            String::new(),
        ),
        ("longest allowed name", "n".repeat(255), String::new()),
        ("smallest page", "small-page-monitor".to_string(), "MaxResults=1".to_string()),
        ("largest page", "large-page-monitor".to_string(), "MaxResults=25".to_string()),
        (
            "status filter",
            "filtered-monitor".to_string(),
            format!("EventStatus={HEALTH_EVENT_STATUS_ACTIVE}"),
        ),
        (
            "window filter",
            "windowed-monitor".to_string(),
            format!(
                "StartTime={}&EndTime={}",
                query_time(at - Duration::from_secs(86_400)),
                query_time(at)
            ),
        ),
        (
            "own account as the linked account",
            "linked-monitor".to_string(),
            format!("LinkedAccountId={ACCOUNT}"),
        ),
    ];

    for (case, monitor, query) in cases {
        create_monitor(&router, json!({ "MonitorName": monitor })).await;
        let (status, listed) = list(&router, &monitor, &query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {listed}"
        );
        assert_eq!(
            event_ids(&listed),
            Vec::<String>::new(),
            "case {case}: expected no health events, got {listed}"
        );
        assert!(
            member(&listed, "NextToken").is_none(),
            "case {case}: expected no NextToken, got {listed}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// What a stored event reports
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn list_health_events_reports_every_member_the_store_holds() {
    let (router, at) = emulator_with_seeded_events().await;

    let (status, listed) = list(&router, MONITOR, "").await;
    assert_eq!(
        status, 200,
        "listing seeded events: expected 200, got {status} with {listed}"
    );

    let reported: BTreeMap<String, Value> = listed["HealthEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("HealthEvents must be a list, got {listed}"))
        .iter()
        .map(|event| (event["EventId"].as_str().unwrap_or_default().to_string(), event.clone()))
        .collect();

    let seeds = seeds();
    assert_eq!(
        event_ids(&listed),
        seeds.iter().map(|seed| seed.event_id.to_string()).collect::<Vec<_>>(),
        "every stored event must be reported once, in EventId order, got {listed}"
    );

    for seed in &seeds {
        let case = seed.event_id;
        let event = reported
            .get(case)
            .unwrap_or_else(|| panic!("case {case}: not reported in {listed}"));

        assert_eq!(
            event["EventArn"].as_str().unwrap_or_default(),
            health_event_arn(REGION, MONITOR, seed.event_id),
            "case {case}: expected the event's own ARN, got {event}"
        );
        assert_eq!(
            timestamp_secs(event, "StartedAt"),
            seconds_since_epoch(at) - seed.started_secs_ago as f64,
            "case {case}: expected the stored StartedAt, got {event}"
        );
        match seed.ended_secs_ago {
            Some(ago) => assert_eq!(
                timestamp_secs(event, "EndedAt"),
                seconds_since_epoch(at) - ago as f64,
                "case {case}: expected the stored EndedAt, got {event}"
            ),
            None => assert!(
                member(event, "EndedAt").is_none(),
                "case {case}: an ongoing event has no EndedAt, got {event}"
            ),
        }
        assert_eq!(
            timestamp_secs(event, "CreatedAt"),
            seconds_since_epoch(at) - seed.started_secs_ago as f64 + 1.0,
            "case {case}: expected the stored CreatedAt, got {event}"
        );
        assert_eq!(
            timestamp_secs(event, "LastUpdatedAt"),
            match seed.ended_secs_ago {
                Some(ago) => seconds_since_epoch(at) - ago as f64,
                None => seconds_since_epoch(at),
            },
            "case {case}: expected the stored LastUpdatedAt, got {event}"
        );
        assert_eq!(
            event["Status"].as_str().unwrap_or_default(),
            seed_status(seed),
            "case {case}: Status follows from whether the event ended, got {event}"
        );
        assert_eq!(
            event["ImpactType"].as_str().unwrap_or_default(),
            seed.impact_type,
            "case {case}: expected the stored ImpactType, got {event}"
        );
        assert_eq!(
            event["HealthScoreThreshold"].as_f64(),
            Some(seed.threshold),
            "case {case}: expected the stored HealthScoreThreshold, got {event}"
        );
        assert_eq!(
            member(event, "PercentOfTotalTrafficImpacted").and_then(Value::as_f64),
            seed.percent,
            "case {case}: expected the stored PercentOfTotalTrafficImpacted, got {event}"
        );
        let locations = event["ImpactedLocations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: ImpactedLocations must be a list, got {event}"));
        assert_eq!(
            locations.len(),
            seed.locations,
            "case {case}: expected {} impacted locations, got {event}",
            seed.locations
        );
        for (index, location) in locations.iter().enumerate() {
            assert_eq!(
                location["ASName"].as_str().unwrap_or_default(),
                format!("AS-{index}"),
                "case {case}: expected the stored impacted location, got {location}"
            );
            assert_eq!(
                location["City"].as_str().unwrap_or_default(),
                format!("City-{index}"),
                "case {case}: expected the stored impacted location, got {location}"
            );
        }
    }
}

/// An instant as epoch seconds, the scale the wire reports a timestamp in.
fn seconds_since_epoch(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_secs_f64()
}

// ---------------------------------------------------------------------------------------
// The filters
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn list_health_events_filters_by_event_status() {
    let (router, _at) = emulator_with_seeded_events().await;

    // One row per published `HealthEventStatus` (`HEALTH_EVENT_STATUS_VALUES`), plus the
    // unfiltered listing.
    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "no filter",
            String::new(),
            vec![
                "AWS-event-0001",
                "AWS-event-0002",
                "AWS-event-0003",
                "AWS-event-0004",
            ],
        ),
        (
            "ACTIVE",
            format!("EventStatus={}", HEALTH_EVENT_STATUS_VALUES[0]),
            vec!["AWS-event-0002", "AWS-event-0004"],
        ),
        (
            "RESOLVED",
            format!("EventStatus={}", HEALTH_EVENT_STATUS_VALUES[1]),
            vec!["AWS-event-0001", "AWS-event-0003"],
        ),
    ];

    for (case, query, expected) in cases {
        let (status, listed) = list(&router, MONITOR, &query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {listed}"
        );
        assert_eq!(
            event_ids(&listed),
            expected,
            "case {case}: expected {expected:?}, got {listed}"
        );
    }
}

/// One window case: the requested window, as offsets in whole seconds from the seeded
/// anchor, and the events whose own interval meets it.
struct WindowCase {
    /// Names the row in every assertion message.
    case: &'static str,
    /// `StartTime`, absent for an open-ended window.
    start: Option<i64>,
    /// `EndTime`, absent for an open-ended window.
    end: Option<i64>,
    /// The `EventId`s the listing must report, in order.
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn list_health_events_reports_the_events_whose_interval_meets_the_window() {
    let (router, at) = emulator_with_seeded_events().await;
    let every_event = vec![
        "AWS-event-0001",
        "AWS-event-0002",
        "AWS-event-0003",
        "AWS-event-0004",
    ];

    // Each row is a window and the events whose own interval overlaps it. An ongoing event
    // (0002, 0004) runs from its start to now, so it meets every window that has not
    // already ended before it started.
    let cases = vec![
        WindowCase {
            case: "no window",
            start: None,
            end: None,
            expected: every_event.clone(),
        },
        WindowCase {
            case: "the whole span",
            start: Some(-3_600),
            end: Some(0),
            expected: every_event.clone(),
        },
        WindowCase {
            case: "open-ended start, before the first event ended",
            start: Some(-3_100),
            end: None,
            expected: every_event.clone(),
        },
        WindowCase {
            case: "open-ended start, after the first event ended",
            start: Some(-2_900),
            end: None,
            expected: vec!["AWS-event-0002", "AWS-event-0003", "AWS-event-0004"],
        },
        WindowCase {
            case: "open-ended end, before the second event started",
            start: None,
            end: Some(-3_300),
            expected: vec!["AWS-event-0001"],
        },
        WindowCase {
            case: "a window only the ongoing event spans",
            start: Some(-2_500),
            end: Some(-1_500),
            expected: vec!["AWS-event-0002"],
        },
        WindowCase {
            case: "a window inside a resolved event",
            start: Some(-700),
            end: Some(-600),
            expected: vec!["AWS-event-0002", "AWS-event-0003"],
        },
        WindowCase {
            case: "an instant window on an event's start",
            start: Some(-1_000),
            end: Some(-1_000),
            expected: vec!["AWS-event-0002", "AWS-event-0003"],
        },
        WindowCase {
            case: "a window entirely in the future",
            start: Some(3_600),
            end: Some(7_200),
            expected: vec![],
        },
        WindowCase {
            case: "a window entirely before the first event",
            start: Some(-10_000),
            end: Some(-5_000),
            expected: vec![],
        },
    ];

    for WindowCase {
        case,
        start,
        end,
        expected,
    } in cases
    {
        let mut query = Vec::new();
        if let Some(offset) = start {
            query.push(format!("StartTime={}", query_time(offset_from(at, offset))));
        }
        if let Some(offset) = end {
            query.push(format!("EndTime={}", query_time(offset_from(at, offset))));
        }
        let (status, listed) = list(&router, MONITOR, &query.join("&")).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {listed}"
        );
        assert_eq!(
            event_ids(&listed),
            expected,
            "case {case}: expected {expected:?}, got {listed}"
        );
    }
}

/// `at` shifted by `offset` seconds, which may be negative.
fn offset_from(at: SystemTime, offset: i64) -> SystemTime {
    if offset >= 0 {
        at + Duration::from_secs(offset.unsigned_abs())
    } else {
        at - Duration::from_secs(offset.unsigned_abs())
    }
}

// ---------------------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn list_health_events_pages_over_every_stored_event() {
    let (router, _at) = emulator_with_seeded_events().await;
    let all: Vec<String> = seeds().iter().map(|seed| seed.event_id.to_string()).collect();

    // Page sizes below, at and above the number of stored events, plus the modelled
    // maximum: the pages concatenated are the whole listing, each one at most the
    // requested size, and the last carries no token.
    for page_size in [1_usize, 2, 3, 4, 5, 25] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("MaxResults={page_size}");
            if let Some(next) = &token {
                query.push_str(&format!("&NextToken={next}"));
            }
            let (status, listed) = list(&router, MONITOR, &query).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected 200, got {status} with {listed}"
            );
            let ids = event_ids(&listed);
            assert!(
                ids.len() <= page_size,
                "page size {page_size}: a page must hold at most {page_size} events, got {listed}"
            );
            seen.extend(ids);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "page size {page_size}: paging must terminate, got {seen:?}"
            );
            match member(&listed, "NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "page size {page_size}: expected every stored event exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_health_events_pages_a_monitor_filled_to_its_bound() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_monitor(&router, json!({ "MonitorName": MONITOR })).await;

    let at = anchor();
    let state = service_state(&*engine).expect("the service's own store");
    let expected: Vec<String> = (0..MAX_HEALTH_EVENTS_PER_MONITOR)
        .map(|index| format!("AWS-bound-{index:04}"))
        .collect();
    for event_id in &expected {
        let stored = state.lock().record_health_event(HealthEventRecord::new(
            REGION,
            MONITOR,
            event_id,
            HEALTH_EVENT_IMPACT_TYPE_VALUES[0],
            at,
        ));
        assert!(stored, "seeding {event_id}: the store refused the record");
    }

    // One past the per-monitor ceiling is refused, and re-recording an event already held
    // is not growth, so it is taken.
    let over = state.lock().record_health_event(HealthEventRecord::new(
        REGION,
        MONITOR,
        "AWS-bound-one-too-many",
        HEALTH_EVENT_IMPACT_TYPE_VALUES[0],
        at,
    ));
    assert!(!over, "the store must refuse an event past its per-monitor bound");
    let again = state.lock().record_health_event(HealthEventRecord::new(
        REGION,
        MONITOR,
        &expected[0],
        HEALTH_EVENT_IMPACT_TYPE_VALUES[0],
        at,
    ));
    assert!(again, "re-recording an event already held must replace it");

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = "MaxResults=25".to_string();
        if let Some(next) = &token {
            query.push_str(&format!("&NextToken={next}"));
        }
        let (status, listed) = list(&router, MONITOR, &query).await;
        assert_eq!(
            status, 200,
            "paging a full monitor: expected 200, got {status} with {listed}"
        );
        seen.extend(event_ids(&listed));
        match member(&listed, "NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen, expected,
        "paging a full monitor: expected every stored event exactly once, got {} events",
        seen.len()
    );
}

// ---------------------------------------------------------------------------------------
// Scoping and refusals
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn one_monitors_health_events_are_reported_for_that_monitor_only() {
    let (router, _at) = emulator_with_seeded_events().await;
    create_monitor(&router, json!({ "MonitorName": "other-monitor" })).await;

    let (status, listed) = list(&router, "other-monitor", "").await;
    assert_eq!(
        status, 200,
        "another monitor: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        event_ids(&listed),
        Vec::<String>::new(),
        "another monitor's events must not be reported, got {listed}"
    );

    // Deleting the monitor releases its events: a monitor later created with the same name
    // is a new monitor, which has had none.
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/v20210603/Monitors/{MONITOR}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the monitor: expected 200, got {status} with {deleted}"
    );
    create_monitor(&router, json!({ "MonitorName": MONITOR })).await;
    let (status, relisted) = list(&router, MONITOR, "").await;
    assert_eq!(
        status, 200,
        "the recreated monitor: expected 200, got {status} with {relisted}"
    );
    assert_eq!(
        event_ids(&relisted),
        Vec::<String>::new(),
        "a recreated monitor must report no events, got {relisted}"
    );
}

#[tokio::test]
async fn list_health_events_refuses_a_request_it_cannot_serve() {
    let (router, at) = emulator_with_seeded_events().await;

    let cases: Vec<(&str, String, String, u16)> = vec![
        (
            "a monitor no monitor carries",
            "no-such-monitor".to_string(),
            String::new(),
            400,
        ),
        (
            "MaxResults below its modelled minimum",
            MONITOR.to_string(),
            "MaxResults=0".to_string(),
            400,
        ),
        (
            "MaxResults at its modelled minimum",
            MONITOR.to_string(),
            "MaxResults=1".to_string(),
            200,
        ),
        (
            "MaxResults at its modelled maximum",
            MONITOR.to_string(),
            "MaxResults=25".to_string(),
            200,
        ),
        (
            "MaxResults above its modelled maximum",
            MONITOR.to_string(),
            "MaxResults=26".to_string(),
            400,
        ),
        (
            "an EventStatus outside the published catalog",
            MONITOR.to_string(),
            "EventStatus=PENDING".to_string(),
            400,
        ),
        (
            "a NextToken this service did not issue",
            MONITOR.to_string(),
            "NextToken=not-a-token".to_string(),
            400,
        ),
        (
            "a window that ends before it starts",
            MONITOR.to_string(),
            format!(
                "StartTime={}&EndTime={}",
                query_time(at),
                query_time(at - Duration::from_secs(3_600))
            ),
            400,
        ),
        (
            "an account that is not linked here",
            MONITOR.to_string(),
            "LinkedAccountId=999999999999".to_string(),
            400,
        ),
        (
            "a LinkedAccountId shorter than its modelled length",
            MONITOR.to_string(),
            "LinkedAccountId=1".to_string(),
            400,
        ),
        (
            "this emulator's own account as the linked account",
            MONITOR.to_string(),
            format!("LinkedAccountId={ACCOUNT}"),
            200,
        ),
    ];

    for (case, monitor, query, expected) in cases {
        let (status, body) = list(&router, &monitor, &query).await;
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// GetHealthEvent: the single-event read of the same store
// ---------------------------------------------------------------------------------------

/// `GET /HealthEvents/{EventId}` for `monitor`, with the id percent-encoded into the path
/// label the way an SDK encodes it (the model's `EventId` doc: "Because EventID contains
/// the forward slash (\u201c/\u201d) character, you must URL-encode the EventID field in the
/// request URL").
async fn get_event(router: &MultiServiceRouter, monitor: &str, event_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!(
            "/v20210603/Monitors/{monitor}/HealthEvents/{}",
            label(event_id)
        ),
        None,
    )
    .await
}

#[tokio::test]
async fn get_health_event_reads_back_each_event_the_listing_reports() {
    let (router, at) = emulator_with_seeded_events().await;

    let (status, listed) = list(&router, MONITOR, "").await;
    assert_eq!(
        status, 200,
        "listing seeded events: expected 200, got {status} with {listed}"
    );

    // Every member of every listed event must read back identically through the
    // single-event reader: both compute from the one store.
    for (index, event) in listed["HealthEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("HealthEvents must be a list, got {listed}"))
        .iter()
        .enumerate()
    {
        let event_id = event["EventId"].as_str().unwrap_or_default();
        let (status, fetched) = get_event(&router, MONITOR, event_id).await;
        assert_eq!(
            status, 200,
            "case {event_id}: expected 200, got {status} with {fetched}"
        );
        for field in [
            "EventArn",
            "EventId",
            "Status",
            "ImpactType",
            "HealthScoreThreshold",
            "PercentOfTotalTrafficImpacted",
            "ImpactedLocations",
            "EndedAt",
        ] {
            assert_eq!(
                member(&fetched, field),
                member(event, field),
                "case {event_id}: {field} must match the listing, got {fetched}"
            );
        }
        for field in ["StartedAt", "CreatedAt", "LastUpdatedAt"] {
            assert_eq!(
                timestamp_secs(&fetched, field),
                timestamp_secs(event, field),
                "case {event_id}: {field} must match the listing, got {fetched}"
            );
        }
        let seed = &seeds()[index];
        assert_eq!(
            timestamp_secs(&fetched, "StartedAt"),
            seconds_since_epoch(at) - seed.started_secs_ago as f64,
            "case {event_id}: expected the stored StartedAt, got {fetched}"
        );
    }
}

#[tokio::test]
async fn get_health_event_reads_back_an_event_id_carrying_slashes() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_monitor(&router, json!({ "MonitorName": MONITOR })).await;

    // `HealthEventName`'s pattern `^[a-zA-Z0-9/_.-]+$` admits the forward slash, and
    // AWS's own event ids carry several.
    let event_id = "AWS/us-east-1/2026-10-23/f0e1d2c3";
    let at = anchor();
    let stored = service_state(&*engine)
        .expect("the service's own store")
        .lock()
        .record_health_event(HealthEventRecord::new(
            REGION,
            MONITOR,
            event_id,
            HEALTH_EVENT_IMPACT_TYPE_VALUES[0],
            at,
        ));
    assert!(stored, "seeding {event_id}: the store refused the record");

    let (status, listed) = list(&router, MONITOR, "").await;
    assert_eq!(
        status, 200,
        "listing a slashed event id: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        event_ids(&listed),
        vec![event_id.to_string()],
        "a slashed event id must be listed verbatim, got {listed}"
    );

    let (status, fetched) = get_event(&router, MONITOR, event_id).await;
    assert_eq!(
        status, 200,
        "reading a slashed event id: expected 200, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["EventId"].as_str().unwrap_or_default(),
        event_id,
        "expected the stored EventId, got {fetched}"
    );
    assert_eq!(
        fetched["EventArn"].as_str().unwrap_or_default(),
        health_event_arn(REGION, MONITOR, event_id),
        "expected the event's own ARN, got {fetched}"
    );
}

/// One `GetHealthEvent` case: the event asked for, and what the answer must be.
struct EventCase {
    /// Names the row in every assertion message.
    case: &'static str,
    /// The `MonitorName` in the path.
    monitor: String,
    /// The `EventId` in the path.
    event_id: String,
    /// The HTTP status the answer must carry.
    status: u16,
    /// The `message` a refusal must carry, where the rule that builds it is known: AWS
    /// builds a not-found health event's message from the resource the request named.
    message: Option<String>,
}

/// The not-found message rule: AWS answers a health event this account does not hold with
/// `resourceID is invalid: health-event/<MonitorName>/<EventId>`, built from the request.
fn invalid_resource_message(monitor: &str, event_id: &str) -> String {
    format!("resourceID is invalid: health-event/{monitor}/{event_id}")
}

#[tokio::test]
async fn get_health_event_refuses_an_event_the_store_does_not_hold() {
    let (router, _at) = emulator_with_seeded_events().await;
    create_monitor(&router, json!({ "MonitorName": "eventless-monitor" })).await;

    // An `EventId` nothing recorded names no event, whichever monitor is asked and however
    // plausible the id looks: that is a refusal, never a fabricated event.
    let cases = vec![
        EventCase {
            case: "a stored event of its own monitor",
            monitor: MONITOR.to_string(),
            event_id: "AWS-event-0001".to_string(),
            status: 200,
            message: None,
        },
        EventCase {
            case: "an event id nothing recorded",
            monitor: MONITOR.to_string(),
            event_id: "AWS-event-9999".to_string(),
            status: 400,
            message: Some(invalid_resource_message(MONITOR, "AWS-event-9999")),
        },
        EventCase {
            case: "an AWS-shaped event id nothing recorded",
            monitor: MONITOR.to_string(),
            event_id: "AWS/us-east-1/2026-01-01/deadbeef".to_string(),
            status: 400,
            message: Some(invalid_resource_message(
                MONITOR,
                "AWS/us-east-1/2026-01-01/deadbeef",
            )),
        },
        EventCase {
            case: "another monitor's event id",
            monitor: "eventless-monitor".to_string(),
            event_id: "AWS-event-0001".to_string(),
            status: 400,
            message: Some(invalid_resource_message(
                "eventless-monitor",
                "AWS-event-0001",
            )),
        },
        EventCase {
            case: "a monitor no monitor carries",
            monitor: "no-such-monitor".to_string(),
            event_id: "AWS-event-0001".to_string(),
            status: 400,
            message: Some(invalid_resource_message("no-such-monitor", "AWS-event-0001")),
        },
        EventCase {
            case: "an event id outside the modelled pattern",
            monitor: MONITOR.to_string(),
            event_id: "AWS event 0001".to_string(),
            status: 400,
            message: None,
        },
    ];

    for EventCase {
        case,
        monitor,
        event_id,
        status: expected_status,
        message: expected_message,
    } in cases
    {
        let (status, body) = get_event(&router, &monitor, &event_id).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected {expected_status}, got {status} with {body}"
        );
        if let Some(expected) = expected_message {
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                expected,
                "case {case}: expected the request-derived message, got {body}"
            );
        }
    }
}
