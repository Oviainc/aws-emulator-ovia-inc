// SPDX-License-Identifier: CC0-1.0

//! The assessment lifecycle as the SERVERS report it, end to end over HTTP.
//!
//! The rule under test is the one `ServerDetail$dataCollectionStatus` states — "The
//! status of assessment for the server" — for ANY portfolio, not for one request: an
//! assessment covering a server is observable on that server as well as on the assessment
//! record. `StartAssessment` puts every server it covers into a running
//! `RunTimeAssessmentStatus`, and the terminal `AssessmentStatus` the assessment reaches
//! moves each server still collecting to the matching per-server status — a caller's stop
//! to `dataCollectionTaskStopped`, the clock's completion to `dataCollectionTaskSuccess` —
//! which every read of a server reports (`ListServers`, `GetServerDetails`) and
//! `ListServers`'s `serverCriteria=ANALYSIS_STATUS` selects on. A server whose collection
//! task was not running keeps the status it had.
//!
//! The tables below therefore span the class the rule generalises over rather than the one
//! stop the issue cited: inventories of no, one and several servers, every value of the
//! `RunTimeAssessmentStatus` catalog as the status a server held before the stop, both
//! terminal assessment statuses, and both server read paths.
//!
//! A server enters this service only through an import file task's ingestion of the import
//! file, which is filed separately (the import-loop issue: `StartImportFileTask` reaches
//! `ImportSuccess` having ingested nothing). So these cases seed the inventory records
//! directly through the shared engine — the same `servers` map that ingestion writes and
//! these operations read — and then drive every assertion through the router.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    Assessment, ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
    ServerRecord, COMPLETION_SPAN,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    at, call, get_assessment, minimal_assessment, start_assessment, stop_assessment, Answer,
    TEST_REGION,
};
use crate::server_inventory::{emulator_with, server};

/// Every value `RunTimeAssessmentStatus` admits, from
/// `smithy_query(query="shape",shape="RunTimeAssessmentStatus")`. Spelled out here so the
/// tables below span the whole catalog rather than the one status the issue cited.
const RUN_TIME_ASSESSMENT_STATUSES: &[&str] = &[
    "dataCollectionTaskToBeScheduled",
    "dataCollectionTaskScheduled",
    "dataCollectionTaskStarted",
    "dataCollectionTaskStopped",
    "dataCollectionTaskSuccess",
    "dataCollectionTaskFailed",
    "dataCollectionTaskPartialSuccess",
];

/// The catalog values that mean a collection task is RUNNING on the server, so a terminal
/// assessment status moves the server off them. Spelled out independently of the crate's
/// own constants, so a wrong value in `src/state.rs` fails here rather than agreeing with
/// itself.
const RUNNING_STATUSES: &[&str] = &["dataCollectionTaskScheduled", "dataCollectionTaskStarted"];

/// The per-server face of a stop, of a completion, and the `AssessmentStatus` values they
/// mirror — again spelled out independently of the crate.
const COLLECTION_STOPPED: &str = "dataCollectionTaskStopped";
const COLLECTION_SUCCESS: &str = "dataCollectionTaskSuccess";
const ASSESSMENT_STOPPED: &str = "STOPPED";
const ASSESSMENT_COMPLETE: &str = "COMPLETE";

/// `ResourceId`s (`^[0-9a-b]+`, max 44) for the seeded servers, one per catalog value.
const SERVER_IDS: &[&str] = &["0a", "0b", "1a", "1b", "2a", "2b", "ab"];

async fn list_servers(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/list-servers", Some(request)).await
}

async fn get_server_details(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-server-details/{id}"), None).await
}

/// The `dataCollectionStatus` `ListServers` reports for `id`, or the empty string when it
/// reports none.
fn listed_status(answer: &Answer, id: &str) -> String {
    let servers = answer
        .body
        .get("serverInfos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    servers
        .iter()
        .find(|server| server["id"].as_str() == Some(id))
        .map(|server| {
            server["dataCollectionStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_default()
}

/// The ids `ListServers` reported, in the order it reported them.
fn reported_ids(answer: &Answer) -> Vec<String> {
    answer
        .body
        .get("serverInfos")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|server| server["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One inventory a lifecycle case runs against, and what it is called.
struct Inventory {
    what: &'static str,
    servers: Vec<ServerRecord>,
}

/// The inventories the start-then-stop case runs against: they differ in how many servers
/// the portfolio holds (none, one, several) and in the status each server held before the
/// assessment started — no status at all, a status still collecting, and terminal statuses
/// from the catalog above.
fn inventories() -> Vec<Inventory> {
    vec![
        Inventory {
            what: "a portfolio holding no server at all",
            servers: Vec::new(),
        },
        Inventory {
            what: "one server that no assessment ever covered",
            servers: vec![server(SERVER_IDS[0])],
        },
        Inventory {
            what: "one server a previous collection left successful",
            servers: vec![ServerRecord {
                data_collection_status: Some(COLLECTION_SUCCESS.to_string()),
                ..server(SERVER_IDS[1])
            }],
        },
        Inventory {
            what: "three servers, each holding a different catalog status",
            servers: vec![
                ServerRecord {
                    data_collection_status: Some(RUN_TIME_ASSESSMENT_STATUSES[0].to_string()),
                    ..server(SERVER_IDS[2])
                },
                ServerRecord {
                    data_collection_status: Some(RUN_TIME_ASSESSMENT_STATUSES[5].to_string()),
                    ..server(SERVER_IDS[3])
                },
                ServerRecord {
                    data_collection_status: Some(RUN_TIME_ASSESSMENT_STATUSES[6].to_string()),
                    ..server(SERVER_IDS[4])
                },
            ],
        },
        Inventory {
            what: "a server for every value of the catalog",
            servers: RUN_TIME_ASSESSMENT_STATUSES
                .iter()
                .zip(SERVER_IDS)
                .map(|(status, id)| ServerRecord {
                    data_collection_status: Some((*status).to_string()),
                    ..server(id)
                })
                .collect(),
        },
    ]
}

/// `StartAssessment` makes the assessment observable on every server it covers, and
/// `StopAssessment` moves those servers to the per-server face of `STOPPED` — which both
/// server read paths report and `ANALYSIS_STATUS` selects on.
#[tokio::test]
async fn an_assessment_is_observed_on_the_servers_it_covers() {
    for Inventory { what, servers } in inventories() {
        let ids: Vec<String> = servers.iter().map(|record| record.id.clone()).collect();
        let router = emulator_with(servers);

        let started_at = Instant::now();
        let started = start_assessment(&router, minimal_assessment()).await;
        assert_eq!(
            started.status, 200,
            "case {what:?}: the start must be accepted, got {} {:?}",
            started.status, started.body
        );
        let assessment_id = started.string("/assessmentId");

        // While the assessment collects, every server it covers reports a RUNNING status,
        // through the list and through the single-server read alike.
        let listed = list_servers(&router, json!({})).await;
        if started_at.elapsed() < COMPLETION_SPAN {
            for id in &ids {
                let status = listed_status(&listed, id);
                assert!(
                    RUNNING_STATUSES.contains(&status.as_str()),
                    "case {what:?}: server {id:?} is collecting, so ListServers must report a \
                     running RunTimeAssessmentStatus, got {status:?}"
                );
                let detail = get_server_details(&router, id).await;
                assert_eq!(
                    at(&detail.body, "/serverDetail/dataCollectionStatus"),
                    json!(status),
                    "case {what:?}: GetServerDetails must report server {id:?} exactly as \
                     ListServers does, got {:?}",
                    at(&detail.body, "/serverDetail/dataCollectionStatus")
                );
            }
        }

        let stopped = stop_assessment(&router, &assessment_id).await;
        assert_eq!(
            stopped.status, 200,
            "case {what:?}: the stop must be accepted, got {} {:?}",
            stopped.status, stopped.body
        );
        if started_at.elapsed() >= COMPLETION_SPAN {
            // The clock completed this assessment before the stop reached it, so there
            // was nothing left collecting: the row proves nothing about the stop and is
            // skipped rather than asserted against the wrong terminal status.
            continue;
        }

        let reported = get_assessment(&router, &assessment_id).await;
        assert_eq!(
            at(&reported.body, "/dataCollectionDetails/status"),
            json!(ASSESSMENT_STOPPED),
            "case {what:?}: the stopped assessment reports its own terminal status, got {:?}",
            at(&reported.body, "/dataCollectionDetails/status")
        );

        let after = list_servers(&router, json!({})).await;
        for id in &ids {
            assert_eq!(
                listed_status(&after, id),
                COLLECTION_STOPPED,
                "case {what:?}: server {id:?} was collecting when the assessment was stopped, \
                 got {:?}",
                listed_status(&after, id)
            );
            let detail = get_server_details(&router, id).await;
            assert_eq!(
                at(&detail.body, "/serverDetail/dataCollectionStatus"),
                json!(COLLECTION_STOPPED),
                "case {what:?}: GetServerDetails must report server {id:?} as stopped too, got {:?}",
                at(&detail.body, "/serverDetail/dataCollectionStatus")
            );
        }

        // And the status the stop produced is one `ANALYSIS_STATUS` selects on.
        let filtered = list_servers(
            &router,
            json!({"serverCriteria": "ANALYSIS_STATUS", "filterValue": COLLECTION_STOPPED}),
        )
        .await;
        let mut expected = ids.clone();
        expected.sort();
        assert_eq!(
            reported_ids(&filtered),
            expected,
            "case {what:?}: ANALYSIS_STATUS={COLLECTION_STOPPED:?} must report the stopped \
             servers, got {:?}",
            reported_ids(&filtered)
        );
    }
}

/// The completion the clock owes an assessment is mirrored on its servers too: once it is
/// due, every server that was collecting reports the per-server face of `COMPLETE`, and a
/// stop arriving after it leaves that status alone.
#[tokio::test]
async fn a_completion_the_clock_owes_is_observed_on_the_servers() {
    for Inventory { what, servers } in inventories() {
        let ids: Vec<String> = servers.iter().map(|record| record.id.clone()).collect();
        let router = emulator_with(servers);

        let assessment_id = start_assessment(&router, minimal_assessment())
            .await
            .string("/assessmentId");
        std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));

        let listed = list_servers(&router, json!({})).await;
        for id in &ids {
            assert_eq!(
                listed_status(&listed, id),
                COLLECTION_SUCCESS,
                "case {what:?}: the assessment completed, so server {id:?} finished collecting, \
                 got {:?}",
                listed_status(&listed, id)
            );
        }
        let reported = get_assessment(&router, &assessment_id).await;
        assert_eq!(
            at(&reported.body, "/dataCollectionDetails/status"),
            json!(ASSESSMENT_COMPLETE),
            "case {what:?}: the assessment the clock completed reports COMPLETE, got {:?}",
            at(&reported.body, "/dataCollectionDetails/status")
        );

        // A stop of an assessment that already completed changes nothing — on the
        // assessment or on its servers.
        assert_eq!(
            stop_assessment(&router, &assessment_id).await.status,
            200,
            "case {what:?}: a stop of a completed assessment is still accepted"
        );
        let again = list_servers(&router, json!({})).await;
        for id in &ids {
            assert_eq!(
                listed_status(&again, id),
                COLLECTION_SUCCESS,
                "case {what:?}: nothing was collecting any more, so server {id:?} keeps the \
                 status the completion gave it, got {:?}",
                listed_status(&again, id)
            );
        }
    }
}

/// One emulator whose portfolio holds `servers` and ONE assessment still collecting, so a
/// stop can be observed against servers that are not all in the same status.
///
/// It seeds the same records the handlers write — the `assessments` and `servers` maps of
/// the Region every request here is signed for — through the shared engine.
fn emulator_collecting(servers: Vec<ServerRecord>, assessment_id: &str) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let portfolio = store.region_entry(TEST_REGION);
        let count = i32::try_from(servers.len()).unwrap_or(0);
        for record in servers {
            portfolio.servers.insert(record.id.clone(), record);
        }
        let now = SystemTime::now();
        portfolio.assessments.insert(
            assessment_id.to_string(),
            Assessment {
                id: assessment_id.to_string(),
                status: "IN_PROGRESS".to_string(),
                assessment_targets: Vec::new(),
                assessment_data_source_type: None,
                s3bucket_for_analysis_data: Some("migrationhub-strategy-analysis".to_string()),
                s3bucket_for_report_data: None,
                start_time: now,
                completion_time: None,
                status_message: None,
                servers: count,
                success: 0,
                failed: 0,
                in_progress: count,
                // Still collecting: the clock owes it a completion a long way off, so the
                // stop below is what gives it its terminal status.
                completion_due: Some(now + Duration::from_secs(3_600)),
            },
        );
        portfolio.latest_assessment_id = Some(assessment_id.to_string());
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A stop moves only the servers whose collection task was RUNNING: a server holding any
/// other `RunTimeAssessmentStatus` keeps it, because a terminal assessment status says
/// nothing about a collection that was not running.
///
/// One row per value of the catalog, so every status is checked on the side of the rule it
/// falls: the two running values move, the five others do not.
#[tokio::test]
async fn a_stop_moves_only_the_servers_that_were_still_collecting() {
    let servers: Vec<ServerRecord> = RUN_TIME_ASSESSMENT_STATUSES
        .iter()
        .zip(SERVER_IDS)
        .map(|(status, id)| ServerRecord {
            data_collection_status: Some((*status).to_string()),
            ..server(id)
        })
        .collect();
    let router = emulator_collecting(servers, "assessment-seeded");

    let stopped = stop_assessment(&router, "assessment-seeded").await;
    assert_eq!(
        stopped.status, 200,
        "the stop of the collecting assessment must be accepted, got {} {:?}",
        stopped.status, stopped.body
    );

    let listed = list_servers(&router, json!({})).await;
    for (held, id) in RUN_TIME_ASSESSMENT_STATUSES.iter().zip(SERVER_IDS) {
        let expected = if RUNNING_STATUSES.contains(held) {
            COLLECTION_STOPPED
        } else {
            held
        };
        assert_eq!(
            listed_status(&listed, id),
            expected,
            "case {held:?}: server {id:?} must report {expected:?} after the stop, got {:?}",
            listed_status(&listed, id)
        );
    }

    // The stop is observable through the filter as well: exactly the two servers that were
    // collecting are the ones `ANALYSIS_STATUS` now selects as stopped.
    let filtered = list_servers(
        &router,
        json!({"serverCriteria": "ANALYSIS_STATUS", "filterValue": COLLECTION_STOPPED}),
    )
    .await;
    let mut expected: Vec<String> = RUN_TIME_ASSESSMENT_STATUSES
        .iter()
        .zip(SERVER_IDS)
        .filter(|(held, _)| RUNNING_STATUSES.contains(held) || **held == COLLECTION_STOPPED)
        .map(|(_, id)| (*id).to_string())
        .collect();
    expected.sort();
    assert_eq!(
        reported_ids(&filtered),
        expected,
        "ANALYSIS_STATUS={COLLECTION_STOPPED:?} must report the servers the stop moved beside \
         the one already stopped, got {:?}",
        reported_ids(&filtered)
    );
}
