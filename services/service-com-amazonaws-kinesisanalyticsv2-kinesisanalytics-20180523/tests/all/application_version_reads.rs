// SPDX-License-Identifier: CC0-1.0

//! Reading ONE version of an application: `DescribeApplicationVersion`.
//!
//! Every test drives the service the way a client does — one `POST /` per call through the
//! real `MultiServiceRouter`, with `X-Amz-Target: KinesisAnalytics_20180523.<Operation>`.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! * a per-version read is served only for an application of the Managed Service for
//!   Apache Flink runtime family ("This operation is supported only for Managed Service for
//!   Apache Flink", `DescribeApplicationVersion`'s own doc), so an application of any other
//!   family — a Studio notebook or a SQL application — is refused with the modelled
//!   `UnsupportedOperationException` (HTTP 400) whatever version it names;
//! * a version the application has LEFT is reported as it ran: the detail kept for that
//!   version, not the configuration in force — so the two readers of the history
//!   (`ListApplicationVersions` and `DescribeApplicationVersion`) agree on every version an
//!   application has;
//! * reading a version is reporting, not mutating: it leaves the version in force, the
//!   history and the application's detail exactly as a later caller finds them.
//!
//! The runtime table is the WHOLE `RuntimeEnvironment` enum, read from the generated
//! `RUNTIME_ENVIRONMENT_VALUES`, so a family the issue never cited (the SQL one) is covered
//! and a value added to the model later joins the table on its own.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::RUNTIME_ENVIRONMENT_VALUES;
use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::state;

use super::harness::{call, create, describe, error_code, error_message, router_on};

/// The runtime every application a case does not vary the family of runs on.
const FLINK_RUNTIME: &str = "FLINK-1_20";

/// Whether `runtime` names the Managed Service for Apache Flink family — the plain
/// `FLINK-` runtimes, as opposed to the `SQL-` and `ZEPPELIN-FLINK-` families the same enum
/// declares (`smithy_query(query="shape",shape="RuntimeEnvironment")`). Spelled out here so
/// this suite decides the expectation itself rather than from the service's own helper.
fn is_flink_family(runtime: &str) -> bool {
    runtime.starts_with("FLINK-")
}

/// A router with this service mounted on a fresh engine, and the engine itself, so a case
/// can store an application the create path no longer provisions.
fn emulator() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    (engine, router)
}

/// Store an application under `name` running on `runtime`.
///
/// `CreateApplication` no longer provisions the retired SQL runtime family, so for that
/// family the application is created through the real create path and its stored
/// `RuntimeEnvironment` re-stamped in the store — which is what an application created
/// before that family was retired looks like, and what a reader of its versions decides
/// against.
async fn application_on(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    name: &str,
    runtime: &str,
) {
    if !runtime.starts_with("SQL-") {
        create(router, name, runtime).await;
        return;
    }
    create(router, name, FLINK_RUNTIME).await;
    let store = state::service_state(&**engine).expect("the service holds state");
    let mut guard = store.lock();
    let application = guard
        .applications
        .get_mut(name)
        .unwrap_or_else(|| panic!("case {name:?}: the application was just created"));
    application.runtime_environment = runtime.to_string();
}

/// `DescribeApplicationVersion` on `name`, as `(http status, parsed body)`.
async fn describe_version(
    router: &MultiServiceRouter,
    name: &str,
    version_id: i64,
) -> (u16, Value) {
    call(
        router,
        "DescribeApplicationVersion",
        &json!({"ApplicationName": name, "ApplicationVersionId": version_id}),
    )
    .await
}

/// `ListApplicationVersions` on `name`, asserting the call succeeded.
async fn list_versions(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, listed) = call(
        router,
        "ListApplicationVersions",
        &json!({"ApplicationName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: ListApplicationVersions got {status} {listed}"
    );
    listed
}

/// Apply one `UpdateApplication` to `name` against the version it is on now, and answer the
/// version the service moved it to.
async fn update(
    router: &MultiServiceRouter,
    name: &str,
    current: i64,
    mut body: Value,
) -> i64 {
    body["ApplicationName"] = json!(name);
    body["CurrentApplicationVersionId"] = json!(current);
    let (status, updated) = call(router, "UpdateApplication", &body).await;
    assert_eq!(
        status, 200,
        "case {name:?} v{current}: UpdateApplication got {status} {updated}"
    );
    updated["ApplicationDetail"]["ApplicationVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {name:?} v{current}: expected a version id, got {updated}"))
}

/// The `(ServiceExecutionRole, ParallelismConfigurationType, Parallelism)` one version of an
/// application ran under, read off an `ApplicationDetail`.
fn ran_under(detail: &Value) -> (String, String, i64) {
    let parallelism = &detail["ApplicationConfigurationDescription"]
        ["FlinkApplicationConfigurationDescription"]["ParallelismConfigurationDescription"];
    (
        detail["ServiceExecutionRole"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        parallelism["ConfigurationType"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        parallelism["Parallelism"].as_i64().unwrap_or_default(),
    )
}

// ---------------------------------------------------------------------------
// Which applications have a version history to read
// ---------------------------------------------------------------------------

/// A per-version read is served for the Flink family and refused for every other family the
/// `RuntimeEnvironment` enum declares, with the modelled `UnsupportedOperationException`,
/// HTTP 400 and a `Message` naming the operation and the runtime the application runs on.
///
/// The rows are the whole enum, so the `SQL-` family and the `ZEPPELIN-FLINK-` notebooks are
/// both covered; the version asked for is varied too, because what decides is the
/// application's runtime and never the version named.
#[tokio::test]
async fn a_version_read_is_served_only_for_the_flink_runtime_family() {
    let (engine, router) = emulator();
    for runtime in RUNTIME_ENVIRONMENT_VALUES {
        let name = format!("version-read-{}", runtime.replace(['-', '_'], "."));
        application_on(&engine, &router, &name, runtime).await;

        // The version in force, one the application never had, and the top of the modelled
        // `ApplicationVersionId` range: a supported application answers each on its own
        // terms, an unsupported one is refused the same way for all three.
        for version_id in [1_i64, 7, 999_999_999] {
            let (status, answered) = describe_version(&router, &name, version_id).await;
            if !is_flink_family(runtime) {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "UnsupportedOperationException"),
                    "case {runtime:?} v{version_id}: expected 400 UnsupportedOperationException, \
                     got {status} {answered}"
                );
                assert_eq!(
                    error_message(&answered),
                    format!(
                        "DescribeApplicationVersion is not supported for applications with \
                         runtime environment {runtime}."
                    ),
                    "case {runtime:?} v{version_id}: expected the refusal to name the operation \
                     and the runtime, got {answered}"
                );
                continue;
            }
            if version_id == 1 {
                assert_eq!(
                    status, 200,
                    "case {runtime:?} v{version_id}: expected the version in force to be \
                     served, got {status} {answered}"
                );
                let detail = &answered["ApplicationVersionDetail"];
                assert_eq!(
                    (
                        detail["ApplicationName"].as_str(),
                        detail["RuntimeEnvironment"].as_str(),
                        detail["ApplicationVersionId"].as_i64(),
                    ),
                    (Some(name.as_str()), Some(*runtime), Some(1)),
                    "case {runtime:?} v{version_id}: expected the stored application's own \
                     detail, got {answered}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ResourceNotFoundException"),
                "case {runtime:?} v{version_id}: a version the application never had is a \
                 miss, got {status} {answered}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What a version reports
// ---------------------------------------------------------------------------

/// Every version an application has is readable by id and reports the configuration THAT
/// version ran under, and the two readers of the history agree: the `(id, status)` pairs
/// `ListApplicationVersions` summarises are exactly the ones read back one version at a
/// time.
///
/// The history is built from updates that differ along WHICH member they name — the service
/// execution role and the Flink parallelism — so a version left behind cannot be answered
/// from the record in force whichever member moved on.
#[tokio::test]
async fn a_version_the_application_has_left_reports_what_it_ran_under() {
    let (_engine, router) = emulator();
    let name = "version-history";
    let created = create(&router, name, FLINK_RUNTIME).await;
    let role_v1 = created["ServiceExecutionRole"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let role = |n: u8| format!("arn:aws:iam::000000000000:role/kda-role-{n}");

    // (the update, what the version it mints must report back)
    let steps: Vec<(Value, (String, String, i64))> = vec![
        (
            json!({"ServiceExecutionRoleUpdate": role(2)}),
            (role(2), "DEFAULT".to_string(), 1),
        ),
        (
            json!({
                "ApplicationConfigurationUpdate": {
                    "FlinkApplicationConfigurationUpdate": {
                        "ParallelismConfigurationUpdate": {
                            "ConfigurationTypeUpdate": "CUSTOM",
                            "ParallelismUpdate": 4,
                        }
                    }
                }
            }),
            (role(2), "CUSTOM".to_string(), 4),
        ),
        (
            json!({"ServiceExecutionRoleUpdate": role(3)}),
            (role(3), "CUSTOM".to_string(), 4),
        ),
    ];

    // What every version must report, oldest first: the create's own values and then one
    // row per update.
    let mut expected: Vec<(String, String, i64)> =
        vec![(role_v1, "DEFAULT".to_string(), 1)];
    let mut version = 1_i64;
    for (body, ran) in steps {
        version = update(&router, name, version, body).await;
        expected.push(ran);
    }
    assert_eq!(
        version,
        expected.len() as i64,
        "case history: expected one version per accepted update, got v{version}"
    );

    for (index, want) in expected.iter().enumerate() {
        let version_id = index as i64 + 1;
        let (status, answered) = describe_version(&router, name, version_id).await;
        assert_eq!(
            status, 200,
            "case v{version_id}: DescribeApplicationVersion got {status} {answered}"
        );
        let detail = &answered["ApplicationVersionDetail"];
        assert_eq!(
            detail["ApplicationVersionId"].as_i64(),
            Some(version_id),
            "case v{version_id}: expected the version asked for, got {detail}"
        );
        assert_eq!(
            &ran_under(detail),
            want,
            "case v{version_id}: expected the configuration that version ran under, got {detail}"
        );
    }

    // The readers of the history agree, version for version.
    let listed = list_versions(&router, name).await;
    let summaries = listed["ApplicationVersionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case history: expected the summaries member, got {listed}"))
        .clone();
    assert_eq!(
        summaries.len(),
        expected.len(),
        "case history: expected one summary per version, got {listed}"
    );
    for summary in &summaries {
        let version_id = summary["ApplicationVersionId"]
            .as_i64()
            .unwrap_or_else(|| panic!("case history: expected a version id, got {summary}"));
        let (status, answered) = describe_version(&router, name, version_id).await;
        assert_eq!(
            status, 200,
            "case summary v{version_id}: DescribeApplicationVersion got {status} {answered}"
        );
        assert_eq!(
            answered["ApplicationVersionDetail"]["ApplicationStatus"].as_str(),
            summary["ApplicationStatus"].as_str(),
            "case summary v{version_id}: expected both readers to report one status, got \
             {summary} and {answered}"
        );
    }
}

// ---------------------------------------------------------------------------
// A read is reporting, not mutating
// ---------------------------------------------------------------------------

/// Reading versions changes nothing a later caller sees: the application's detail, its
/// history and the version in force are the same after a run of reads — served, missing and
/// refused alike — as before them, and two reads of one version answer the same body.
#[tokio::test]
async fn reading_versions_does_not_change_what_a_later_caller_sees() {
    let (engine, router) = emulator();
    let name = "version-reads-are-reads";
    create(&router, name, FLINK_RUNTIME).await;
    update(
        &router,
        name,
        1,
        json!({"ServiceExecutionRoleUpdate": "arn:aws:iam::000000000000:role/kda-role-2"}),
    )
    .await;

    // A notebook beside it, so the refused path is exercised against stored state too.
    let notebook = "version-reads-notebook";
    application_on(&engine, &router, notebook, "ZEPPELIN-FLINK-3_0").await;

    let detail_before = describe(&router, name).await;
    let listed_before = list_versions(&router, name).await;

    // (case, the application read, the version asked for) — a version in force, a version
    // left behind, a miss, and a read refused for the runtime family.
    let reads: [(&str, &str, i64); 5] = [
        ("version-in-force", name, 2),
        ("version-left-behind", name, 1),
        ("version-never-had", name, 9),
        ("version-in-force-again", name, 2),
        ("unsupported-runtime", notebook, 1),
    ];
    let mut first_answer: Option<Value> = None;
    for (case, application, version_id) in reads {
        let (status, answered) = describe_version(&router, application, version_id).await;
        if case == "version-in-force" {
            first_answer = Some(answered.clone());
        }
        if case == "version-in-force-again" {
            assert_eq!(
                Some(&answered),
                first_answer.as_ref(),
                "case {case}: expected two reads of one version to answer the same body, got \
                 {answered}"
            );
        }
        assert!(
            status == 200 || status == 400,
            "case {case}: expected the read to be served or refused, got {status} {answered}"
        );
    }

    assert_eq!(
        describe(&router, name).await,
        detail_before,
        "case after-reads: expected the application's detail to be untouched by reads"
    );
    assert_eq!(
        list_versions(&router, name).await,
        listed_before,
        "case after-reads: expected the version history to be untouched by reads"
    );

    // And the version in force is still the one an update has to name: a read neither
    // advanced nor rolled back the version.
    let version = update(
        &router,
        name,
        2,
        json!({"ServiceExecutionRoleUpdate": "arn:aws:iam::000000000000:role/kda-role-3"}),
    )
    .await;
    assert_eq!(
        version, 3,
        "case after-reads: expected the next version to follow the one in force, got v{version}"
    );
}
