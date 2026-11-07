// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the global internet-event feed and for `GetQueryResults`.
//!
//! The rules under test:
//!
//! * An internet event is an observation the SERVICE makes of the public internet, shown
//!   on one global outages map to every account, so the feed is populated for an account
//!   that has created nothing: `ListInternetEvents` reports it — filtered, bounded and
//!   paged — and `GetInternetEvent` reads one event of it by id, refusing an id the feed
//!   holds no event for.
//! * A request whose time window starts after it ends selects no instant, so every
//!   operation that carries such a window refuses it with one status, one code and one
//!   fixed wording.
//! * A `QueryId` identifies a query of ONE monitor, so `GetQueryResults` refuses an id no
//!   query of the monitor it names carries, with the same `ValidationException` and the
//!   same wording the rest of the query interface answers with; for a query it does
//!   resolve, it reports that query's repository — the columns the data is made of and the
//!   rows projected from the Internet Monitor data this service holds for the monitor,
//!   inside the query's own window and narrowed by its `FilterParameters`.
//!
//! Internet Monitor raises health events and observes internet events itself, and its API
//! models no operation that creates either, so the cases that need them put them in the
//! one place they can live — the service's own store, reached exactly as a handler reaches
//! it — and then drive real HTTP through the same `MultiServiceRouter` the emulator binary
//! mounts. The wire helpers live in `crate::monitor_lifecycle` and are reused rather than
//! copied.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::ops::types::{
    AvailabilityMeasurement, FilterParameter, ImpactedLocation, InternetHealth,
    PerformanceMeasurement, RoundTripTime, INTERNET_EVENT_STATUS_VALUES,
    INTERNET_EVENT_TYPE_VALUES, OPERATOR_VALUES, QUERY_TYPE_VALUES,
};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::state::{
    epoch_seconds, internet_event_resource_path, invalid_resource_id_message, seed_event_id,
    service_state, HealthEventRecord, QueryRecord, INTERNET_EVENT_FEED_WINDOW,
    INVALID_QUERY_ID_MESSAGE, INVALID_TIME_WINDOW_MESSAGE, QUERY_STATUS_QUEUED,
    QUERY_STATUS_SUCCEEDED, SEEDED_INTERNET_EVENTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::monitor_lifecycle::{
    call, create_monitor, emulator_on, error_code, error_message, label, member, REGION,
};

/// `ListInternetEvents`' route, and the prefix `GetInternetEvent`'s `@httpLabel` extends
/// (`smithy_query(query="operation",operation="ListInternetEvents,GetInternetEvent")`).
const EVENTS_PATH: &str = "/v20210603/InternetEvents";

/// `MaxResults`' own `@httpQuery` name on `ListInternetEvents`, which is NOT the member
/// name (`smithy_query(query="operation",operation="ListInternetEvents")`).
const MAX_RESULTS_QUERY: &str = "InternetEventMaxResults";

/// The monitor the query cases hang their measurements off.
const MONITOR: &str = "query-results-monitor";

/// A resource ARN a monitor can monitor, so a query against it is not refused for having
/// nothing to report on.
const VPC: &str = "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-aaaa0001";

/// Now, truncated to a whole second, so an instant survives the wire's timestamp forms
/// exactly.
fn anchor() -> SystemTime {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// An instant as the `date-time` value a `@timestampFormat` query parameter carries,
/// percent-encoded for the query string.
fn query_time(at: SystemTime) -> String {
    label(&wire_time(at))
}

/// An instant as the `date-time` value a `@timestampFormat` body member carries.
fn wire_time(at: SystemTime) -> String {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_secs();
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(secs).expect("an in-range instant"))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable timestamp")
}

// ---------------------------------------------------------------------------------------
// The service's own internet-event feed
// ---------------------------------------------------------------------------------------

/// One internet event as the store holds it, which is what every expectation below is
/// computed from: the operation is asserted against the service's own state, not against
/// a second copy of the seeding rule.
#[derive(Debug, Clone)]
struct FeedRecord {
    event_id: String,
    event_arn: String,
    started_at: f64,
    ended_at: Option<f64>,
    event_type: String,
    event_status: String,
}

/// The feed the store holds, in the order `ListInternetEvents` pages over it.
fn feed_snapshot(engine: &Arc<engine::InMemoryEngine>) -> Vec<FeedRecord> {
    let state = service_state(&**engine).expect("the service's own store");
    let store = state.lock();
    store
        .internet_events
        .values()
        .map(|event| FeedRecord {
            event_id: event.event_id.clone(),
            event_arn: event.event_arn.clone(),
            started_at: epoch_seconds(event.started_at),
            ended_at: event.ended_at.map(epoch_seconds),
            event_type: event.event_type.clone(),
            event_status: event.status().to_string(),
        })
        .collect()
}

/// One listing row: the filters and bounds the request carries.
struct FeedCase {
    case: &'static str,
    /// `EventType`, absent for an unfiltered listing.
    event_type: Option<&'static str>,
    /// `EventStatus`, absent for an unfiltered listing.
    event_status: Option<&'static str>,
    /// How far back `StartTime` reaches, absent for an open-ended window.
    window_secs: Option<u64>,
    /// `MaxResults`, absent for a full page.
    max_results: Option<i64>,
}

/// The `EventId`s the feed holds for `case`, bounded by the same window the request
/// carries.
fn expected_ids(feed: &[FeedRecord], case: &FeedCase, now: SystemTime) -> Vec<String> {
    let end = epoch_seconds(now);
    let start = case
        .window_secs
        .map(|secs| end - f64::from(u32::try_from(secs).unwrap_or(u32::MAX)));
    feed.iter()
        .filter(|event| start.is_none_or(|start| event.started_at >= start))
        .filter(|event| event.started_at <= end)
        .filter(|event| case.event_type.is_none_or(|kind| event.event_type == kind))
        .filter(|event| {
            case.event_status
                .is_none_or(|status| event.event_status == status)
        })
        .map(|event| event.event_id.clone())
        .collect()
}

/// The `EventId`s a listing reported, in the order it reported them.
fn listed_ids(listed: &Value) -> Vec<String> {
    summaries(listed)
        .iter()
        .map(|event| {
            event["EventId"]
                .as_str()
                .unwrap_or_else(|| panic!("every summary carries an EventId, got {event}"))
                .to_string()
        })
        .collect()
}

/// The summaries a listing reported.
fn summaries(listed: &Value) -> Vec<Value> {
    listed["InternetEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("InternetEvents must be a list, got {listed}"))
        .clone()
}

#[tokio::test]
async fn the_global_feed_is_populated_for_an_account_that_created_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let now = anchor();
    let feed = feed_snapshot(&engine);

    // The feed is the service's own: it is there before any call, and nothing in this
    // account created it.
    assert_eq!(
        feed.len(),
        SEEDED_INTERNET_EVENTS,
        "the service's feed must hold {SEEDED_INTERNET_EVENTS} events, got {}",
        feed.len()
    );
    let ids: BTreeSet<&str> = feed.iter().map(|event| event.event_id.as_str()).collect();
    assert_eq!(
        ids.len(),
        feed.len(),
        "every EventId must be distinct, got {} ids for {} events",
        ids.len(),
        feed.len()
    );
    let types: BTreeSet<&str> = feed.iter().map(|event| event.event_type.as_str()).collect();
    let statuses: BTreeSet<&str> = feed
        .iter()
        .map(|event| event.event_status.as_str())
        .collect();
    for kind in INTERNET_EVENT_TYPE_VALUES {
        assert!(
            types.contains(kind),
            "case the feed's EventType domain: expected {kind} to be observed, got {types:?}"
        );
    }
    for status in INTERNET_EVENT_STATUS_VALUES {
        assert!(
            statuses.contains(status),
            "case the feed's EventStatus domain: expected {status} to be observed, got {statuses:?}"
        );
    }
    let oldest = epoch_seconds(now) - INTERNET_EVENT_FEED_WINDOW.as_secs_f64();
    for event in &feed {
        assert!(
            event.started_at >= oldest && event.started_at <= epoch_seconds(now),
            "case {:?}: StartedAt {} must lie in the feed's window [{oldest}, {}]",
            event.event_id,
            event.started_at,
            epoch_seconds(now)
        );
    }

    // Rows differ along every dimension the rule generalises over: which type and status
    // are asked for, how wide the window is, and how big a page is.
    let cases = vec![
        FeedCase {
            case: "the whole feed",
            event_type: None,
            event_status: None,
            window_secs: None,
            max_results: None,
        },
        FeedCase {
            case: "a day of AVAILABILITY events, the recorded shape of the call",
            event_type: Some(INTERNET_EVENT_TYPE_VALUES[0]),
            event_status: None,
            window_secs: Some(24 * 60 * 60),
            max_results: Some(10),
        },
        FeedCase {
            case: "a day of PERFORMANCE events",
            event_type: Some(INTERNET_EVENT_TYPE_VALUES[1]),
            event_status: None,
            window_secs: Some(24 * 60 * 60),
            max_results: Some(10),
        },
        FeedCase {
            case: "the events still unresolved",
            event_type: None,
            event_status: Some(INTERNET_EVENT_STATUS_VALUES[0]),
            window_secs: None,
            max_results: None,
        },
        FeedCase {
            case: "the events already resolved",
            event_type: None,
            event_status: Some(INTERNET_EVENT_STATUS_VALUES[1]),
            window_secs: None,
            max_results: None,
        },
        FeedCase {
            case: "status and type together",
            event_type: Some(INTERNET_EVENT_TYPE_VALUES[0]),
            event_status: Some(INTERNET_EVENT_STATUS_VALUES[1]),
            window_secs: None,
            max_results: Some(5),
        },
        FeedCase {
            case: "the last hour only",
            event_type: None,
            event_status: None,
            window_secs: Some(60 * 60),
            max_results: None,
        },
        FeedCase {
            case: "the smallest page",
            event_type: None,
            event_status: None,
            window_secs: None,
            max_results: Some(1),
        },
        FeedCase {
            case: "the largest page",
            event_type: None,
            event_status: None,
            window_secs: None,
            max_results: Some(100),
        },
    ];

    for case in &cases {
        let mut query = vec![format!("EndTime={}", query_time(now))];
        if let Some(secs) = case.window_secs {
            query.push(format!(
                "StartTime={}",
                query_time(now - Duration::from_secs(secs))
            ));
        }
        if let Some(kind) = case.event_type {
            query.push(format!("EventType={kind}"));
        }
        if let Some(status) = case.event_status {
            query.push(format!("EventStatus={status}"));
        }
        if let Some(max) = case.max_results {
            query.push(format!("{MAX_RESULTS_QUERY}={max}"));
        }
        let path = format!("{EVENTS_PATH}?{}", query.join("&"));
        let (status, listed) = call(&router, "GET", &path, None).await;
        let label = case.case;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {listed}"
        );

        let expected = expected_ids(&feed, case, now);
        let page_size = usize::try_from(case.max_results.unwrap_or(100)).unwrap_or(100);
        let expected_page: Vec<String> = expected.iter().take(page_size).cloned().collect();
        let actual = listed_ids(&listed);
        assert_eq!(
            actual, expected_page,
            "case {label:?}: expected {expected_page:?}, got {actual:?} from {listed}"
        );
        assert_eq!(
            member(&listed, "NextToken").is_some(),
            expected.len() > page_size,
            "case {label:?}: the NextToken must be present exactly when a further event \
             remains, got {listed}"
        );

        // Every summary carries the members the model requires, in the shape it requires.
        for summary in summaries(&listed) {
            let event_id = summary["EventId"].as_str().unwrap_or_default().to_string();
            let held = feed
                .iter()
                .find(|event| event.event_id == event_id)
                .unwrap_or_else(|| panic!("case {label:?}: {event_id} is not in the feed"));
            assert_eq!(
                summary["EventArn"],
                Value::String(held.event_arn.clone()),
                "case {label:?}: unexpected EventArn in {summary}"
            );
            assert_eq!(
                summary["EventArn"],
                Value::String(format!(
                    "arn:aws:internetmonitor::000000000000:internet-event/{event_id}"
                )),
                "case {label:?}: an internet event's ARN is global and names the event, \
                 got {summary}"
            );
            let reported_status = summary["EventStatus"].as_str().unwrap_or_default();
            assert!(
                INTERNET_EVENT_STATUS_VALUES.contains(&reported_status),
                "case {label:?}: EventStatus {reported_status:?} is outside the modelled \
                 catalog, got {summary}"
            );
            let reported_type = summary["EventType"].as_str().unwrap_or_default();
            assert!(
                INTERNET_EVENT_TYPE_VALUES.contains(&reported_type),
                "case {label:?}: EventType {reported_type:?} is outside the modelled \
                 catalog, got {summary}"
            );
            assert_eq!(
                member(&summary, "EndedAt").is_some(),
                reported_status == INTERNET_EVENT_STATUS_VALUES[1],
                "case {label:?}: EndedAt is set exactly for an event that has ended, got \
                 {summary}"
            );
            assert!(
                member(&summary, "StartedAt").is_some(),
                "case {label:?}: StartedAt is required, got {summary}"
            );
            assert!(
                member(&summary, "ClientLocation").is_some(),
                "case {label:?}: ClientLocation is required, got {summary}"
            );
            if let Some(kind) = case.event_type {
                assert_eq!(
                    reported_type, kind,
                    "case {label:?}: the EventType filter must be applied, got {summary}"
                );
            }
            if let Some(wanted) = case.event_status {
                assert_eq!(
                    reported_status, wanted,
                    "case {label:?}: the EventStatus filter must be applied, got {summary}"
                );
            }
        }
    }

    // The recorded shape of the call — a day's window, AVAILABILITY only, for an account
    // with no monitor — reports events rather than an empty map.
    let (status, listed) = call(
        &router,
        "GET",
        &format!(
            "{EVENTS_PATH}?StartTime={}&EndTime={}&EventType={}&{MAX_RESULTS_QUERY}=10",
            query_time(now - Duration::from_secs(24 * 60 * 60)),
            query_time(now),
            INTERNET_EVENT_TYPE_VALUES[0]
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "a day of AVAILABILITY events: expected 200, got {status} with {listed}"
    );
    let reported = listed_ids(&listed);
    assert!(
        !reported.is_empty(),
        "a day of AVAILABILITY events must not be empty, got {listed}"
    );

    // A read changes nothing: the same request answers the same way afterwards.
    let (_, again) = call(&router, "GET", EVENTS_PATH, None).await;
    assert_eq!(
        listed_ids(&again),
        feed.iter()
            .map(|event| event.event_id.clone())
            .collect::<Vec<String>>(),
        "a read must not change what a later caller sees, got {again}"
    );
}

#[tokio::test]
async fn the_whole_feed_is_reachable_one_page_at_a_time() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let feed = feed_snapshot(&engine);

    for page_size in [1_usize, 7, 25, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut query = vec![format!("{MAX_RESULTS_QUERY}={page_size}")];
            if let Some(next) = &token {
                query.push(format!("NextToken={}", label(next)));
            }
            let (status, listed) =
                call(&router, "GET", &format!("{EVENTS_PATH}?{}", query.join("&")), None).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status} with {listed}"
            );
            let page = listed_ids(&listed);
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must not exceed it, got {} ids",
                page.len()
            );
            walked.extend(page);
            token = member(&listed, "NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= feed.len() + 1,
                "case page size {page_size}: the walk must terminate, got {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        let expected: Vec<String> = feed.iter().map(|event| event.event_id.clone()).collect();
        assert_eq!(
            walked, expected,
            "case page size {page_size}: the walk must report every event exactly once, in \
             order"
        );
    }
}

#[tokio::test]
async fn every_event_of_the_feed_is_readable_by_id() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let feed = feed_snapshot(&engine);
    assert!(
        feed.len() >= 3,
        "the feed must hold enough events to sample, got {}",
        feed.len()
    );

    // The first, a middle and the last event of the feed: each reads back with exactly
    // the members the listing reported for it.
    let samples = [0, feed.len() / 2, feed.len() - 1];
    let (_, listed) = call(
        &router,
        "GET",
        &format!("{EVENTS_PATH}?{MAX_RESULTS_QUERY}=100"),
        None,
    )
    .await;
    let page = summaries(&listed);
    for index in samples {
        let held = &feed[index];
        let path = format!("{EVENTS_PATH}/{}", label(&held.event_id));
        let (status, got) = call(&router, "GET", &path, None).await;
        let case = &held.event_id;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {got}"
        );
        assert_eq!(
            got["EventId"],
            Value::String(held.event_id.clone()),
            "case {case:?}: unexpected EventId in {got}"
        );
        assert_eq!(
            got["EventArn"],
            Value::String(held.event_arn.clone()),
            "case {case:?}: unexpected EventArn in {got}"
        );
        assert_eq!(
            got["EventStatus"],
            Value::String(held.event_status.clone()),
            "case {case:?}: unexpected EventStatus in {got}"
        );
        assert_eq!(
            got["EventType"],
            Value::String(held.event_type.clone()),
            "case {case:?}: unexpected EventType in {got}"
        );
        assert_eq!(
            member(&got, "EndedAt").is_some(),
            held.ended_at.is_some(),
            "case {case:?}: EndedAt is set exactly for an event that has ended, got {got}"
        );

        // The two readers of one feed agree member for member.
        if let Some(summary) = page
            .iter()
            .find(|event| event["EventId"] == Value::String(held.event_id.clone()))
        {
            for field in ["EventArn", "StartedAt", "ClientLocation", "EventType", "EventStatus"] {
                assert_eq!(
                    got[field], summary[field],
                    "case {case:?}: GetInternetEvent and ListInternetEvents must report the \
                     same {field}, got {got} and {summary}"
                );
            }
        }

        // A read changes nothing.
        let (status, twice) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a second read expected 200, got {status} with {twice}"
        );
        assert_eq!(
            twice, got,
            "case {case:?}: a read must not change what a later caller sees"
        );
    }

    // An id the feed holds no event for is refused, named by its own resource path — one
    // past the feed's last event included.
    for unknown in [
        "no-such-event".to_string(),
        seed_event_id(SEEDED_INTERNET_EVENTS),
        "0".to_string(),
    ] {
        let (status, refused) = call(
            &router,
            "GET",
            &format!("{EVENTS_PATH}/{}", label(&unknown)),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {unknown:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {unknown:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            invalid_resource_id_message(&internet_event_resource_path(&unknown)),
            "case {unknown:?}: unexpected message in {refused}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// One wording for a window that selects no instant
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_window_whose_start_is_after_its_end_is_refused_with_one_wording() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_monitor(&router, json!({ "MonitorName": MONITOR })).await;
    let now = anchor();

    /// One refusal row: the path the window goes on, and how far the end precedes the
    /// start.
    struct WindowCase {
        case: &'static str,
        path: String,
        /// Seconds by which `EndTime` precedes `StartTime`.
        inverted_by: u64,
    }

    let paths = [
        ("ListInternetEvents", EVENTS_PATH.to_string()),
        (
            "ListHealthEvents",
            format!("/v20210603/Monitors/{MONITOR}/HealthEvents"),
        ),
    ];

    // Rows differ in the operation that carries the window and in how far the bounds are
    // inverted — one second is as impossible a window as a day.
    let mut cases: Vec<WindowCase> = Vec::new();
    for (operation, path) in &paths {
        for inverted_by in [1_u64, 60, 3_600, 86_400] {
            cases.push(WindowCase {
                case: operation,
                path: path.clone(),
                inverted_by,
            });
        }
    }

    for WindowCase {
        case,
        path,
        inverted_by,
    } in cases
    {
        let query = format!(
            "StartTime={}&EndTime={}",
            query_time(now),
            query_time(now - Duration::from_secs(inverted_by))
        );
        let (status, refused) = call(&router, "GET", &format!("{path}?{query}"), None).await;
        assert_eq!(
            status, 400,
            "case {case} inverted by {inverted_by}s: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case} inverted by {inverted_by}s: expected ValidationException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            INVALID_TIME_WINDOW_MESSAGE,
            "case {case} inverted by {inverted_by}s: unexpected message in {refused}"
        );
    }

    // The positive controls: a window that holds an instant — including one of a single
    // instant, and one with a single bound — is served.
    for (operation, path) in &paths {
        for (control, query) in [
            (
                "start before end",
                format!(
                    "StartTime={}&EndTime={}",
                    query_time(now - Duration::from_secs(3_600)),
                    query_time(now)
                ),
            ),
            (
                "one instant",
                format!(
                    "StartTime={}&EndTime={}",
                    query_time(now),
                    query_time(now)
                ),
            ),
            ("a start and no end", format!("StartTime={}", query_time(now - Duration::from_secs(60)))),
            ("an end and no start", format!("EndTime={}", query_time(now))),
        ] {
            let (status, served) = call(&router, "GET", &format!("{path}?{query}"), None).await;
            assert_eq!(
                status, 200,
                "case {operation} with {control}: expected 200, got {status} with {served}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// GetQueryResults
// ---------------------------------------------------------------------------------------

/// One measured client location the store holds, and the values a projected row must then
/// carry for it.
struct Measured {
    as_name: &'static str,
    as_number: i64,
    city: &'static str,
    availability: Option<f64>,
    performance: Option<f64>,
    rtt_p50: Option<f64>,
}

/// The locations the seeded health events carry: rows differing in which measurements they
/// hold, so a projection that invents a value or drops one is caught.
const MEASURED: &[Measured] = &[
    Measured {
        as_name: "AMAZON-02",
        as_number: 16509,
        city: "Seattle",
        availability: Some(98.5),
        performance: Some(95.25),
        rtt_p50: Some(42.0),
    },
    Measured {
        as_name: "EIRCOM",
        as_number: 5466,
        city: "Dublin",
        availability: Some(80.0),
        performance: None,
        rtt_p50: None,
    },
    Measured {
        as_name: "NTT-COMMUNICATIONS",
        as_number: 2914,
        city: "Tokyo",
        availability: None,
        performance: Some(70.125),
        rtt_p50: Some(180.5),
    },
    Measured {
        as_name: "BHARTI-AIRTEL",
        as_number: 9498,
        city: "Mumbai",
        availability: Some(99.0),
        performance: Some(88.0),
        rtt_p50: Some(96.25),
    },
];

/// The stored form of one [`Measured`] location.
fn impacted(measured: &Measured) -> ImpactedLocation {
    ImpactedLocation {
        as_name: measured.as_name.to_string(),
        as_number: measured.as_number,
        country: "Global".to_string(),
        city: Some(measured.city.to_string()),
        status: "ACTIVE".to_string(),
        internet_health: Some(InternetHealth {
            availability: measured.availability.map(|score| AvailabilityMeasurement {
                experience_score: Some(score),
                percent_of_total_traffic_impacted: Some(1.5),
                percent_of_client_location_impacted: Some(2.5),
            }),
            performance: measured.performance.map(|score| PerformanceMeasurement {
                experience_score: Some(score),
                percent_of_total_traffic_impacted: Some(3.5),
                percent_of_client_location_impacted: Some(4.5),
                round_trip_time: measured.rtt_p50.map(|p50| RoundTripTime {
                    p50: Some(p50),
                    p90: Some(p50 + 10.0),
                    p95: Some(p50 + 20.0),
                }),
            }),
        }),
        ..ImpactedLocation::default()
    }
}

/// One seeded health event: how long before the anchor it was measured, and which
/// [`MEASURED`] locations it carries.
struct EventSeed {
    event_id: &'static str,
    measured_secs_ago: u64,
    locations: &'static [usize],
}

/// The seeded events: one inside every window a case uses, one only inside the wider ones,
/// and one location measured twice so a per-location report has something to collapse.
const EVENT_SEEDS: &[EventSeed] = &[
    EventSeed {
        event_id: "AWS-event-0001",
        measured_secs_ago: 3_600,
        locations: &[0, 1],
    },
    EventSeed {
        event_id: "AWS-event-0002",
        measured_secs_ago: 600,
        locations: &[0, 2],
    },
    EventSeed {
        event_id: "AWS-event-0003",
        measured_secs_ago: 7_200,
        locations: &[3],
    },
];

/// An emulator holding [`MONITOR`] with a resource and the measurements of
/// [`EVENT_SEEDS`], plus the engine the cases seed queries into and the anchor the seeded
/// instants are relative to.
async fn emulator_with_measurements() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, SystemTime)
{
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_monitor(
        &router,
        json!({ "MonitorName": MONITOR, "Resources": [VPC] }),
    )
    .await;

    let at = anchor();
    let state = service_state(&*engine).expect("the service's own store");
    for seed in EVENT_SEEDS {
        let measured_at = at - Duration::from_secs(seed.measured_secs_ago);
        let mut record =
            HealthEventRecord::new(REGION, MONITOR, seed.event_id, "AVAILABILITY", measured_at);
        record.impacted_locations = seed
            .locations
            .iter()
            .map(|index| impacted(&MEASURED[*index]))
            .collect();
        let stored = state.lock().record_health_event(record);
        assert!(
            stored,
            "seeding {}: the store refused the record",
            seed.event_id
        );
    }
    (router, engine, at)
}

/// Seed a query of [`MONITOR`] whose clock-owed transitions are already due (so it reports
/// `SUCCEEDED`) unless `pending`, and hand back its `QueryId`.
fn seed_query(
    engine: &Arc<engine::InMemoryEngine>,
    query_id: &str,
    query_type: &str,
    window: (SystemTime, SystemTime),
    filters: Vec<FilterParameter>,
    pending: bool,
) -> String {
    let state = service_state(&**engine).expect("the service's own store");
    let now = SystemTime::now();
    let mut record = QueryRecord::new(
        query_id,
        MONITOR,
        query_type,
        epoch_seconds(window.0),
        epoch_seconds(window.1),
        filters,
        now,
    );
    if !pending {
        // The transitions the clock owes are already due, so the first read settles the
        // query into its terminal status rather than this test writing that status.
        record.running_at = Some(now - Duration::from_secs(60));
        record.succeeded_at = Some(now - Duration::from_secs(30));
    }
    state.lock().record_query(record);
    query_id.to_string()
}

/// `GET /Monitors/{monitor}/Queries/{query_id}/Results` with `query` appended.
async fn query_results(
    router: &MultiServiceRouter,
    monitor: &str,
    query_id: &str,
    query: &str,
) -> (u16, Value) {
    let path = format!(
        "/v20210603/Monitors/{monitor}/Queries/{}/Results",
        label(query_id)
    );
    let path = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, "GET", &path, None).await
}

/// The `Fields` of a result, as `Name` -> column index.
fn field_index(result: &Value) -> BTreeMap<String, usize> {
    result["Fields"]
        .as_array()
        .unwrap_or_else(|| panic!("Fields must be a list, got {result}"))
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let name = field["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("every field carries a Name, got {field}"))
                .to_string();
            assert!(
                field["Type"].as_str().is_some_and(|kind| !kind.is_empty()),
                "every field carries a Type, got {field}"
            );
            (name, index)
        })
        .collect()
}

/// The `Data` rows of a result.
fn rows(result: &Value) -> Vec<Vec<String>> {
    result["Data"]
        .as_array()
        .unwrap_or_else(|| panic!("Data must be a list, got {result}"))
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or_else(|| panic!("every row is a list, got {row}"))
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .collect()
}

/// One column of every row, in the order the rows were reported.
fn column(result: &Value, name: &str) -> Vec<String> {
    let index = *field_index(result)
        .get(name)
        .unwrap_or_else(|| panic!("the repository must carry a {name} column, got {result}"));
    rows(result)
        .into_iter()
        .map(|row| row[index].clone())
        .collect()
}

#[tokio::test]
async fn get_query_results_refuses_a_query_id_the_named_monitor_has_no_query_for() {
    let (router, engine, at) = emulator_with_measurements().await;
    create_monitor(&router, json!({ "MonitorName": "other-monitor", "Resources": [VPC] })).await;
    let window = (at - Duration::from_secs(5_400), at);
    let mine = seed_query(
        &engine,
        "11111111-1111-4111-8111-111111111111",
        QUERY_TYPE_VALUES[0],
        window,
        Vec::new(),
        false,
    );

    // Rows differ in WHY the id resolves nothing: never minted, minted for another
    // monitor, asked for under a monitor this account does not hold, and not an id form
    // this service mints at all.
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "a syntactically valid QueryId nothing minted",
            MONITOR,
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "a QueryId of another monitor's query",
            "other-monitor",
            mine.clone(),
        ),
        (
            "a monitor this account does not hold",
            "no-such-monitor",
            mine.clone(),
        ),
        ("an id no form of this service mints", MONITOR, "not-a-query-id".to_string()),
        ("the empty-looking id", MONITOR, "0".to_string()),
    ];

    for (case, monitor, query_id) in cases {
        let (status, refused) = query_results(&router, monitor, &query_id, "").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            INVALID_QUERY_ID_MESSAGE,
            "case {case:?}: unexpected message in {refused}"
        );
    }

    // The query that WAS minted is served, so the refusals above are about the id and not
    // about the operation.
    let (status, served) = query_results(&router, MONITOR, &mine, "").await;
    assert_eq!(
        status, 200,
        "the minted QueryId: expected 200, got {status} with {served}"
    );
}

#[tokio::test]
async fn get_query_results_reports_the_measurements_the_store_holds() {
    let (router, engine, at) = emulator_with_measurements().await;

    /// One query row: the type it was started with, the window it covers, and the cities
    /// its rows must carry — in order.
    struct ResultCase {
        case: &'static str,
        query_type: &'static str,
        window_secs: u64,
        filters: Vec<FilterParameter>,
        expected_cities: Vec<&'static str>,
    }

    let cases = vec![
        ResultCase {
            case: "every measurement in a 90-minute window",
            query_type: "MEASUREMENTS",
            window_secs: 5_400,
            filters: Vec::new(),
            expected_cities: vec!["Seattle", "Dublin", "Seattle", "Tokyo"],
        },
        ResultCase {
            case: "a narrower window drops the older measurements",
            query_type: "MEASUREMENTS",
            window_secs: 900,
            filters: Vec::new(),
            expected_cities: vec!["Seattle", "Tokyo"],
        },
        ResultCase {
            case: "a window wide enough for every measurement",
            query_type: "MEASUREMENTS",
            window_secs: 86_400,
            filters: Vec::new(),
            expected_cities: vec!["Seattle", "Dublin", "Seattle", "Tokyo", "Mumbai"],
        },
        ResultCase {
            case: "a window that holds no measurement",
            query_type: "MEASUREMENTS",
            window_secs: 60,
            filters: Vec::new(),
            expected_cities: Vec::new(),
        },
        ResultCase {
            case: "one row per location, ordered by the location's identity",
            query_type: "TOP_LOCATIONS",
            window_secs: 5_400,
            filters: Vec::new(),
            expected_cities: vec!["Tokyo", "Dublin", "Seattle"],
        },
        ResultCase {
            case: "the detailed location report collapses the same way",
            query_type: "TOP_LOCATION_DETAILS",
            window_secs: 86_400,
            filters: Vec::new(),
            expected_cities: vec!["Tokyo", "Dublin", "Mumbai", "Seattle"],
        },
        ResultCase {
            case: "an equality filter keeps the rows of the city it names",
            query_type: "MEASUREMENTS",
            window_secs: 86_400,
            filters: vec![FilterParameter {
                field: Some("city".to_string()),
                operator: Some(OPERATOR_VALUES[0].to_string()),
                values: Some(vec!["Seattle".to_string()]),
            }],
            expected_cities: vec!["Seattle", "Seattle"],
        },
        ResultCase {
            case: "a negated filter keeps exactly the rest",
            query_type: "MEASUREMENTS",
            window_secs: 86_400,
            filters: vec![FilterParameter {
                field: Some("city".to_string()),
                operator: Some(OPERATOR_VALUES[1].to_string()),
                values: Some(vec!["Seattle".to_string()]),
            }],
            expected_cities: vec!["Dublin", "Tokyo", "Mumbai"],
        },
        ResultCase {
            case: "a filter listing several values keeps them all",
            query_type: "MEASUREMENTS",
            window_secs: 86_400,
            filters: vec![FilterParameter {
                field: Some("city".to_string()),
                operator: Some(OPERATOR_VALUES[0].to_string()),
                values: Some(vec!["Dublin".to_string(), "Mumbai".to_string()]),
            }],
            expected_cities: vec!["Dublin", "Mumbai"],
        },
        ResultCase {
            case: "two filters are both applied",
            query_type: "MEASUREMENTS",
            window_secs: 86_400,
            filters: vec![
                FilterParameter {
                    field: Some("country".to_string()),
                    operator: Some(OPERATOR_VALUES[0].to_string()),
                    values: Some(vec!["Global".to_string()]),
                },
                FilterParameter {
                    field: Some("as_number".to_string()),
                    operator: Some(OPERATOR_VALUES[0].to_string()),
                    values: Some(vec!["2914".to_string()]),
                },
            ],
            expected_cities: vec!["Tokyo"],
        },
        ResultCase {
            case: "a filter on a column the repository does not carry keeps nothing",
            query_type: "MEASUREMENTS",
            window_secs: 86_400,
            filters: vec![FilterParameter {
                field: Some("not_a_column".to_string()),
                operator: Some(OPERATOR_VALUES[0].to_string()),
                values: Some(vec!["anything".to_string()]),
            }],
            expected_cities: Vec::new(),
        },
        ResultCase {
            case: "the suggestion reports have no rows here",
            query_type: "OVERALL_TRAFFIC_SUGGESTIONS",
            window_secs: 86_400,
            filters: Vec::new(),
            expected_cities: Vec::new(),
        },
        ResultCase {
            case: "the routing suggestions have no rows here",
            query_type: "ROUTING_SUGGESTIONS",
            window_secs: 86_400,
            filters: Vec::new(),
            expected_cities: Vec::new(),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let query_id = format!("22222222-2222-4222-8222-{index:012}");
        seed_query(
            &engine,
            &query_id,
            case.query_type,
            (at - Duration::from_secs(case.window_secs), at),
            case.filters.clone(),
            false,
        );
        let (status, result) = query_results(&router, MONITOR, &query_id, "").await;
        let label = case.case;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {result}"
        );
        // Every result reports the repository's columns, whether or not it found rows.
        let fields = field_index(&result);
        assert!(
            fields.contains_key("city") && fields.contains_key("availability_score"),
            "case {label:?}: the repository's columns must be reported, got {result}"
        );
        for row in rows(&result) {
            assert_eq!(
                row.len(),
                fields.len(),
                "case {label:?}: every row must align with Fields, got {row:?} for {} columns",
                fields.len()
            );
        }
        let cities = column(&result, "city");
        assert_eq!(
            cities,
            case.expected_cities
                .iter()
                .map(|city| (*city).to_string())
                .collect::<Vec<String>>(),
            "case {label:?}: unexpected rows in {result}"
        );
        assert!(
            member(&result, "NextToken").is_none(),
            "case {label:?}: a single full page carries no NextToken, got {result}"
        );
    }

    // The values of a row are the stored measurement's, not placeholders: the oldest
    // window's first row is the Seattle measurement of the first seeded event.
    let query_id = seed_query(
        &engine,
        "33333333-3333-4333-8333-333333333333",
        "MEASUREMENTS",
        (at - Duration::from_secs(5_400), at),
        vec![FilterParameter {
            field: Some("as_number".to_string()),
            operator: Some(OPERATOR_VALUES[0].to_string()),
            values: Some(vec![MEASURED[0].as_number.to_string()]),
        }],
        false,
    );
    let (status, result) = query_results(&router, MONITOR, &query_id, "").await;
    assert_eq!(
        status, 200,
        "the Seattle rows: expected 200, got {status} with {result}"
    );
    assert_eq!(
        column(&result, "availability_score"),
        vec!["98.5".to_string(), "98.5".to_string()],
        "the availability score must be the stored measurement's, got {result}"
    );
    assert_eq!(
        column(&result, "performance_score"),
        vec!["95.25".to_string(), "95.25".to_string()],
        "the performance score must be the stored measurement's, got {result}"
    );
    assert_eq!(
        column(&result, "rtt_p50"),
        vec!["42".to_string(), "42".to_string()],
        "the round-trip time must be the stored measurement's, got {result}"
    );
    assert_eq!(
        column(&result, "as_name"),
        vec![
            MEASURED[0].as_name.to_string(),
            MEASURED[0].as_name.to_string()
        ],
        "the network name must be the stored location's, got {result}"
    );
    assert_eq!(
        column(&result, "timestamp"),
        vec![
            wire_time(at - Duration::from_secs(EVENT_SEEDS[0].measured_secs_ago)),
            wire_time(at - Duration::from_secs(EVENT_SEEDS[1].measured_secs_ago)),
        ],
        "each row must carry the instant its measurement was taken, got {result}"
    );

    // A measurement the store leaves unset is reported as no value, never as an invented
    // one: Dublin carries no performance measurement.
    let query_id = seed_query(
        &engine,
        "44444444-4444-4444-8444-444444444444",
        "MEASUREMENTS",
        (at - Duration::from_secs(5_400), at),
        vec![FilterParameter {
            field: Some("city".to_string()),
            operator: Some(OPERATOR_VALUES[0].to_string()),
            values: Some(vec![MEASURED[1].city.to_string()]),
        }],
        false,
    );
    let (status, result) = query_results(&router, MONITOR, &query_id, "").await;
    assert_eq!(
        status, 200,
        "the Dublin row: expected 200, got {status} with {result}"
    );
    assert_eq!(
        column(&result, "availability_score"),
        vec!["80".to_string()],
        "the stored availability score must be reported, got {result}"
    );
    assert_eq!(
        column(&result, "performance_score"),
        vec![String::new()],
        "a measurement the store does not hold must be reported as no value, got {result}"
    );

    // Every modelled QueryType is answered — no type falls through to a refusal or a 501.
    for (index, query_type) in QUERY_TYPE_VALUES.iter().enumerate() {
        let query_id = format!("55555555-5555-4555-8555-{index:012}");
        seed_query(
            &engine,
            &query_id,
            query_type,
            (at - Duration::from_secs(86_400), at),
            Vec::new(),
            false,
        );
        let (status, result) = query_results(&router, MONITOR, &query_id, "").await;
        assert_eq!(
            status, 200,
            "case {query_type:?}: expected 200, got {status} with {result}"
        );
        assert!(
            !field_index(&result).is_empty(),
            "case {query_type:?}: a repository reports its columns, got {result}"
        );
    }
}

#[tokio::test]
async fn a_query_that_has_not_completed_reports_no_rows_yet() {
    let (router, engine, at) = emulator_with_measurements().await;
    let query_id = seed_query(
        &engine,
        "66666666-6666-4666-8666-666666666666",
        "MEASUREMENTS",
        (at - Duration::from_secs(86_400), at),
        Vec::new(),
        true,
    );

    let (status, result) = query_results(&router, MONITOR, &query_id, "").await;
    assert_eq!(
        status, 200,
        "a query still in progress: expected 200, got {status} with {result}"
    );
    assert!(
        !field_index(&result).is_empty(),
        "a query still in progress reports the columns it asked for, got {result}"
    );
    assert!(
        rows(&result).is_empty(),
        "a query still in progress has produced no rows, got {result}"
    );

    // And the status the poller sees is the non-terminal one, so the empty result is not a
    // claim that the query finished.
    let (status, polled) = call(
        &router,
        "GET",
        &format!(
            "/v20210603/Monitors/{MONITOR}/Queries/{}/Status",
            label(&query_id)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "polling the query: expected 200, got {status} with {polled}"
    );
    assert_eq!(
        polled["Status"],
        Value::String(QUERY_STATUS_QUEUED.to_string()),
        "a freshly started query is QUEUED, got {polled}"
    );

    // A completed query over the same window does report rows, so the difference above is
    // the query's status and nothing else.
    let completed = seed_query(
        &engine,
        "77777777-7777-4777-8777-777777777777",
        "MEASUREMENTS",
        (at - Duration::from_secs(86_400), at),
        Vec::new(),
        false,
    );
    let (status, result) = query_results(&router, MONITOR, &completed, "").await;
    assert_eq!(
        status, 200,
        "the completed query: expected 200, got {status} with {result}"
    );
    assert!(
        !rows(&result).is_empty(),
        "the completed query must report the stored measurements, got {result}"
    );
    let (_, polled) = call(
        &router,
        "GET",
        &format!(
            "/v20210603/Monitors/{MONITOR}/Queries/{}/Status",
            label(&completed)
        ),
        None,
    )
    .await;
    assert_eq!(
        polled["Status"],
        Value::String(QUERY_STATUS_SUCCEEDED.to_string()),
        "the completed query must report SUCCEEDED, got {polled}"
    );
}

#[tokio::test]
async fn the_rows_of_a_query_are_reachable_one_page_at_a_time() {
    let (router, engine, at) = emulator_with_measurements().await;
    let query_id = seed_query(
        &engine,
        "88888888-8888-4888-8888-888888888888",
        "MEASUREMENTS",
        (at - Duration::from_secs(86_400), at),
        Vec::new(),
        false,
    );

    let (_, whole) = query_results(&router, MONITOR, &query_id, "").await;
    let every_row = rows(&whole);
    assert!(
        every_row.len() >= 4,
        "the seeded measurements must give the walk several rows, got {}",
        every_row.len()
    );

    for page_size in [1_usize, 2, 1_000] {
        let mut walked: Vec<Vec<String>> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut query = vec![format!("MaxResults={page_size}")];
            if let Some(next) = &token {
                query.push(format!("NextToken={}", label(next)));
            }
            let (status, page) =
                query_results(&router, MONITOR, &query_id, &query.join("&")).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status} with {page}"
            );
            let reported = rows(&page);
            assert!(
                reported.len() <= page_size,
                "case page size {page_size}: a page must not exceed it, got {} rows",
                reported.len()
            );
            walked.extend(reported);
            token = member(&page, "NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= every_row.len() + 1,
                "case page size {page_size}: the walk must terminate, got {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, every_row,
            "case page size {page_size}: the walk must report every row exactly once, in order"
        );
    }

    // A token this service did not mint is a bad request, not an empty page.
    let (status, refused) =
        query_results(&router, MONITOR, &query_id, "NextToken=not-a-token").await;
    assert_eq!(
        status, 400,
        "an unminted NextToken: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "an unminted NextToken: expected ValidationException, got {refused}"
    );

    // `MaxResults` is modelled 1..1000: each published bound is served and one past it is
    // refused by the boundary validator
    // (`smithy_query(query="operation",operation="GetQueryResults")`).
    for (case, value, expected) in [
        ("one below the minimum", 0, 400),
        ("the minimum", 1, 200),
        ("the maximum", 1_000, 200),
        ("one past the maximum", 1_001, 400),
    ] {
        let (status, body) =
            query_results(&router, MONITOR, &query_id, &format!("MaxResults={value}")).await;
        assert_eq!(
            status, expected,
            "case {case:?} (MaxResults={value}): expected {expected}, got {status} with {body}"
        );
    }
}
