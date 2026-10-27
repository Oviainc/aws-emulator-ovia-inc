// SPDX-License-Identifier: CC0-1.0

//! The CRITERIA an assessment is scoped by, end to end over HTTP.
//!
//! `StartAssessmentRequest$assessmentTargets` is "List of criteria for assessment" over
//! `AssessmentTarget` ("Defines the criteria of assessment", a required `condition`, `name`
//! and `values`), and `DataCollectionDetails$servers` is "The total number of servers in
//! the assessment" with `$inProgress` "The number of servers with the assessment status
//! IN_PROGESS". The rule under test is the one those state, for ANY portfolio rather than
//! for one request: the criteria SELECT which of the account's servers the assessment
//! collects data for — a server is covered when every criterion holds for it, comparing the
//! values of the attribute the criterion's `name` names (the names `ServerCriteria` gives
//! this service's server attributes) against the criterion's `values` under its `condition`
//! — and everything the assessment then reports and records applies to that set and to no
//! other server: its `dataCollectionDetails` counters, the per-server
//! `dataCollectionStatus` ("the status of assessment for the server"), and the analysis its
//! completion records (`lastAnalyzedTimestamp`). Naming no criterion scopes nothing, so the
//! whole inventory is covered; criteria that select nothing give an assessment of no server
//! at all.
//!
//! The table below therefore spans the class the rule generalises over rather than the one
//! criterion the issue cited: every value of the `Condition` catalog, every attribute name
//! of the `ServerCriteria` catalog that names a member, one value per criterion and several,
//! one criterion and the documented maximum of ten, a name outside the catalog, and servers
//! holding no value for the attribute compared. Beside it, the components the analysis of a
//! completed assessment covers, and the bound on how many criterion values one start call
//! may ask the service to select on.
//!
//! A server enters this service only through an import file task's ingestion of the import
//! file, so these cases seed the inventory records directly through the shared engine — the
//! same `servers` map that ingestion writes and these operations read — and drive every
//! assertion through the router, exactly as a client reaches them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ApplicationComponentRecord,
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ImportFileTask,
    ServerRecord, MAX_ASSESSMENT_TARGET_VALUES,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    at, call, get_assessment, register_collector_in, start_assessment, Answer, TEST_REGION,
};
use crate::server_inventory::server;

/// Every value `Condition` admits, from `smithy_query(query="shape",shape="Condition")`.
/// Spelled out here so the table can be checked to span the whole catalog rather than the
/// one condition the issue cited (`EQUALS`).
const CONDITION_VALUES: &[&str] = &["EQUALS", "NOT_EQUALS", "CONTAINS", "NOT_CONTAINS"];

/// The `ServerCriteria` values that NAME a server attribute, from
/// `smithy_query(query="shape",shape="ServerCriteria")` less its `NOT_DEFINED` (which names
/// none). Spelled out for the same reason: the table below is checked to use every one of
/// them as a criterion's `name`.
const NAMED_ATTRIBUTES: &[&str] = &[
    "OS_NAME",
    "SERVER_ID",
    "STRATEGY",
    "DESTINATION",
    "ANALYSIS_STATUS",
    "ERROR_CATEGORY",
];

/// `AssessmentTargets`' documented maximum, from
/// `smithy_query(query="shape",shape="AssessmentTargets")` (`@length(0, 10)`).
const MAX_TARGETS: usize = 10;

/// The `AssessmentStatus` values this suite observes, and the per-server
/// `RunTimeAssessmentStatus` values an assessment moves the servers it covers between.
/// Spelled out independently of the crate's own constants, so a wrong value in
/// `src/state.rs` fails here instead of agreeing with itself.
const ASSESSMENT_IN_PROGRESS: &str = "IN_PROGRESS";
const ASSESSMENT_COMPLETE: &str = "COMPLETE";
const COLLECTION_STARTED: &str = "dataCollectionTaskStarted";
const COLLECTION_SUCCESS: &str = "dataCollectionTaskSuccess";

/// The analysis-data bucket `StartAssessment` cannot start without, named with the prefix
/// its documentation requires.
const ANALYSIS_BUCKET: &str = "migrationhub-strategy-analysis";

/// The inventory every case runs against: five servers differing in operating system (both
/// `OSType` values, each with its own free-text version), in recommended strategy and
/// destination, in the data-collection status they already held, and in whether an error
/// category was recorded — plus one server holding NO value for any of those attributes, so
/// each condition is exercised against a server there is nothing to compare.
///
/// Values come from the model's own catalogs
/// (`smithy_query(query="shape",shape="OSType,Strategy,TargetDestination,RunTimeAssessmentStatus,ServerErrorCategory")`).
/// Ids are `ResourceId`s (`^[0-9a-b]+`) and are deliberately not inserted in order.
fn inventory() -> Vec<ServerRecord> {
    vec![
        ServerRecord {
            os_type: Some("LINUX".to_string()),
            os_version: Some("Ubuntu 20.04".to_string()),
            strategy: Some("Rehost".to_string()),
            target_destination: Some("Amazon Elastic Cloud Compute (EC2)".to_string()),
            data_collection_status: Some("dataCollectionTaskToBeScheduled".to_string()),
            ..server("0a")
        },
        ServerRecord {
            os_type: Some("WINDOWS".to_string()),
            os_version: Some("WindowsServer 2019".to_string()),
            strategy: Some("Rehost".to_string()),
            target_destination: Some("Amazon Elastic Cloud Compute (EC2)".to_string()),
            data_collection_status: Some("dataCollectionTaskFailed".to_string()),
            error_category: Some("CONNECTIVITY_ERROR".to_string()),
            ..server("1a")
        },
        ServerRecord {
            os_type: Some("LINUX".to_string()),
            os_version: Some("Amazon Linux 2".to_string()),
            strategy: Some("Refactor".to_string()),
            target_destination: Some("AWS Fargate".to_string()),
            data_collection_status: Some("dataCollectionTaskToBeScheduled".to_string()),
            ..server("0b")
        },
        ServerRecord {
            os_type: Some("WINDOWS".to_string()),
            os_version: Some("WindowsServer 2022".to_string()),
            strategy: Some("Replatform".to_string()),
            target_destination: Some("AWS Elastic BeanStalk".to_string()),
            data_collection_status: Some("dataCollectionTaskSuccess".to_string()),
            ..server("1b")
        },
        // A server an import brought in holding nothing but its id.
        server("2a"),
    ]
}

/// The application components running on that inventory: one on a Linux server, one on a
/// Windows server, one running on both, and one no server association was recorded for.
///
/// A component is the other resource an assessment analyses, and it runs ON a server
/// (`ApplicationComponentDetail$associatedServerId`), so these four are what the scope of
/// an assessment reaches through its servers. `inclusionStatus` is the including one and the
/// app type is one the service recognises, so nothing but the SCOPE keeps a component out of
/// the analysis (`smithy_query(query="shape",shape="InclusionStatus,AppType,ResourceSubType")`).
fn components() -> Vec<ApplicationComponentRecord> {
    [
        ("aa", vec!["0a"]),
        ("ab", vec!["1a"]),
        ("ba", vec!["0a", "1a"]),
        ("bb", Vec::new()),
    ]
    .into_iter()
    .map(|(id, servers)| ApplicationComponentRecord {
        id: id.to_string(),
        name: Some(format!("component-{id}")),
        app_type: Some("Tomcat".to_string()),
        resource_sub_type: Some("Process".to_string()),
        inclusion_status: Some("includeInAssessment".to_string()),
        associated_server_ids: servers.into_iter().map(str::to_string).collect(),
        ..Default::default()
    })
    .collect()
}

/// One `assessmentTargets` entry.
fn target(condition: &str, name: &str, values: &[&str]) -> Value {
    assert!(
        CONDITION_VALUES.contains(&condition),
        "a condition Condition does not declare: {condition:?}"
    );
    json!({"condition": condition, "name": name, "values": values})
}

/// One case: the criteria a start call names, and the servers of [`inventory`] they select.
struct Case {
    what: &'static str,
    /// The `assessmentTargets` member of the request, or `Value::Null` to leave it out.
    targets: Value,
    /// The ids the criteria cover, written out here rather than computed, so a wrong
    /// selection in the crate fails this suite instead of agreeing with itself.
    covered: &'static [&'static str],
}

/// The criteria every case runs, and what each one covers.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "no assessmentTargets member at all",
            targets: Value::Null,
            covered: &["0a", "0b", "1a", "1b", "2a"],
        },
        Case {
            what: "an empty list of criteria",
            targets: json!([]),
            covered: &["0a", "0b", "1a", "1b", "2a"],
        },
        Case {
            what: "EQUALS on the OSType half of OS_NAME",
            targets: json!([target("EQUALS", "OS_NAME", &["LINUX"])]),
            covered: &["0a", "0b"],
        },
        Case {
            what: "EQUALS on the free-text version half of OS_NAME",
            targets: json!([target("EQUALS", "OS_NAME", &["Ubuntu 20.04"])]),
            covered: &["0a"],
        },
        Case {
            what: "EQUALS with two values, which widen the selection",
            targets: json!([target("EQUALS", "OS_NAME", &["LINUX", "WINDOWS"])]),
            covered: &["0a", "0b", "1a", "1b"],
        },
        Case {
            what: "NOT_EQUALS, the complement of EQUALS",
            targets: json!([target("NOT_EQUALS", "OS_NAME", &["LINUX"])]),
            covered: &["1a", "1b", "2a"],
        },
        Case {
            what: "CONTAINS, matching the version text of two servers",
            targets: json!([target("CONTAINS", "OS_NAME", &["Windows"])]),
            covered: &["1a", "1b"],
        },
        Case {
            what: "NOT_CONTAINS, the complement of CONTAINS",
            targets: json!([target("NOT_CONTAINS", "OS_NAME", &["Windows"])]),
            covered: &["0a", "0b", "2a"],
        },
        Case {
            what: "EQUALS on SERVER_ID, which selects the one server",
            targets: json!([target("EQUALS", "SERVER_ID", &["1b"])]),
            covered: &["1b"],
        },
        Case {
            what: "CONTAINS on SERVER_ID",
            targets: json!([target("CONTAINS", "SERVER_ID", &["b"])]),
            covered: &["0b", "1b"],
        },
        Case {
            what: "EQUALS on STRATEGY",
            targets: json!([target("EQUALS", "STRATEGY", &["Rehost"])]),
            covered: &["0a", "1a"],
        },
        Case {
            what: "EQUALS on DESTINATION",
            targets: json!([target("EQUALS", "DESTINATION", &["AWS Fargate"])]),
            covered: &["0b"],
        },
        Case {
            what: "EQUALS on ANALYSIS_STATUS, the status each server already held",
            targets: json!([target(
                "EQUALS",
                "ANALYSIS_STATUS",
                &["dataCollectionTaskToBeScheduled"]
            )]),
            covered: &["0a", "0b"],
        },
        Case {
            what: "EQUALS on ERROR_CATEGORY",
            targets: json!([target("EQUALS", "ERROR_CATEGORY", &["CONNECTIVITY_ERROR"])]),
            covered: &["1a"],
        },
        Case {
            what: "two criteria, both of which have to hold",
            targets: json!([
                target("EQUALS", "OS_NAME", &["LINUX"]),
                target("EQUALS", "STRATEGY", &["Rehost"]),
            ]),
            covered: &["0a"],
        },
        Case {
            what: "two criteria that no server satisfies together",
            targets: json!([
                target("EQUALS", "OS_NAME", &["LINUX"]),
                target("EQUALS", "ERROR_CATEGORY", &["CONNECTIVITY_ERROR"]),
            ]),
            covered: &[],
        },
        Case {
            what: "a criterion matching nothing, so the assessment covers no server",
            targets: json!([target("EQUALS", "OS_NAME", &["Solaris"])]),
            covered: &[],
        },
        Case {
            what: "EQUALS on a name that is no server attribute, which nothing holds",
            targets: json!([target("EQUALS", "HOST_NAME", &["app-01"])]),
            covered: &[],
        },
        Case {
            what: "NOT_EQUALS on a name that is no server attribute",
            targets: json!([target("NOT_EQUALS", "HOST_NAME", &["app-01"])]),
            covered: &["0a", "0b", "1a", "1b", "2a"],
        },
        Case {
            what: "the documented maximum of ten criteria",
            targets: Value::Array(
                (0..MAX_TARGETS)
                    .map(|_| target("NOT_EQUALS", "OS_NAME", &["Solaris"]))
                    .collect(),
            ),
            covered: &["0a", "0b", "1a", "1b", "2a"],
        },
    ]
}

/// One emulator holding [`inventory`] in the Region every request here is signed for, with
/// a registered data collector beside it — an assessment is made of the data a collector
/// collects, so an account that registered none cannot start one at all.
///
/// The engine is handed back beside the router so a case can bring the completion the clock
/// owes an assessment forward instead of sleeping through it.
fn portfolio() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let region = store.region_entry(TEST_REGION);
        for record in inventory() {
            region.servers.insert(record.id.clone(), record);
        }
        for record in components() {
            region
                .application_components
                .insert(record.id.clone(), record);
        }
    }
    register_collector_in(&engine, &[TEST_REGION]);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine.clone()));
    (engine, router)
}

/// Move the instant the clock owes every assessment of this portfolio its completion to
/// `due`.
///
/// The due instant is what the store compares `now` against, so this is what lets a case
/// observe the collecting phase and then the completion without racing the completion span
/// or spending it asleep. An assessment that already reached a terminal status is owed
/// nothing and is left alone.
fn owe_completion_at(engine: &Arc<engine::InMemoryEngine>, due: SystemTime) {
    let state =
        engine.state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
    let mut store = state.lock();
    for assessment in store.region_entry(TEST_REGION).assessments.values_mut() {
        if assessment.completion_due.is_some() {
            assessment.completion_due = Some(due);
        }
    }
}

/// The instant a case pushes a completion to while it observes the collecting phase, and
/// the one it brings it back to when the completion is to fall due.
fn still_collecting() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3_600)
}

fn completion_owed() -> SystemTime {
    SystemTime::now() - Duration::from_millis(100)
}

/// The start request a case's criteria make.
fn request(targets: &Value) -> Value {
    let mut body = json!({"s3bucketForAnalysisData": ANALYSIS_BUCKET});
    if !targets.is_null() {
        body["assessmentTargets"] = targets.clone();
    }
    body
}

/// Every server `ListServers` reports, by id.
async fn listed(router: &MultiServiceRouter) -> BTreeMap<String, Value> {
    let answer = call(router, "POST", "/list-servers", Some(json!({}))).await;
    assert_eq!(
        answer.status, 200,
        "ListServers must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .body
        .get("serverInfos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|server| {
            (
                server["id"].as_str().unwrap_or_default().to_string(),
                server,
            )
        })
        .collect()
}

/// One `dataCollectionDetails` counter of a `GetAssessment` answer.
fn counter(case: &str, answer: &Answer, member: &str) -> i64 {
    at(&answer.body, &format!("/dataCollectionDetails/{member}"))
        .as_i64()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: dataCollectionDetails must report {member}, got {:?}",
                at(&answer.body, "/dataCollectionDetails")
            )
        })
}

/// The criteria scope the assessment: its counters count the servers they select, the
/// servers they select are the ones collecting for it, and the analysis its completion
/// records covers those servers and no other.
#[tokio::test]
async fn an_assessment_covers_the_servers_its_criteria_select() {
    // The table has to span both catalogs, or a condition or an attribute could be wrong
    // with nothing to catch it.
    let spelled = cases()
        .iter()
        .map(|case| case.targets.to_string())
        .collect::<String>();
    for condition in CONDITION_VALUES {
        assert!(
            spelled.contains(condition),
            "the table must exercise the {condition:?} condition"
        );
    }
    for name in NAMED_ATTRIBUTES {
        assert!(
            spelled.contains(name),
            "the table must exercise a criterion on {name:?}"
        );
    }

    let seeded: BTreeMap<String, Value> = inventory()
        .into_iter()
        .map(|record| {
            (
                record.id.clone(),
                record
                    .data_collection_status
                    .map_or(Value::Null, Value::String),
            )
        })
        .collect();

    for case in cases() {
        let what = case.what;
        let covered = case.covered;
        let expected = i64::try_from(covered.len()).expect("the row covers a counted number");
        let (engine, router) = portfolio();

        let started = start_assessment(&router, request(&case.targets)).await;
        assert_eq!(
            started.status, 200,
            "case {what:?}: the start must be accepted, got {} {:?}",
            started.status, started.body
        );
        let id = started.string("/assessmentId");
        // Nothing has read the assessment yet, so pushing the completion it is owed out
        // leaves it collecting however slow this run is.
        owe_completion_at(&engine, still_collecting());

        // The counters count the servers the criteria selected, and the criteria come back
        // exactly as they were supplied.
        let reported = get_assessment(&router, &id).await;
        assert_eq!(
            counter(what, &reported, "servers"),
            expected,
            "case {what:?}: the assessment covers {covered:?}, got {:?}",
            at(&reported.body, "/dataCollectionDetails")
        );
        assert_eq!(
            at(&reported.body, "/assessmentTargets"),
            if case.targets.is_null() {
                json!([])
            } else {
                case.targets.clone()
            },
            "case {what:?}: the criteria must come back as supplied, got {:?}",
            at(&reported.body, "/assessmentTargets")
        );
        assert_eq!(
            at(&reported.body, "/dataCollectionDetails/status"),
            json!(ASSESSMENT_IN_PROGRESS),
            "case {what:?}: the assessment is still collecting, got {:?}",
            at(&reported.body, "/dataCollectionDetails/status")
        );
        assert_eq!(
            counter(what, &reported, "inProgress"),
            expected,
            "case {what:?}: every covered server is collecting, got {:?}",
            at(&reported.body, "/dataCollectionDetails")
        );
        assert_eq!(
            counter(what, &reported, "success")
                + counter(what, &reported, "failed")
                + counter(what, &reported, "inProgress"),
            expected,
            "case {what:?}: the per-server counters must account for all {expected} servers, \
             got {:?}",
            at(&reported.body, "/dataCollectionDetails")
        );

        // A server outside the scope is not collecting for this assessment: it keeps the
        // status it held and is analysed by nothing.
        let servers = listed(&router).await;
        for (id, record) in &servers {
            if covered.contains(&id.as_str()) {
                assert_eq!(
                    record["dataCollectionStatus"],
                    json!(COLLECTION_STARTED),
                    "case {what:?}: server {id:?} is covered, so it is collecting, got {:?}",
                    record["dataCollectionStatus"]
                );
            } else {
                assert_eq!(
                    record["dataCollectionStatus"],
                    seeded[id],
                    "case {what:?}: server {id:?} is outside the scope, so it keeps the status \
                     it held, got {:?}",
                    record["dataCollectionStatus"]
                );
            }
        }

        // And once the completion the clock owes falls due, the assessment reports every
        // covered server as a success and the analysis it records covers exactly them.
        owe_completion_at(&engine, completion_owed());
        let settled = get_assessment(&router, &id).await;
        assert_eq!(
            at(&settled.body, "/dataCollectionDetails/status"),
            json!(ASSESSMENT_COMPLETE),
            "case {what:?}: the completion the clock owed is materialised, got {:?}",
            at(&settled.body, "/dataCollectionDetails/status")
        );
        assert_eq!(
            counter(what, &settled, "success"),
            expected,
            "case {what:?}: the completion collected all {expected} covered servers, got {:?}",
            at(&settled.body, "/dataCollectionDetails")
        );
        assert_eq!(
            counter(what, &settled, "inProgress"),
            0,
            "case {what:?}: nothing is collecting once the assessment completed, got {:?}",
            at(&settled.body, "/dataCollectionDetails")
        );

        let after = listed(&router).await;
        for (id, record) in &after {
            if covered.contains(&id.as_str()) {
                assert_eq!(
                    record["dataCollectionStatus"],
                    json!(COLLECTION_SUCCESS),
                    "case {what:?}: server {id:?} finished collecting for the assessment that \
                     covered it, got {:?}",
                    record["dataCollectionStatus"]
                );
                assert!(
                    record["lastAnalyzedTimestamp"].is_number(),
                    "case {what:?}: server {id:?} was analysed by the assessment covering it, \
                     got {:?}",
                    record["lastAnalyzedTimestamp"]
                );
            } else {
                assert_eq!(
                    record["dataCollectionStatus"],
                    seeded[id],
                    "case {what:?}: server {id:?} was never collecting, so the completion \
                     leaves its status alone, got {:?}",
                    record["dataCollectionStatus"]
                );
                assert_eq!(
                    record["lastAnalyzedTimestamp"],
                    Value::Null,
                    "case {what:?}: server {id:?} was outside the scope, so no analysis covered \
                     it, got {:?}",
                    record["lastAnalyzedTimestamp"]
                );
            }
        }
    }
}

/// One pair of assessments started against the SAME portfolio, and what each one covers.
struct Pair {
    what: &'static str,
    first: Value,
    first_covered: &'static [&'static str],
    second: Value,
    second_covered: &'static [&'static str],
}

/// Two assessments of one portfolio report their OWN scopes: criteria that select different
/// servers produce different counters, and the servers only one of them covers are
/// collecting for only that one.
#[tokio::test]
async fn two_assessments_of_one_portfolio_report_their_own_scopes() {
    let pairs = vec![
        Pair {
            what: "mutually exclusive criteria",
            first: json!([target("EQUALS", "OS_NAME", &["LINUX"])]),
            first_covered: &["0a", "0b"],
            second: json!([target("EQUALS", "OS_NAME", &["WINDOWS"])]),
            second_covered: &["1a", "1b"],
        },
        Pair {
            what: "criteria selecting nothing beside criteria selecting everything",
            first: json!([target("EQUALS", "OS_NAME", &["Solaris"])]),
            first_covered: &[],
            second: json!([]),
            second_covered: &["0a", "0b", "1a", "1b", "2a"],
        },
        Pair {
            what: "one server beside the whole inventory",
            first: json!([target("EQUALS", "SERVER_ID", &["2a"])]),
            first_covered: &["2a"],
            second: Value::Null,
            second_covered: &["0a", "0b", "1a", "1b", "2a"],
        },
    ];

    for pair in pairs {
        let what = pair.what;
        let (_engine, router) = portfolio();

        let first = start_assessment(&router, request(&pair.first)).await;
        let second = start_assessment(&router, request(&pair.second)).await;
        for (which, answer) in [("first", &first), ("second", &second)] {
            assert_eq!(
                answer.status, 200,
                "case {what:?}: the {which} start must be accepted, got {} {:?}",
                answer.status, answer.body
            );
        }
        let first_id = first.string("/assessmentId");
        let second_id = second.string("/assessmentId");
        assert_ne!(
            first_id, second_id,
            "case {what:?}: two starts mint two assessments, got {first_id:?} twice"
        );

        for (which, id, covered) in [
            ("first", &first_id, pair.first_covered),
            ("second", &second_id, pair.second_covered),
        ] {
            let expected = i64::try_from(covered.len()).expect("the row covers a counted number");
            let reported = get_assessment(&router, id).await;
            assert_eq!(
                counter(what, &reported, "servers"),
                expected,
                "case {what:?}: the {which} assessment covers {covered:?}, got {:?}",
                at(&reported.body, "/dataCollectionDetails")
            );
        }

        // A server only the second assessment covers is collecting, and one neither covers
        // is not: the scopes are observable on the servers, not only on the records.
        let servers = listed(&router).await;
        for (id, record) in &servers {
            let in_either = pair.first_covered.contains(&id.as_str())
                || pair.second_covered.contains(&id.as_str());
            if !in_either {
                assert_ne!(
                    record["dataCollectionStatus"],
                    json!(COLLECTION_STARTED),
                    "case {what:?}: server {id:?} is outside both scopes, so neither assessment \
                     collects for it, got {:?}",
                    record["dataCollectionStatus"]
                );
            }
        }
    }
}

/// The application components the analysis of a completed assessment covers: the ones
/// running on a server the assessment covered, and no other.
///
/// The rows differ in how far the criteria narrow the scope — the whole inventory, one
/// operating system, one server, and nothing at all — and the components differ in which
/// servers they run on (one covered, one not, both, none), so each component is checked on
/// the side of the rule it falls.
#[tokio::test]
async fn the_analysis_covers_the_components_of_the_servers_an_assessment_covered() {
    let rows: Vec<(&str, Value, &[&str])> = vec![
        ("no criteria at all", Value::Null, &["aa", "ab", "ba"]),
        (
            "the Linux servers only",
            json!([target("EQUALS", "OS_NAME", &["LINUX"])]),
            // "ab" runs on a Windows server alone, and "bb" on no server at all.
            &["aa", "ba"],
        ),
        (
            "the one Windows server the components run on",
            json!([target("EQUALS", "SERVER_ID", &["1a"])]),
            &["ab", "ba"],
        ),
        (
            "criteria no server satisfies",
            json!([target("EQUALS", "OS_NAME", &["Solaris"])]),
            &[],
        ),
    ];

    for (what, targets, analyzed) in rows {
        let (engine, router) = portfolio();

        let started = start_assessment(&router, request(&targets)).await;
        assert_eq!(
            started.status, 200,
            "case {what:?}: the start must be accepted, got {} {:?}",
            started.status, started.body
        );
        let id = started.string("/assessmentId");

        // Before the completion falls due the assessment has analysed nothing at all.
        owe_completion_at(&engine, still_collecting());
        for component in components() {
            let detail = component_details(&router, &component.id).await;
            assert_eq!(
                at(&detail.body, "/applicationComponentDetail/lastAnalyzedTimestamp"),
                Value::Null,
                "case {what:?}: component {:?} cannot be analysed while the assessment is \
                 still collecting, got {:?}",
                component.id,
                at(&detail.body, "/applicationComponentDetail/lastAnalyzedTimestamp")
            );
        }

        owe_completion_at(&engine, completion_owed());
        let settled = get_assessment(&router, &id).await;
        assert_eq!(
            at(&settled.body, "/dataCollectionDetails/status"),
            json!(ASSESSMENT_COMPLETE),
            "case {what:?}: the completion the clock owed is materialised, got {:?}",
            at(&settled.body, "/dataCollectionDetails/status")
        );

        for component in components() {
            let detail = component_details(&router, &component.id).await;
            let reported = at(
                &detail.body,
                "/applicationComponentDetail/lastAnalyzedTimestamp",
            );
            if analyzed.contains(&component.id.as_str()) {
                assert!(
                    reported.is_number(),
                    "case {what:?}: component {:?} runs on a covered server, so the analysis \
                     covered it, got {reported:?}",
                    component.id
                );
                assert!(
                    at(&detail.body, "/applicationComponentDetail/recommendationSet").is_object(),
                    "case {what:?}: component {:?} carries the recommendation the analysis \
                     computed, got {:?}",
                    component.id,
                    at(&detail.body, "/applicationComponentDetail/recommendationSet")
                );
            } else {
                assert_eq!(
                    reported,
                    Value::Null,
                    "case {what:?}: component {:?} runs on no server the assessment covered, so \
                     no analysis covered it, got {reported:?}",
                    component.id
                );
            }
        }
    }
}

/// `GetApplicationComponentDetails` for one component.
async fn component_details(router: &MultiServiceRouter, id: &str) -> Answer {
    let answer = call(
        router,
        "GET",
        &format!("/get-applicationcomponent-details/{id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "GetApplicationComponentDetails on {id:?} must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// One criterion listing `values` values, all of them legal.
fn values_of(values: usize) -> Value {
    let listed: Vec<String> = (0..values).map(|index| format!("os-{index}")).collect();
    json!([{"condition": "EQUALS", "name": "OS_NAME", "values": listed}])
}

/// `AssessmentTargetValues` declares no `@length`, so how many values the criteria of one
/// assessment may list is the emulator's own bound: AT it the criteria are accepted, past it
/// the start is refused with 400 `ValidationException` and mints nothing.
#[tokio::test]
async fn criteria_past_the_accepted_number_of_values_are_refused() {
    let (_engine, router) = portfolio();

    for over in [
        MAX_ASSESSMENT_TARGET_VALUES + 1,
        MAX_ASSESSMENT_TARGET_VALUES * 3,
    ] {
        let case =
            format!("{over} criterion values, past the accepted {MAX_ASSESSMENT_TARGET_VALUES}");
        let answer = start_assessment(&router, request(&values_of(over))).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/assessmentId"),
            Value::Null,
            "case {case:?}: a refused start must mint no assessment, got {:?}",
            answer.body
        );
    }

    // AT the bound the criteria are accepted: the refusal is for passing it, not for
    // naming many values at all. None of them selects a server of this inventory, so the
    // assessment it starts covers none.
    let at_bound = start_assessment(
        &router,
        request(&values_of(MAX_ASSESSMENT_TARGET_VALUES)),
    )
    .await;
    assert_eq!(
        at_bound.status, 200,
        "criteria listing exactly {MAX_ASSESSMENT_TARGET_VALUES} values are accepted, got {} \
         with {:?}",
        at_bound.status, at_bound.body
    );
    let reported = get_assessment(&router, &at_bound.string("/assessmentId")).await;
    assert_eq!(
        counter("at the bound", &reported, "servers"),
        0,
        "no value of the criteria names an operating system this inventory holds, so the \
         assessment covers no server, got {:?}",
        at(&reported.body, "/dataCollectionDetails")
    );
}

/// The id of the component the seeded import file task brings in, and of the covered server
/// it runs on. Both follow `ResourceId`'s `^[0-9a-b]+`, as the ingestion's own ids do.
const PENDING_COMPONENT: &str = "bab";
const COVERED_SERVER: &str = "0a";

/// Materialising the completion an assessment is owed first materialises every completion an
/// IMPORT is owed, so the analysis that completion records covers the records the clock had
/// already brought in: a component an import whose completion fell due before the
/// assessment's brought in, running on a server the assessment covers, is analysed by it.
///
/// Without that ordering the record is skipped for good — a completion is materialised once,
/// and the assessment's due instant is cleared by it.
#[tokio::test]
async fn a_completion_analyses_the_records_a_due_import_had_already_brought_in() {
    let (engine, router) = portfolio();

    let started = start_assessment(
        &router,
        request(&json!([target("EQUALS", "SERVER_ID", &[COVERED_SERVER])])),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the start must be accepted, got {} {:?}",
        started.status, started.body
    );
    let id = started.string("/assessmentId");
    owe_completion_at(&engine, still_collecting());

    // An import file task holding one component for the covered server, with its own
    // completion already due and nothing having materialised it.
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let now = SystemTime::now();
        let task = ImportFileTask {
            id: "import-file-task-000000000001".to_string(),
            import_name: "pending-import".to_string(),
            input_s3_bucket: "migrationhub-strategy-import".to_string(),
            input_s3_key: "inventory.json".to_string(),
            data_source_type: None,
            group_id: Vec::new(),
            status_report_s3_bucket: None,
            status_report_s3_key: None,
            // The non-terminal status and the terminal one the clock owes it, spelled out
            // from `smithy_query(query="shape",shape="ImportFileTaskStatus")`.
            status: "ImportInProgress".to_string(),
            terminal_status: "ImportSuccess".to_string(),
            start_time: now - Duration::from_secs(1),
            completion_time: None,
            number_of_records_success: 1,
            number_of_records_failed: 0,
            completion_due: Some(now - Duration::from_millis(200)),
            pending_servers: Vec::new(),
            pending_components: vec![ApplicationComponentRecord {
                id: PENDING_COMPONENT.to_string(),
                name: Some("component-from-the-import".to_string()),
                app_type: Some("Tomcat".to_string()),
                resource_sub_type: Some("Process".to_string()),
                inclusion_status: Some("includeInAssessment".to_string()),
                associated_server_ids: vec![COVERED_SERVER.to_string()],
                ..Default::default()
            }],
        };
        store
            .region_entry(TEST_REGION)
            .import_tasks
            .insert(task.id.clone(), task);
    }

    // The read that materialises the assessment's completion is the only call in between,
    // so the import can only have been settled by it.
    owe_completion_at(&engine, completion_owed());
    let settled = get_assessment(&router, &id).await;
    assert_eq!(
        at(&settled.body, "/dataCollectionDetails/status"),
        json!(ASSESSMENT_COMPLETE),
        "the completion the clock owed is materialised, got {:?}",
        at(&settled.body, "/dataCollectionDetails/status")
    );

    let detail = component_details(&router, PENDING_COMPONENT).await;
    assert_eq!(
        at(
            &detail.body,
            "/applicationComponentDetail/lastAnalyzedTimestamp"
        ),
        at(&settled.body, "/dataCollectionDetails/completionTime"),
        "the component the due import brought in runs on a covered server, so the \
         completion's own analysis covered it, got {:?}",
        at(
            &detail.body,
            "/applicationComponentDetail/lastAnalyzedTimestamp"
        )
    );
}
