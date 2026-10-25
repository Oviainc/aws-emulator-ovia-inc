// SPDX-License-Identifier: CC0-1.0

//! `GetApplicationComponentDetails`: one application component of the account's
//! portfolio, read end to end over HTTP.
//!
//! The rule under test is the one the operation states for ANY application component, not
//! for one request: the answer is the stored component record the `applicationComponentId`
//! names — every member of the reported `ApplicationComponentDetail` is that record's
//! member, every member the record does not hold is absent, the associations it holds are
//! reported in the stored order, and the two "is there more than one" flags
//! (`moreApplicationResource`, `moreServerAssociationExists`) are counted off those very
//! lists — while an `applicationComponentId` naming no component is refused with 404
//! `ResourceNotFoundException`.
//!
//! The table below therefore spans the class the rule generalises over rather than one
//! component: one row per `AppType` the model declares (so the whole catalog is covered),
//! cycling independently through `ResourceSubType`, `InclusionStatus`,
//! `SrcCodeOrDbAnalysisStatus`, `RuntimeAnalysisStatus`, `Strategy`, `TargetDestination`
//! and `AppUnitErrorCategory`, and crossing those with zero, one, two and three associated
//! servers and zero, one and two associated applications and source-code repositories — so
//! a reported single-valued member, an absent member, an empty list, a one-entry list and
//! a many-entry list all appear — plus a row holding nothing but an id.
//!
//! An application component enters this service only through the account's data being
//! brought in (the import-loop issue, filed separately: `StartImportFileTask` reaches
//! `ImportSuccess` having ingested nothing). So these cases seed the component records
//! directly through the shared engine — the same `application_components` map that
//! ingestion writes and this operation reads — and then drive every assertion through the
//! router, so the handler, its refusal and the serialized `ApplicationComponentDetail` are
//! exercised exactly as a client reaches them.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::{
    AssociatedApplication, SourceCodeRepository,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ApplicationComponentRecord,
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, Answer, TEST_REGION};

/// This service's message for an id that resolves to no resource, on an operation that
/// reports the resource as absent. Spelled out independently of `src/state.rs`'s
/// constants, so a wrong message in the crate fails here rather than agreeing with itself.
const INVALID_RESOURCE_ID: &str = "Invalid resourceId: ";

/// Every value `AppType` admits, from `smithy_query(query="shape",shape="AppType")`. The
/// table below has one row per entry, so the whole catalog is read back rather than the
/// handful an example would carry.
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

/// Every value `ResourceSubType` admits, from
/// `smithy_query(query="shape",shape="ResourceSubType")`.
const RESOURCE_SUB_TYPES: &[&str] = &["Database", "Process", "DatabaseProcess"];

/// Every value `InclusionStatus` admits, from
/// `smithy_query(query="shape",shape="InclusionStatus")`.
const INCLUSION_STATUSES: &[&str] = &["excludeFromAssessment", "includeInAssessment"];

/// Every value `SrcCodeOrDbAnalysisStatus` admits, from
/// `smithy_query(query="shape",shape="SrcCodeOrDbAnalysisStatus")`.
const ANALYSIS_STATUSES: &[&str] = &[
    "ANALYSIS_TO_BE_SCHEDULED",
    "ANALYSIS_STARTED",
    "ANALYSIS_SUCCESS",
    "ANALYSIS_FAILED",
    "ANALYSIS_PARTIAL_SUCCESS",
    "UNCONFIGURED",
    "CONFIGURED",
];

/// Every value `RuntimeAnalysisStatus` admits, from
/// `smithy_query(query="shape",shape="RuntimeAnalysisStatus")`.
const RUNTIME_STATUSES: &[&str] = &[
    "ANALYSIS_TO_BE_SCHEDULED",
    "ANALYSIS_STARTED",
    "ANALYSIS_SUCCESS",
    "ANALYSIS_FAILED",
];

/// Every value `AppUnitErrorCategory` admits, from
/// `smithy_query(query="shape",shape="AppUnitErrorCategory")`.
const APP_UNIT_ERROR_CATEGORIES: &[&str] = &[
    "PERMISSION_ERROR",
    "CONNECTIVITY_ERROR",
    "OTHER_ERROR",
    "CREDENTIAL_ERROR",
    "UNSUPPORTED_ERROR",
];

/// Values from `Strategy` and `TargetDestination`
/// (`smithy_query(query="shape",shape="Strategy,TargetDestination")`), paired so each row
/// reports a recommendation of its own.
const STRATEGIES: &[&str] = &["Rehost", "Replatform", "Refactor", "Retain", "Repurchase"];
const TARGET_DESTINATIONS: &[&str] = &[
    "Amazon Elastic Cloud Compute (EC2)",
    "AWS Fargate",
    "Amazon Elastic Container Service (ECS)",
    "None specified",
    "Aurora PostgreSQL",
];

/// Every value `VersionControlType` admits, from
/// `smithy_query(query="shape",shape="VersionControlType")`.
const VERSION_CONTROL_TYPES: &[&str] = &["GITHUB", "GITHUB_ENTERPRISE", "AZURE_DEVOPS_GIT"];

/// The instant a row's `lastAnalyzedTimestamp` is stored at, as seconds since the epoch:
/// a distinct one per row, so no assertion can pass on a shared constant.
fn analyzed_at(row: usize) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000 + row as u64 * 3_600)
}

/// The id row `row` is seeded under: a `ResourceId` (`^[0-9a-b]+`, at most 44 characters),
/// which is also what `ApplicationComponentId` (`[0-9a-zA-Z-]+`) admits on the request.
fn component_id(row: usize) -> String {
    format!("{row}a")
}

/// One associated application, as the ingestion that brought the component in would have
/// stored it. `AssociatedApplication` carries exactly a `name` and an `id`
/// (`smithy_query(query="shape",shape="AssociatedApplication")`).
fn application(row: usize, index: usize) -> AssociatedApplication {
    AssociatedApplication {
        name: Some(format!("app-group-{row}-{index}")),
        id: Some(format!("d-application-{row}-{index:02}")),
    }
}

/// One source code repository of a component.
fn repository(row: usize, index: usize) -> SourceCodeRepository {
    SourceCodeRepository {
        repository: Some(format!("https://example.invalid/repo-{row}-{index}")),
        branch: Some(format!("release-{index}")),
        version_control_type: Some(
            VERSION_CONTROL_TYPES[index % VERSION_CONTROL_TYPES.len()].to_string(),
        ),
        project_name: Some(format!("project-{row}")),
    }
}

/// The component record for table row `row`.
///
/// Each member cycles through its own catalog at its own period, and the optional ones are
/// present on some rows and absent on others, so a row cannot be satisfied by a canned
/// detail and absence is covered as well as presence.
fn component(row: usize) -> ApplicationComponentRecord {
    let has_recommendation = !row.is_multiple_of(4);
    ApplicationComponentRecord {
        id: component_id(row),
        name: Some(format!("component-{row}")),
        app_type: Some(APP_TYPES[row % APP_TYPES.len()].to_string()),
        resource_sub_type: Some(RESOURCE_SUB_TYPES[row % RESOURCE_SUB_TYPES.len()].to_string()),
        inclusion_status: Some(INCLUSION_STATUSES[row % INCLUSION_STATUSES.len()].to_string()),
        analysis_status: Some(ANALYSIS_STATUSES[row % ANALYSIS_STATUSES.len()].to_string()),
        status_message: row
            .is_multiple_of(2)
            .then(|| format!("analysis note for row {row}")),
        strategy: has_recommendation.then(|| STRATEGIES[row % STRATEGIES.len()].to_string()),
        target_destination: has_recommendation
            .then(|| TARGET_DESTINATIONS[row % TARGET_DESTINATIONS.len()].to_string()),
        os_version: (!row.is_multiple_of(3)).then(|| format!("Ubuntu 20.0{}", row % 10)),
        os_driver: (!row.is_multiple_of(3)).then(|| format!("driver-{row}")),
        last_analyzed: row.is_multiple_of(2).then(|| analyzed_at(row)),
        database_secret_name: row.is_multiple_of(5).then(|| format!("secret-{row}")),
        source_code_repositories: (0..row % 3).map(|index| repository(row, index)).collect(),
        runtime_status: Some(RUNTIME_STATUSES[row % RUNTIME_STATUSES.len()].to_string()),
        runtime_status_message: (!row.is_multiple_of(2))
            .then(|| format!("runtime note for row {row}")),
        app_unit_error_category: row
            .is_multiple_of(7)
            .then(|| APP_UNIT_ERROR_CATEGORIES[row % APP_UNIT_ERROR_CATEGORIES.len()].to_string()),
        // Zero, one, two or three servers, and zero, one or two application groups: the
        // two "more than one" flags are decided off these two lists.
        associated_server_ids: (0..row % 4).map(|index| format!("{index}b{row}")).collect(),
        associated_applications: (0..row % 3).map(|index| application(row, index)).collect(),
        // No caller has chosen a preferred strategy for these rows: the preference
        // `UpdateApplicationComponentConfig` stores is the subject of
        // `application_component_config.rs`, and this detail read reports the service's own
        // recommendation rather than a choice.
        preferred_strategy: None,
        // The import groups a component came in with are not a member of the detail this
        // read reports — they are what a list's `groupIdFilter` scopes by
        // (`application_component_listing.rs`) — so these rows carry none.
        group_id: Vec::new(),
    }
}

/// Every row of the table: one per `AppType` value, plus a last row holding nothing but an
/// id — the component the ingestion knew nothing else about.
fn table() -> Vec<ApplicationComponentRecord> {
    let mut rows: Vec<ApplicationComponentRecord> =
        (0..APP_TYPES.len()).map(component).collect();
    rows.push(ApplicationComponentRecord {
        id: component_id(APP_TYPES.len()),
        ..Default::default()
    });
    rows
}

/// The id of the row holding nothing but an id.
fn bare_row_id() -> String {
    component_id(APP_TYPES.len())
}

/// One emulator over one shared engine, with `application_components` seeded into this
/// service's store — the same map an ingestion writes and this operation reads.
///
/// `pub(crate)` so the suites beside this one (the update this read reports, in
/// `application_component_config.rs`) seed the same store through the same harness rather
/// than standing up a second copy of it.
pub(crate) fn emulator_with(components: Vec<ApplicationComponentRecord>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let portfolio = store.region_entry(TEST_REGION);
        for record in components {
            portfolio
                .application_components
                .insert(record.id.clone(), record);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// `GET /get-applicationcomponent-details/{applicationComponentId}`.
///
/// `pub(crate)` for the same reason as [`emulator_with`]: the suite that drives the update
/// reads what it stored back through this very route.
pub(crate) async fn get_component(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-applicationcomponent-details/{id}"),
        None,
    )
    .await
}

/// A stored optional string as the JSON the answer must carry for it: the value, or `null`
/// for a member the record does not hold.
fn string_or_null(value: &Option<String>) -> Value {
    match value {
        Some(value) => json!(value),
        None => Value::Null,
    }
}

/// The `(name, id)` pairs `associatedApplications` reported, in the order reported. A
/// missing member reads as the empty list, so an empty list and an omitted one are not
/// distinguished here — the count is what the rule is about.
fn reported_applications(answer: &Answer) -> Vec<(String, String)> {
    match answer
        .body
        .get("associatedApplications")
        .and_then(Value::as_array)
    {
        Some(items) => items
            .iter()
            .map(|item| {
                (
                    item["name"].as_str().unwrap_or_default().to_string(),
                    item["id"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        None => Vec::new(),
    }
}

/// The `(name, id)` pairs a record holds, in the order it holds them.
fn stored_applications(record: &ApplicationComponentRecord) -> Vec<(String, String)> {
    record
        .associated_applications
        .iter()
        .map(|app| {
            (
                app.name.clone().unwrap_or_default(),
                app.id.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// The server ids `associatedServerIds` reported, in the order reported.
fn reported_server_ids(answer: &Answer) -> Vec<String> {
    match answer
        .body
        .get("associatedServerIds")
        .and_then(Value::as_array)
    {
        Some(items) => items
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_string())
            .collect(),
        None => Vec::new(),
    }
}

/// Every single-valued member of the reported detail, against the record it must come
/// from.
fn detail_members(record: &ApplicationComponentRecord) -> Vec<(String, Value)> {
    let prefix = "/applicationComponentDetail";
    let mut expected: Vec<(String, Value)> = vec![
        (format!("{prefix}/id"), json!(record.id)),
        (format!("{prefix}/name"), string_or_null(&record.name)),
        (format!("{prefix}/appType"), string_or_null(&record.app_type)),
        (
            format!("{prefix}/resourceSubType"),
            string_or_null(&record.resource_sub_type),
        ),
        (
            format!("{prefix}/inclusionStatus"),
            string_or_null(&record.inclusion_status),
        ),
        (
            format!("{prefix}/analysisStatus"),
            string_or_null(&record.analysis_status),
        ),
        (
            format!("{prefix}/statusMessage"),
            string_or_null(&record.status_message),
        ),
        (
            format!("{prefix}/osVersion"),
            string_or_null(&record.os_version),
        ),
        (
            format!("{prefix}/osDriver"),
            string_or_null(&record.os_driver),
        ),
        (
            format!("{prefix}/runtimeStatus"),
            string_or_null(&record.runtime_status),
        ),
        (
            format!("{prefix}/runtimeStatusMessage"),
            string_or_null(&record.runtime_status_message),
        ),
        (
            format!("{prefix}/appUnitError/appUnitErrorCategory"),
            string_or_null(&record.app_unit_error_category),
        ),
        (
            format!("{prefix}/databaseConfigDetail/secretName"),
            string_or_null(&record.database_secret_name),
        ),
        // The single server member is the FIRST of the ids the record holds, and the flag
        // beside it is whether there is more than one of them.
        (
            format!("{prefix}/associatedServerId"),
            string_or_null(&record.associated_server_ids.first().cloned()),
        ),
        (
            format!("{prefix}/moreServerAssociationExists"),
            json!(record.associated_server_ids.len() > 1),
        ),
        // A recommendation is reported only when the record holds one.
        (
            format!("{prefix}/recommendationSet/strategy"),
            string_or_null(&record.strategy),
        ),
        (
            format!("{prefix}/recommendationSet/targetDestination"),
            string_or_null(&record.target_destination),
        ),
        // Nothing in this emulator picks a transformation tool, generates an anti-pattern
        // report or analyses source code, so none of these is ever reported.
        (
            format!("{prefix}/recommendationSet/transformationTool"),
            Value::Null,
        ),
        (
            format!("{prefix}/listAntipatternSeveritySummary"),
            Value::Null,
        ),
        (format!("{prefix}/antipatternReportS3Object"), Value::Null),
        (format!("{prefix}/antipatternReportStatus"), Value::Null),
        (
            format!("{prefix}/antipatternReportStatusMessage"),
            Value::Null,
        ),
        (format!("{prefix}/resultList"), Value::Null),
    ];
    if record.database_secret_name.is_none() {
        expected.push((format!("{prefix}/databaseConfigDetail"), Value::Null));
    }
    if record.app_unit_error_category.is_none() {
        expected.push((format!("{prefix}/appUnitError"), Value::Null));
    }
    if record.strategy.is_none() && record.target_destination.is_none() {
        expected.push((format!("{prefix}/recommendationSet"), Value::Null));
    }
    if record.source_code_repositories.is_empty() {
        expected.push((format!("{prefix}/sourceCodeRepositories"), Value::Null));
    } else {
        for (index, repo) in record.source_code_repositories.iter().enumerate() {
            for (member, value) in [
                ("repository", &repo.repository),
                ("branch", &repo.branch),
                ("versionControlType", &repo.version_control_type),
                ("projectName", &repo.project_name),
            ] {
                expected.push((
                    format!("{prefix}/sourceCodeRepositories/{index}/{member}"),
                    string_or_null(value),
                ));
            }
        }
    }
    expected
}

/// Every row's detail is its stored record, member for member, and every member the record
/// does not hold is absent.
#[tokio::test]
async fn the_detail_is_the_stored_record() {
    let rows = table();
    let router = emulator_with(rows.clone());

    // The table really does span the whole `AppType` catalog.
    let covered: BTreeSet<String> = rows
        .iter()
        .filter_map(|record| record.app_type.clone())
        .collect();
    let catalog: BTreeSet<String> = APP_TYPES.iter().map(|value| (*value).to_string()).collect();
    assert_eq!(
        covered, catalog,
        "the table must cover every AppType value, it covers {covered:?}"
    );

    for record in &rows {
        let what = format!("component {:?}", record.id);
        let answer = get_component(&router, &record.id).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );

        for (pointer, expected) in detail_members(record) {
            assert_eq!(
                at(&answer.body, &pointer),
                expected,
                "case {what:?}: expected {pointer} to be {expected:?}, got {:?}",
                at(&answer.body, &pointer)
            );
        }

        let reported_instant =
            at(&answer.body, "/applicationComponentDetail/lastAnalyzedTimestamp");
        match record.last_analyzed {
            Some(stored) => {
                let expected = stored
                    .duration_since(UNIX_EPOCH)
                    .expect("the fixture's instants are after the epoch")
                    .as_secs_f64();
                assert_eq!(
                    reported_instant.as_f64().unwrap_or_default(),
                    expected,
                    "case {what:?}: expected lastAnalyzedTimestamp {expected}, got \
                     {reported_instant:?}"
                );
            }
            None => assert_eq!(
                reported_instant,
                Value::Null,
                "case {what:?}: a record holding no analysis instant reports none, got \
                 {reported_instant:?}"
            ),
        }
    }
}

/// Both associations come back in the stored order, and the two "more than one" flags are
/// counted off those lists rather than fixed.
#[tokio::test]
async fn the_associations_and_their_flags_come_from_the_stored_lists() {
    let rows = table();
    let router = emulator_with(rows.clone());

    // The rows between them cover a component with no association, with exactly one, and
    // with several of each kind.
    let server_counts: BTreeSet<usize> = rows
        .iter()
        .map(|record| record.associated_server_ids.len())
        .collect();
    let application_counts: BTreeSet<usize> = rows
        .iter()
        .map(|record| record.associated_applications.len())
        .collect();
    assert!(
        server_counts.contains(&0) && server_counts.contains(&1) && server_counts.len() >= 3,
        "the table must span server-association counts, it spans {server_counts:?}"
    );
    assert!(
        application_counts.contains(&0)
            && application_counts.contains(&1)
            && application_counts.len() >= 3,
        "the table must span application-group counts, it spans {application_counts:?}"
    );

    for record in &rows {
        let what = format!(
            "component {:?} with {} server(s) and {} application group(s)",
            record.id,
            record.associated_server_ids.len(),
            record.associated_applications.len()
        );
        let answer = get_component(&router, &record.id).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported_server_ids(&answer),
            record.associated_server_ids,
            "case {what:?}: expected the stored server ids in order, got {:?}",
            answer.body.get("associatedServerIds")
        );
        assert_eq!(
            reported_applications(&answer),
            stored_applications(record),
            "case {what:?}: expected the stored application groups in order, got {:?}",
            answer.body.get("associatedApplications")
        );
        assert_eq!(
            at(&answer.body, "/moreApplicationResource"),
            json!(record.associated_applications.len() > 1),
            "case {what:?}: expected moreApplicationResource {}, got {:?}",
            record.associated_applications.len() > 1,
            answer.body
        );
    }
}

/// An `applicationComponentId` that names no component of this account's portfolio is
/// reported as an absent resource — whatever the id looks like, and whether or not the
/// store holds other components.
#[tokio::test]
async fn an_id_naming_no_component_is_not_found() {
    for (fixture, router) in [
        ("a store holding every row", emulator_with(table())),
        ("an account holding no component", emulator_with(Vec::new())),
    ] {
        for (what, id) in [
            ("an id no record was seeded under", "9999a"),
            ("a one-character id", "b"),
            (
                "an id ApplicationComponentId admits that no ResourceId could be",
                "App-Component-01",
            ),
            ("an id at the 44-character maximum", &"0".repeat(44)[..]),
        ] {
            let answer = get_component(&router, id).await;
            assert_eq!(
                answer.status, 404,
                "case {fixture:?}/{what:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {fixture:?}/{what:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("/message"),
                INVALID_RESOURCE_ID,
                "case {fixture:?}/{what:?}: expected the service's {INVALID_RESOURCE_ID:?}, got \
                 {:?}",
                answer.body
            );
        }
    }
}

/// The read changes nothing a later caller sees: the same request answers the same thing
/// however many times it is made, a read of one component does not disturb another, and
/// reading an id that names nothing does not bring it into being.
#[tokio::test]
async fn reading_a_component_changes_nothing() {
    let rows = table();
    let router = emulator_with(rows.clone());
    let unknown = "0b0b";

    for record in &rows {
        let what = format!("component {:?}", record.id);
        let first = get_component(&router, &record.id).await;
        let other = get_component(&router, &bare_row_id()).await;
        let missing = get_component(&router, unknown).await;
        let again = get_component(&router, &record.id).await;

        assert_eq!(
            first.body, again.body,
            "case {what:?}: expected two identical reads to answer the same thing, got {:?} then \
             {:?}",
            first.body, again.body
        );
        assert_eq!(
            other.status, 200,
            "case {what:?}: expected the neighbouring read to stay readable, got {} with {:?}",
            other.status, other.body
        );
        assert_eq!(
            missing.status, 404,
            "case {what:?}: expected a read of {unknown:?} to stay a 404, got {} with {:?}",
            missing.status, missing.body
        );
    }

    let after = get_component(&router, unknown).await;
    assert_eq!(
        after.status, 404,
        "expected reading {unknown:?} never to create it, got {} with {:?}",
        after.status, after.body
    );
}
