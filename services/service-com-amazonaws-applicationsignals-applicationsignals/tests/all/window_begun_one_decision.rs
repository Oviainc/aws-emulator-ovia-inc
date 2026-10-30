// SPDX-License-Identifier: CC0-1.0

//! That "has the requested period begun?" is ONE decision per read, end to end over HTTP
//! through the router production mounts.
//!
//! The rule every row asserts: whether the requested period has begun says nothing about any
//! stored record — it is a property of the REQUEST — so each windowed read decides it once,
//! from the single instant that read is served at, and admits or rejects EVERY entity of
//! that one answer on that one decision. A read therefore never answers with a proper subset
//! of what it holds, and a period that had begun for one answer has begun for every later
//! one: the answer is a function of the request and the store, not of how far through the
//! store the walk had got when the clock moved.
//!
//! The rows vary what the rule generalises over: the SIGN and the SIZE of the gap between
//! the requested `StartTime` and the instant the read is served at (an hour and a second
//! behind it, exactly on it, a few hundred microseconds, a few milliseconds and tens of
//! milliseconds ahead of it, a minute and an hour ahead), how many services the store holds
//! evidence of (one, a few, many), and which read makes the decision (`ListServices` over
//! every stored service, `ListServiceDependents` over every edge into one shared
//! dependency, and `GetService` on each identity separately).
//!
//! Every row's objectives are created BEFORE the window bounds are chosen, so the half of
//! the window test that looks at a record — the creation against the window's end — admits
//! all of them, and the only thing left to decide is the request's own half.
//!
//! No request can name the instant a read is served at, so the second test hands each
//! projection ONE chosen instant through the store the handlers share — the one engine
//! [`crate::instrumentation_configuration_tags::router_with_engine`] hands back, as this
//! crate's other clock-exact suites do — and pins that every projection decides the period
//! on the instant it was given rather than on a clock it reads for itself.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::ops::types::{
    AuditTarget, AuditTargetEntity, ServiceEntity,
};
use service_com_amazonaws_applicationsignals_applicationsignals::state::ComAmazonawsApplicationsignalsApplicationsignalsState;
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_tags::router_with_engine;
use crate::slo_lifecycle::{call, create, router};
// The instant arithmetic, the request spelling of an instant and one service identity come
// from the suite that owns the window-resolution rule, so this crate's tests carry one copy
// of each.
use crate::window_instant_resolution::{entity, query_instant, shifted, Instant, NANOS_PER_SECOND};

/// How long every requested window runs for, from the `StartTime` the row chose: an hour, so
/// the window's end is past every stored creation whatever the row shifts its start by.
const WINDOW_SECONDS: i64 = 3_600;

/// The page bound every read here states, so one answer carries every entity the store holds
/// evidence of rather than a first page of it: the modelled maximum of both reads'
/// `MaxResults` (`smithy_query(operation="ListServices")` and
/// `smithy_query(operation="ListServiceDependents")`: `range 1..100`).
const PAGE_BOUND: i32 = 100;

/// How many times each row asks the SAME window, so that an answer which reported the period
/// as begun cannot be followed by one that reports it as not yet begun.
const ROUNDS: usize = 4;

/// The operation each stored objective monitors, the dependency every one of them names and
/// the operation of that dependency they call — one shared callee, so a single
/// `ListServiceDependents` answer covers every stored record.
const OPERATION: &str = "POST /checkout";
const DEPENDENCY: &str = "payments";
const DEPENDENCY_OPERATION: &str = "Charge";

/// The instant this test is at, at the resolution a window bound is compared on.
fn sampled_now() -> Instant {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch");
    let secs = i64::try_from(since_epoch.as_secs()).expect("epoch seconds fit an i64");
    (secs, since_epoch.subsec_nanos())
}

/// The name of the `index`th service a row stores evidence of. Zero-padded, so the name order
/// the topology reads report in is the index order.
fn service_name(index: usize) -> String {
    format!("svc-{index:03}")
}

/// Store one objective per service, each monitoring an operation of its own service and
/// calling the one shared dependency: `count` distinct service entities, every one of them
/// evidence over any window whose end is past this moment.
async fn store_objectives(router: &MultiServiceRouter, case: &str, count: usize) {
    for index in 0..count {
        let service = service_name(index);
        let (status, answer) = create(
            router,
            json!({
                "Name": format!("{service}-latency"),
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": entity(&service),
                        "OperationName": OPERATION,
                        "MetricType": "LATENCY",
                        "DependencyConfig": {
                            "DependencyKeyAttributes": entity(DEPENDENCY),
                            "DependencyOperationName": DEPENDENCY_OPERATION
                        },
                        "PeriodSeconds": 60
                    },
                    "MetricThreshold": 100.0,
                    "ComparisonOperator": "LessThan"
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create of {service:?} to answer 200, got {status} \
             with {answer:?}"
        );
    }
}

/// The service names an answer's `member` list reports, each read off the `identity` member
/// of its entry.
fn reported_names(answer: &Value, case: &str, member: &str, identity: &str) -> Vec<String> {
    answer[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} list, got {answer:?}"))
        .iter()
        .map(|entry| {
            entry[identity]["Name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a {identity} Name, got {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// The services `ListServices` reports over the window, in the order it reports them.
async fn reported_services(
    router: &MultiServiceRouter,
    case: &str,
    start: Instant,
    end: Instant,
) -> Vec<String> {
    let uri = format!(
        "/services?StartTime={}&EndTime={}&MaxResults={PAGE_BOUND}",
        query_instant(start),
        query_instant(end)
    );
    let (status, answer) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListServices, got {status} with {answer:?}"
    );
    reported_names(&answer, case, "ServiceSummaries", "KeyAttributes")
}

/// The services `ListServiceDependents` reports as calling the shared dependency over the
/// window, in the order it reports them.
async fn reported_dependents(
    router: &MultiServiceRouter,
    case: &str,
    start: Instant,
    end: Instant,
) -> Vec<String> {
    let uri = format!(
        "/service-dependents?StartTime={}&EndTime={}&MaxResults={PAGE_BOUND}",
        query_instant(start),
        query_instant(end)
    );
    let (status, answer) = call(
        router,
        "POST",
        &uri,
        Some(json!({ "KeyAttributes": entity(DEPENDENCY) })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListServiceDependents, got {status} with \
         {answer:?}"
    );
    reported_names(&answer, case, "ServiceDependents", "DependentKeyAttributes")
}

/// Whether `GetService` answers for one identity with the DISCOVERED projection over the
/// window: an entity the store is evidence of carries the metrics Application Signals
/// collects for it, and one it observed nothing for carries none.
async fn discovered(
    router: &MultiServiceRouter,
    case: &str,
    service: &str,
    start: Instant,
    end: Instant,
) -> bool {
    let uri = format!(
        "/service?StartTime={}&EndTime={}",
        query_instant(start),
        query_instant(end)
    );
    let (status, answer) = call(
        router,
        "POST",
        &uri,
        Some(json!({ "KeyAttributes": entity(service) })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetService on {service:?}, got {status} with \
         {answer:?}"
    );
    !answer["Service"]["MetricReferences"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a MetricReferences list, got {answer:?}"))
        .is_empty()
}

/// What the rule settles for a row's window, given where its `StartTime` sits relative to the
/// instant the reads are served at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Observed {
    /// The period had begun before the first read was served, so every answer reports every
    /// entity the store holds evidence of.
    Every,
    /// The period had not begun by the last read, so every answer reports none.
    Nothing,
    /// The period begins so close to the reads that either answer is correct — but ONE of
    /// them for the whole answer, which is what the row pins.
    Either,
}

/// One row: where the requested `StartTime` sits relative to the instant the reads are served
/// at, and what the rule settles for it.
#[derive(Debug)]
struct Row {
    label: &'static str,
    start_offset_nanos: i64,
    observed: Observed,
}

/// The table: a requested start on either side of the serving instant and exactly on it, at
/// gaps from an hour down to a few hundred microseconds.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a start an hour behind the serving instant",
            start_offset_nanos: -WINDOW_SECONDS * NANOS_PER_SECOND,
            observed: Observed::Every,
        },
        Row {
            label: "a start a second behind the serving instant",
            start_offset_nanos: -NANOS_PER_SECOND,
            observed: Observed::Every,
        },
        Row {
            label: "a start exactly on the serving instant",
            start_offset_nanos: 0,
            observed: Observed::Every,
        },
        Row {
            label: "a start a few hundred microseconds ahead of the serving instant",
            start_offset_nanos: 300_000,
            observed: Observed::Either,
        },
        Row {
            label: "a start five milliseconds ahead of the serving instant",
            start_offset_nanos: 5 * 1_000_000,
            observed: Observed::Either,
        },
        Row {
            label: "a start fifty milliseconds ahead of the serving instant",
            start_offset_nanos: 50 * 1_000_000,
            observed: Observed::Either,
        },
        Row {
            label: "a start a minute ahead of the serving instant",
            start_offset_nanos: 60 * NANOS_PER_SECOND,
            observed: Observed::Nothing,
        },
        Row {
            label: "a start an hour ahead of the serving instant",
            start_offset_nanos: WINDOW_SECONDS * NANOS_PER_SECOND,
            observed: Observed::Nothing,
        },
    ]
}

/// How many services each row stores evidence of: one, a few, and many — the dimension the
/// per-record decision would have shown itself along, since a walk over many records is a
/// walk the clock can move during.
const STORES: [usize; 3] = [1, 6, 64];

/// Every windowed read decides whether the requested period has begun ONCE for the whole
/// answer, so it reports every entity it holds evidence of or none of them, never a subset —
/// and a period one answer reported as begun is begun for every later answer.
#[tokio::test]
async fn one_answer_admits_every_entity_or_none_of_them() {
    for count in STORES {
        for row in rows() {
            let case = format!("{} / {count} stored services", row.label);
            let router = router();
            store_objectives(&router, &case, count).await;
            let expected: Vec<String> = (0..count).map(service_name).collect();

            // Sampled AFTER the writes, so the gap the row states is a gap from the instant
            // the reads below are served at rather than from the instant the store was
            // filled at.
            let start = shifted(sampled_now(), row.start_offset_nanos);
            let end = shifted(start, WINDOW_SECONDS * NANOS_PER_SECOND);

            let mut begun = false;
            for round in 0..ROUNDS {
                let case = format!("{case} / round {round}");
                let services = reported_services(&router, &case, start, end).await;
                let dependents = reported_dependents(&router, &case, start, end).await;

                for (read, reported) in [
                    ("ListServices", &services),
                    ("ListServiceDependents", &dependents),
                ] {
                    assert!(
                        reported.is_empty() || *reported == expected,
                        "case {case:?}: {read} decides one window once, so it reports every \
                         stored service or none of them; expected {expected:?} or [], got \
                         {reported:?}"
                    );
                }

                // The dependents read was served after the services read, and a period that
                // has begun stays begun, so it cannot have observed less.
                if !services.is_empty() {
                    assert_eq!(
                        dependents, expected,
                        "case {case:?}: ListServices reported {expected:?} over this window, so \
                         the ListServiceDependents served after it reports every dependent; got \
                         {dependents:?}"
                    );
                }
                if begun {
                    assert_eq!(
                        services, expected,
                        "case {case:?}: an earlier answer reported this window as begun, so \
                         ListServices still reports {expected:?}; got {services:?}"
                    );
                    assert_eq!(
                        dependents, expected,
                        "case {case:?}: an earlier answer reported this window as begun, so \
                         ListServiceDependents still reports {expected:?}; got {dependents:?}"
                    );
                }
                begun = begun || !services.is_empty() || !dependents.is_empty();

                match row.observed {
                    Observed::Every => {
                        assert_eq!(
                            services, expected,
                            "case {case:?}: the requested period had begun, so ListServices \
                             reports {expected:?}; got {services:?}"
                        );
                        assert_eq!(
                            dependents, expected,
                            "case {case:?}: the requested period had begun, so \
                             ListServiceDependents reports {expected:?}; got {dependents:?}"
                        );
                    }
                    Observed::Nothing => {
                        assert!(
                            services.is_empty() && dependents.is_empty(),
                            "case {case:?}: the requested period has not begun, so nothing was \
                             observed in it; got {services:?} from ListServices and \
                             {dependents:?} from ListServiceDependents"
                        );
                    }
                    Observed::Either => {}
                }
            }

            // `GetService` answers for one identity at a time, and it cannot disagree with the
            // listing about which of them the store is evidence of: once an answer reports the
            // window as begun, every identity asked about after it is discovered.
            let listed = reported_services(&router, &case, start, end).await;
            let mut any_discovered = false;
            for service in &expected {
                let found = discovered(&router, &case, service, start, end).await;
                if !listed.is_empty() {
                    assert!(
                        found,
                        "case {case:?}: ListServices reported {listed:?} as discovered over this \
                         window, so GetService on {service:?} answers with a discovered entity; \
                         got MetricReferences empty"
                    );
                }
                if any_discovered {
                    assert!(
                        found,
                        "case {case:?}: GetService already reported a discovered entity over this \
                         window, so {service:?} is discovered too; got MetricReferences empty"
                    );
                }
                any_discovered = any_discovered || found;
            }
            if row.observed == Observed::Nothing {
                assert!(
                    !any_discovered,
                    "case {case:?}: the requested period has not begun, so GetService reports \
                     every identity as undiscovered; got a discovered one"
                );
            }
        }
    }
}

/// The key attributes of one Application Signals service entity, as the store holds them.
fn attributes(name: &str) -> HashMap<String, String> {
    HashMap::from([
        ("Type".to_string(), "Service".to_string()),
        ("Name".to_string(), name.to_string()),
        ("Environment".to_string(), "prod".to_string()),
    ])
}

/// One audit target naming a service entity by its attributes, as `ListAuditFindings` carries
/// it (`smithy_query(shape="ServiceEntity")`: `Type`, `Name` and `Environment` are the
/// `Attributes` entries of the identity).
fn audit_target(name: &str) -> AuditTarget {
    AuditTarget {
        r#type: "service".to_string(),
        data: AuditTargetEntity::Service(ServiceEntity {
            r#type: Some("Service".to_string()),
            name: Some(name.to_string()),
            environment: Some("prod".to_string()),
            aws_account_id: None,
        }),
    }
}

/// How many services the deterministic rows below store evidence of.
const DETERMINISTIC_STORE: usize = 2;

/// Every projection decides whether the requested period has begun on the ONE instant it was
/// given — the sample its handler took when it started serving — rather than on a clock of
/// its own, so a projection handed an instant before the period's start observes nothing and
/// one handed an instant at or after it observes everything, however far the wall clock has
/// since moved.
///
/// The rows vary the SIGN and SIZE of the gap between the given instant and the requested
/// start, down to a single nanosecond on either side of it and exactly on it (the bound is
/// inclusive, as every other window bound of this service is), and every one of the seven
/// windowed projections is asked on each of them: one decision, one answer, for all of them.
#[tokio::test]
async fn every_projection_decides_the_period_on_the_instant_it_was_given() {
    let (router, engine) = router_with_engine();
    store_objectives(&router, "the shared store", DETERMINISTIC_STORE).await;

    // A window that began an hour ago and ends an hour from now, so the stored creations are
    // inside it and the only thing left to decide is whether the period has begun — which the
    // rows then settle with the instant they hand the projections, NOT with the wall clock
    // (which is an hour past this start in every row).
    let base = sampled_now();
    let start = shifted(base, -WINDOW_SECONDS * NANOS_PER_SECOND);
    let window_start = DateTime::from_secs_and_nanos(start.0, start.1);
    let end = shifted(base, WINDOW_SECONDS * NANOS_PER_SECOND);
    let window_end = DateTime::from_secs_and_nanos(end.0, end.1);

    let rows: Vec<(&str, i64, bool)> = vec![
        ("an hour before the period starts", -WINDOW_SECONDS * NANOS_PER_SECOND, false),
        ("a second before the period starts", -NANOS_PER_SECOND, false),
        ("a microsecond before the period starts", -1_000, false),
        ("a nanosecond before the period starts", -1, false),
        ("exactly when the period starts", 0, true),
        ("a nanosecond after the period starts", 1, true),
        ("a second after the period starts", NANOS_PER_SECOND, true),
        ("an hour after the period starts", WINDOW_SECONDS * NANOS_PER_SECOND, true),
    ];

    let monitored = attributes(&service_name(0));
    let callee = attributes(DEPENDENCY);
    let targets: Vec<AuditTarget> = (0..DETERMINISTIC_STORE)
        .map(|index| audit_target(&service_name(index)))
        .collect();

    for (label, offset, begun) in rows {
        let served = shifted(start, offset);
        let now = DateTime::from_secs_and_nanos(served.0, served.1);
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let store = state.lock();

        // One entry per observation when the period has begun, none when it has not — the
        // counts the store holds evidence for: one service entity per objective, one
        // operation and one dependency edge each, and every one of them calling the one
        // shared dependency.
        let counted: Vec<(&str, usize)> = vec![
            (
                "ListServices",
                store
                    .observed_services(&window_start, &window_end, &now)
                    .len(),
            ),
            (
                "GetService",
                store
                    .observed_service(&monitored, &window_start, &window_end, &now)
                    .metric_references
                    .len(),
            ),
            (
                "ListServiceOperations",
                store
                    .observed_operations(&monitored, &window_start, &window_end, &now)
                    .len(),
            ),
            (
                "ListServiceDependencies",
                store
                    .observed_dependencies(&monitored, &window_start, &window_end, &now)
                    .len(),
            ),
            (
                "ListServiceDependents",
                store
                    .observed_dependents(&callee, &window_start, &window_end, &now)
                    .len(),
            ),
            (
                "ListServiceStates",
                store
                    .observed_service_states(&window_start, &window_end, &now, &[])
                    .len(),
            ),
            (
                "ListAuditFindings",
                store
                    .audited_findings(&targets, None, false, &window_start, &window_end, &now)
                    .len(),
            ),
        ];

        for (read, reported) in counted {
            if begun {
                assert!(
                    reported > 0,
                    "case {label:?} / {read}: the period had begun at the instant the read was \
                     given, so it observed what the store holds evidence of; got {reported} \
                     entries"
                );
            } else {
                assert_eq!(
                    reported, 0,
                    "case {label:?} / {read}: the period had not begun at the instant the read \
                     was given, so nothing was observed in it; got {reported} entries"
                );
            }
        }
    }
}
