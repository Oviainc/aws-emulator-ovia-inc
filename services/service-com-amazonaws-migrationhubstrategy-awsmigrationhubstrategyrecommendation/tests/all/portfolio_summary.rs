// SPDX-License-Identifier: CC0-1.0

//! `GetPortfolioSummary` over HTTP: the summary it reports for the account's portfolio, as
//! a function of the records that portfolio holds.
//!
//! The rule under test is the one the operation states for ANY portfolio, not for one
//! request: every list of the `AssessmentSummary` it answers is COUNTED off the stored
//! records of the Region the request was signed for, grouped by the member the list's own
//! element type names — the servers by the `ServerOsType` their recorded operating system
//! names and by the status of their assessment, the application components by their
//! `AppType` and by the status of their analysis, and each kind by the strategy the
//! analysis recommended for it — one count per resource holding that value, with no group
//! for a value nothing holds and no list where nothing was counted; `lastAnalyzedTimestamp`
//! is the latest instant an analysis was recorded on any of those resources; and the read
//! computes all of it without changing anything a later caller sees.
//!
//! The table below therefore spans the class the rule generalises over rather than one
//! portfolio shape: a portfolio holding nothing, servers only, components only and both;
//! resources holding no value for the member they would be grouped by; several resources
//! sharing one value and several values held once; and an operating system spelled
//! differently from the catalog entry it names. The catalogs are spelled out from the model
//! here, so a value outside one fails here rather than agreeing with the crate.
//!
//! Servers and application components enter this service through an import file task's
//! ingestion, so they are seeded through the shared engine — the same maps that ingestion
//! writes — and every assertion is driven through the router, the path a client takes. The
//! last case runs the import-then-assess loop's analysis instead of seeding its output, so
//! the summary is checked against an analysis the service itself performed.

use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ApplicationComponentRecord,
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ServerRecord,
    COMPLETION_SPAN,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    at, bring_every_completion_forward, call, get_assessment, minimal_assessment,
    mount_object_storage, register_collector_in, start_assessment, Answer, TEST_BUCKETS,
    TEST_REGION,
};

/// `GET /get-portfolio-summary`, from
/// `smithy_query(query="operation",operation="GetPortfolioSummary")`.
const SUMMARY_URI: &str = "/get-portfolio-summary";

/// Every value `ServerOsType` admits, from
/// `smithy_query(query="shape",shape="ServerOsType")` — the catalog
/// `ServerSummary$ServerOsType` draws on.
const SERVER_OS_TYPES: &[&str] = &[
    "WindowsServer",
    "AmazonLinux",
    "EndOfSupportWindowsServer",
    "Redhat",
    "Other",
];

/// Every value `Strategy` admits, from `smithy_query(query="shape",shape="Strategy")` —
/// the catalog `StrategySummary$strategy` draws on.
const STRATEGIES: &[&str] = &[
    "Rehost",
    "Retirement",
    "Refactor",
    "Replatform",
    "Retain",
    "Relocate",
    "Repurchase",
];

/// Every value `RunTimeAssessmentStatus` admits, from
/// `smithy_query(query="shape",shape="RunTimeAssessmentStatus")` — the catalog
/// `ServerStatusSummary$runTimeAssessmentStatus` draws on.
const RUN_TIME_ASSESSMENT_STATUSES: &[&str] = &[
    "dataCollectionTaskToBeScheduled",
    "dataCollectionTaskScheduled",
    "dataCollectionTaskStarted",
    "dataCollectionTaskStopped",
    "dataCollectionTaskSuccess",
    "dataCollectionTaskFailed",
    "dataCollectionTaskPartialSuccess",
];

/// Every value `SrcCodeOrDbAnalysisStatus` admits, from
/// `smithy_query(query="shape",shape="SrcCodeOrDbAnalysisStatus")` — the catalog
/// `ApplicationComponentStatusSummary$srcCodeOrDbAnalysisStatus` draws on.
const SRC_CODE_OR_DB_ANALYSIS_STATUSES: &[&str] = &[
    "ANALYSIS_TO_BE_SCHEDULED",
    "ANALYSIS_STARTED",
    "ANALYSIS_SUCCESS",
    "ANALYSIS_FAILED",
    "ANALYSIS_PARTIAL_SUCCESS",
    "UNCONFIGURED",
    "CONFIGURED",
];

/// Every value `AppType` admits, from `smithy_query(query="shape",shape="AppType")` — the
/// catalog `ApplicationComponentSummary$appType` draws on.
const APP_TYPES: &[&str] = &[
    "DotNetFramework",
    "Java",
    "SQLServer",
    "IIS",
    "Oracle",
    "Other",
    "Tomcat",
    "JBoss",
    "Spring",
    "Mongo DB",
    "DB2",
    "Maria DB",
    "MySQL",
    "Sybase",
    "PostgreSQLServer",
    "Cassandra",
    "IBM WebSphere",
    "Oracle WebLogic",
    "Visual Basic",
    "Unknown",
    "DotnetCore",
    "Dotnet",
];

/// The `AssessmentSummary` list every count comes back in, with the member its entries are
/// keyed by and the catalog that member's value must belong to. From
/// `smithy_query(query="shape",shape="AssessmentSummary,StrategySummary,ServerSummary,ApplicationComponentSummary,ServerStatusSummary,ApplicationComponentStatusSummary")`.
const SUMMARY_LISTS: &[(&str, &str, &[&str])] = &[
    ("listServerStrategySummary", "strategy", STRATEGIES),
    (
        "listApplicationComponentStrategySummary",
        "strategy",
        STRATEGIES,
    ),
    ("listServerSummary", "ServerOsType", SERVER_OS_TYPES),
    ("listApplicationComponentSummary", "appType", APP_TYPES),
    (
        "listServerStatusSummary",
        "runTimeAssessmentStatus",
        RUN_TIME_ASSESSMENT_STATUSES,
    ),
    (
        "listApplicationComponentStatusSummary",
        "srcCodeOrDbAnalysisStatus",
        SRC_CODE_OR_DB_ANALYSIS_STATUSES,
    ),
];

/// One emulator over one shared engine, with `servers` and `components` seeded into
/// `region`'s portfolio — the same two maps an import file task's ingestion writes.
///
/// Neither of the harnesses beside this suite seeds BOTH maps at once, which a summary over
/// a portfolio of servers AND application components needs, so this one seeds the pair; the
/// engine comes back with it so a case can drive the analysis and bring its completion
/// forward.
fn portfolio_in(
    region: &str,
    servers: Vec<ServerRecord>,
    components: Vec<ApplicationComponentRecord>,
) -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let portfolio = store.region_entry(region);
        for record in servers {
            portfolio.servers.insert(record.id.clone(), record);
        }
        for record in components {
            portfolio
                .application_components
                .insert(record.id.clone(), record);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine.clone()));
    // And the buckets a start call names, in the object storage it resolves them against.
    mount_object_storage(&mut router, &engine, TEST_BUCKETS);
    (engine, router)
}

/// [`portfolio_in`] for the one Region every request this suite builds is signed for.
fn portfolio(
    servers: Vec<ServerRecord>,
    components: Vec<ApplicationComponentRecord>,
) -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    portfolio_in(TEST_REGION, servers, components)
}

async fn get_summary(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", SUMMARY_URI, None).await
}

/// A server record holding only its id — a `ResourceId` (`^[0-9a-b]+`, at most 44
/// characters), checked here rather than through any crate predicate.
fn server(id: &str) -> ServerRecord {
    assert!(
        !id.is_empty()
            && id.len() <= 44
            && id.chars().all(|c| c.is_ascii_digit() || c == 'a' || c == 'b'),
        "a server id ResourceId (^[0-9a-b]+, max 44) does not admit: {id:?}"
    );
    ServerRecord {
        id: id.to_string(),
        ..Default::default()
    }
}

/// A server whose record names the free-text operating system it reports and nothing else.
fn server_running(id: &str, os_version: &str) -> ServerRecord {
    ServerRecord {
        os_version: Some(os_version.to_string()),
        ..server(id)
    }
}

/// A server carrying the strategy an analysis recommended for it and the status of its
/// assessment, either of which the record may hold none of.
fn analyzed_server(id: &str, strategy: Option<&str>, status: Option<&str>) -> ServerRecord {
    if let Some(strategy) = strategy {
        assert!(
            STRATEGIES.contains(&strategy),
            "a strategy Strategy does not declare: {strategy:?}"
        );
    }
    if let Some(status) = status {
        assert!(
            RUN_TIME_ASSESSMENT_STATUSES.contains(&status),
            "a status RunTimeAssessmentStatus does not declare: {status:?}"
        );
    }
    ServerRecord {
        strategy: strategy.map(str::to_string),
        data_collection_status: status.map(str::to_string),
        ..server(id)
    }
}

/// An application component record holding only its id.
fn component(id: &str) -> ApplicationComponentRecord {
    ApplicationComponentRecord {
        id: id.to_string(),
        ..Default::default()
    }
}

/// An application component carrying its application type, the strategy an analysis
/// recommended for it and the status of its analysis, any of which the record may hold none
/// of.
fn analyzed_component(
    id: &str,
    app_type: Option<&str>,
    strategy: Option<&str>,
    analysis_status: Option<&str>,
) -> ApplicationComponentRecord {
    if let Some(app_type) = app_type {
        assert!(
            APP_TYPES.contains(&app_type),
            "an application type AppType does not declare: {app_type:?}"
        );
    }
    if let Some(status) = analysis_status {
        assert!(
            SRC_CODE_OR_DB_ANALYSIS_STATUSES.contains(&status),
            "a status SrcCodeOrDbAnalysisStatus does not declare: {status:?}"
        );
    }
    ApplicationComponentRecord {
        app_type: app_type.map(str::to_string),
        strategy: strategy.map(str::to_string),
        analysis_status: analysis_status.map(str::to_string),
        ..component(id)
    }
}

/// The `(value, count)` pairs the summary list `list` reported, keyed by `member` and
/// sorted — what was counted, independently of the order it came back in.
fn reported(answer: &Answer, list: &str, member: &str) -> Vec<(String, i64)> {
    let reported = at(&answer.body, &format!("/assessmentSummary/{list}"));
    let Some(entries) = reported.as_array() else {
        return Vec::new();
    };
    let mut counted: Vec<(String, i64)> = entries
        .iter()
        .map(|entry| {
            (
                entry[member].as_str().unwrap_or_default().to_string(),
                entry["count"].as_i64().unwrap_or_default(),
            )
        })
        .collect();
    counted.sort();
    counted
}

/// One row: the records a portfolio holds, and the groups each summary list must then
/// report. A list this row does not name must not be reported at all.
struct Row {
    case: &'static str,
    servers: Vec<ServerRecord>,
    components: Vec<ApplicationComponentRecord>,
    expected: Vec<(&'static str, Vec<(&'static str, i64)>)>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a portfolio that holds nothing at all",
            servers: Vec::new(),
            components: Vec::new(),
            expected: Vec::new(),
        },
        Row {
            case: "one server the ingestion knew nothing about beyond its id",
            servers: vec![server("0a")],
            components: Vec::new(),
            expected: Vec::new(),
        },
        Row {
            case: "one application component known only by its id",
            servers: Vec::new(),
            components: vec![component("0b")],
            expected: Vec::new(),
        },
        Row {
            case: "a server per operating system the ServerOsType catalog names, one of \
                   them spelled differently and one the catalog does not name",
            servers: vec![
                server_running("0a", "WindowsServer 2019"),
                server_running("1a", "Amazon Linux 2"),
                server_running("2a", "amazonlinux2"),
                server_running("3a", "EndOfSupportWindowsServer 2008 R2"),
                server_running("4a", "Redhat Enterprise Linux 8"),
                server_running("5a", "Ubuntu 20.04"),
            ],
            components: Vec::new(),
            expected: vec![(
                "listServerSummary",
                vec![
                    ("AmazonLinux", 2),
                    ("EndOfSupportWindowsServer", 1),
                    ("Other", 1),
                    ("Redhat", 1),
                    ("WindowsServer", 1),
                ],
            )],
        },
        Row {
            case: "a server whose record names its OSType and no system version",
            servers: vec![
                ServerRecord {
                    os_type: Some("LINUX".to_string()),
                    ..server("0a")
                },
                ServerRecord {
                    os_type: Some("WINDOWS".to_string()),
                    ..server("1a")
                },
            ],
            components: Vec::new(),
            // Neither `OSType` value is one of the operating systems `ServerOsType` names,
            // so both servers are counted under that catalog's catch-all.
            expected: vec![("listServerSummary", vec![("Other", 2)])],
        },
        Row {
            case: "servers sharing a strategy and a status, beside one holding neither",
            servers: vec![
                analyzed_server("0a", Some("Rehost"), Some("dataCollectionTaskSuccess")),
                analyzed_server("1a", Some("Rehost"), Some("dataCollectionTaskSuccess")),
                analyzed_server("2a", Some("Retain"), Some("dataCollectionTaskFailed")),
                analyzed_server("3a", None, None),
            ],
            components: Vec::new(),
            expected: vec![
                (
                    "listServerStrategySummary",
                    vec![("Rehost", 2), ("Retain", 1)],
                ),
                (
                    "listServerStatusSummary",
                    vec![
                        ("dataCollectionTaskFailed", 1),
                        ("dataCollectionTaskSuccess", 2),
                    ],
                ),
            ],
        },
        Row {
            case: "application components spanning application types, strategies and \
                   analysis statuses",
            servers: Vec::new(),
            components: vec![
                analyzed_component("0b", Some("Java"), Some("Replatform"), Some("CONFIGURED")),
                analyzed_component(
                    "1b",
                    Some("Java"),
                    Some("Replatform"),
                    Some("ANALYSIS_SUCCESS"),
                ),
                analyzed_component("2b", Some("SQLServer"), None, Some("CONFIGURED")),
                analyzed_component("3b", None, None, None),
            ],
            expected: vec![
                (
                    "listApplicationComponentStrategySummary",
                    vec![("Replatform", 2)],
                ),
                (
                    "listApplicationComponentSummary",
                    vec![("Java", 2), ("SQLServer", 1)],
                ),
                (
                    "listApplicationComponentStatusSummary",
                    vec![("ANALYSIS_SUCCESS", 1), ("CONFIGURED", 2)],
                ),
            ],
        },
        Row {
            case: "a portfolio of servers and application components together",
            servers: vec![
                ServerRecord {
                    os_version: Some("WindowsServer 2019".to_string()),
                    ..analyzed_server("0a", Some("Rehost"), Some("dataCollectionTaskSuccess"))
                },
                ServerRecord {
                    os_version: Some("Amazon Linux 2".to_string()),
                    ..analyzed_server("1a", Some("Relocate"), Some("dataCollectionTaskStopped"))
                },
            ],
            components: vec![
                analyzed_component("0b", Some("IIS"), Some("Rehost"), Some("UNCONFIGURED")),
                analyzed_component("1b", Some("Tomcat"), Some("Refactor"), Some("UNCONFIGURED")),
                analyzed_component("2b", Some("IIS"), Some("Rehost"), Some("ANALYSIS_FAILED")),
            ],
            expected: vec![
                (
                    "listServerStrategySummary",
                    vec![("Rehost", 1), ("Relocate", 1)],
                ),
                (
                    "listServerSummary",
                    vec![("AmazonLinux", 1), ("WindowsServer", 1)],
                ),
                (
                    "listServerStatusSummary",
                    vec![
                        ("dataCollectionTaskStopped", 1),
                        ("dataCollectionTaskSuccess", 1),
                    ],
                ),
                (
                    "listApplicationComponentStrategySummary",
                    vec![("Refactor", 1), ("Rehost", 2)],
                ),
                (
                    "listApplicationComponentSummary",
                    vec![("IIS", 2), ("Tomcat", 1)],
                ),
                (
                    "listApplicationComponentStatusSummary",
                    vec![("ANALYSIS_FAILED", 1), ("UNCONFIGURED", 2)],
                ),
            ],
        },
    ]
}

/// Every list of the summary is counted off the stored records, group for group: a value
/// several resources hold is counted once per resource, a value nothing holds gets no
/// group, and a list nothing was counted into is not reported at all.
#[tokio::test]
async fn the_summary_counts_the_records_the_portfolio_holds() {
    for Row {
        case,
        servers,
        components,
        expected,
    } in rows()
    {
        let (_engine, router) = portfolio(servers, components);
        let answer = get_summary(&router).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the summary is reported for any portfolio, got {} with {:?}",
            answer.status, answer.body
        );

        for (list, member, catalog) in SUMMARY_LISTS {
            let wanted: Option<&Vec<(&str, i64)>> = expected
                .iter()
                .find(|(named, _)| named == list)
                .map(|(_, groups)| groups);
            let Some(wanted) = wanted else {
                assert!(
                    at(&answer.body, &format!("/assessmentSummary/{list}")).is_null(),
                    "case {case:?}: nothing was counted into {list}, so it is not reported, \
                     got {:?}",
                    answer.body
                );
                continue;
            };
            let mut wanted: Vec<(String, i64)> = wanted
                .iter()
                .map(|(value, count)| ((*value).to_string(), *count))
                .collect();
            wanted.sort();
            let counted = reported(&answer, list, member);
            assert_eq!(
                counted, wanted,
                "case {case:?}: expected {list} to count {wanted:?}, got {counted:?} in {:?}",
                answer.body
            );
            for (value, count) in &counted {
                assert!(
                    catalog.contains(&value.as_str()),
                    "case {case:?}: {list} reported {value:?}, which its catalog \
                     {catalog:?} does not declare"
                );
                assert!(
                    *count > 0,
                    "case {case:?}: {list} reported {value:?} with a count of {count}, \
                     which stands for no resource"
                );
            }
        }

        // Nothing in this portfolio has been analysed, so there is no instant to report,
        // and no anti-pattern report is generated for one either.
        for absent in [
            "lastAnalyzedTimestamp",
            "listAntipatternSeveritySummary",
            "antipatternReportS3Object",
            "antipatternReportStatus",
        ] {
            assert!(
                at(&answer.body, &format!("/assessmentSummary/{absent}")).is_null(),
                "case {case:?}: {absent} stands for something nothing produced, got {:?}",
                answer.body
            );
        }
    }
}

/// The summary is a READ: asking twice answers the same thing, and a read of the inventory
/// taken after it reports exactly what it reported before.
#[tokio::test]
async fn the_summary_changes_nothing_a_later_caller_sees() {
    let (_engine, router) = portfolio(
        vec![
            ServerRecord {
                os_version: Some("WindowsServer 2019".to_string()),
                ..analyzed_server("0a", Some("Rehost"), Some("dataCollectionTaskSuccess"))
            },
            analyzed_server("1a", Some("Retain"), Some("dataCollectionTaskFailed")),
        ],
        vec![analyzed_component(
            "0b",
            Some("Java"),
            Some("Replatform"),
            Some("CONFIGURED"),
        )],
    );

    let before = call(&router, "POST", "/list-servers", Some(json!({}))).await;
    let first = get_summary(&router).await;
    let second = get_summary(&router).await;
    assert_eq!(
        (first.status, second.status),
        (200, 200),
        "both reads must report, got {} and {}",
        first.status,
        second.status
    );
    assert_eq!(
        second.body, first.body,
        "asking twice must answer the same summary, got {:?} then {:?}",
        first.body, second.body
    );

    let after = call(&router, "POST", "/list-servers", Some(json!({}))).await;
    assert_eq!(
        (after.status, after.body.clone()),
        (before.status, before.body.clone()),
        "the summary must leave the inventory exactly as it was, got {:?} after {:?}",
        after.body,
        before.body
    );
}

/// The portfolio summarised is the one of the Region the request was signed for: records
/// stored in another Region are counted by neither list.
#[tokio::test]
async fn the_summary_counts_only_the_region_the_request_was_signed_for() {
    let (_engine, router) = portfolio_in(
        "us-east-1",
        vec![analyzed_server(
            "0a",
            Some("Rehost"),
            Some("dataCollectionTaskSuccess"),
        )],
        vec![analyzed_component(
            "0b",
            Some("Java"),
            Some("Replatform"),
            Some("CONFIGURED"),
        )],
    );

    let answer = get_summary(&router).await;
    assert_eq!(
        answer.status, 200,
        "a Region holding nothing still reports a summary, got {} with {:?}",
        answer.status, answer.body
    );
    for (list, _, _) in SUMMARY_LISTS {
        assert!(
            at(&answer.body, &format!("/assessmentSummary/{list}")).is_null(),
            "{list} must count nothing stored in another Region, got {:?}",
            answer.body
        );
    }
}

/// The summary follows the ANALYSIS: before an assessment the portfolio has no recommended
/// strategy and no analysis instant to report, and once the assessment the account started
/// completes, the summary counts the strategies and the per-server statuses that completion
/// recorded and reports the instant it recorded them at — the very instant the resources
/// themselves report.
#[tokio::test]
async fn the_summary_reports_what_a_completed_assessment_recorded() {
    let servers = vec![
        ServerRecord {
            os_type: Some("WINDOWS".to_string()),
            os_version: Some("WindowsServer 2019".to_string()),
            ..server("0a")
        },
        ServerRecord {
            os_type: Some("LINUX".to_string()),
            os_version: Some("Amazon Linux 2".to_string()),
            ..server("1a")
        },
    ];
    let components = vec![
        ApplicationComponentRecord {
            associated_server_ids: vec!["0a".to_string()],
            resource_sub_type: Some("Process".to_string()),
            inclusion_status: Some("includeInAssessment".to_string()),
            ..analyzed_component("0b", Some("IIS"), None, Some("UNCONFIGURED"))
        },
        ApplicationComponentRecord {
            associated_server_ids: vec!["1a".to_string()],
            resource_sub_type: Some("Process".to_string()),
            inclusion_status: Some("includeInAssessment".to_string()),
            ..analyzed_component("1b", Some("Tomcat"), None, Some("UNCONFIGURED"))
        },
    ];
    let (engine, router) = portfolio(servers, components);
    // An assessment collects what a data collector gathered from the environment, so the
    // account has to hold one registered for the start call to be accepted.
    register_collector_in(&engine, &[TEST_REGION]);

    let before = get_summary(&router).await;
    assert_eq!(
        before.status, 200,
        "the summary of an unassessed portfolio is still reported, got {} with {:?}",
        before.status, before.body
    );
    for list in [
        "listServerStrategySummary",
        "listApplicationComponentStrategySummary",
        "listServerStatusSummary",
    ] {
        assert!(
            at(&before.body, &format!("/assessmentSummary/{list}")).is_null(),
            "nothing has analysed this portfolio, so {list} counts nothing, got {:?}",
            before.body
        );
    }
    assert!(
        at(&before.body, "/assessmentSummary/lastAnalyzedTimestamp").is_null(),
        "nothing has analysed this portfolio, so there is no instant to report, got {:?}",
        before.body
    );
    // The two application types ARE known before any assessment: they are what the
    // ingestion stored, not what an analysis produced.
    assert_eq!(
        reported(&before, "listApplicationComponentSummary", "appType"),
        vec![("IIS".to_string(), 1), ("Tomcat".to_string(), 1)],
        "the application types the ingestion stored are counted already, got {:?}",
        before.body
    );

    let started_at = Instant::now();
    let started = start_assessment(&router, minimal_assessment()).await;
    assert_eq!(
        started.status, 200,
        "the assessment must start, got {} with {:?}",
        started.status, started.body
    );
    // While the collection is still running, every covered server is counted under the
    // status that running collection gives it.
    let running = get_summary(&router).await;
    if started_at.elapsed() < COMPLETION_SPAN {
        assert_eq!(
            reported(&running, "listServerStatusSummary", "runTimeAssessmentStatus"),
            vec![("dataCollectionTaskStarted".to_string(), 2)],
            "a started assessment puts both covered servers in the running status, got {:?}",
            running.body
        );
    }

    bring_every_completion_forward(&engine);
    let assessment = get_assessment(&router, &started.string("/assessmentId")).await;
    let completed_at = at(&assessment.body, "/dataCollectionDetails/completionTime");
    assert!(
        completed_at.is_number(),
        "a completed assessment reports its completion time, got {completed_at:?}"
    );

    let after = get_summary(&router).await;
    assert_eq!(
        reported(&after, "listServerStrategySummary", "strategy"),
        vec![("Retain".to_string(), 2)],
        "with no saved preference the analysis retains both servers, got {:?}",
        after.body
    );
    assert_eq!(
        reported(
            &after,
            "listApplicationComponentStrategySummary",
            "strategy"
        ),
        vec![("Retain".to_string(), 2)],
        "and both application components it covered, got {:?}",
        after.body
    );
    assert_eq!(
        reported(&after, "listServerStatusSummary", "runTimeAssessmentStatus"),
        vec![("dataCollectionTaskSuccess".to_string(), 2)],
        "the completion moves both covered servers to the collected status, got {:?}",
        after.body
    );
    assert_eq!(
        at(&after.body, "/assessmentSummary/lastAnalyzedTimestamp"),
        completed_at,
        "the summary reports the instant the assessment completed at, got {:?}",
        after.body
    );

    // The same instant each analysed resource reports for itself, so the summary and the
    // records cannot disagree.
    for id in ["0a", "1a"] {
        let detail = call(&router, "GET", &format!("/get-server-details/{id}"), None).await;
        assert_eq!(
            at(&detail.body, "/serverDetail/lastAnalyzedTimestamp"),
            completed_at,
            "server {id:?} reports the same analysis instant, got {:?}",
            detail.body
        );
    }

    // The operating systems the servers run are unchanged by the assessment: they are
    // counted off the records either way.
    assert_eq!(
        reported(&after, "listServerSummary", "ServerOsType"),
        vec![
            ("AmazonLinux".to_string(), 1),
            ("WindowsServer".to_string(), 1)
        ],
        "the operating systems are counted off the records, got {:?}",
        after.body
    );
}
