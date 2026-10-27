// SPDX-License-Identifier: CC0-1.0

//! The inventory a call COUNTS or REPORTS, end to end over HTTP.
//!
//! The rule under test is one of ordering, for ANY handler of this service that answers
//! from the portfolio's server inventory rather than for one request: before it counts or
//! reports those servers it materialises every completion the clock already owed that
//! portfolio at the instant of the call — owed then even if nobody had called — so the
//! servers an import file task whose completion has fallen due ingested are part of the
//! inventory the answer is computed from, and that answer never depends on whether some
//! unrelated caller happened to read the portfolio first. The counterpart is asserted in
//! the same rows: a record of an import that is still running has not arrived, so it is
//! counted and reported nowhere.
//!
//! Two handlers answer from that inventory in a way a client can observe without first
//! bringing the records in by hand, and both are driven here:
//!
//! * `StartRecommendationReportGeneration`, whose task keeps the number of servers the
//!   request's `groupIdFilter` selected — observable through the status message
//!   `GetRecommendationReportDetails` reports once the clock has settled the task;
//! * `ListAnalyzableServers`, which reports a summary for each server of that inventory
//!   the service holds collector discovery data for.
//!
//! Each table spans the class the rule generalises over rather than the one sequence a
//! report cites: every shape of scope a `groupIdFilter` can name (none, empty, one group,
//! two groups, the other `GroupName`, a group no server came in with), both orders (one
//! unrelated read before the call and none at all), and an import whose completion has
//! fallen due beside one that is still running.
//!
//! A server enters this service only through the account's data being brought in, so the
//! import file task holding them is seeded through the shared engine — the same
//! `import_tasks` map `StartImportFileTask` writes, with the records still held by the task
//! exactly as an ingestion leaves them — while every call and every assertion goes through
//! the router, as a client reaches them.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::Group;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ImportFileTask,
    ServerDiscovery, ServerRecord, COMPLETION_SPAN,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{call, Answer, TEST_REGION};

/// Every value `GroupName` admits, from `smithy_query(query="shape",shape="GroupName")`.
const GROUP_NAMES: &[&str] = &["ExternalId", "ExternalSourceType"];

/// `ImportFileTaskStatus`'s value for an import still running and the one a fully
/// successful import reaches, from
/// `smithy_query(query="shape",shape="ImportFileTaskStatus")`.
const IMPORT_IN_PROGRESS: &str = "ImportInProgress";
const IMPORT_SUCCESS: &str = "ImportSuccess";

/// The terminal `RecommendationReportStatus` a started task settles on, from
/// `smithy_query(query="shape",shape="RecommendationReportStatus")` — spelled out here
/// rather than taken from the crate, so a wrong constant fails this suite.
const REPORT_SETTLED: &str = "SUCCESS";

/// The one server of the fixture the service holds collector discovery data for, which is
/// therefore the only analyzable one.
const DISCOVERED_SERVER: &str = "0a";

async fn start_report(router: &MultiServiceRouter, request: Value) -> Answer {
    call(
        router,
        "POST",
        "/start-recommendation-report-generation",
        Some(request),
    )
    .await
}

async fn report_details(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-recommendation-report-details/{id}"),
        None,
    )
    .await
}

async fn list_servers(router: &MultiServiceRouter) -> Answer {
    call(router, "POST", "/list-servers", Some(json!({}))).await
}

async fn list_analyzable_servers(router: &MultiServiceRouter) -> Answer {
    call(router, "POST", "/list-analyzable-servers", Some(json!({}))).await
}

/// One `groupIdFilter` entry, held to the `GroupName` catalog above.
fn group(name: &str, value: &str) -> Value {
    assert!(
        GROUP_NAMES.contains(&name),
        "a group name GroupName does not declare: {name:?}"
    );
    json!({"name": name, "value": value})
}

fn stored_group(name: &str, value: &str) -> Group {
    Group {
        name: Some(name.to_string()),
        value: Some(value.to_string()),
    }
}

/// The servers the seeded import ingested: two in one group, one in another pair of
/// groups, and one that came in with no group at all — the first of them carrying the
/// collector discovery data that makes it an analyzable server.
///
/// Ids are `ResourceId`s (`^[0-9a-b]+`, max 44) like the ones an ingestion mints.
fn ingested_servers() -> Vec<ServerRecord> {
    vec![
        ServerRecord {
            id: DISCOVERED_SERVER.to_string(),
            group_id: vec![stored_group(GROUP_NAMES[0], "wave-1")],
            discovery: Some(ServerDiscovery {
                hostname: Some("vcenter-web-01".to_string()),
                ip_address: Some("10.0.3.17".to_string()),
                source: Some("Import".to_string()),
                vm_id: Some("vm-1042".to_string()),
            }),
            ..Default::default()
        },
        ServerRecord {
            id: "0b".to_string(),
            group_id: vec![stored_group(GROUP_NAMES[0], "wave-1")],
            ..Default::default()
        },
        ServerRecord {
            id: "1a".to_string(),
            group_id: vec![
                stored_group(GROUP_NAMES[0], "wave-2"),
                stored_group(GROUP_NAMES[1], "MPA"),
            ],
            ..Default::default()
        },
        ServerRecord {
            id: "2b".to_string(),
            group_id: Vec::new(),
            ..Default::default()
        },
    ]
}

/// One emulator whose portfolio holds an import file task that has ingested the servers
/// above, with its completion either already due or still a long way ahead.
///
/// Nothing has read the portfolio, so the records are still the task's: only materialising
/// what the clock owes moves them into the inventory.
fn portfolio_with_pending_import(due: bool) -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let now = SystemTime::now();
        let completion_due = if due {
            now - Duration::from_millis(100)
        } else {
            now + Duration::from_secs(3_600)
        };
        let ingested = ingested_servers();
        let task = ImportFileTask {
            id: "import-file-task-000000000001".to_string(),
            import_name: "pending-import".to_string(),
            input_s3_bucket: "migrationhub-strategy-import".to_string(),
            input_s3_key: "inventory.csv".to_string(),
            data_source_type: None,
            group_id: Vec::new(),
            status_report_s3_bucket: None,
            status_report_s3_key: None,
            status: IMPORT_IN_PROGRESS.to_string(),
            terminal_status: IMPORT_SUCCESS.to_string(),
            start_time: now - Duration::from_secs(1),
            completion_time: None,
            number_of_records_success: ingested.len() as i32,
            number_of_records_failed: 0,
            completion_due: Some(completion_due),
            pending_servers: ingested,
            pending_components: Vec::new(),
        };
        store
            .region_entry(TEST_REGION)
            .import_tasks
            .insert(task.id.clone(), task);
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// Move the instant the clock owes every report generation task of this portfolio its
/// terminal status into the PAST, so the next read materialises it.
///
/// The same thing the passing of the completion span does — the due instant is what the
/// store compares `now` against — without a case spending that span asleep.
fn complete_the_report_span(engine: &Arc<engine::InMemoryEngine>) {
    let state =
        engine.state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
    let mut store = state.lock();
    let region = store.region_entry(TEST_REGION);
    for report in region.recommendation_reports.values_mut() {
        if let Some(due) = report.completion_due {
            report.completion_due = Some(due - COMPLETION_SPAN - Duration::from_millis(100));
        }
    }
}

/// How many servers the settled task reports were in scope: the count the task kept opens
/// its status message, which is where the service reports it.
fn scope_reported(case: &str, message: &str) -> usize {
    message
        .split_whitespace()
        .next()
        .and_then(|token| token.parse::<usize>().ok())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the settled task to report how many servers were in \
                 scope, got {message:?}"
            )
        })
}

/// One row of the scope table: the filter a start call carries and how many of the
/// ingested servers it selects once they have arrived.
struct Scope {
    what: &'static str,
    filter: Value,
    selected: usize,
}

/// Every shape of scope a `groupIdFilter` names, over the seeded ingestion: no filter at
/// all, an empty one, one group two servers came in with, two groups that widen it, the
/// other `GroupName`, and a group no server came in with.
fn scope_table() -> Vec<Scope> {
    vec![
        Scope {
            what: "no filter: the whole portfolio",
            filter: json!({}),
            selected: 4,
        },
        Scope {
            what: "an empty filter scopes nothing, so again the whole portfolio",
            filter: json!({"groupIdFilter": []}),
            selected: 4,
        },
        Scope {
            what: "one group, which two of the ingested servers came in with",
            filter: json!({"groupIdFilter": [group(GROUP_NAMES[0], "wave-1")]}),
            selected: 2,
        },
        Scope {
            what: "two groups widen the scope",
            filter: json!({"groupIdFilter": [
                group(GROUP_NAMES[0], "wave-1"),
                group(GROUP_NAMES[0], "wave-2"),
            ]}),
            selected: 3,
        },
        Scope {
            what: "the other GroupName, which one ingested server came in with",
            filter: json!({"groupIdFilter": [group(GROUP_NAMES[1], "MPA")]}),
            selected: 1,
        },
        Scope {
            what: "a group no ingested server came in with",
            filter: json!({"groupIdFilter": [group(GROUP_NAMES[0], "wave-9")]}),
            selected: 0,
        },
    ]
}

/// Start a report with `filter` against a portfolio whose seeded import is `due` or still
/// running, optionally reading the portfolio once first, and report the scope the settled
/// task says it covered.
async fn started_scope(case: &str, filter: Value, due: bool, read_first: bool) -> usize {
    let (engine, router) = portfolio_with_pending_import(due);
    if read_first {
        let listed = list_servers(&router).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the read before the start call must succeed, got {} with {:?}",
            listed.status, listed.body
        );
    }

    let started = start_report(&router, filter).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: expected HTTP 200 from StartRecommendationReportGeneration, got {} with \
         {:?}",
        started.status, started.body
    );
    let id = started.string("/id");

    // Nothing sleeps: the instant the clock owes the task its terminal status is moved
    // into the past, which is what the passing of the span does.
    complete_the_report_span(&engine);
    let settled = report_details(&router, &id).await;
    assert_eq!(
        settled.status, 200,
        "case {case:?}: expected HTTP 200 from GetRecommendationReportDetails, got {} with {:?}",
        settled.status, settled.body
    );
    assert_eq!(
        settled.string("/recommendationReportDetails/status"),
        REPORT_SETTLED,
        "case {case:?}: once the clock owes it the task is {REPORT_SETTLED}, got {:?}",
        settled.body
    );
    let message = settled.string("/recommendationReportDetails/statusMessage");
    scope_reported(case, &message)
}

/// The scope a start call stores is counted from the inventory the clock has already
/// produced: the servers an import whose completion has fallen due ingested are in scope,
/// whether or not anything read the portfolio between that instant and the start call —
/// and the servers of an import that is still running are in scope nowhere.
#[tokio::test]
async fn a_report_scope_counts_the_servers_whose_arrival_the_clock_already_owed() {
    for row in scope_table() {
        for due in [true, false] {
            // An import that is still running has brought nothing in, so nothing is in
            // scope however the filter selects.
            let expected = if due { row.selected } else { 0 };
            let mut observed: Vec<(bool, usize)> = Vec::new();
            for read_first in [false, true] {
                let order = if read_first {
                    "one read before the start call"
                } else {
                    "no read before the start call"
                };
                let state = if due {
                    "the import's completion has fallen due"
                } else {
                    "the import is still running"
                };
                let case = format!("{}, {state}, {order}", row.what);
                let scope = started_scope(&case, row.filter.clone(), due, read_first).await;
                assert_eq!(
                    scope, expected,
                    "case {case:?}: expected {expected} server(s) in scope, got {scope}"
                );
                observed.push((read_first, scope));
            }
            assert!(
                observed.windows(2).all(|pair| pair[0].1 == pair[1].1),
                "case {:?}: the stored scope must not depend on whether an unrelated read \
                 happened first, got {observed:?}",
                row.what
            );
        }
    }
}

/// The same rule for the other handler that answers from this inventory: the analyzable
/// servers reported are the ones the clock has already brought in, in both orders.
#[tokio::test]
async fn analyzable_servers_report_the_arrivals_the_clock_already_owed() {
    for due in [true, false] {
        // Only the discovered server of the ingestion is analyzable, and only once the
        // import's completion has brought it in.
        let expected = if due { 1 } else { 0 };
        for read_first in [false, true] {
            let state = if due {
                "the import's completion has fallen due"
            } else {
                "the import is still running"
            };
            let order = if read_first {
                "one read before the list call"
            } else {
                "no read before the list call"
            };
            let case = format!("{state}, {order}");
            let (_engine, router) = portfolio_with_pending_import(due);
            if read_first {
                let listed = list_servers(&router).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: the read before the list call must succeed, got {} with {:?}",
                    listed.status, listed.body
                );
            }

            let answer = list_analyzable_servers(&router).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from ListAnalyzableServers, got {} with {:?}",
                answer.status, answer.body
            );
            let reported = answer
                .body
                .get("analyzableServers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                reported.len(),
                expected,
                "case {case:?}: expected {expected} analyzable server(s), got {reported:?}"
            );
            if expected == 1 {
                assert_eq!(
                    reported[0].get("vmId").and_then(Value::as_str),
                    Some("vm-1042"),
                    "case {case:?}: the summary must be the discovered server's, got {:?}",
                    reported[0]
                );
            }
        }
    }
}
