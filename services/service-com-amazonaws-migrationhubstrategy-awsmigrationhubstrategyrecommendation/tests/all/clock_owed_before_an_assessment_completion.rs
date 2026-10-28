// SPDX-License-Identifier: CC0-1.0

//! The portfolio an ASSESSMENT COMPLETION is materialised over, end to end over HTTP.
//!
//! The rule under test is one of ordering, for ANY handler of this service that
//! materialises what the clock owes a portfolio rather than for one request: every
//! completion the clock already owed at the instant of the call is materialised, in the
//! order the completions FELL DUE, and each one is materialised over the records the clock
//! had already produced at its own due instant. For the two completions this service runs
//! that is:
//!
//! * an import file task's completion, which is what moves the records it ingested into
//!   the inventory the read path reports;
//! * an assessment's completion, which is what records the analysis on the resources that
//!   assessment covered.
//!
//! So the analysis an assessment's completion stands for covers the records an import whose
//! completion fell due BEFORE it brought in — a component running on a server the
//! assessment covered among them — and does not cover the records of one that fell due
//! after it, which no analysis had been owed for yet. Neither outcome depends on whether
//! some unrelated caller happened to read the portfolio first, and neither depends on which
//! handler materialised the completion.
//!
//! The rows span the class the rule generalises over rather than the one sequence a report
//! cites: both orders of the two completions, both orders of the calls (one unrelated read
//! before the materialising call and none at all), both handlers that materialise an
//! assessment completion (`GetAssessment` and `StopAssessment`), and four saved
//! preferences spanning every strategy an analysis recommends — including the one whose
//! recommendation is identical to the baseline a freshly imported server carries, where the
//! analysis instant is the only thing that tells an analysed record from an unanalysed one.
//!
//! Nothing here seeds a record: the emulator mounts S3 beside Strategy Recommendations over
//! one engine, each case writes its import files through S3's own front door and every
//! server and component arrives through a real `StartImportFileTask` ingestion, exactly as
//! a client brings the account's data in.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::assessment_analysis::{assert_recommendation, expected_application, Expected};
use crate::import_and_assess::{
    at, bring_every_completion_forward, call, get_assessment, get_import, hold_buckets,
    minimal_assessment, register_collector_in, start_assessment, start_import, stop_assessment,
    Answer, TEST_BUCKETS, TEST_REGION,
};
use crate::import_ingestion::{create_bucket, put_import_file, IMPORT_BUCKET};

/// The `AssessmentStatus` an assessment the clock completed reports.
const ASSESSMENT_COMPLETE: &str = "COMPLETE";

/// The keys the two import files of a case are written under, and the names the resources
/// they declare come in with.
const SERVER_KEY: &str = "inventory/servers.json";
const COMPONENT_KEY: &str = "inventory/processes.json";
const SERVER_NAME: &str = "app-01";
const COMPONENT_NAME: &str = "web-tier";

/// The resource id the first import file DECLARES for the machine — the account's own name
/// for it, which is what a LATER import file refers to that server by.
const DECLARED_SERVER_ID: &str = "server-1";

/// `AppType`'s value for the component the second import brings in: one the service
/// recognises and that is not a Windows web stack, so the destination the saved preference
/// names is the only thing its recommendation depends on. From
/// `smithy_query(query="shape",shape="AppType")`.
const COMPONENT_APP_TYPE: &str = "Tomcat";

/// One emulator holding S3 beside Strategy Recommendations over one shared engine — what
/// the assembled emulator does — with a data collector registered in the Region every
/// request here is signed for, since an assessment is made of the data a collector
/// collects and an account with no data source cannot start one at all.
///
/// The engine is handed back beside the router so a case can bring the completions the
/// clock owes this portfolio forward ([`bring_every_completion_forward`]) instead of
/// sleeping through their spans.
fn emulator() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    register_collector_in(&engine, &[TEST_REGION]);
    // And the buckets a start call names, in the object storage it resolves them against.
    hold_buckets(&engine, TEST_BUCKETS);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (engine, router)
}

async fn put_preferences(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/put-portfolio-preferences", Some(body)).await
}

async fn list_servers(router: &MultiServiceRouter) -> Answer {
    call(router, "POST", "/list-servers", Some(json!({}))).await
}

async fn list_components(router: &MultiServiceRouter) -> Answer {
    call(router, "POST", "/list-applicationcomponents", Some(json!({}))).await
}

async fn server_details(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-server-details/{id}"), None).await
}

async fn component_details(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-applicationcomponent-details/{id}"),
        None,
    )
    .await
}

/// The import file declaring the one SERVER of these cases, in the format an import
/// document is written in.
fn server_document() -> String {
    json!({
        "ImportFormatVersion": 1,
        "Resources": [{
            "ResourceType": "SERVER",
            "ResourceId": DECLARED_SERVER_ID,
            "ResourceName": SERVER_NAME,
            "OSType": "Linux",
            "OSVersion": "Ubuntu 20.04",
            "CPUArchitecture": "64bit",
        }],
    })
    .to_string()
}

/// The import file declaring a PROCESS running on the server the FIRST import brought in,
/// named by the resource id that file declared for it.
fn component_document() -> String {
    json!({
        "ImportFormatVersion": 1,
        "Resources": [{
            "ResourceType": "PROCESS",
            "ResourceId": "process-1",
            "ResourceName": COMPONENT_NAME,
            "AssociatedServerId": DECLARED_SERVER_ID,
            "AppType": COMPONENT_APP_TYPE,
        }],
    })
    .to_string()
}

/// The id a list reported for the entry named `name`, or the empty string when it reported
/// no such entry.
fn listed_id(answer: &Answer, pointer: &str, name: &str) -> String {
    at(&answer.body, pointer)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|entry| at(entry, "/name").as_str() == Some(name))
        .map(|entry| at(entry, "/id").as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}

/// Start the import of the file at `key` and answer the id of the task.
async fn import(case: &str, router: &MultiServiceRouter, name: &str, key: &str) -> String {
    let started = start_import(
        router,
        json!({"name": name, "S3Bucket": IMPORT_BUCKET, "s3key": key}),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: the import of {key:?} must start, got {} with {:?}",
        started.status, started.body
    );
    started.string("/id")
}

/// Which handler a row has materialise the completion the clock owes the assessment: both
/// reach the store through the same settlement, so both are driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Materialise {
    Read,
    Stop,
}

impl Materialise {
    fn what(self) -> &'static str {
        match self {
            Self::Read => "GetAssessment materialises the completion",
            Self::Stop => "StopAssessment materialises the completion",
        }
    }

    async fn call(self, case: &str, router: &MultiServiceRouter, assessment_id: &str) {
        let answer = match self {
            Self::Read => get_assessment(router, assessment_id).await,
            Self::Stop => stop_assessment(router, assessment_id).await,
        };
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the call that materialises the completion must be accepted, got {} \
             with {:?}",
            answer.status, answer.body
        );
    }
}

/// What one case observed of the two resources once the completion had been materialised:
/// each resource's reported `RecommendationSet` and the instant it was analysed at.
#[derive(Debug)]
struct Observed {
    server_recommendation: Value,
    server_analyzed_at: Value,
    component_recommendation: Value,
    component_analyzed_at: Value,
}

/// The same outcome with the absolute instants reduced to what two runs on two clocks can
/// be compared by: WHETHER each resource carries an analysis instant, and whether both
/// carry the same one.
///
/// Each run stands up its own emulator and so completes at its own instant; everything else
/// about what the completion recorded has to be identical, which is what makes it
/// independent of the order the calls came in.
#[derive(Debug, PartialEq, Eq)]
struct Shape {
    server_recommendation: Value,
    server_analysed: bool,
    component_recommendation: Value,
    component_analysed: bool,
    analysed_together: bool,
}

impl Observed {
    fn shape(&self) -> Shape {
        Shape {
            server_recommendation: self.server_recommendation.clone(),
            server_analysed: self.server_analyzed_at.is_number(),
            component_recommendation: self.component_recommendation.clone(),
            component_analysed: self.component_analyzed_at.is_number(),
            analysed_together: self.component_analyzed_at == self.server_analyzed_at,
        }
    }
}

/// One row's saved preference: the `ManagementPreference` variant and the destination of
/// that variant's own enumeration it names.
struct Preference {
    variant: &'static str,
    destination: &'static str,
}

/// The preferences the rows run against: a managed container platform, a self-managed one,
/// the enumerations' own "no destination chosen" value, and Amazon EC2 — whose
/// recommendation is exactly the baseline a freshly imported server already carries, so
/// that row turns on the analysis instant alone. Each destination is one its variant's own
/// enumeration declares (`AwsManagedTargetDestination`, `SelfManageTargetDestination`,
/// `NoPreferenceTargetDestination`).
const PREFERENCES: &[Preference] = &[
    Preference {
        variant: "awsManagedResources",
        destination: "AWS Fargate",
    },
    Preference {
        variant: "selfManageResources",
        destination: "Amazon Elastic Container Service (ECS)",
    },
    Preference {
        variant: "awsManagedResources",
        destination: "None specified",
    },
    Preference {
        variant: "noPreference",
        destination: "Amazon Elastic Cloud Compute (EC2)",
    },
];

/// Run one row: bring the account's data in through two imports, assess the portfolio, and
/// report what the two resources carry once `handler` has materialised the completion.
///
/// `component_import_first` is whether the import that brings the COMPONENT in is started
/// before the assessment — which, with every completion owed the same span, is what decides
/// whether its completion falls due before the assessment's. `read_first` inserts one
/// unrelated read of the portfolio before the materialising call.
async fn run(
    case: &str,
    preference: &Preference,
    component_import_first: bool,
    read_first: bool,
    handler: Materialise,
) -> Observed {
    let (engine, router) = emulator();

    // The preferences are the inputs the analysis is computed from, and a save made while
    // a task is running is refused, so they are saved before anything is started.
    let saved = put_preferences(
        &router,
        json!({
            "applicationPreferences": {
                "managementPreference": {
                    preference.variant: {"targetDestination": [preference.destination]}
                }
            }
        }),
    )
    .await;
    assert_eq!(
        saved.status, 200,
        "case {case:?}: the preferences must be saved, got {} with {:?}",
        saved.status, saved.body
    );

    create_bucket(case, &router).await;
    put_import_file(case, &router, SERVER_KEY, &server_document()).await;
    put_import_file(case, &router, COMPONENT_KEY, &component_document()).await;

    // The server arrives first and its import is already complete before anything else
    // runs, so it is part of the inventory the assessment's criteria are applied to.
    import(case, &router, "servers", SERVER_KEY).await;
    bring_every_completion_forward(&engine);

    // Then the component's import and the assessment, in the order this row puts them in.
    let component_import = if component_import_first {
        Some(import(case, &router, "processes", COMPONENT_KEY).await)
    } else {
        None
    };
    let started = start_assessment(&router, minimal_assessment()).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: the assessment must start, got {} with {:?}",
        started.status, started.body
    );
    let assessment_id = started.string("/assessmentId");
    let component_import = match component_import {
        Some(id) => id,
        None => import(case, &router, "processes", COMPONENT_KEY).await,
    };

    // Both of the remaining completions are now owed, one span further back, in the order
    // the calls above put them in.
    bring_every_completion_forward(&engine);

    if read_first {
        let listed = list_servers(&router).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the unrelated read before the materialising call must succeed, got \
             {} with {:?}",
            listed.status, listed.body
        );
    }
    handler.call(case, &router, &assessment_id).await;

    // What the two completions produced, as the wire reports it.
    let reported = get_assessment(&router, &assessment_id).await;
    assert_eq!(
        at(&reported.body, "/dataCollectionDetails/status"),
        json!(ASSESSMENT_COMPLETE),
        "case {case:?}: the assessment the clock completed reports {ASSESSMENT_COMPLETE}, got {:?}",
        reported.body
    );
    let assessment_completed_at = at(&reported.body, "/dataCollectionDetails/completionTime");
    assert!(
        assessment_completed_at.is_number(),
        "case {case:?}: a completed assessment reports its completion time, got \
         {assessment_completed_at:?}"
    );

    let imported = get_import(&router, &component_import).await;
    let import_completed_at = at(&imported.body, "/completionTime");
    assert!(
        import_completed_at.is_number(),
        "case {case:?}: a completed import reports its completion time, got \
         {import_completed_at:?}"
    );

    // The two completions this row is about fell due at different instants, and in the
    // order the row put them in: a row whose clock says otherwise would assert the wrong
    // half of the rule, so it fails here instead.
    let import_was_first = import_completed_at.as_f64() < assessment_completed_at.as_f64();
    assert_eq!(
        import_was_first, component_import_first,
        "case {case:?}: the component's import completed at {import_completed_at:?} and the \
         assessment at {assessment_completed_at:?}, which is not the order this case starts them \
         in"
    );

    let servers = list_servers(&router).await;
    let server_id = listed_id(&servers, "/serverInfos", SERVER_NAME);
    assert!(
        !server_id.is_empty(),
        "case {case:?}: the imported server must be in the inventory, got {:?}",
        servers.body
    );
    let components = list_components(&router).await;
    let component_id = listed_id(&components, "/applicationComponentInfos", COMPONENT_NAME);
    assert!(
        !component_id.is_empty(),
        "case {case:?}: the imported component must be in the inventory, got {:?}",
        components.body
    );

    let server = server_details(&router, &server_id).await;
    let component = component_details(&router, &component_id).await;
    Observed {
        server_recommendation: at(&server.body, "/serverDetail/recommendationSet"),
        server_analyzed_at: at(&server.body, "/serverDetail/lastAnalyzedTimestamp"),
        component_recommendation: at(
            &component.body,
            "/applicationComponentDetail/recommendationSet",
        ),
        component_analyzed_at: at(
            &component.body,
            "/applicationComponentDetail/lastAnalyzedTimestamp",
        ),
    }
}

/// Assert `observed` is what the rule requires of this row.
///
/// The server was part of the inventory when the assessment started, so the completion
/// analysed it either way. The component was brought in by the other import: it carries
/// that analysis when its own completion fell due FIRST, and carries no recommendation and
/// no analysis instant at all when it fell due after — nothing had been owed an analysis of
/// a record that had not arrived.
fn assert_observed(case: &str, observed: &Observed, analysed: bool, expected: Expected) {
    assert_recommendation(
        case,
        "the server's detail",
        &json!({"recommendationSet": observed.server_recommendation}),
        "/recommendationSet",
        expected,
    );
    assert!(
        observed.server_analyzed_at.is_number(),
        "case {case:?}: the server the assessment covered was analysed when the completion was \
         materialised, got {:?}",
        observed.server_analyzed_at
    );
    if analysed {
        assert_recommendation(
            case,
            "the component's detail",
            &json!({"recommendationSet": observed.component_recommendation}),
            "/recommendationSet",
            expected,
        );
        assert_eq!(
            observed.component_analyzed_at, observed.server_analyzed_at,
            "case {case:?}: the component was analysed by the same completion as the server it \
             runs on, got {:?} against {:?}",
            observed.component_analyzed_at, observed.server_analyzed_at
        );
    } else {
        assert_eq!(
            observed.component_recommendation,
            Value::Null,
            "case {case:?}: the component arrived after the completion fell due, so nothing \
             recommended anything for it, got {:?}",
            observed.component_recommendation
        );
        assert_eq!(
            observed.component_analyzed_at,
            Value::Null,
            "case {case:?}: the component arrived after the completion fell due, so no analysis \
             instant is reported for it, got {:?}",
            observed.component_analyzed_at
        );
    }
}

/// A completion is materialised over the records the clock had already produced when it
/// fell due: the component an import whose completion fell due FIRST brought in carries the
/// analysis of the assessment covering the server it runs on, the component of an import
/// that fell due after it carries none, and neither answer depends on whether an unrelated
/// read ran first or on which handler materialised the completion.
#[tokio::test]
async fn a_completion_analyses_the_records_the_clock_had_already_brought_in() {
    for preference in PREFERENCES {
        for component_import_first in [true, false] {
            for handler in [Materialise::Read, Materialise::Stop] {
                let order = if component_import_first {
                    "the component's import completes before the assessment"
                } else {
                    "the component's import completes after the assessment"
                };
                // Every resource of these cases is an application workload running on
                // Linux, so the destination the saved preference names is what selects the
                // recommendation.
                let expected = expected_application(preference.destination, false);
                let mut both: Vec<Observed> = Vec::new();
                for read_first in [false, true] {
                    let reads = if read_first {
                        "one unrelated read before it"
                    } else {
                        "no unrelated read before it"
                    };
                    let case = format!(
                        "{order}, {}, {reads}, preference {} to {:?}",
                        handler.what(),
                        preference.variant,
                        preference.destination
                    );
                    let observed = run(
                        &case,
                        preference,
                        component_import_first,
                        read_first,
                        handler,
                    )
                    .await;
                    assert_observed(&case, &observed, component_import_first, expected);
                    both.push(observed);
                }
                assert_eq!(
                    both[0].shape(),
                    both[1].shape(),
                    "case {order:?} with {}: what the completion recorded must not depend on \
                     whether an unrelated read happened first, got {:?} against {:?}",
                    handler.what(),
                    both[0],
                    both[1]
                );
            }
        }
    }
}

/// The analysis a completion records is the one owed at its DUE instant, not at the instant
/// it was first read: the records of an import that completed after it are in the inventory
/// a later read reports, and they carry no analysis from it.
///
/// This is the half of the rule a caller can observe on the timestamps alone — a component
/// whose own import completed after the assessment did can never carry that assessment's
/// analysis instant, which would be an analysis of a record that had not arrived.
#[tokio::test]
async fn nothing_is_analysed_by_a_completion_that_fell_due_before_it_arrived() {
    for preference in PREFERENCES {
        let case = format!(
            "preference {} to {:?}",
            preference.variant, preference.destination
        );
        let observed = run(&case, preference, false, false, Materialise::Read).await;
        assert_eq!(
            observed.component_analyzed_at,
            Value::Null,
            "case {case:?}: the component's import completed after the assessment, so no \
             analysis of it was ever owed, got {:?}",
            observed.component_analyzed_at
        );
        assert_ne!(
            observed.server_analyzed_at,
            Value::Null,
            "case {case:?}: the server the assessment covered still carries the analysis the \
             completion recorded, got {:?}",
            observed.server_analyzed_at
        );
    }
}
