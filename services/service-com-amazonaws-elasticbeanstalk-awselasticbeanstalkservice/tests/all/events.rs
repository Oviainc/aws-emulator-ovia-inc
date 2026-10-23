// SPDX-License-Identifier: CC0-1.0

//! `DescribeEvents`: the event log this service's lifecycle calls append to, and
//! the filters, the ordering, the time window and the pagination the operation
//! reports it through.
//!
//! Driven over the same awsQuery HTTP surface as `lifecycle.rs`, through the
//! shared [`Emulator`] harness, so nothing here reaches into the store: a
//! lifecycle call that records no event, or a filter answered from anything
//! other than the log, fails these tests.
//!
//! Each test asserts a RULE over a set of inputs rather than one recorded
//! request: the filter table spans every member `DescribeEvents` filters on, the
//! severity table spans the whole modelled `EventSeverity` set, the paging table
//! spans page sizes from one to past the log's length plus both modelled bounds
//! of `MaxRecords`, and the window table spans both sides of each bound.

use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use super::harness::{Answer, Emulator, PAST_DUE, SOLUTION_STACK};

/// `EventSeverity` from least to most severe, in the order the model declares
/// the enum's members —
/// `smithy_query(query="shape",shape="EventSeverity")` answers
/// `["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"]`. Written out here
/// rather than read from the service so these tests do not grade the service
/// against its own copy of the ladder.
const SEVERITY_LADDER: &[&str] = &["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"];

/// The region the shared harness signs its requests for, which is the region a
/// managed platform version's ARN is resolved in.
const HARNESS_REGION: &str = "us-west-2";

/// A managed platform version from the catalog the service itself publishes
/// (`eb::state::SOLUTION_STACKS`, what `ListAvailableSolutionStacks` reports), so
/// `CreateEnvironment` resolves the `PlatformArn` and `SolutionStackName` below
/// to a platform version that exists instead of refusing them — and so the
/// `PlatformArn` filter has events to select.
fn managed_platform() -> &'static eb::state::SolutionStack {
    eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack")
}

/// A SECOND managed platform version from the same catalog, so a test can tell
/// two platform versions apart.
fn other_managed_platform() -> &'static eb::state::SolutionStack {
    eb::state::SOLUTION_STACKS
        .get(1)
        .expect("the service publishes more than one solution stack")
}

/// Where `severity` sits on [`SEVERITY_LADDER`].
fn severity_rank(severity: &str, case: &str) -> usize {
    SEVERITY_LADDER
        .iter()
        .position(|known| *known == severity)
        .unwrap_or_else(|| panic!("case {case:?}: {severity:?} is not a modelled EventSeverity"))
}

/// One `EventDescription` as the assertions read it. An absent optional member
/// reads as the empty string, which no member this service records ever holds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedEvent {
    event_date: String,
    message: String,
    severity: String,
    request_id: String,
    application_name: String,
    version_label: String,
    template_name: String,
    environment_name: String,
    platform_arn: String,
}

/// Every `EventDescription` in `answer`, in the order the answer lists them.
fn reported(answer: &Answer) -> Vec<ReportedEvent> {
    answer
        .elements_at("Events/member")
        .iter()
        .map(|event| ReportedEvent {
            event_date: member(event, "EventDate"),
            message: member(event, "Message"),
            severity: member(event, "Severity"),
            request_id: member(event, "RequestId"),
            application_name: member(event, "ApplicationName"),
            version_label: member(event, "VersionLabel"),
            template_name: member(event, "TemplateName"),
            environment_name: member(event, "EnvironmentName"),
            platform_arn: member(event, "PlatformArn"),
        })
        .collect()
}

fn member(event: &Answer, name: &str) -> String {
    event.value(name).unwrap_or_default().to_string()
}

/// Call `action`, failing the named case unless it answered 200.
async fn ok(emulator: &Emulator, case: &str, action: &str, params: &[(&str, &str)]) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// The events `DescribeEvents` reports under `filters`.
async fn events_of(emulator: &Emulator, case: &str, filters: &[(&str, &str)]) -> Vec<ReportedEvent> {
    let answer = ok(emulator, case, "DescribeEvents", filters).await;
    reported(&answer)
}

/// An `EventDate` as epoch seconds: the shared timestamp reader
/// ([`crate::lifecycle::instant`]), which orders two renderings as instants.
fn at_secs(event_date: &str, case: &str) -> f64 {
    crate::harness::instant(event_date, case)
}

/// An `EventDate` as the exact `(epoch seconds, subsecond nanos)` pair an instant is
/// ORDERED by.
///
/// The ordering key is the service's own ([`eb::state::instant_key`]), not a second
/// copy of it, so this suite and the handler cannot disagree about how two instants
/// compare. The pair, not [`at_secs`]: an epoch second count near 1.8e9 leaves an
/// `f64` barely half a microsecond of resolution, so two reported dates a microsecond
/// apart — the precision these timestamps are reported at — can round to the same
/// `f64`, and a window assertion built on `f64` would then disagree with the order the
/// REPORTED values are in.
fn at_parts(event_date: &str, case: &str) -> (i64, u32) {
    let parsed = DateTime::from_str(event_date, Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {event_date:?} is not a timestamp: {error}"));
    eb::state::instant_key(&parsed)
}

/// The instant `delta_secs` whole seconds from `parts`, rendered as a timestamp: a
/// bound outside the log, built from a reported date so it keeps that date's
/// subsecond part.
fn shifted_timestamp(parts: (i64, u32), delta_secs: i64, case: &str) -> String {
    DateTime::from_secs_and_nanos(parts.0 + delta_secs, parts.1)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {parts:?} is not renderable: {error}"))
}

// ---------------------------------------------------------------------------
// The log: what the lifecycle calls record
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_untouched_service_reports_an_empty_event_log() {
    let emulator = Emulator::new();
    let case = "empty log";
    let reported = events_of(&emulator, case, &[]).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: a service nobody has called must report no events, got {reported:?}"
    );
}

#[tokio::test]
async fn the_lifecycle_calls_record_the_events_describe_events_reports() {
    let emulator = Emulator::new();
    let case = "lifecycle events";

    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", "events-app")],
    )
    .await;
    ok(
        &emulator,
        case,
        "CreateApplicationVersion",
        &[
            ("ApplicationName", "events-app"),
            ("VersionLabel", "v1"),
            ("SourceBundle.S3Bucket", "events-bucket"),
            ("SourceBundle.S3Key", "app.zip"),
        ],
    )
    .await;
    ok(
        &emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", "events-app"),
            ("EnvironmentName", "events-env"),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
            ("VersionLabel", "v1"),
        ],
    )
    .await;
    // The launch completion is owed one status-transition span after the create,
    // and recorded by the next call that observes it as due.
    tokio::time::sleep(PAST_DUE).await;
    ok(
        &emulator,
        case,
        "TerminateEnvironment",
        &[("EnvironmentName", "events-env")],
    )
    .await;
    tokio::time::sleep(PAST_DUE).await;

    // Oldest first: the message, and the resources the event is about. A
    // synchronous call records the sequence its activity consists of — the
    // operation starting, the resource it created, the operation completing —
    // while a call the clock finishes later records its start now and its
    // completion when that completion comes due.
    let expected: &[(&str, &str, &str, &str)] = &[
        ("createApplication is starting.", "events-app", "", ""),
        (
            "Created new Application: events-app",
            "events-app",
            "",
            "",
        ),
        (
            "createApplication completed successfully.",
            "events-app",
            "",
            "",
        ),
        (
            "createApplicationVersion is starting.",
            "events-app",
            "v1",
            "",
        ),
        (
            "Created new Application Version: v1",
            "events-app",
            "v1",
            "",
        ),
        (
            "createApplicationVersion completed successfully.",
            "events-app",
            "v1",
            "",
        ),
        (
            "createEnvironment is starting.",
            "events-app",
            "v1",
            "events-env",
        ),
        (
            "Successfully launched environment: events-env",
            "events-app",
            "v1",
            "events-env",
        ),
        (
            "terminateEnvironment is starting.",
            "events-app",
            "v1",
            "events-env",
        ),
        (
            "terminateEnvironment completed successfully.",
            "events-app",
            "v1",
            "events-env",
        ),
    ];

    let reported = events_of(&emulator, case, &[]).await;
    // "beginning with the most recent event": the answer is newest first, so
    // the lifecycle order is its reverse.
    let oldest_first: Vec<&ReportedEvent> = reported.iter().rev().collect();
    assert_eq!(
        oldest_first.len(),
        expected.len(),
        "case {case:?}: expected {} events, got {:?}",
        expected.len(),
        reported
    );
    for (event, (message, application, version, environment)) in oldest_first.iter().zip(expected) {
        let case = format!("{case}: {message}");
        assert_eq!(
            &event.message, message,
            "case {case:?}: unexpected message, got {:?}",
            event.message
        );
        assert_eq!(
            &event.application_name, application,
            "case {case:?}: unexpected ApplicationName, got {:?}",
            event.application_name
        );
        assert_eq!(
            &event.version_label, version,
            "case {case:?}: unexpected VersionLabel, got {:?}",
            event.version_label
        );
        assert_eq!(
            &event.environment_name, environment,
            "case {case:?}: unexpected EnvironmentName, got {:?}",
            event.environment_name
        );
        assert_eq!(
            event.severity, "INFO",
            "case {case:?}: a completed lifecycle call is informational, got {:?}",
            event.severity
        );
        assert!(
            !event.request_id.is_empty(),
            "case {case:?}: expected a RequestId, got {event:?}"
        );
        // The date parses as the modelled timestamp, and is not the epoch.
        assert!(
            at_secs(&event.event_date, &case) > 1_000_000_000.0,
            "case {case:?}: unexpected EventDate {:?}",
            event.event_date
        );
    }

    // Event dates do not go backwards along the lifecycle.
    for pair in oldest_first.windows(2) {
        let (earlier, later) = (pair[0], pair[1]);
        assert!(
            at_secs(&earlier.event_date, case) <= at_secs(&later.event_date, case),
            "case {case:?}: {:?} must not post-date {:?}",
            earlier.message,
            later.message
        );
    }

    // A change the clock owed belongs to the request that STARTED it, not to
    // whichever later call observed it as due.
    assert_eq!(
        oldest_first[7].request_id, oldest_first[6].request_id,
        "case {case:?}: the launch completion must carry the createEnvironment request id, got {:?} and {:?}",
        oldest_first[7].request_id, oldest_first[6].request_id
    );
    assert_eq!(
        oldest_first[9].request_id, oldest_first[8].request_id,
        "case {case:?}: the termination completion must carry the terminateEnvironment request id, got {:?} and {:?}",
        oldest_first[9].request_id, oldest_first[8].request_id
    );
    // One call's activity is ONE RequestId: the start, the resource it created
    // and the completion all carry it, which is what lets a client follow one
    // operation by filtering the log on it.
    for activity in [[0usize, 1, 2], [3, 4, 5]] {
        let first = &oldest_first[activity[0]];
        for index in activity {
            assert_eq!(
                oldest_first[index].request_id, first.request_id,
                "case {case:?}: {:?} must carry the same RequestId as {:?}, got {:?} and {:?}",
                oldest_first[index].message,
                first.message,
                oldest_first[index].request_id,
                first.request_id
            );
        }
    }
    // Two different calls are two different activities.
    assert_ne!(
        oldest_first[0].request_id, oldest_first[3].request_id,
        "case {case:?}: two calls must not share a RequestId, got {:?}",
        oldest_first[0].request_id
    );
}

// ---------------------------------------------------------------------------
// The filters
// ---------------------------------------------------------------------------

/// One row of the filter table: the member filtered on, the value, how many
/// events that must select, and which events those are.
struct FilterCase {
    member: &'static str,
    value: String,
    expected_count: usize,
    matches: Box<dyn Fn(&ReportedEvent) -> bool>,
}

#[tokio::test]
async fn each_filter_selects_exactly_the_events_of_the_resource_it_names() {
    let emulator = Emulator::new();
    let case = "filters";

    // Two applications, each with its own version and environment, so every
    // filter has events on both sides of it.
    for (application, version) in [("app-one", "v1"), ("app-two", "v2")] {
        ok(
            &emulator,
            case,
            "CreateApplication",
            &[("ApplicationName", application)],
        )
        .await;
        ok(
            &emulator,
            case,
            "CreateApplicationVersion",
            &[
                ("ApplicationName", application),
                ("VersionLabel", version),
                ("SourceBundle.S3Bucket", "events-bucket"),
                ("SourceBundle.S3Key", "app.zip"),
            ],
        )
        .await;
    }
    // env-one is launched by solution stack NAME and env-two by platform ARN,
    // and the two name DIFFERENT platform versions — an environment reports the
    // platform version it runs on however the request named it, so the
    // `PlatformArn` filter has env-two's events to select and env-one's to
    // reject.
    let solution_stack = other_managed_platform().name();
    let platform_arn = managed_platform().platform_arn(HARNESS_REGION);
    let env_one = ok(
        &emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", "app-one"),
            ("EnvironmentName", "env-one"),
            ("VersionLabel", "v1"),
            ("SolutionStackName", solution_stack.as_str()),
        ],
    )
    .await;
    let env_one_id = env_one.expect("EnvironmentId", case).to_string();
    ok(
        &emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", "app-two"),
            ("EnvironmentName", "env-two"),
            ("VersionLabel", "v2"),
            ("PlatformArn", platform_arn.as_str()),
        ],
    )
    .await;
    // Both launches are due by now, so the log is settled and the first read
    // records their completion events.
    tokio::time::sleep(PAST_DUE).await;

    let all = events_of(&emulator, case, &[]).await;
    assert_eq!(
        all.len(),
        16,
        "case {case:?}: expected 16 events (per application: the createApplication and \
         createApplicationVersion activities of three events each, the environment start and \
         its launch), got {all:?}"
    );

    let launch_request_id = all
        .iter()
        .find(|event| event.message == "createEnvironment is starting." && event.environment_name == "env-one")
        .map(|event| event.request_id.clone())
        .unwrap_or_else(|| panic!("case {case:?}: no createEnvironment event for env-one in {all:?}"));
    let wanted_request_id = launch_request_id.clone();

    let table: Vec<FilterCase> = vec![
        FilterCase {
            member: "ApplicationName",
            value: "app-one".to_string(),
            expected_count: 8,
            matches: Box::new(|event| event.application_name == "app-one"),
        },
        FilterCase {
            member: "ApplicationName",
            value: "app-two".to_string(),
            expected_count: 8,
            matches: Box::new(|event| event.application_name == "app-two"),
        },
        FilterCase {
            member: "ApplicationName",
            value: "no-such-app".to_string(),
            expected_count: 0,
            matches: Box::new(|_| false),
        },
        FilterCase {
            member: "VersionLabel",
            value: "v1".to_string(),
            expected_count: 5,
            matches: Box::new(|event| event.version_label == "v1"),
        },
        FilterCase {
            member: "VersionLabel",
            value: "v9".to_string(),
            expected_count: 0,
            matches: Box::new(|_| false),
        },
        FilterCase {
            member: "EnvironmentName",
            value: "env-one".to_string(),
            expected_count: 2,
            matches: Box::new(|event| event.environment_name == "env-one"),
        },
        FilterCase {
            member: "EnvironmentName",
            value: "env-none".to_string(),
            expected_count: 0,
            matches: Box::new(|_| false),
        },
        FilterCase {
            // No configuration template exists — `CreateConfigurationTemplate`
            // is its own operation — so no event carries a template name.
            member: "TemplateName",
            value: "no-template".to_string(),
            expected_count: 0,
            matches: Box::new(|_| false),
        },
        FilterCase {
            member: "PlatformArn",
            value: platform_arn.clone(),
            expected_count: 2,
            matches: {
                let wanted = platform_arn.clone();
                Box::new(move |event| event.platform_arn == wanted)
            },
        },
        FilterCase {
            // One request's activity: the createEnvironment call and the launch
            // completion the clock owed it.
            member: "RequestId",
            value: launch_request_id,
            expected_count: 2,
            matches: Box::new(move |event| event.request_id == wanted_request_id),
        },
    ];

    for row in table {
        let case = format!("{}={}", row.member, row.value);
        let expected: Vec<&ReportedEvent> = all.iter().filter(|event| (row.matches)(event)).collect();
        assert_eq!(
            expected.len(),
            row.expected_count,
            "case {case:?}: the unfiltered log holds {} matching events, not the {} this row declares",
            expected.len(),
            row.expected_count
        );
        let selected = events_of(&emulator, &case, &[(row.member, row.value.as_str())]).await;
        assert_eq!(
            selected.len(),
            expected.len(),
            "case {case:?}: expected {} events, got {selected:?}",
            expected.len()
        );
        for (got, want) in selected.iter().zip(&expected) {
            assert_eq!(
                &got, want,
                "case {case:?}: the filter must select the log's own events, in the log's order"
            );
        }
    }

    // `EnvironmentId` is not an `EventDescription` member, so the filter reads
    // the id the event was recorded against: it must select the same events as
    // the name of the environment that id identifies.
    let by_id = events_of(
        &emulator,
        "EnvironmentId=env-one",
        &[("EnvironmentId", env_one_id.as_str())],
    )
    .await;
    let by_name = events_of(
        &emulator,
        "EnvironmentName=env-one",
        &[("EnvironmentName", "env-one")],
    )
    .await;
    assert_eq!(
        by_id, by_name,
        "case \"EnvironmentId={env_one_id}\": filtering by id must select the same events as filtering by name"
    );
    assert!(
        !by_id.is_empty(),
        "case \"EnvironmentId={env_one_id}\": expected the environment's own events"
    );
    let unknown = events_of(
        &emulator,
        "EnvironmentId=e-00000000",
        &[("EnvironmentId", "e-00000000")],
    )
    .await;
    assert!(
        unknown.is_empty(),
        "case \"EnvironmentId=e-00000000\": an id no environment holds selects nothing, got {unknown:?}"
    );

    // Two filters narrow to their intersection, not to either one alone.
    let both = events_of(
        &emulator,
        "ApplicationName=app-one&VersionLabel=v1",
        &[("ApplicationName", "app-one"), ("VersionLabel", "v1")],
    )
    .await;
    let expected: Vec<&ReportedEvent> = all
        .iter()
        .filter(|event| event.application_name == "app-one" && event.version_label == "v1")
        .collect();
    assert_eq!(
        both.len(),
        expected.len(),
        "case \"ApplicationName=app-one&VersionLabel=v1\": expected {} events, got {both:?}",
        expected.len()
    );
}

// ---------------------------------------------------------------------------
// The ordering and the pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_backdated_clock_owed_event_is_reported_in_its_place_in_time() {
    let emulator = Emulator::new();
    let case = "backdated ordering";

    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", "order-app")],
    )
    .await;
    ok(
        &emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", "order-app"),
            ("EnvironmentName", "order-env"),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ],
    )
    .await;
    // The launch completion is now due, and dated one transition span after the
    // create that owed it.
    tokio::time::sleep(PAST_DUE).await;
    // `CreateApplication` settles only the application removals the clock owes,
    // never an environment's launch, so the launch completion is MATERIALISED
    // after this call's events are appended — yet it is DATED before them.
    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", "order-app-later")],
    )
    .await;

    let all = events_of(&emulator, case, &[]).await;
    let messages: Vec<&str> = all.iter().map(|event| event.message.as_str()).collect();
    assert_eq!(
        messages,
        vec![
            "createApplication completed successfully.",
            "Created new Application: order-app-later",
            "createApplication is starting.",
            "Successfully launched environment: order-env",
            "createEnvironment is starting.",
            "createApplication completed successfully.",
            "Created new Application: order-app",
            "createApplication is starting.",
        ],
        "case {case:?}: the answer must be ordered by EventDate, not by the order the log was appended to"
    );
    assert_eq!(
        all[0].application_name, "order-app-later",
        "case {case:?}: the newest-dated event must come first, got {all:?}"
    );
    for pair in all.windows(2) {
        let (newer, older) = (&pair[0], &pair[1]);
        assert!(
            at_secs(&newer.event_date, case) >= at_secs(&older.event_date, case),
            "case {case:?}: {:?} must not pre-date {:?}",
            newer.message,
            older.message
        );
    }
}

#[tokio::test]
async fn describe_events_reports_most_recent_first_and_pages_through_the_whole_log() {
    let emulator = Emulator::new();
    let case = "paging";
    const COUNT: usize = 7;

    for index in 0..COUNT {
        let name = format!("page-app-{index:02}");
        ok(
            &emulator,
            case,
            "CreateApplication",
            &[("ApplicationName", name.as_str())],
        )
        .await;
    }

    let all = events_of(&emulator, case, &[]).await;
    // Each create records its whole activity: the operation starting, the
    // application it created, and the operation completing.
    const EVENTS_PER_CREATE: usize = 3;
    assert_eq!(
        all.len(),
        COUNT * EVENTS_PER_CREATE,
        "case {case:?}: expected one activity of {EVENTS_PER_CREATE} events per \
         CreateApplication, got {all:?}"
    );
    // "beginning with the most recent event".
    let reported_order: Vec<&str> = all
        .iter()
        .map(|event| event.application_name.as_str())
        .collect();
    let newest_first: Vec<String> = (0..COUNT)
        .rev()
        .flat_map(|index| {
            std::iter::repeat_n(format!("page-app-{index:02}"), EVENTS_PER_CREATE)
        })
        .collect();
    assert_eq!(
        reported_order,
        newest_first.iter().map(String::as_str).collect::<Vec<&str>>(),
        "case {case:?}: the log must be reported newest first"
    );

    // Every page size from one to past the log's length walks the same log in
    // the same order, in pages no larger than it asked for.
    for page_size in [1usize, 2, 3, 6, 7, 8, 20, 21, 22, 1000] {
        let case = format!("MaxRecords={page_size}");
        let max_records = page_size.to_string();
        let mut collected: Vec<ReportedEvent> = Vec::new();
        let mut token: Option<String> = None;
        for round in 0..=all.len() + 1 {
            assert!(
                round <= all.len(),
                "case {case:?}: pagination did not terminate after {round} pages"
            );
            let carried = token.clone().unwrap_or_default();
            let mut params: Vec<(&str, &str)> = vec![("MaxRecords", max_records.as_str())];
            if token.is_some() {
                params.push(("NextToken", carried.as_str()));
            }
            let answer = ok(&emulator, &case, "DescribeEvents", &params).await;
            let page = reported(&answer);
            let remaining = all.len() - collected.len();
            assert_eq!(
                page.len(),
                remaining.min(page_size),
                "case {case:?}: page {round} must hold min(MaxRecords, remaining) events, got {page:?}"
            );
            collected.extend(page);
            match answer.value("NextToken") {
                // "If returned, this indicates that there are more results to
                // obtain."
                Some(next) => {
                    assert!(
                        collected.len() < all.len(),
                        "case {case:?}: a NextToken was returned with nothing left to obtain"
                    );
                    token = Some(next.to_string());
                }
                None => {
                    assert_eq!(
                        collected.len(),
                        all.len(),
                        "case {case:?}: the last page must leave nothing behind, got {collected:?}"
                    );
                    break;
                }
            }
        }
        assert_eq!(
            collected, all,
            "case {case:?}: paging must walk the same log, in the same order, as one unpaged call"
        );
    }

    // `MaxRecords` is modelled `range 1..1000`: both bounds are accepted and
    // one past each is refused before the handler runs.
    for (max_records, accepted) in [("1", true), ("1000", true), ("0", false), ("1001", false)] {
        let case = format!("MaxRecords={max_records}");
        let answer = emulator
            .call("DescribeEvents", &[("MaxRecords", max_records)])
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a MaxRecords inside the modelled range must be accepted, got {} and body {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a MaxRecords outside the modelled range must be refused, got {} and body {}",
                answer.status, answer.body
            );
        }
    }

    // A token the log does not hold is refused, not silently read as page one.
    let refused = emulator
        .call("DescribeEvents", &[("NextToken", "not-a-token")])
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"NextToken=not-a-token\": expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case \"NextToken=not-a-token\": unexpected error code in {}",
        refused.body
    );
}

#[tokio::test]
async fn a_page_token_keeps_its_place_when_the_log_grows_between_calls() {
    let emulator = Emulator::new();
    let case = "token stability";

    for index in 0..4 {
        let name = format!("token-app-{index:02}");
        ok(
            &emulator,
            case,
            "CreateApplication",
            &[("ApplicationName", name.as_str())],
        )
        .await;
    }

    // The log as it stands before the second page is asked for: the walk must
    // report its head, in its order.
    let before = events_of(&emulator, case, &[]).await;
    let first = ok(&emulator, case, "DescribeEvents", &[("MaxRecords", "2")]).await;
    let page_one = reported(&first);
    let token = first
        .value("NextToken")
        .unwrap_or_else(|| panic!("case {case:?}: expected a NextToken, got {}", first.body))
        .to_string();

    // A NEWER event arrives between the two page calls. It belongs at the HEAD
    // of the reported order, so a token resolved by position in the previous
    // answer would shift the boundary and the second page would repeat an event.
    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", "token-app-late")],
    )
    .await;

    let second = ok(
        &emulator,
        case,
        "DescribeEvents",
        &[("MaxRecords", "2"), ("NextToken", token.as_str())],
    )
    .await;
    let page_two = reported(&second);

    let walked: Vec<&ReportedEvent> = page_one.iter().chain(page_two.iter()).collect();
    let expected: Vec<&ReportedEvent> = before.iter().take(walked.len()).collect();
    assert_eq!(
        walked, expected,
        "case {case:?}: the two pages must walk the log the first page saw, in order, with nothing repeated or skipped"
    );
    assert!(
        !walked
            .iter()
            .any(|event| event.application_name == "token-app-late"),
        "case {case:?}: an event recorded after the first page must not appear inside the walk, got {walked:?}"
    );
}

// ---------------------------------------------------------------------------
// The severity filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_severity_filter_includes_the_named_severity_and_every_higher_one() {
    let emulator = Emulator::new();
    let case = "severity";

    // The ladder is the modelled value set, re-ordered by severity.
    let mut ladder = SEVERITY_LADDER.to_vec();
    ladder.sort_unstable();
    assert_eq!(
        ladder,
        eb::ops::types::EVENT_SEVERITY_VALUES.to_vec(),
        "case {case:?}: the severity ladder must be exactly the modelled EventSeverity set"
    );

    for name in ["sev-app-a", "sev-app-b", "sev-app-c"] {
        ok(
            &emulator,
            case,
            "CreateApplication",
            &[("ApplicationName", name)],
        )
        .await;
    }
    let all = events_of(&emulator, case, &[]).await;
    assert!(
        !all.is_empty(),
        "case {case:?}: expected a non-empty log to filter"
    );

    for (rank, severity) in SEVERITY_LADDER.iter().enumerate() {
        let case = format!("Severity={severity}");
        let expected: Vec<&ReportedEvent> = all
            .iter()
            .filter(|event| severity_rank(&event.severity, &case) >= rank)
            .collect();
        let selected = events_of(&emulator, &case, &[("Severity", severity)]).await;
        assert_eq!(
            selected.len(),
            expected.len(),
            "case {case:?}: expected {} of {} events, got {selected:?}",
            expected.len(),
            all.len()
        );
        for event in &selected {
            assert!(
                severity_rank(&event.severity, &case) >= rank,
                "case {case:?}: {:?} is below the requested severity",
                event.severity
            );
        }
    }

    // The boundary the stored events sit on: `INFO` and below select the whole
    // log, every severity above it selects nothing.
    for severity in ["TRACE", "DEBUG", "INFO"] {
        let case = format!("Severity={severity}");
        let selected = events_of(&emulator, &case, &[("Severity", severity)]).await;
        assert_eq!(
            selected.len(),
            all.len(),
            "case {case:?}: an INFO log is at or above {severity}, got {selected:?}"
        );
    }
    for severity in ["WARN", "ERROR", "FATAL"] {
        let case = format!("Severity={severity}");
        let selected = events_of(&emulator, &case, &[("Severity", severity)]).await;
        assert!(
            selected.is_empty(),
            "case {case:?}: an INFO log is below {severity}, got {selected:?}"
        );
    }

    // A value `EventSeverity` does not model is refused, not ignored.
    let refused = emulator
        .call("DescribeEvents", &[("Severity", "VERBOSE")])
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"Severity=VERBOSE\": expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
}

// ---------------------------------------------------------------------------
// The bounds on the log
// ---------------------------------------------------------------------------

/// The two bounds are on the STORE, not on the wire: the count bound needs more
/// events than an HTTP test can drive and the age bound needs events older than
/// the process, so both are exercised against the state the handlers write
/// through, the way `state_roundtrip.rs` does.
#[test]
fn the_event_log_keeps_its_newest_records_inside_its_count_bound() {
    let mut store = eb::ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::default();
    let over = eb::state::MAX_RETAINED_EVENTS + 5;
    for index in 0..over {
        store.record_event(
            "00000000-0000-4000-8000-000000000000",
            eb::state::SEVERITY_INFO,
            format!("event {index}"),
            eb::state::EventSubject::default(),
        );
    }
    let case = "count bound";
    assert_eq!(
        store.events.len(),
        eb::state::MAX_RETAINED_EVENTS,
        "case {case:?}: the log must not grow past its bound, got {}",
        store.events.len()
    );
    assert_eq!(
        store.events.last().map(|event| event.message.as_str()),
        Some(format!("event {}", over - 1).as_str()),
        "case {case:?}: the newest event must be kept"
    );
    assert_eq!(
        store.events.first().map(|event| event.message.as_str()),
        Some("event 5"),
        "case {case:?}: the OLDEST events are the ones dropped"
    );
}

#[test]
fn an_event_older_than_the_reporting_window_is_dropped_when_the_clock_is_consulted() {
    let mut store = eb::ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::default();
    let case = "6-week window";
    let now = DateTime::from(std::time::SystemTime::now()).as_secs_f64();
    let retention = eb::state::EVENT_RETENTION.as_secs_f64();
    // One a minute outside the window, one a minute inside it.
    for (message, at) in [("stale", now - retention - 60.0), ("fresh", now - 60.0)] {
        store.record_event_at(
            "00000000-0000-4000-8000-000000000000",
            eb::state::SEVERITY_INFO,
            message,
            eb::state::EventSubject::default(),
            DateTime::from_secs_f64(at),
        );
    }
    store.materialize_due_transitions();
    let kept: Vec<&str> = store
        .events
        .iter()
        .map(|event| event.message.as_str())
        .collect();
    assert_eq!(
        kept,
        vec!["fresh"],
        "case {case:?}: only the event inside the 6-week window is reportable, got {kept:?}"
    );
}

// ---------------------------------------------------------------------------
// The time window
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_time_window_includes_start_time_and_excludes_end_time() {
    let emulator = Emulator::new();
    let case = "window";

    for name in ["win-app-a", "win-app-b", "win-app-c"] {
        ok(
            &emulator,
            case,
            "CreateApplication",
            &[("ApplicationName", name)],
        )
        .await;
        // Far enough apart that the window bounds below fall between the
        // events rather than on a tie.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let all = events_of(&emulator, case, &[]).await;
    assert_eq!(
        all.len(),
        9,
        "case {case:?}: expected one activity of three events per CreateApplication, got {all:?}"
    );
    // Every reported date as the exact `(seconds, subsecond nanos)` pair the window
    // is asserted on, so a pair of events a microsecond apart is ordered here the
    // same way the service must order them.
    let dates: Vec<(i64, u32)> = all
        .iter()
        .map(|event| at_parts(&event.event_date, case))
        .collect();
    let newest = all[0].event_date.clone();
    let oldest = all[all.len() - 1].event_date.clone();
    let newest_parts = dates[0];
    let oldest_parts = dates[dates.len() - 1];
    let future = shifted_timestamp(newest_parts, 3_600, case);
    let past = shifted_timestamp(oldest_parts, -3_600, case);

    // Each row is a window and the bounds it stands for: `StartTime` is "those
    // that occur ON OR AFTER this time", `EndTime` is "up to, but NOT
    // INCLUDING, the EndTime".
    struct WindowCase<'a> {
        filters: Vec<(&'a str, &'a str)>,
        /// The inclusive lower bound the filters stand for.
        start: Option<(i64, u32)>,
        /// The exclusive upper bound the filters stand for.
        end: Option<(i64, u32)>,
    }
    let table: Vec<WindowCase<'_>> = vec![
        WindowCase {
            filters: vec![("StartTime", oldest.as_str())],
            start: Some(oldest_parts),
            end: None,
        },
        WindowCase {
            filters: vec![("StartTime", newest.as_str())],
            start: Some(newest_parts),
            end: None,
        },
        WindowCase {
            filters: vec![("StartTime", past.as_str())],
            start: Some((oldest_parts.0 - 3_600, oldest_parts.1)),
            end: None,
        },
        WindowCase {
            filters: vec![("StartTime", future.as_str())],
            start: Some((newest_parts.0 + 3_600, newest_parts.1)),
            end: None,
        },
        WindowCase {
            filters: vec![("EndTime", oldest.as_str())],
            start: None,
            end: Some(oldest_parts),
        },
        WindowCase {
            filters: vec![("EndTime", newest.as_str())],
            start: None,
            end: Some(newest_parts),
        },
        WindowCase {
            filters: vec![("EndTime", past.as_str())],
            start: None,
            end: Some((oldest_parts.0 - 3_600, oldest_parts.1)),
        },
        WindowCase {
            filters: vec![("EndTime", future.as_str())],
            start: None,
            end: Some((newest_parts.0 + 3_600, newest_parts.1)),
        },
        WindowCase {
            filters: vec![
                ("StartTime", oldest.as_str()),
                ("EndTime", newest.as_str()),
            ],
            start: Some(oldest_parts),
            end: Some(newest_parts),
        },
    ];

    for row in table {
        let (filters, start, end) = (row.filters, row.start, row.end);
        let case = format!("{filters:?}");
        let expected = dates
            .iter()
            .filter(|at| {
                start.is_none_or(|start| **at >= start) && end.is_none_or(|end| **at < end)
            })
            .count();
        let selected = events_of(&emulator, &case, &filters).await;
        assert_eq!(
            selected.len(),
            expected,
            "case {case:?}: expected {expected} of {} events, got {selected:?}",
            all.len()
        );
        for event in &selected {
            let at = at_parts(&event.event_date, &case);
            assert!(
                start.is_none_or(|start| at >= start),
                "case {case:?}: {:?} is before StartTime",
                event.event_date
            );
            assert!(
                end.is_none_or(|end| at < end),
                "case {case:?}: {:?} is at or after EndTime",
                event.event_date
            );
        }
    }

    // The exclusive end and the inclusive start together: the newest event is
    // the one the two bounds disagree about.
    let from_newest = events_of(
        &emulator,
        "StartTime=newest",
        &[("StartTime", newest.as_str())],
    )
    .await;
    let until_newest = events_of(&emulator, "EndTime=newest", &[("EndTime", newest.as_str())]).await;
    assert_eq!(
        from_newest.len() + until_newest.len(),
        all.len(),
        "case {case:?}: StartTime and EndTime at one instant must partition the log, got {from_newest:?} and {until_newest:?}"
    );
}

#[tokio::test]
async fn a_bound_echoed_back_from_a_reported_date_windows_on_that_very_instant() {
    let emulator = Emulator::new();
    let case = "echoed bound";

    // No sleep between the calls, so the log holds events recorded inside the same
    // millisecond: the rule under test is that a bound a client takes from a
    // reported `EventDate` and sends back names THAT instant, however close two
    // events are, and the rows are every reported date in such a log.
    for name in ["echo-app-a", "echo-app-b", "echo-app-c", "echo-app-d"] {
        ok(
            &emulator,
            case,
            "CreateApplication",
            &[("ApplicationName", name)],
        )
        .await;
    }

    let all = events_of(&emulator, case, &[]).await;
    assert!(
        all.len() >= 4,
        "case {case:?}: expected the CreateApplication activities to be logged, got {all:?}"
    );
    let dates: Vec<(i64, u32)> = all
        .iter()
        .map(|event| at_parts(&event.event_date, case))
        .collect();

    for (index, event) in all.iter().enumerate() {
        let bound = event.event_date.as_str();
        let at = dates[index];
        let row = format!("bound {bound:?}");

        // "StartTime: … those that occur on or after this time" (inclusive) and
        // "EndTime: … up to, but not including, the EndTime" (exclusive) at ONE
        // instant partition the log, and each side holds exactly the dates on its
        // side of that instant.
        let from = events_of(&emulator, &row, &[("StartTime", bound)]).await;
        let until = events_of(&emulator, &row, &[("EndTime", bound)]).await;

        let expected_from = dates.iter().filter(|date| **date >= at).count();
        let expected_until = dates.iter().filter(|date| **date < at).count();
        assert_eq!(
            from.len(),
            expected_from,
            "case {row:?}: expected {expected_from} events on or after it, got {from:?}"
        );
        assert_eq!(
            until.len(),
            expected_until,
            "case {row:?}: expected {expected_until} events before it, got {until:?}"
        );
        assert_eq!(
            from.len() + until.len(),
            all.len(),
            "case {row:?}: the inclusive start and the exclusive end must partition \
             the log, got {from:?} and {until:?}"
        );
        assert!(
            from.iter()
                .any(|reported| reported.event_date == event.event_date),
            "case {row:?}: the event the bound was read from occurs on that instant, \
             so StartTime must select it, got {from:?}"
        );
        for reported in &from {
            let reported_at = at_parts(&reported.event_date, &row);
            assert!(
                reported_at >= at,
                "case {row:?}: {:?} is before the bound",
                reported.event_date
            );
        }
        for reported in &until {
            let reported_at = at_parts(&reported.event_date, &row);
            assert!(
                reported_at < at,
                "case {row:?}: {:?} is at or after the bound",
                reported.event_date
            );
        }
    }
}
