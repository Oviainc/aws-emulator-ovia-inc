// SPDX-License-Identifier: CC0-1.0

//! The portfolio a CONFIGURATION UPDATE lands on, end to end over HTTP.
//!
//! The rule under test is one of ordering, for ANY write of this service rather than for
//! one request: a handler that writes a record of the portfolio first materialises every
//! completion the clock already owed that portfolio at the instant of the call — owed then
//! even if nobody had called — so work the clock owed is computed from the records as they
//! stood when it fell due, and the write applies only from there on. Two consequences are
//! observable over the wire, and both are asserted below:
//!
//! * the analysis an assessment's completion records is the one that was due at the
//!   completion instant, whether or not anything read the portfolio between that instant
//!   and the update — the service's own rule for this write-on-read is that the same
//!   analysis is reported however late it is first read, and `appType` and
//!   `inclusionStatus` are two of the members the analysis is computed from;
//! * the configuration the update stores governs the NEXT assessment, which is what
//!   makes the update a change of the inputs rather than a rewrite of an analysis that
//!   already ran;
//! * and a record whose arrival the clock already owed — one an import file task whose
//!   completion has fallen due brought in — is a record a write finds, rather than one it
//!   refuses as missing while the clock has already produced it.
//!
//! Each table spans the class the rule generalises over rather than the one sequence a
//! report cites: which analysis input the update rewrites (`inclusionStatus`, `appType`,
//! both, and neither), every `ManagementPreference` variant with a destination of its own
//! enumeration, the `applicationMode` values that decide which components the analysis
//! covers, both orders (a read before the update and none), and both update handlers of
//! this service against a due and a not-yet-due import.
//!
//! A server and an application component enter this service only through the account's
//! data being brought in, so the portfolio is seeded through the shared engine — the same
//! `servers` and `application_components` maps an ingestion writes — while the
//! preferences, the assessment, the updates and every assertion go through the router,
//! exactly as a client reaches them.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ApplicationComponentRecord,
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ImportFileTask,
    ServerRecord,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::assessment_analysis::{assert_recommendation, complete_the_span, expected_application};
use crate::import_and_assess::{
    at, call, get_assessment, minimal_assessment, register_collector_in, start_assessment, Answer,
    TEST_REGION,
};

/// The component and the server every assessment case runs against, and the configuration
/// the component carries BEFORE any update: an `AppType` the service recognises and that
/// is not a Windows web stack, included in the assessment, on a Linux machine. The
/// analysis due at the completion instant is the one computed from exactly these.
const SEED_COMPONENT: &str = "1a";
const SEED_SERVER: &str = "0a";
const SEED_APP_TYPE: &str = "Java";
const SEED_INCLUSION: &str = "includeInAssessment";

/// `ResourceSubType`'s value for a component that is not a database, so the APPLICATION
/// half of the saved preferences is what selects its destination. From
/// `smithy_query(query="shape",shape="ResourceSubType")`.
const PROCESS: &str = "Process";

/// Every value `ApplicationMode` declares, from
/// `smithy_query(query="shape",shape="ApplicationMode")` — the member that decides WHICH
/// components the analysis covers.
const MODE_ALL: &str = "ALL";
const MODE_KNOWN: &str = "KNOWN";
const MODE_UNKNOWN: &str = "UNKNOWN";

/// `AppType`'s value for a component whose type the service does not recognise, and two it
/// does — one a Windows IIS / .NET web stack and one not. From
/// `smithy_query(query="shape",shape="AppType")`.
const APP_TYPE_UNKNOWN: &str = "Unknown";
const APP_TYPE_WINDOWS_WEB: &str = "IIS";
const APP_TYPE_TOMCAT: &str = "Tomcat";

/// Both values `InclusionStatus` declares, from
/// `smithy_query(query="shape",shape="InclusionStatus")`.
const EXCLUDE: &str = "excludeFromAssessment";
const INCLUDE: &str = "includeInAssessment";

/// `ImportFileTaskStatus`'s value for an import still running and the one a fully
/// successful import reaches, from
/// `smithy_query(query="shape",shape="ImportFileTaskStatus")`.
const IMPORT_IN_PROGRESS: &str = "ImportInProgress";
const IMPORT_SUCCESS: &str = "ImportSuccess";

/// One emulator over one shared engine, with the component and the server above seeded
/// into the Region every request here is signed for, and a registered data collector so
/// an assessment can be started at all.
///
/// The engine is handed back beside the router so a case can bring the completion the
/// clock owes an assessment forward instead of sleeping through it.
fn portfolio() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let region = store.region_entry(TEST_REGION);
        region.servers.insert(
            SEED_SERVER.to_string(),
            ServerRecord {
                id: SEED_SERVER.to_string(),
                name: Some("server-0a".to_string()),
                os_type: Some("LINUX".to_string()),
                ..Default::default()
            },
        );
        region
            .application_components
            .insert(SEED_COMPONENT.to_string(), seed_component());
    }
    register_collector_in(&engine, &[TEST_REGION]);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine.clone()));
    (engine, router)
}

/// The component record the cases start from, holding only the members the analysis reads.
fn seed_component() -> ApplicationComponentRecord {
    ApplicationComponentRecord {
        id: SEED_COMPONENT.to_string(),
        name: Some("component-1a".to_string()),
        app_type: Some(SEED_APP_TYPE.to_string()),
        resource_sub_type: Some(PROCESS.to_string()),
        inclusion_status: Some(SEED_INCLUSION.to_string()),
        associated_server_ids: vec![SEED_SERVER.to_string()],
        ..Default::default()
    }
}

async fn put_preferences(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/put-portfolio-preferences", Some(body)).await
}

async fn update_component(router: &MultiServiceRouter, body: Value) -> Answer {
    call(
        router,
        "POST",
        "/update-applicationcomponent-config/",
        Some(body),
    )
    .await
}

async fn update_server(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/update-server-config/", Some(body)).await
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

async fn server_strategies(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-server-strategies/{id}"), None).await
}

/// An `applicationPreferences` naming one `ManagementPreference` variant and one target of
/// that variant's own destination enumeration, with the `applicationMode` a row names.
fn preferences(variant: &str, destination: &str, mode: Option<&str>) -> Value {
    let mut body = json!({
        "applicationPreferences": {
            "managementPreference": {variant: {"targetDestination": [destination]}}
        }
    });
    if let Some(mode) = mode {
        body["applicationMode"] = json!(mode);
    }
    body
}

/// One case of the ordering table: the preferences the account saved before the
/// assessment, and the update issued after its completion fell due.
struct Row {
    what: &'static str,
    /// The `ManagementPreference` variant the preferences chose.
    variant: &'static str,
    /// The target destination that variant names, from its own enumeration.
    destination: &'static str,
    /// The `applicationMode` the preferences name, if any.
    mode: Option<&'static str>,
    /// `appType` the update writes, if it writes one.
    app_type: Option<&'static str>,
    /// `inclusionStatus` the update writes, if it writes one.
    inclusion_status: Option<&'static str>,
    /// `secretsManagerKey` the update writes — a member the analysis does NOT read, so a
    /// row carrying only this one is the control.
    secret: Option<&'static str>,
}

impl Row {
    fn request(&self) -> Value {
        let mut body = json!({"applicationComponentId": SEED_COMPONENT});
        if let Some(app_type) = self.app_type {
            body["appType"] = json!(app_type);
        }
        if let Some(status) = self.inclusion_status {
            body["inclusionStatus"] = json!(status);
        }
        if let Some(secret) = self.secret {
            body["secretsManagerKey"] = json!(secret);
        }
        body
    }

    /// Whether the analysis due at the completion instant covered the SEEDED component:
    /// it was included, and its app type is one the service recognises, so only the mode
    /// that selects the other classification leaves it out.
    fn covered(&self) -> bool {
        self.mode != Some(MODE_UNKNOWN)
    }

    /// The configuration the read must report after the update: each member the update
    /// named, and the seeded value for each one it left out.
    fn stored(&self) -> Vec<(&'static str, Value)> {
        let mut want = vec![
            (
                "/applicationComponentDetail/appType",
                json!(self.app_type.unwrap_or(SEED_APP_TYPE)),
            ),
            (
                "/applicationComponentDetail/inclusionStatus",
                json!(self.inclusion_status.unwrap_or(SEED_INCLUSION)),
            ),
        ];
        if let Some(secret) = self.secret {
            want.push((
                "/applicationComponentDetail/databaseConfigDetail/secretName",
                json!(secret),
            ));
        }
        want
    }
}

/// Every row of the ordering table: each `ManagementPreference` variant with a
/// destination its own enumeration declares, every `ApplicationMode` value and none, and
/// each analysis input the update can rewrite — on its own, together, and not at all.
fn table() -> Vec<Row> {
    vec![
        Row {
            what: "the update excludes the component from the assessment",
            variant: "selfManageResources",
            destination: "Amazon Elastic Cloud Compute (EC2)",
            mode: None,
            app_type: None,
            inclusion_status: Some(EXCLUDE),
            secret: None,
        },
        Row {
            what: "the update makes the component a Windows web stack",
            variant: "awsManagedResources",
            destination: "AWS Elastic BeanStalk",
            mode: None,
            app_type: Some(APP_TYPE_WINDOWS_WEB),
            inclusion_status: None,
            secret: None,
        },
        Row {
            what: "the update makes the app type unrecognised, under the KNOWN mode",
            variant: "awsManagedResources",
            destination: "AWS Fargate",
            mode: Some(MODE_KNOWN),
            app_type: Some(APP_TYPE_UNKNOWN),
            inclusion_status: None,
            secret: None,
        },
        Row {
            what: "the update makes the app type one only the UNKNOWN mode covers",
            variant: "noPreference",
            destination: "Amazon Elastic Container Service (ECS)",
            mode: Some(MODE_UNKNOWN),
            app_type: Some(APP_TYPE_UNKNOWN),
            inclusion_status: None,
            secret: None,
        },
        Row {
            what: "the update rewrites both analysis inputs at once",
            variant: "awsManagedResources",
            destination: "None specified",
            mode: Some(MODE_ALL),
            app_type: Some("Dotnet"),
            inclusion_status: Some(EXCLUDE),
            secret: None,
        },
        Row {
            what: "the update configures a database secret only",
            variant: "selfManageResources",
            destination: "Amazon Elastic Kubernetes Service (EKS)",
            mode: None,
            app_type: None,
            inclusion_status: None,
            secret: Some("database-secret-1a"),
        },
        Row {
            what: "the update re-states the inclusion status it already had",
            variant: "noPreference",
            destination: "AWS Fargate",
            mode: Some(MODE_ALL),
            app_type: None,
            inclusion_status: Some(INCLUDE),
            secret: None,
        },
        Row {
            what: "the update makes the app type another recognised one",
            variant: "noPreference",
            destination: "Amazon Elastic Kubernetes Service (EKS)",
            mode: Some(MODE_KNOWN),
            app_type: Some(APP_TYPE_TOMCAT),
            inclusion_status: None,
            secret: None,
        },
    ]
}

/// What the portfolio reports after one run of the sequence.
struct Observed {
    /// `GetApplicationComponentDetails` after the update.
    component: Answer,
    /// The instant that read says the component was analysed at.
    analyzed_at: Value,
    /// The instant the assessment reports it completed at.
    completion_time: Value,
}

/// Run one row: save its preferences, start an assessment, let its completion fall due
/// WITHOUT anything materialising it, optionally read the portfolio once, issue the
/// update, and report what the portfolio then answers.
async fn run(case: &str, row: &Row, read_first: bool) -> Observed {
    let (engine, router) = portfolio();
    let saved = put_preferences(&router, preferences(row.variant, row.destination, row.mode)).await;
    assert_eq!(
        saved.status, 200,
        "case {case:?}: the preferences must be saved, got {} {:?}",
        saved.status, saved.body
    );
    let started = start_assessment(&router, minimal_assessment()).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: the assessment must start, got {} {:?}",
        started.status, started.body
    );
    let assessment_id = started.string("/assessmentId");
    // The completion is now OWED: its due instant is in the past and nothing has
    // materialised it yet.
    complete_the_span(&engine);
    if read_first {
        let early = component_details(&router, SEED_COMPONENT).await;
        assert_eq!(
            early.status, 200,
            "case {case:?}: the read before the update must be served, got {} {:?}",
            early.status, early.body
        );
    }
    let updated = update_component(&router, row.request()).await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: the update must be accepted, got {} {:?}",
        updated.status, updated.body
    );
    let component = component_details(&router, SEED_COMPONENT).await;
    assert_eq!(
        component.status, 200,
        "case {case:?}: the component must be reported, got {} {:?}",
        component.status, component.body
    );
    let assessment = get_assessment(&router, &assessment_id).await;
    Observed {
        analyzed_at: at(
            &component.body,
            "/applicationComponentDetail/lastAnalyzedTimestamp",
        ),
        completion_time: at(&assessment.body, "/dataCollectionDetails/completionTime"),
        component,
    }
}

/// The analysis a due completion records is computed from the configuration that was
/// stored when it fell due, so an update issued after that instant does not change it —
/// and the analysis is the same whether or not anything read the portfolio in between.
#[tokio::test]
async fn an_update_does_not_change_the_analysis_the_clock_already_owed() {
    for row in table() {
        let mut recommendations: Vec<Value> = Vec::new();
        let mut analyzed: Vec<bool> = Vec::new();
        for read_first in [false, true] {
            let order = if read_first {
                "one read before the update"
            } else {
                "no read before the update"
            };
            let case = format!("{} ({order})", row.what);
            let observed = run(&case, &row, read_first).await;
            assert!(
                observed.completion_time.is_number(),
                "case {case:?}: the assessment the clock completed reports its completion \
                 time, got {:?}",
                observed.completion_time
            );
            let reported = at(
                &observed.component.body,
                "/applicationComponentDetail/recommendationSet",
            );
            if row.covered() {
                assert_recommendation(
                    &case,
                    "the component's detail",
                    &observed.component.body,
                    "/applicationComponentDetail/recommendationSet",
                    expected_application(row.destination, false),
                );
                assert_eq!(
                    observed.analyzed_at, observed.completion_time,
                    "case {case:?}: the component was analysed when the assessment \
                     completed, got {:?}",
                    observed.analyzed_at
                );
            } else {
                assert_eq!(
                    reported,
                    Value::Null,
                    "case {case:?}: the analysis due at the completion instant did not \
                     cover this component, so it carries no recommendation, got {reported:?}"
                );
                assert_eq!(
                    observed.analyzed_at,
                    Value::Null,
                    "case {case:?}: a component no analysis covered reports no analysis \
                     instant, got {:?}",
                    observed.analyzed_at
                );
            }
            for (pointer, want) in row.stored() {
                assert_eq!(
                    at(&observed.component.body, pointer),
                    want,
                    "case {case:?}: the update stored {want:?} at {pointer}, got {:?}",
                    at(&observed.component.body, pointer)
                );
            }
            recommendations.push(reported);
            analyzed.push(observed.analyzed_at.is_number());
        }
        assert_eq!(
            recommendations[0], recommendations[1],
            "case {:?}: the analysis must not depend on whether the portfolio was read \
             before the update, got {:?} with no read and {:?} after one",
            row.what, recommendations[0], recommendations[1]
        );
        assert_eq!(
            analyzed[0], analyzed[1],
            "case {:?}: whether the component carries an analysis instant must not depend \
             on when the portfolio was first read, got {:?} with no read and {:?} after one",
            row.what, analyzed[0], analyzed[1]
        );
    }
}

/// One case of the next-assessment table: the update, and what the analysis of the SECOND
/// assessment must leave on the component.
struct Later {
    what: &'static str,
    variant: &'static str,
    destination: &'static str,
    mode: Option<&'static str>,
    app_type: Option<&'static str>,
    inclusion_status: Option<&'static str>,
    /// Whether the second analysis covers the component the update left behind. A
    /// component it does not cover is left exactly as the first analysis left it,
    /// recommendation and analysis instant included.
    covered_now: bool,
    /// Whether the component's app type is a Windows web stack AFTER the update, which is
    /// what the tool for one destination depends on.
    windows_now: bool,
}

/// Every row of that table: an update that takes the component out of the analysis, one
/// that changes the tool the analysis picks, one that moves it out of the mode's
/// classification, and one that leaves it covered.
fn later_table() -> Vec<Later> {
    vec![
        Later {
            what: "a component excluded from the assessment",
            variant: "selfManageResources",
            destination: "Amazon Elastic Cloud Compute (EC2)",
            mode: None,
            app_type: None,
            inclusion_status: Some(EXCLUDE),
            covered_now: false,
            windows_now: false,
        },
        Later {
            what: "a component whose app type became a Windows web stack",
            variant: "awsManagedResources",
            destination: "AWS Elastic BeanStalk",
            mode: None,
            app_type: Some(APP_TYPE_WINDOWS_WEB),
            inclusion_status: None,
            covered_now: true,
            windows_now: true,
        },
        Later {
            what: "a component whose app type the KNOWN mode no longer covers",
            variant: "awsManagedResources",
            destination: "AWS Fargate",
            mode: Some(MODE_KNOWN),
            app_type: Some(APP_TYPE_UNKNOWN),
            inclusion_status: None,
            covered_now: false,
            windows_now: false,
        },
        Later {
            what: "a component whose app type stays covered",
            variant: "noPreference",
            destination: "Amazon Elastic Kubernetes Service (EKS)",
            mode: Some(MODE_ALL),
            app_type: Some(APP_TYPE_TOMCAT),
            inclusion_status: None,
            covered_now: true,
            windows_now: false,
        },
    ]
}

/// The configuration an update stored is an INPUT, so the next assessment's analysis is
/// computed from it: a component the update took out of the analysis keeps the analysis
/// the first completion recorded, and one it left covered is re-analysed from what the
/// update stored.
#[tokio::test]
async fn the_configuration_an_update_stored_governs_the_next_assessment() {
    for row in later_table() {
        let case = row.what;
        let (engine, router) = portfolio();
        let saved = put_preferences(&router, preferences(row.variant, row.destination, row.mode))
            .await;
        assert_eq!(
            saved.status, 200,
            "case {case:?}: the preferences must be saved, got {} {:?}",
            saved.status, saved.body
        );

        let first = start_assessment(&router, minimal_assessment()).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first assessment must start, got {} {:?}",
            first.status, first.body
        );
        let first_id = first.string("/assessmentId");
        complete_the_span(&engine);

        let mut request = json!({"applicationComponentId": SEED_COMPONENT});
        if let Some(app_type) = row.app_type {
            request["appType"] = json!(app_type);
        }
        if let Some(status) = row.inclusion_status {
            request["inclusionStatus"] = json!(status);
        }
        let updated = update_component(&router, request).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: the update must be accepted, got {} {:?}",
            updated.status, updated.body
        );
        let first_completion = at(
            &get_assessment(&router, &first_id).await.body,
            "/dataCollectionDetails/completionTime",
        );

        let second = start_assessment(&router, minimal_assessment()).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: the second assessment must start, got {} {:?}",
            second.status, second.body
        );
        let second_id = second.string("/assessmentId");
        complete_the_span(&engine);
        let component = component_details(&router, SEED_COMPONENT).await;
        let second_completion = at(
            &get_assessment(&router, &second_id).await.body,
            "/dataCollectionDetails/completionTime",
        );

        // What the second analysis recommends: the destination the saved preference still
        // names, with the tool the configuration the update stored selects. A component it
        // does not cover keeps what the FIRST analysis left, which is the recommendation
        // computed from the seeded configuration.
        let expected = expected_application(row.destination, row.windows_now && row.covered_now);
        assert_recommendation(
            case,
            "the component's detail",
            &component.body,
            "/applicationComponentDetail/recommendationSet",
            expected,
        );
        let analyzed_at = at(
            &component.body,
            "/applicationComponentDetail/lastAnalyzedTimestamp",
        );
        let want = if row.covered_now {
            &second_completion
        } else {
            &first_completion
        };
        assert_eq!(
            analyzed_at, *want,
            "case {case:?}: the component carries the instant of the analysis that last \
             covered it, got {analyzed_at:?}"
        );
        assert_ne!(
            first_completion, second_completion,
            "case {case:?}: the two assessments completed at different instants, got \
             {first_completion:?} twice"
        );
    }
}

/// One case of the due-import table: whether the import's completion has fallen due, and
/// what each write must then answer for a record it brought in.
struct Pending {
    what: &'static str,
    /// Whether the clock already owes the import its completion.
    due: bool,
    /// The status each write must answer with: a record whose arrival the clock owes is
    /// found, one whose import is still running is not.
    status: u16,
}

/// The id of the component and of the server the seeded import file task holds. They
/// follow `ResourceId`'s `^[0-9a-b]+`, the same pattern the ingestion mints under.
const PENDING_COMPONENT: &str = "1b";
const PENDING_SERVER: &str = "0b";

/// One emulator whose portfolio holds an import file task that has ingested one server and
/// one application component, with its completion either already due or still ahead.
///
/// Nothing has read the portfolio, so the records are still the task's: only materialising
/// what the clock owes moves them into the inventory.
fn portfolio_with_pending_import(due: bool) -> MultiServiceRouter {
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
            number_of_records_success: 2,
            number_of_records_failed: 0,
            completion_due: Some(completion_due),
            pending_servers: vec![ServerRecord {
                id: PENDING_SERVER.to_string(),
                name: Some("server-0b".to_string()),
                os_type: Some("LINUX".to_string()),
                ..Default::default()
            }],
            pending_components: vec![ApplicationComponentRecord {
                id: PENDING_COMPONENT.to_string(),
                name: Some("component-1b".to_string()),
                app_type: Some(SEED_APP_TYPE.to_string()),
                resource_sub_type: Some(PROCESS.to_string()),
                inclusion_status: Some(SEED_INCLUSION.to_string()),
                associated_server_ids: vec![PENDING_SERVER.to_string()],
                ..Default::default()
            }],
        };
        store
            .region_entry(TEST_REGION)
            .import_tasks
            .insert(task.id.clone(), task);
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A write is served against the portfolio the clock has already produced: a record an
/// import whose completion has fallen due brought in is one the update finds, while a
/// record of an import that is still running is not in the inventory at all.
#[tokio::test]
async fn a_write_finds_a_record_whose_arrival_the_clock_already_owed() {
    let cases = [
        Pending {
            what: "the import's completion has fallen due",
            due: true,
            status: 200,
        },
        Pending {
            what: "the import is still running",
            due: false,
            status: 404,
        },
    ];
    for case in cases {
        let what = case.what;
        let router = portfolio_with_pending_import(case.due);

        let component = update_component(
            &router,
            json!({"applicationComponentId": PENDING_COMPONENT, "appType": APP_TYPE_TOMCAT}),
        )
        .await;
        assert_eq!(
            component.status, case.status,
            "case {what:?}: the component update answers {}, got {} {:?}",
            case.status, component.status, component.body
        );

        let server = update_server(
            &router,
            json!({
                "serverId": PENDING_SERVER,
                "strategyOption": {"strategy": "Rehost", "isPreferred": true},
            }),
        )
        .await;
        assert_eq!(
            server.status, case.status,
            "case {what:?}: the server update answers {}, got {} {:?}",
            case.status, server.status, server.body
        );

        // What each write stored is reported by the read beside it, so a write the clock's
        // completion made possible is a write that really landed.
        let read = component_details(&router, PENDING_COMPONENT).await;
        assert_eq!(
            read.status, case.status,
            "case {what:?}: the component read answers {}, got {} {:?}",
            case.status, read.status, read.body
        );
        if case.status == 200 {
            assert_eq!(
                at(&read.body, "/applicationComponentDetail/appType"),
                json!(APP_TYPE_TOMCAT),
                "case {what:?}: the component reports the app type the update stored, got {:?}",
                at(&read.body, "/applicationComponentDetail/appType")
            );
            let strategies = server_strategies(&router, PENDING_SERVER).await;
            assert_eq!(
                at(&strategies.body, "/serverStrategies/0/isPreferred"),
                json!(true),
                "case {what:?}: the server reports the preference the update stored, got {:?}",
                at(&strategies.body, "/serverStrategies/0/isPreferred")
            );
        }
    }
}
