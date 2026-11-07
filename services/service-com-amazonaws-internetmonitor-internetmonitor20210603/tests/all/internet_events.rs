// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInternetEvents`.
//!
//! The rule under test: the operation reports one `InternetEventSummary` per internet
//! event the service holds, in `EventId` order, keeping exactly those whose START instant
//! lies in the requested time frame (`StartTime` inclusive, `EndTime` inclusive and
//! defaulting to the current time) and whose status and type match the `EventStatus` /
//! `EventType` filters when given, paged by `MaxResults` with an opaque `NextToken`; it
//! refuses a filter value outside the modelled enum, a token it did not mint and a time
//! frame whose start is after its end, and it writes nothing.
//!
//! Internet events are the service's own observations of the public internet and no
//! modelled operation creates one, so this emulator observes none and invents none: each
//! case seeds the records it reports on into the service's own store (the engine store a
//! handler reaches through `state::service_state`) and then drives real HTTP through the
//! same `MultiServiceRouter` the emulator binary mounts.
//!
//! The wire helpers (`call`, `label`, `member`, `timestamp_secs`) live in
//! `crate::monitor_lifecycle` and are reused here rather than copied.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_internetmonitor_internetmonitor20210603::state::{
    service_state, InternetEventRecord, INTERNET_EVENT_STATUS_ACTIVE,
    INTERNET_EVENT_STATUS_RESOLVED,
};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::monitor_lifecycle::{call, label, member, timestamp_secs};

/// An internet event's ARN, whose region segment is EMPTY: an internet event is a global
/// observation, not a resource of the region the request was signed for.
fn expected_event_arn(event_id: &str) -> String {
    format!("arn:aws:internetmonitor::000000000000:internet-event/{event_id}")
}

/// `ListInternetEvents`' route (`smithy_query(query="operation",operation="ListInternetEvents")`).
const EVENTS_PATH: &str = "/v20210603/InternetEvents";

/// `MaxResults`' own `@httpQuery` name, which is NOT the member name
/// (`smithy_query(query="operation",operation="ListInternetEvents")`).
const MAX_RESULTS_QUERY: &str = "InternetEventMaxResults";

/// The members `InternetEventType` declares
/// (`smithy_query(query="shape",shape="InternetEventType")`).
const EVENT_TYPE_AVAILABILITY: &str = "AVAILABILITY";
/// See [`EVENT_TYPE_AVAILABILITY`].
const EVENT_TYPE_PERFORMANCE: &str = "PERFORMANCE";

/// `InternetEventMaxResults`' published range, 1..100
/// (`smithy_query(query="shape",shape="InternetEventMaxResults")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 100;

/// One emulator whose internet-event feed is exactly `events`.
///
/// `crate::monitor_lifecycle::emulator` drops the engine it builds, and a seeded case
/// needs it. The store comes with the service's own feed (an internet event is an
/// observation the service makes, so no account starts without one, and
/// `crate::feed_and_query_results` covers it); a case whose rows name the exact set a
/// listing must report replaces that feed with its own fixture, so what it asserts is its
/// own records and nothing else.
fn emulator_holding(events: &[InternetEventRecord]) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = service_state(&*engine).expect("the service's own store");
        let mut store = state.lock();
        store.internet_events.clear();
        for event in events {
            store.record_internet_event(event.clone());
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// `SystemTime::now()` truncated to a whole second, so a fixture instant survives the
/// RFC 3339 round trip through the query string exactly.
fn base_now() -> SystemTime {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// `base` shifted by `offset` seconds, negative into the past.
fn shifted(base: SystemTime, offset: i64) -> SystemTime {
    if offset >= 0 {
        base + Duration::from_secs(offset.unsigned_abs())
    } else {
        base - Duration::from_secs(offset.unsigned_abs())
    }
}

/// An instant as the `date-time` (RFC 3339) query value `@timestampFormat` asks for, ready
/// to go into a query string.
fn query_time(at: SystemTime) -> String {
    let formatted = ::aws_smithy_types::DateTime::from_secs_f64(
        at.duration_since(SystemTime::UNIX_EPOCH)
            .expect("the test clock is after the Unix epoch")
            .as_secs_f64(),
    )
    .fmt(::aws_smithy_types::date_time::Format::DateTime)
    .expect("a representable date-time");
    label(&formatted)
}

/// A distinct impacted location per fixture event, so a case can tell one summary's
/// `ClientLocation` from another's.
fn location(city: &str, country: &str, as_number: i64) -> ClientLocationFixture {
    ClientLocationFixture {
        city: city.to_string(),
        country: country.to_string(),
        as_number,
    }
}

/// The parts of a `ClientLocation` a fixture varies; the rest is filled in by
/// [`ClientLocationFixture::into_modelled`].
struct ClientLocationFixture {
    city: String,
    country: String,
    as_number: i64,
}

impl ClientLocationFixture {
    fn into_modelled(
        self,
    ) -> service_com_amazonaws_internetmonitor_internetmonitor20210603::ops::types::ClientLocation
    {
        service_com_amazonaws_internetmonitor_internetmonitor20210603::ops::types::ClientLocation {
            as_name: format!("AS{} Network", self.as_number),
            as_number: self.as_number,
            country: self.country,
            subdivision: Some(format!("{}-subdivision", self.city)),
            metro: Some(format!("{} metro", self.city)),
            city: self.city,
            latitude: 47.5,
            longitude: -122.3,
        }
    }
}

/// One fixture event: an `EventId`, when it started and (if it has) ended relative to the
/// case's `base`, its type and its location.
struct EventFixture {
    event_id: &'static str,
    started_offset: i64,
    ended_offset: Option<i64>,
    event_type: &'static str,
    location: ClientLocationFixture,
}

/// The fixture set every filter row is evaluated against: both types, both statuses
/// (`RESOLVED` is exactly "has an `EndedAt`"), start instants hours and a day apart, and
/// one event whose start is in the FUTURE, which the default end time (now) excludes.
fn fixture_events(base: SystemTime) -> Vec<InternetEventRecord> {
    vec![
        EventFixture {
            event_id: "ev-availability-active",
            started_offset: -3600,
            ended_offset: None,
            event_type: EVENT_TYPE_AVAILABILITY,
            location: location("Seattle", "United States", 16509),
        },
        EventFixture {
            event_id: "ev-availability-resolved",
            started_offset: -86_400,
            ended_offset: Some(-80_000),
            event_type: EVENT_TYPE_AVAILABILITY,
            location: location("Dublin", "Ireland", 5466),
        },
        EventFixture {
            event_id: "ev-future-start",
            started_offset: 3600,
            ended_offset: None,
            event_type: EVENT_TYPE_PERFORMANCE,
            location: location("Sydney", "Australia", 1221),
        },
        EventFixture {
            event_id: "ev-performance-active",
            started_offset: -600,
            ended_offset: None,
            event_type: EVENT_TYPE_PERFORMANCE,
            location: location("Tokyo", "Japan", 2497),
        },
        EventFixture {
            event_id: "ev-performance-resolved",
            started_offset: -7200,
            ended_offset: Some(-3600),
            event_type: EVENT_TYPE_PERFORMANCE,
            location: location("Mumbai", "India", 9498),
        },
    ]
    .into_iter()
    .map(|fixture| {
        InternetEventRecord::new(
            fixture.event_id,
            shifted(base, fixture.started_offset),
            fixture.ended_offset.map(|offset| shifted(base, offset)),
            fixture.location.into_modelled(),
            fixture.event_type,
        )
    })
    .collect()
}

/// The `EventId`s a listing reported, as a set, so a row asserts membership rather than
/// an order it does not own.
fn listed_ids(listed: &Value) -> BTreeSet<String> {
    listed["InternetEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("InternetEvents must be a list, got {listed}"))
        .iter()
        .map(|event| {
            event["EventId"]
                .as_str()
                .unwrap_or_else(|| panic!("every summary carries an EventId, got {event}"))
                .to_string()
        })
        .collect()
}

/// `ids` as a set, for a row's expectation.
fn id_set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

#[tokio::test]
async fn an_emulator_whose_feed_holds_nothing_lists_no_internet_events() {
    // The reporting path invents nothing: an internet-event store that holds no record
    // reports an empty list, under every filter and page size.
    let router = emulator_holding(&[]);

    let (status, listed) = call(&router, "GET", EVENTS_PATH, None).await;
    assert_eq!(
        status, 200,
        "an empty store: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        listed["InternetEvents"],
        Value::Array(Vec::new()),
        "an empty store must report an empty InternetEvents list, got {listed}"
    );
    assert!(
        member(&listed, "NextToken").is_none(),
        "an empty store must report no NextToken, got {listed}"
    );

    // Every filter and page size answers the same way over an empty store, and none of
    // them is refused.
    for (case, query) in [
        ("a status filter", format!("EventStatus={INTERNET_EVENT_STATUS_ACTIVE}")),
        ("a type filter", format!("EventType={EVENT_TYPE_AVAILABILITY}")),
        (
            "a time frame",
            format!(
                "StartTime={}&EndTime={}",
                query_time(shifted(base_now(), -86_400)),
                query_time(base_now())
            ),
        ),
        ("the smallest page", format!("{MAX_RESULTS_QUERY}={MAX_RESULTS_MIN}")),
    ] {
        let (status, empty) = call(&router, "GET", &format!("{EVENTS_PATH}?{query}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {empty}"
        );
        assert_eq!(
            empty["InternetEvents"],
            Value::Array(Vec::new()),
            "case {case:?}: expected an empty list, got {empty}"
        );
        assert!(
            member(&empty, "NextToken").is_none(),
            "case {case:?}: expected no NextToken, got {empty}"
        );
    }
}

#[tokio::test]
async fn every_filter_keeps_exactly_the_events_it_names() {
    let base = base_now();
    let events = fixture_events(base);
    let router = emulator_holding(&events);

    /// One filter row: the query string the request carries and the `EventId`s that must
    /// come back.
    struct FilterCase {
        case: &'static str,
        query: String,
        expected: BTreeSet<String>,
    }

    let cases = vec![
        FilterCase {
            case: "no filter: every event whose start is not in the future",
            query: String::new(),
            expected: id_set(&[
                "ev-availability-active",
                "ev-availability-resolved",
                "ev-performance-active",
                "ev-performance-resolved",
            ]),
        },
        FilterCase {
            case: "EventStatus=ACTIVE",
            query: format!("EventStatus={INTERNET_EVENT_STATUS_ACTIVE}"),
            expected: id_set(&["ev-availability-active", "ev-performance-active"]),
        },
        FilterCase {
            case: "EventStatus=RESOLVED",
            query: format!("EventStatus={INTERNET_EVENT_STATUS_RESOLVED}"),
            expected: id_set(&["ev-availability-resolved", "ev-performance-resolved"]),
        },
        FilterCase {
            case: "EventType=AVAILABILITY",
            query: format!("EventType={EVENT_TYPE_AVAILABILITY}"),
            expected: id_set(&["ev-availability-active", "ev-availability-resolved"]),
        },
        FilterCase {
            case: "EventType=PERFORMANCE",
            query: format!("EventType={EVENT_TYPE_PERFORMANCE}"),
            expected: id_set(&["ev-performance-active", "ev-performance-resolved"]),
        },
        FilterCase {
            case: "status and type together",
            query: format!(
                "EventStatus={INTERNET_EVENT_STATUS_ACTIVE}&EventType={EVENT_TYPE_PERFORMANCE}"
            ),
            expected: id_set(&["ev-performance-active"]),
        },
        FilterCase {
            case: "status and type together, the resolved half",
            query: format!(
                "EventStatus={INTERNET_EVENT_STATUS_RESOLVED}&EventType={EVENT_TYPE_AVAILABILITY}"
            ),
            expected: id_set(&["ev-availability-resolved"]),
        },
        FilterCase {
            case: "StartTime is inclusive at the bound",
            query: format!("StartTime={}", query_time(shifted(base, -3600))),
            expected: id_set(&["ev-availability-active", "ev-performance-active"]),
        },
        FilterCase {
            case: "StartTime one second past an event's start excludes it",
            query: format!("StartTime={}", query_time(shifted(base, -3599))),
            expected: id_set(&["ev-performance-active"]),
        },
        FilterCase {
            case: "EndTime is inclusive at the bound",
            query: format!("EndTime={}", query_time(shifted(base, -3600))),
            expected: id_set(&[
                "ev-availability-active",
                "ev-availability-resolved",
                "ev-performance-resolved",
            ]),
        },
        FilterCase {
            case: "a bounded time frame keeps the starts inside it",
            query: format!(
                "StartTime={}&EndTime={}",
                query_time(shifted(base, -7200)),
                query_time(shifted(base, -3600))
            ),
            expected: id_set(&["ev-availability-active", "ev-performance-resolved"]),
        },
        FilterCase {
            case: "a day-wide frame reaches the oldest event",
            query: format!(
                "StartTime={}&EndTime={}",
                query_time(shifted(base, -86_400)),
                query_time(shifted(base, -80_000))
            ),
            expected: id_set(&["ev-availability-resolved"]),
        },
        FilterCase {
            case: "an EndTime past a future start brings that event into the frame",
            query: format!("EndTime={}", query_time(shifted(base, 7200))),
            expected: id_set(&[
                "ev-availability-active",
                "ev-availability-resolved",
                "ev-future-start",
                "ev-performance-active",
                "ev-performance-resolved",
            ]),
        },
        FilterCase {
            case: "a frame entirely in the future keeps only what starts in it",
            query: format!(
                "StartTime={}&EndTime={}",
                query_time(shifted(base, 1800)),
                query_time(shifted(base, 7200))
            ),
            expected: id_set(&["ev-future-start"]),
        },
        FilterCase {
            case: "a time frame and a type filter are both applied",
            query: format!(
                "EventType={EVENT_TYPE_PERFORMANCE}&StartTime={}&EndTime={}",
                query_time(shifted(base, -7200)),
                query_time(shifted(base, -600))
            ),
            expected: id_set(&["ev-performance-active", "ev-performance-resolved"]),
        },
        FilterCase {
            case: "a frame that holds no start reports nothing",
            query: format!(
                "StartTime={}&EndTime={}",
                query_time(shifted(base, -70_000)),
                query_time(shifted(base, -60_000))
            ),
            expected: BTreeSet::new(),
        },
    ];

    for FilterCase {
        case,
        query,
        expected,
    } in cases
    {
        let path = if query.is_empty() {
            EVENTS_PATH.to_string()
        } else {
            format!("{EVENTS_PATH}?{query}")
        };
        let (status, listed) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {listed}"
        );
        let actual = listed_ids(&listed);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} from {listed}"
        );
    }

    // The listing is a report: the same request answers the same way afterwards.
    let (_, again) = call(&router, "GET", EVENTS_PATH, None).await;
    let repeated = listed_ids(&again);
    assert_eq!(
        repeated,
        id_set(&[
            "ev-availability-active",
            "ev-availability-resolved",
            "ev-performance-active",
            "ev-performance-resolved",
        ]),
        "a read must not change what a later caller sees, got {repeated:?}"
    );
}

#[tokio::test]
async fn a_summary_reports_every_modelled_member_of_the_stored_event() {
    let base = base_now();
    let events = fixture_events(base);
    let router = emulator_holding(&events);

    let (status, listed) = call(&router, "GET", EVENTS_PATH, None).await;
    assert_eq!(
        status, 200,
        "listing the fixture: expected 200, got {status} with {listed}"
    );

    for record in &events {
        // The future-starting event is outside the default time frame, so it has its own
        // row in the filter table rather than a summary here.
        if record.event_id == "ev-future-start" {
            continue;
        }
        let summary = listed["InternetEvents"]
            .as_array()
            .unwrap_or_else(|| panic!("InternetEvents must be a list, got {listed}"))
            .iter()
            .find(|event| event["EventId"] == Value::String(record.event_id.clone()))
            .unwrap_or_else(|| {
                panic!("case {:?}: not listed, got {listed}", record.event_id)
            });
        let case = &record.event_id;

        assert_eq!(
            summary["EventArn"],
            Value::String(expected_event_arn(case)),
            "case {case:?}: unexpected EventArn in {summary}"
        );
        let started = timestamp_secs(summary, "StartedAt");
        let expected_started = record
            .started_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs_f64();
        assert!(
            (started - expected_started).abs() < 0.001,
            "case {case:?}: expected StartedAt {expected_started}, got {started}"
        );
        match record.ended_at {
            None => {
                assert!(
                    member(summary, "EndedAt").is_none(),
                    "case {case:?}: an event that has not ended carries no EndedAt, got {summary}"
                );
                assert_eq!(
                    summary["EventStatus"],
                    Value::String(INTERNET_EVENT_STATUS_ACTIVE.to_string()),
                    "case {case:?}: an event with no EndedAt is ACTIVE, got {summary}"
                );
            }
            Some(ended_at) => {
                let ended = timestamp_secs(summary, "EndedAt");
                let expected_ended = ended_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("after the epoch")
                    .as_secs_f64();
                assert!(
                    (ended - expected_ended).abs() < 0.001,
                    "case {case:?}: expected EndedAt {expected_ended}, got {ended}"
                );
                assert_eq!(
                    summary["EventStatus"],
                    Value::String(INTERNET_EVENT_STATUS_RESOLVED.to_string()),
                    "case {case:?}: an event with an EndedAt is RESOLVED, got {summary}"
                );
            }
        }
        assert_eq!(
            summary["EventType"],
            Value::String(record.event_type.clone()),
            "case {case:?}: unexpected EventType in {summary}"
        );

        let location = &summary["ClientLocation"];
        assert_eq!(
            location["City"],
            Value::String(record.client_location.city.clone()),
            "case {case:?}: unexpected City in {location}"
        );
        assert_eq!(
            location["Country"],
            Value::String(record.client_location.country.clone()),
            "case {case:?}: unexpected Country in {location}"
        );
        assert_eq!(
            location["ASName"],
            Value::String(record.client_location.as_name.clone()),
            "case {case:?}: unexpected ASName in {location}"
        );
        assert_eq!(
            location["ASNumber"].as_i64(),
            Some(record.client_location.as_number),
            "case {case:?}: unexpected ASNumber in {location}"
        );
        assert_eq!(
            location["Subdivision"].as_str(),
            record.client_location.subdivision.as_deref(),
            "case {case:?}: unexpected Subdivision in {location}"
        );
        assert_eq!(
            location["Metro"].as_str(),
            record.client_location.metro.as_deref(),
            "case {case:?}: unexpected Metro in {location}"
        );
        assert_eq!(
            location["Latitude"].as_f64(),
            Some(record.client_location.latitude),
            "case {case:?}: unexpected Latitude in {location}"
        );
        assert_eq!(
            location["Longitude"].as_f64(),
            Some(record.client_location.longitude),
            "case {case:?}: unexpected Longitude in {location}"
        );
    }
}

#[tokio::test]
async fn a_page_walk_visits_every_event_once_at_every_page_size() {
    let base = base_now();
    // Ten events, each started a minute further back, so a page size that divides the set
    // and one that does not are both exercised.
    let events: Vec<InternetEventRecord> = (1..=10)
        .map(|index| {
            InternetEventRecord::new(
                format!("ev-page-{index:02}"),
                shifted(base, -60 * i64::from(index)),
                if index % 2 == 0 {
                    Some(shifted(base, -30 * i64::from(index)))
                } else {
                    None
                },
                location("Seattle", "United States", 16509).into_modelled(),
                if index % 3 == 0 {
                    EVENT_TYPE_AVAILABILITY
                } else {
                    EVENT_TYPE_PERFORMANCE
                },
            )
        })
        .collect();
    let every_id: Vec<String> = events
        .iter()
        .map(|event| event.event_id.clone())
        .collect();
    let router = emulator_holding(&events);

    for page_size in [
        MAX_RESULTS_MIN,
        2,
        3,
        i64::try_from(events.len()).expect("a small count"),
        MAX_RESULTS_MAX,
    ] {
        let case = format!("{MAX_RESULTS_QUERY}={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One request per page over the seeded events, plus a margin, so a token that
        // never ends fails the case instead of looping.
        let page_limit = (events.len() / usize::try_from(page_size).expect("a positive page size"))
            + 2;
        for _ in 0..page_limit {
            let path = match &token {
                None => format!("{EVENTS_PATH}?{MAX_RESULTS_QUERY}={page_size}"),
                Some(next) => format!(
                    "{EVENTS_PATH}?{MAX_RESULTS_QUERY}={page_size}&NextToken={}",
                    label(next)
                ),
            };
            let (status, listed) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {listed}"
            );
            let page = listed["InternetEvents"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no InternetEvents in {listed}"))
                .len();
            assert!(
                page <= usize::try_from(page_size).expect("a positive page size"),
                "case {case:?}: a page holds at most MaxResults, got {page}"
            );
            for event in listed["InternetEvents"]
                .as_array()
                .expect("a list")
                .iter()
            {
                walked.push(
                    event["EventId"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: no EventId in {event}"))
                        .to_string(),
                );
            }
            match member(&listed, "NextToken") {
                None => {
                    token = None;
                    break;
                }
                Some(next) => {
                    assert!(
                        page > 0,
                        "case {case:?}: a NextToken must never page to nothing, got {listed}"
                    );
                    token = Some(
                        next.as_str()
                            .unwrap_or_else(|| panic!("case {case:?}: NextToken in {listed}"))
                            .to_string(),
                    );
                }
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end without a NextToken, got {token:?}"
        );
        let visited: BTreeSet<String> = walked.iter().cloned().collect();
        assert_eq!(
            visited.len(),
            walked.len(),
            "case {case:?}: a page walk must visit each event once, got {walked:?}"
        );
        assert_eq!(
            walked, every_id,
            "case {case:?}: expected every event once in EventId order, got {walked:?}"
        );
    }

    // A filter survives the page walk: only the filtered events are ever reported.
    let mut filtered: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One request per seeded event at a page size of one, plus a margin.
    for _ in 0..(events.len() + 2) {
        let path = match &token {
            None => format!("{EVENTS_PATH}?EventType={EVENT_TYPE_AVAILABILITY}&{MAX_RESULTS_QUERY}=1"),
            Some(next) => format!(
                "{EVENTS_PATH}?EventType={EVENT_TYPE_AVAILABILITY}&{MAX_RESULTS_QUERY}=1&NextToken={}",
                label(next)
            ),
        };
        let (status, listed) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "a filtered page walk: expected 200, got {status} with {listed}"
        );
        filtered.extend(listed_ids(&listed));
        match member(&listed, "NextToken") {
            None => {
                token = None;
                break;
            }
            Some(next) => {
                token = Some(next.as_str().expect("a token string").to_string());
            }
        }
    }
    assert!(
        token.is_none(),
        "a filtered page walk must end without a NextToken, got {token:?}"
    );
    assert_eq!(
        filtered,
        vec![
            "ev-page-03".to_string(),
            "ev-page-06".to_string(),
            "ev-page-09".to_string()
        ],
        "a filtered page walk must report only the AVAILABILITY events, got {filtered:?}"
    );
}

#[tokio::test]
async fn a_request_outside_the_contract_is_refused_and_one_inside_it_is_not() {
    let base = base_now();
    let router = emulator_holding(&fixture_events(base));

    /// One refusal row: the query string and the member its message must name.
    struct RefusalCase {
        case: &'static str,
        query: String,
        names: &'static str,
    }

    let refusals = vec![
        RefusalCase {
            case: "an EventStatus that is not a modelled member",
            query: "EventStatus=UNKNOWN".to_string(),
            names: "EventStatus",
        },
        RefusalCase {
            case: "an EventStatus in the wrong case",
            query: "EventStatus=active".to_string(),
            names: "EventStatus",
        },
        RefusalCase {
            case: "an EventType that is not a modelled member",
            query: "EventType=LATENCY".to_string(),
            names: "EventType",
        },
        RefusalCase {
            case: "an EventType in the wrong case",
            query: "EventType=performance".to_string(),
            names: "EventType",
        },
        RefusalCase {
            case: "a NextToken this service never minted",
            query: "NextToken=not-a-token".to_string(),
            names: "NextToken",
        },
    ];

    for RefusalCase { case, query, names } in refusals {
        let (status, refused) =
            call(&router, "GET", &format!("{EVENTS_PATH}?{query}"), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        let message = refused["message"]
            .as_str()
            .or_else(|| refused["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(names),
            "case {case:?}: the refusal must name {names}, got {refused}"
        );
    }

    // `MaxResults` is modelled 1..100: each published bound is accepted and one past it
    // is refused by the boundary validator.
    for (case, value, expected) in [
        ("one below the minimum", MAX_RESULTS_MIN - 1, 400),
        ("the minimum", MAX_RESULTS_MIN, 200),
        ("the maximum", MAX_RESULTS_MAX, 200),
        ("one past the maximum", MAX_RESULTS_MAX + 1, 400),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("{EVENTS_PATH}?{MAX_RESULTS_QUERY}={value}"),
            None,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?} ({MAX_RESULTS_QUERY}={value}): expected {expected}, got {status} with {body}"
        );
    }

    // A frame of one instant is a legal frame, and keeps the events that started at it.
    let (status, listed) = call(
        &router,
        "GET",
        &format!(
            "{EVENTS_PATH}?StartTime={}&EndTime={}",
            query_time(shifted(base, -600)),
            query_time(shifted(base, -600))
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "a single-instant frame: expected 200, got {status} with {listed}"
    );
    let actual = listed_ids(&listed);
    assert_eq!(
        actual,
        id_set(&["ev-performance-active"]),
        "a single-instant frame must keep the event that started at it, got {actual:?}"
    );
}
