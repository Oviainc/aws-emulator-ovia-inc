// SPDX-License-Identifier: CC0-1.0

//! `DescribeEvents`: what a control-plane write RAISED, read back over the
//! awsQuery wire through the same router the production emulator builds.
//!
//! The rules asserted here, each over a TABLE of inputs rather than one request:
//!
//! * a completed write APPENDS the occurrence of the catalog event it raises to
//!   the service's event log, and a read reports that occurrence with the catalog
//!   entry's own identity (its id, the source type that raises it, its categories
//!   and its severity) and the three values the occurrence supplies — the source
//!   it happened to, the instant it happened at and the message it rendered;
//! * a read reports exactly the logged events inside the closed time interval the
//!   request names (`EndTime` defaulting to the time of the request and
//!   `StartTime` to `Duration` minutes before it, `Duration` itself defaulting to
//!   60), of the one source it names when it names one, in the order they
//!   happened, one `MaxRecords` page at a time;
//! * an event is readable for the published retention window and no longer;
//! * `SourceIdentifier` and `SourceType` scope a read TOGETHER, so either on its
//!   own is refused on the parameter combination.
//!
//! The message of an event is the published catalog's own sentence with the
//! values one occurrence fills in, so every case below checks the fixed text
//! exactly and the instant by parsing it.

use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::Event;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, element_blocks, Answer, Emulator};

/// The parameter group family every group below is created in.
const FAMILY: &str = "redshift-1.0";

/// One day, as the retention cases measure an event's age in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Fail unless `answer` was served, naming the case and what came back.
fn assert_served(case: &str, action: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {action} to be served with status 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// `at` in the ISO 8601 form the time members are documented to take
/// (`Example: 2009-07-08T18:00Z`).
fn iso8601(at: SystemTime) -> String {
    DateTime::from(at)
        .fmt(Format::DateTime)
        .expect("an instant renders as ISO 8601")
}

/// `DescribeEvents` with these parameters.
async fn read(emulator: &Emulator, params: &[(&str, &str)]) -> Answer {
    emulator.call("DescribeEvents", params).await
}

/// The `EventId` of every event a served read reported, in the order it
/// reported them.
fn reported_ids(answer: &Answer) -> Vec<String> {
    all_text(&answer.body, "EventId")
}

/// One logged event a read reported, reduced to what the assertions check.
#[derive(Debug)]
struct Reported {
    event_id: String,
    source_identifier: String,
    source_type: String,
    severity: String,
    categories: Vec<String>,
    message: String,
    date: String,
}

/// Every event a served read reported, in the order it reported them.
///
/// `<Event>` blocks are read whole rather than member by member across the
/// body, so each reported event's members are asserted TOGETHER: a read that
/// paired one event's id with another's message would otherwise pass.
fn reported(answer: &Answer) -> Vec<Reported> {
    element_blocks(&answer.body, "Event")
        .iter()
        .map(|block| Reported {
            event_id: all_text(block, "EventId").join(","),
            source_identifier: all_text(block, "SourceIdentifier").join(","),
            source_type: all_text(block, "SourceType").join(","),
            severity: all_text(block, "Severity").join(","),
            categories: all_text(block, "EventCategory"),
            message: all_text(block, "Message").join(","),
            date: all_text(block, "Date").join(","),
        })
        .collect()
}

/// Write the events `ages` describes straight into the shared store, newest
/// last: one event per age, dated that long before now.
///
/// The log is only ever appended to by a write that raises an event, and no
/// write can raise one dated days in the past, so a case about the RETENTION
/// window has no other way to make an old event exist at all. The records are
/// put in the same store the handlers read, so the request path is unchanged.
fn seed_events(emulator: &Emulator, prefix: &str, ages: &[Duration]) -> Vec<String> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    let now = SystemTime::now();
    let mut identifiers = Vec::new();
    for (index, age) in ages.iter().enumerate() {
        let identifier = format!("{prefix}-{index}");
        store.record_event(Event {
            source_identifier: Some(identifier.clone()),
            source_type: Some(state::EVENT_SOURCE_TYPE_CLUSTER.to_string()),
            message: Some(format!("seeded event {index}")),
            event_categories: Some(vec![state::EVENT_CATEGORY_MANAGEMENT.to_string()]),
            severity: Some(state::EVENT_SEVERITY_INFO.to_string()),
            date: Some(DateTime::from(now - *age)),
            event_id: Some(state::EVENT_ID_CLUSTER_CREATED.to_string()),
        });
        identifiers.push(identifier);
    }
    identifiers
}

/// One write, and the single catalog event the published catalog lists for it.
struct WriteCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, String)>,
    event_id: &'static str,
    source_type: &'static str,
    source_identifier: String,
    categories: Vec<&'static str>,
    /// The published sentence's text before the instant it names.
    before: String,
    /// The published sentence's text after it.
    after: &'static str,
}

/// The writes asserted below: three source types, and both halves of a
/// resource's life (created, and gone again) for the one that has both.
fn write_cases() -> Vec<WriteCase> {
    let group = "events-group".to_string();
    let cluster = "events-cluster".to_string();
    let snapshot = "events-snapshot".to_string();
    vec![
        WriteCase {
            case: "a cluster parameter group created",
            action: "CreateClusterParameterGroup",
            params: vec![
                ("ParameterGroupName", group.clone()),
                ("ParameterGroupFamily", FAMILY.to_string()),
                ("Description", "sim2real".to_string()),
            ],
            event_id: "REDSHIFT-EVENT-1003",
            source_type: "cluster-parameter-group",
            source_identifier: group.clone(),
            categories: vec!["configuration"],
            before: format!("Cluster parameter group '{group}' was created at "),
            after: " UTC.",
        },
        WriteCase {
            case: "a cluster created",
            action: "CreateCluster",
            params: vec![
                ("ClusterIdentifier", cluster.clone()),
                ("NodeType", "ra3.xlplus".to_string()),
                ("MasterUsername", "admin".to_string()),
            ],
            event_id: "REDSHIFT-EVENT-2000",
            source_type: "cluster",
            source_identifier: cluster.clone(),
            categories: vec!["management"],
            before: format!("Cluster '{cluster}' created at "),
            after: " UTC.",
        },
        WriteCase {
            case: "a snapshot taken of that cluster",
            action: "CreateClusterSnapshot",
            params: vec![
                ("SnapshotIdentifier", snapshot.clone()),
                ("ClusterIdentifier", cluster.clone()),
            ],
            event_id: "REDSHIFT-EVENT-3100",
            source_type: "cluster-snapshot",
            source_identifier: snapshot.clone(),
            categories: vec!["monitoring"],
            before: format!(
                "A snapshot '{snapshot}' of your Amazon Redshift cluster '{cluster}' was created at "
            ),
            after: " UTC.",
        },
        WriteCase {
            case: "that parameter group deleted again",
            action: "DeleteClusterParameterGroup",
            params: vec![("ParameterGroupName", group.clone())],
            event_id: "REDSHIFT-EVENT-1004",
            source_type: "cluster-parameter-group",
            source_identifier: group.clone(),
            categories: vec!["configuration"],
            before: format!("Cluster parameter group '{group}' was deleted at "),
            after: " UTC.",
        },
    ]
}

/// Every write the catalog lists an event for appends that occurrence to the
/// log, and a read reports it with the catalog entry's identity and the values
/// the occurrence filled in — in the order the writes happened.
#[tokio::test]
async fn a_write_appends_the_event_a_read_reports() {
    let emulator = Emulator::new();
    let cases = write_cases();

    let before_any = read(&emulator, &[]).await;
    assert_served("an account that has written nothing", "DescribeEvents", &before_any);
    let empty = reported(&before_any);
    assert!(
        empty.is_empty(),
        "case \"an account that has written nothing\": expected no events, got {empty:?}"
    );

    for row in &cases {
        let params: Vec<(&str, &str)> = row
            .params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let written = emulator.call(row.action, &params).await;
        assert_served(row.case, row.action, &written);
    }

    let answer = read(&emulator, &[]).await;
    assert_served("the whole table", "DescribeEvents", &answer);
    let found = reported(&answer);
    assert_eq!(
        found.len(),
        cases.len(),
        "case \"the whole table\": expected one event per write, got {found:?}"
    );

    for (row, event) in cases.iter().zip(found.iter()) {
        assert_eq!(
            event.event_id, row.event_id,
            "case {:?}: expected EventId {}, got {event:?}",
            row.case, row.event_id
        );
        assert_eq!(
            event.source_identifier, row.source_identifier,
            "case {:?}: expected SourceIdentifier {}, got {event:?}",
            row.case, row.source_identifier
        );
        assert_eq!(
            event.source_type, row.source_type,
            "case {:?}: expected SourceType {}, got {event:?}",
            row.case, row.source_type
        );
        assert_eq!(
            event.severity, "INFO",
            "case {:?}: expected Severity INFO, got {event:?}",
            row.case
        );
        assert_eq!(
            event.categories, row.categories,
            "case {:?}: expected EventCategories {:?}, got {event:?}",
            row.case, row.categories
        );
        assert!(
            event.message.starts_with(&row.before) && event.message.ends_with(row.after),
            "case {:?}: expected the message {:?}<instant>{:?}, got {event:?}",
            row.case,
            row.before,
            row.after
        );
        // The instant the message names and the event's own `Date` are the one
        // instant the event happened at, so the `Date` has to parse and the
        // message has to name the same minute.
        let date = DateTime::from_str(&event.date, Format::DateTime).unwrap_or_else(|error| {
            panic!(
                "case {:?}: expected Date to be an ISO 8601 instant, got {event:?} ({error})",
                row.case
            )
        });
        let minute = iso8601(SystemTime::try_from(date).expect("an instant"))
            .split('T')
            .next_back()
            .map(|clock| clock[..5].to_string())
            .expect("the clock part of an ISO 8601 instant");
        assert!(
            event.message.contains(&minute),
            "case {:?}: expected the message to name the minute {minute} its Date reports, got {event:?}",
            row.case
        );
    }
}

/// One read, and the events it may report.
struct ReadCase {
    case: &'static str,
    params: Vec<(&'static str, String)>,
    /// The `EventId`s the read is expected to report, in order.
    expect: Vec<&'static str>,
}

/// The interval and the source a read names decide exactly which logged events
/// it reports.
#[tokio::test]
async fn a_read_reports_the_interval_and_the_source_it_names() {
    let emulator = Emulator::new();
    let cases = write_cases();
    for row in &cases {
        let params: Vec<(&str, &str)> = row
            .params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let written = emulator.call(row.action, &params).await;
        assert_served(row.case, row.action, &written);
    }
    let every_id: Vec<&'static str> = cases.iter().map(|row| row.event_id).collect();
    let now = SystemTime::now();

    let rows = vec![
        ReadCase {
            case: "no parameters at all: the hour before the request",
            params: Vec::new(),
            expect: every_id.clone(),
        },
        ReadCase {
            case: "a Duration spanning a whole day",
            params: vec![("Duration", "1440".to_string())],
            expect: every_id.clone(),
        },
        ReadCase {
            case: "an EndTime an hour before the writes",
            params: vec![("EndTime", iso8601(now - Duration::from_secs(3600)))],
            expect: Vec::new(),
        },
        ReadCase {
            case: "a StartTime after the request's own instant",
            params: vec![("StartTime", iso8601(now + Duration::from_secs(3600)))],
            expect: Vec::new(),
        },
        ReadCase {
            case: "an interval the writes sit inside",
            params: vec![
                ("StartTime", iso8601(now - Duration::from_secs(3600))),
                ("EndTime", iso8601(now + Duration::from_secs(3600))),
            ],
            expect: every_id.clone(),
        },
        ReadCase {
            case: "a StartTime inside the interval but a Duration that would exclude it",
            // `Duration` is "the number of minutes prior to the time of the
            // request", so a named `StartTime` is what decides the beginning.
            params: vec![
                ("StartTime", iso8601(now - Duration::from_secs(3600))),
                ("Duration", "20".to_string()),
            ],
            expect: every_id.clone(),
        },
        ReadCase {
            case: "the one cluster the writes created",
            params: vec![
                ("SourceType", "cluster".to_string()),
                ("SourceIdentifier", "events-cluster".to_string()),
            ],
            expect: vec!["REDSHIFT-EVENT-2000"],
        },
        ReadCase {
            case: "the one snapshot the writes took",
            params: vec![
                ("SourceType", "cluster-snapshot".to_string()),
                ("SourceIdentifier", "events-snapshot".to_string()),
            ],
            expect: vec!["REDSHIFT-EVENT-3100"],
        },
        ReadCase {
            case: "the parameter group, which two writes are about",
            params: vec![
                ("SourceType", "cluster-parameter-group".to_string()),
                ("SourceIdentifier", "events-group".to_string()),
            ],
            expect: vec!["REDSHIFT-EVENT-1003", "REDSHIFT-EVENT-1004"],
        },
        ReadCase {
            case: "a source identifier nothing was ever raised for",
            params: vec![
                ("SourceType", "cluster".to_string()),
                ("SourceIdentifier", "events-absent".to_string()),
            ],
            expect: Vec::new(),
        },
        ReadCase {
            case: "the right identifier under the wrong source type",
            params: vec![
                ("SourceType", "cluster-snapshot".to_string()),
                ("SourceIdentifier", "events-cluster".to_string()),
            ],
            expect: Vec::new(),
        },
    ];

    for row in rows {
        let params: Vec<(&str, &str)> = row
            .params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = read(&emulator, &params).await;
        assert_served(row.case, "DescribeEvents", &answer);
        let found = reported_ids(&answer);
        assert_eq!(
            found, row.expect,
            "case {:?}: expected the events {:?}, got {found:?}",
            row.case, row.expect
        );
    }
}

/// An event is readable for the published retention window and no longer.
///
/// The bound is checked a minute inside it and a minute past it rather than at
/// the instant itself: the cut-off is measured from the clock when the call is
/// served, so a single second's tick during the call decides an event dated
/// exactly one window ago either way.
#[tokio::test]
async fn the_retention_window_bounds_what_a_read_reports() {
    let emulator = Emulator::new();
    let minute = Duration::from_secs(60);
    let retention = state::EVENT_RETENTION;
    let ages = [
        minute,
        13 * DAY,
        retention - minute,
        retention + minute,
        retention + 6 * DAY,
    ];
    let readable = [true, true, true, false, false];
    let identifiers = seed_events(&emulator, "retention", &ages);

    // An interval wide enough to hold every seeded event, so what a read
    // reports is decided by the retention window alone.
    let now = SystemTime::now();
    let answer = read(
        &emulator,
        &[
            ("StartTime", &iso8601(now - 30 * DAY)),
            ("EndTime", &iso8601(now + minute)),
        ],
    )
    .await;
    assert_served("the whole retention table", "DescribeEvents", &answer);
    let found: Vec<String> = reported(&answer)
        .into_iter()
        .map(|event| event.source_identifier)
        .collect();

    for ((age, identifier), expected) in ages.iter().zip(identifiers.iter()).zip(readable.iter()) {
        let seen = found.iter().any(|source| source == identifier);
        assert_eq!(
            seen, *expected,
            "case \"an event {age:?} old\": expected readable={expected}, got {seen} (reported {found:?})"
        );
    }
}

/// A read serves the log one `MaxRecords` page at a time, in the order the
/// events happened, and refuses a page size outside the published window.
#[tokio::test]
async fn a_read_pages_the_log_in_the_order_it_happened() {
    let emulator = Emulator::new();
    // More events than the smallest page the window admits, so the walk takes
    // more than one page: each is dated a second further back than the last, so
    // the log's order is the order they happened in.
    let total = 25usize;
    let ages: Vec<Duration> = (0..total)
        .map(|index| Duration::from_secs((total - index) as u64))
        .collect();
    let seeded = seed_events(&emulator, "page", &ages);

    let first = read(&emulator, &[("MaxRecords", "20")]).await;
    assert_served("the first page", "DescribeEvents", &first);
    let page_one: Vec<String> = reported(&first)
        .into_iter()
        .map(|event| event.source_identifier)
        .collect();
    assert_eq!(
        page_one.len(),
        20,
        "case \"the first page\": expected 20 events, got {page_one:?}"
    );
    let marker = first.text("Marker").unwrap_or_else(|| {
        panic!(
            "case \"the first page\": expected a Marker while events remain, got body {}",
            first.body
        )
    });

    let second = read(&emulator, &[("MaxRecords", "20"), ("Marker", &marker)]).await;
    assert_served("the second page", "DescribeEvents", &second);
    let page_two: Vec<String> = reported(&second)
        .into_iter()
        .map(|event| event.source_identifier)
        .collect();
    assert_eq!(
        page_two.len(),
        total - 20,
        "case \"the second page\": expected the rest of the log, got {page_two:?}"
    );
    assert!(
        second.text("Marker").is_none(),
        "case \"the second page\": expected no Marker once the log is exhausted, got body {}",
        second.body
    );

    let walked: Vec<String> = page_one.into_iter().chain(page_two).collect();
    assert_eq!(
        walked, seeded,
        "case \"the whole walk\": expected the log in the order it happened, got {walked:?}"
    );

    // The published `MaxRecords` window: both bounds, and one past each. Only
    // the minimum is enforced, so a page past the published maximum is served.
    for (case, max_records, served) in [
        ("the window's own minimum", "20", true),
        ("the window's own maximum", "100", true),
        ("one below the minimum", "19", false),
        ("one above the maximum", "101", true),
        ("far above the maximum", "1000", true),
    ] {
        let answer = read(&emulator, &[("MaxRecords", max_records)]).await;
        if served {
            assert_served(case, "DescribeEvents", &answer);
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected MaxRecords={max_records} to be refused with status 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code().as_deref(),
            Some("InvalidParameterValue"),
            "case {case:?}: expected the refusal to carry InvalidParameterValue, got body {}",
            answer.body
        );
    }
}

/// `SourceIdentifier` and `SourceType` scope a read TOGETHER: either on its own
/// is refused on the parameter combination, and naming both or neither is
/// served.
#[tokio::test]
async fn the_two_source_members_scope_a_read_together() {
    let emulator = Emulator::new();
    let created = emulator.create_minimal("events-pair-cluster").await;
    assert_served("the cluster the pair names", "CreateCluster", &created);

    for (case, params) in [
        ("neither member", Vec::new()),
        (
            "both members",
            vec![
                ("SourceType", "cluster"),
                ("SourceIdentifier", "events-pair-cluster"),
            ],
        ),
    ] {
        let answer = read(&emulator, &params).await;
        assert_served(case, "DescribeEvents", &answer);
    }

    for (case, params) in [
        (
            "an identifier with no source type",
            vec![("SourceIdentifier", "events-pair-cluster")],
        ),
        ("a source type with no identifier", vec![("SourceType", "cluster")]),
    ] {
        let answer = read(&emulator, &params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected status 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code().as_deref(),
            Some("InvalidParameterCombination"),
            "case {case:?}: expected the refusal to carry InvalidParameterCombination, got body {}",
            answer.body
        );
    }
}
