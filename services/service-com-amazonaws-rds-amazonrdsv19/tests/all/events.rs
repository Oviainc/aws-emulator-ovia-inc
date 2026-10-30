// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's event log: `DescribeEvents`, and the lifecycle
//! changes that record into it.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML the
//! emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: this service records an event for each lifecycle change it makes
//! to a resource that is an event source, and `DescribeEvents` reports the events recorded
//! in the region the request is signed for, narrowed by `SourceType` / `SourceIdentifier`,
//! by the `StartTime` / `EndTime` / `Duration` window and by `EventCategories`, and paged
//! by `MaxRecords` / `Marker`. An account that changed nothing has no events.
//!
//! The tables below span what that rule generalises over: each source type a change is
//! recorded for and each category and message it is recorded under, the four shapes of the
//! time window, a narrowing that selects all / some / none of the log, the identifier
//! case-folding, both published `MaxRecords` bounds plus one value past each, a page
//! boundary and the marker that resumes from it, and the regional scope of the log.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DEFAULT_EVENT_WINDOW_MINUTES, DESCRIBE_EVENTS_SOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
    wait_for_settle, REGION,
};

/// A second region, so the regional scope of the log is observable.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The DB instance every lifecycle change below is made to.
const INSTANCE: &str = "emu-ev-instance";

/// The snapshot `CreateDBSnapshot` takes of it.
const SNAPSHOT: &str = "emu-ev-snapshot";

/// The final snapshot `DeleteDBInstance` takes of it.
const FINAL_SNAPSHOT: &str = "emu-ev-final-snapshot";

/// The DB cluster created and deleted below.
const CLUSTER: &str = "emu-ev-cluster";

/// A call carrying no parameters beyond `Action` and `Version`.
const NO_PARAMS: [(&str, &str); 0] = [];

/// The ARN of a resource of this service, as its own record computes it:
/// `arn:aws:rds:<region>:<account>:<resource type>:<name>`.
fn resource_arn(resource_type: &str, name: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:{resource_type}:{name}")
}

/// One `<Event>` of a `DescribeEvents` answer, read out of the XML.
#[derive(Debug, PartialEq, Eq)]
struct ReportedEvent {
    source_identifier: String,
    source_type: String,
    category: String,
    message: String,
    source_arn: String,
}

/// Every `<Event>` the answer carries, in document order.
fn reported_events(xml: &str) -> Vec<ReportedEvent> {
    subtrees(xml, "Event")
        .iter()
        .map(|event| {
            let categories = texts(event, "EventCategory");
            assert_eq!(
                categories.len(),
                1,
                "an event reports the one category it was recorded under, got {categories:?} \
                 in {event}"
            );
            let date = text(event, "Date");
            assert!(
                date.contains('T') && date.ends_with('Z'),
                "an event's Date is an ISO-8601 instant, got {date:?} in {event}"
            );
            ReportedEvent {
                source_identifier: text(event, "SourceIdentifier"),
                source_type: text(event, "SourceType"),
                category: categories
                    .into_iter()
                    .next()
                    .expect("exactly one category, asserted above"),
                message: text(event, "Message"),
                source_arn: text(event, "SourceArn"),
            }
        })
        .collect()
}

/// One row of the published event table this service records.
struct ExpectedEvent {
    case: &'static str,
    source_identifier: &'static str,
    source_type: &'static str,
    /// The ARN resource type of the source: `db`, `snapshot` or `cluster`.
    resource_type: &'static str,
    category: &'static str,
    message: &'static str,
}

impl ExpectedEvent {
    /// The row as `DescribeEvents` must report it.
    fn as_reported(&self) -> ReportedEvent {
        ReportedEvent {
            source_identifier: self.source_identifier.to_string(),
            source_type: self.source_type.to_string(),
            category: self.category.to_string(),
            message: self.message.to_string(),
            source_arn: resource_arn(self.resource_type, self.source_identifier),
        }
    }
}

/// The events [`drive_the_lifecycle`] records, oldest first — the order
/// `DescribeEvents` reports them in.
///
/// The rows span the source types this service records a change for (`db-instance`,
/// `db-snapshot`, `db-cluster`), each category those types publish a recorded change
/// under, and both ends of a resource's life.
const EXPECTED: &[ExpectedEvent] = &[
    ExpectedEvent {
        case: "the DB instance was created",
        source_identifier: INSTANCE,
        source_type: "db-instance",
        resource_type: "db",
        category: "creation",
        message: "DB instance created",
    },
    ExpectedEvent {
        case: "a manual snapshot was taken of it",
        source_identifier: SNAPSHOT,
        source_type: "db-snapshot",
        resource_type: "snapshot",
        category: "creation",
        message: "Creating manual snapshot",
    },
    ExpectedEvent {
        case: "the DB instance was rebooted",
        source_identifier: INSTANCE,
        source_type: "db-instance",
        resource_type: "db",
        category: "availability",
        message: "DB instance restarted",
    },
    ExpectedEvent {
        case: "the DB instance was stopped",
        source_identifier: INSTANCE,
        source_type: "db-instance",
        resource_type: "db",
        category: "notification",
        message: "DB instance stopped",
    },
    ExpectedEvent {
        case: "the DB instance was started again",
        source_identifier: INSTANCE,
        source_type: "db-instance",
        resource_type: "db",
        category: "notification",
        message: "DB instance started",
    },
    ExpectedEvent {
        case: "the delete took a final snapshot",
        source_identifier: FINAL_SNAPSHOT,
        source_type: "db-snapshot",
        resource_type: "snapshot",
        category: "creation",
        message: "Creating manual snapshot",
    },
    ExpectedEvent {
        case: "the DB instance was deleted",
        source_identifier: INSTANCE,
        source_type: "db-instance",
        resource_type: "db",
        category: "deletion",
        message: "DB instance deleted",
    },
    ExpectedEvent {
        case: "the DB cluster was created",
        source_identifier: CLUSTER,
        source_type: "db-cluster",
        resource_type: "cluster",
        category: "creation",
        message: "DB cluster created",
    },
    ExpectedEvent {
        case: "the DB cluster was deleted",
        source_identifier: CLUSTER,
        source_type: "db-cluster",
        resource_type: "cluster",
        category: "deletion",
        message: "DB cluster deleted",
    },
];

/// Make every lifecycle change [`EXPECTED`] names, in that order, over the wire.
async fn drive_the_lifecycle(router: &MultiServiceRouter) {
    expect_ok(
        "create the instance",
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", INSTANCE),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.t3.micro"),
            ],
        )
        .await,
    );
    // A snapshot, a reboot and a stop each need the instance up, so each change waits out
    // the span the previous one owes.
    wait_for_settle();
    expect_ok(
        "snapshot the instance",
        call(
            router,
            "CreateDBSnapshot",
            &[
                ("DBSnapshotIdentifier", SNAPSHOT),
                ("DBInstanceIdentifier", INSTANCE),
            ],
        )
        .await,
    );
    expect_ok(
        "reboot the instance",
        call(
            router,
            "RebootDBInstance",
            &[("DBInstanceIdentifier", INSTANCE)],
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "stop the instance",
        call(
            router,
            "StopDBInstance",
            &[("DBInstanceIdentifier", INSTANCE)],
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "start the instance",
        call(
            router,
            "StartDBInstance",
            &[("DBInstanceIdentifier", INSTANCE)],
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "delete the instance with a final snapshot",
        call(
            router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", INSTANCE),
                ("FinalDBSnapshotIdentifier", FINAL_SNAPSHOT),
            ],
        )
        .await,
    );
    expect_ok(
        "create the cluster",
        call(
            router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", CLUSTER),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "delete the cluster",
        call(
            router,
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", CLUSTER),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
}

#[tokio::test]
async fn an_account_that_changed_nothing_has_no_events() {
    let router = emulator();
    let body = expect_ok(
        "a fresh account",
        call(&router, "DescribeEvents", &NO_PARAMS).await,
    );
    assert!(
        reported_events(&body).is_empty(),
        "a fresh account has no events, got {body}"
    );
    assert_eq!(
        optional_text(&body, "Marker"),
        None,
        "a page that ends the result set issues no Marker, got {body}"
    );
    // A source this service holds no record of narrows the log to nothing rather than
    // failing: `DescribeEvents` declares no not-found error.
    let body = expect_ok(
        "a source nothing created",
        call(
            &router,
            "DescribeEvents",
            &[
                ("SourceType", "db-instance"),
                ("SourceIdentifier", "emu-ev-never-created"),
            ],
        )
        .await,
    );
    assert!(
        reported_events(&body).is_empty(),
        "an identifier nothing created has no events, got {body}"
    );
}

#[tokio::test]
async fn every_lifecycle_change_is_reported_as_its_published_event() {
    let router = emulator();
    drive_the_lifecycle(&router).await;

    // The default window is the published one — "events that were generated in the past
    // hour" — which every change above falls in.
    let body = expect_ok(
        "the whole log",
        call(&router, "DescribeEvents", &NO_PARAMS).await,
    );
    let reported = reported_events(&body);
    assert_eq!(
        reported.len(),
        EXPECTED.len(),
        "the log reports one event per lifecycle change, got {reported:?} in {body}"
    );
    for (expected, got) in EXPECTED.iter().zip(reported) {
        assert_eq!(
            got,
            expected.as_reported(),
            "case {:?}: expected {:?}, got {got:?}",
            expected.case,
            expected.as_reported()
        );
    }
    assert_eq!(
        optional_text(&body, "Marker"),
        None,
        "a page that ends the result set issues no Marker, got {body}"
    );
    // The log outlives the resources it is about: the instance and the cluster above were
    // both deleted, and their events are still the ones reported.
    let (status, read_back) = call(
        &router,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", INSTANCE)],
    )
    .await;
    assert_eq!(
        status, 404,
        "the deleted instance must be gone from the store, got {status} with {read_back}"
    );
}

/// One narrowing of the log, and the messages it must report.
struct NarrowingCase {
    case: &'static str,
    params: &'static [(&'static str, &'static str)],
    /// The `Event$Message` of each event reported, in order.
    messages: &'static [&'static str],
}

#[tokio::test]
async fn a_read_narrows_the_log_by_source_window_and_category() {
    let router = emulator();
    drive_the_lifecycle(&router).await;

    // The window this emulator reports over when the request bounds neither end.
    assert_eq!(
        DEFAULT_EVENT_WINDOW_MINUTES, 60,
        "the default window is the published hour"
    );
    let table: &[NarrowingCase] = &[
        NarrowingCase {
            case: "one source type",
            params: &[("SourceType", "db-snapshot")],
            messages: &["Creating manual snapshot", "Creating manual snapshot"],
        },
        NarrowingCase {
            case: "another source type",
            params: &[("SourceType", "db-cluster")],
            messages: &["DB cluster created", "DB cluster deleted"],
        },
        NarrowingCase {
            case: "a source type nothing was recorded for",
            params: &[("SourceType", "db-proxy")],
            messages: &[],
        },
        NarrowingCase {
            case: "one source, named as created",
            params: &[("SourceType", "db-snapshot"), ("SourceIdentifier", SNAPSHOT)],
            messages: &["Creating manual snapshot"],
        },
        NarrowingCase {
            case: "the same source, named in another case",
            params: &[
                ("SourceType", "db-snapshot"),
                ("SourceIdentifier", "EMU-EV-SNAPSHOT"),
            ],
            messages: &["Creating manual snapshot"],
        },
        NarrowingCase {
            case: "a source of the wrong source type",
            params: &[("SourceType", "db-cluster"), ("SourceIdentifier", SNAPSHOT)],
            messages: &[],
        },
        NarrowingCase {
            case: "one category, across source types",
            params: &[("EventCategories.EventCategory.1", "deletion")],
            messages: &["DB instance deleted", "DB cluster deleted"],
        },
        NarrowingCase {
            case: "two categories, across source types",
            params: &[
                ("EventCategories.EventCategory.1", "availability"),
                ("EventCategories.EventCategory.2", "deletion"),
            ],
            messages: &[
                "DB instance restarted",
                "DB instance deleted",
                "DB cluster deleted",
            ],
        },
        NarrowingCase {
            case: "a category no recorded event carries",
            params: &[("EventCategories.EventCategory.1", "low storage")],
            messages: &[],
        },
        NarrowingCase {
            case: "a source type and a category of it",
            params: &[
                ("SourceType", "db-instance"),
                ("EventCategories.EventCategory.1", "notification"),
            ],
            messages: &["DB instance stopped", "DB instance started"],
        },
        NarrowingCase {
            case: "a window wide open on the past",
            params: &[("StartTime", "2020-01-01T00:00:00Z")],
            messages: &[
                "DB instance created",
                "Creating manual snapshot",
                "DB instance restarted",
                "DB instance stopped",
                "DB instance started",
                "Creating manual snapshot",
                "DB instance deleted",
                "DB cluster created",
                "DB cluster deleted",
            ],
        },
        NarrowingCase {
            case: "a window that closed before anything happened",
            params: &[
                ("StartTime", "2020-01-01T00:00:00Z"),
                ("EndTime", "2020-01-02T00:00:00Z"),
            ],
            messages: &[],
        },
        NarrowingCase {
            case: "a window that ends in the past",
            params: &[("EndTime", "2020-01-02T00:00:00Z")],
            messages: &[],
        },
        NarrowingCase {
            case: "a duration longer than the default hour",
            params: &[("Duration", "1440"), ("SourceType", "db-cluster")],
            messages: &["DB cluster created", "DB cluster deleted"],
        },
    ];

    for row in table {
        let body = expect_ok(row.case, call(&router, "DescribeEvents", row.params).await);
        let messages: Vec<String> = reported_events(&body)
            .into_iter()
            .map(|event| event.message)
            .collect();
        assert_eq!(
            messages, row.messages,
            "case {:?}: expected {:?}, got {messages:?} in {body}",
            row.case, row.messages
        );
    }
}

#[tokio::test]
async fn the_log_is_scoped_to_the_region_the_request_is_signed_for() {
    let router = emulator();
    drive_the_lifecycle(&router).await;

    let body = expect_ok(
        "another region",
        call_in(&router, OTHER_REGION, "DescribeEvents", &NO_PARAMS).await,
    );
    assert!(
        reported_events(&body).is_empty(),
        "a region nothing happened in has no events, got {body}"
    );
    // And the same identifier in that other region names a different resource, whose own
    // creation is the only event it has.
    expect_ok(
        "create an instance of the same name elsewhere",
        call_in(
            &router,
            OTHER_REGION,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", INSTANCE),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.t3.micro"),
            ],
        )
        .await,
    );
    let body = expect_ok(
        "the other region's log",
        call_in(&router, OTHER_REGION, "DescribeEvents", &NO_PARAMS).await,
    );
    let messages: Vec<String> = reported_events(&body)
        .into_iter()
        .map(|event| event.message)
        .collect();
    assert_eq!(
        messages,
        vec!["DB instance created".to_string()],
        "the other region reports only its own event, got {body}"
    );
    let arns: Vec<String> = reported_events(&body)
        .into_iter()
        .map(|event| event.source_arn)
        .collect();
    assert_eq!(
        arns,
        vec![format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:db:{INSTANCE}")],
        "an event's SourceArn is scoped to its own region, got {body}"
    );
}

#[tokio::test]
async fn a_page_boundary_is_resumed_from_the_marker_it_issued() {
    let router = emulator();
    // `MaxRecords` cannot be smaller than the published minimum of 20, so a page boundary
    // needs more than twenty recorded events: one create apiece.
    let page_size = 20usize;
    let recorded = page_size + 3;
    for index in 0..recorded {
        let identifier = format!("emu-ev-page-{index:02}");
        expect_ok(
            &identifier,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier.as_str()),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.t3.micro"),
                ],
            )
            .await,
        );
    }

    let first = expect_ok(
        "the first page",
        call(
            &router,
            "DescribeEvents",
            &[("MaxRecords", page_size.to_string())],
        )
        .await,
    );
    let first_page = reported_events(&first);
    assert_eq!(
        first_page.len(),
        page_size,
        "the first page holds MaxRecords events, got {} in {first}",
        first_page.len()
    );
    let marker = optional_text(&first, "Marker").unwrap_or_else(|| {
        panic!("a page with records left behind it issues a Marker, got {first}")
    });

    let second = expect_ok(
        "the second page",
        call(
            &router,
            "DescribeEvents",
            &[
                ("MaxRecords", page_size.to_string()),
                ("Marker", marker.clone()),
            ],
        )
        .await,
    );
    let second_page = reported_events(&second);
    assert_eq!(
        second_page.len(),
        recorded - page_size,
        "the second page holds what the first left, got {} in {second}",
        second_page.len()
    );
    assert_eq!(
        optional_text(&second, "Marker"),
        None,
        "the last page issues no Marker, got {second}"
    );
    // The two pages together are the log, each event once and in order.
    let paged: Vec<String> = first_page
        .into_iter()
        .chain(second_page)
        .map(|event| event.source_identifier)
        .collect();
    let whole: Vec<String> = reported_events(&expect_ok(
        "the whole log",
        call(&router, "DescribeEvents", &[("MaxRecords", "100")]).await,
    ))
    .into_iter()
    .map(|event| event.source_identifier)
    .collect();
    assert_eq!(
        paged, whole,
        "paging must report the log exactly once, got {paged:?} against {whole:?}"
    );
}

/// One request `DescribeEvents` refuses, and the status and wire code it refuses it with.
struct RefusalCase {
    case: &'static str,
    params: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_request_outside_the_documented_parameters_is_refused() {
    let router = emulator();
    // Every source type the model declares is accepted, and nothing else is.
    assert_eq!(
        DESCRIBE_EVENTS_SOURCE_TYPES.len(),
        11,
        "the modelled SourceType enum has eleven values"
    );
    for source_type in DESCRIBE_EVENTS_SOURCE_TYPES {
        expect_ok(
            source_type,
            call(&router, "DescribeEvents", &[("SourceType", source_type)]).await,
        );
    }

    let table: &[RefusalCase] = &[
        RefusalCase {
            case: "a source type the model does not declare",
            params: &[("SourceType", "db-widget")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a source identifier with no source type",
            params: &[("SourceIdentifier", INSTANCE)],
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "one record below the published MaxRecords minimum",
            params: &[("MaxRecords", "19")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "one record above the published MaxRecords maximum",
            params: &[("MaxRecords", "101")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a negative number of minutes",
            params: &[("Duration", "-1")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a marker no page issued",
            params: &[("Marker", "not-a-marker")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a filter, which the parameter documents as unsupported",
            params: &[
                ("Filters.Filter.1.Name", "event-category"),
                ("Filters.Filter.1.Values.Value.1", "creation"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
    ];
    for row in table {
        expect_error(
            row.case,
            call(&router, "DescribeEvents", row.params).await,
            row.status,
            row.code,
        );
    }

    // Both published bounds of `MaxRecords` are served.
    for bound in ["20", "100"] {
        expect_ok(
            bound,
            call(&router, "DescribeEvents", &[("MaxRecords", bound)]).await,
        );
    }
}
