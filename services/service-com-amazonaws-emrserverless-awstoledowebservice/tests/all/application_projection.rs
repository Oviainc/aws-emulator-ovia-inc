// SPDX-License-Identifier: CC0-1.0

//! Create-time engine validation, and the application projection every read reports.
//!
//! Three rules, each asserted over a SET of inputs rather than over one recorded
//! request, all through the real router (restJson1, signing name `emr-serverless`):
//!
//! 1. `CreateApplication` refuses, with its modelled `ValidationException` (HTTP 400),
//!    a `type` the named Amazon EMR release does not support, and stores a supported
//!    one in the service's own casing rather than the caller's.
//! 2. A precondition on the application lifecycle is judged against the state the
//!    application has SETTLED on, so a transition the clock still owes never refuses a
//!    call the settled application admits — and a delete leaves the application
//!    readable in the terminal `TERMINATED` state.
//! 3. A read reports the members the application CARRIES: the values the create gave,
//!    or the ones the service applied when it gave none — never an omission.

use serde_json::{json, Value};

use super::application_lifecycle::{
    create_application, delete_application, emulator, get_application, list_applications,
    minimal_create, wait_for_transition, Answer,
};

/// The two engines this service serves, with the casing its API reports them in.
///
/// Written out here rather than read from `crate::state`, so a wrong catalog in `src/`
/// cannot make this test pass. Source: `Application$type`'s documentation, "The type of
/// application, such as Spark or Hive"
/// (`smithy_query(query="shape",shape="Application")`).
const ENGINES: &[&str] = &["Spark", "Hive"];

/// The earliest Amazon EMR release that serves either engine, and the release one step
/// below it.
///
/// Source: "EMR Serverless is available with Amazon EMR releases 6.6.0 and later" — so
/// `emr-6.6.0` is the published bound and `emr-6.5.0` is one past it.
const EARLIEST_RELEASE: &str = "emr-6.6.0";
const BEFORE_EARLIEST_RELEASE: &str = "emr-6.5.0";

/// `Architecture` has two values (`ARM64`, `X86_64`) and EMR Serverless applies x86_64
/// to an application that names none.
const DEFAULT_ARCHITECTURE: &str = "X86_64";

/// `AutoStopConfig$idleTimeoutMinutes` is `@range(min: 1, max: 10080)` and documents
/// "Defaults to 15 minutes".
const DEFAULT_IDLE_TIMEOUT_MINUTES: i64 = 15;
const MAX_IDLE_TIMEOUT_MINUTES: i64 = 10080;

/// A `CreateApplication` body naming an engine and a release label.
fn create_with_engine(token: &str, engine_type: &str, release_label: &str) -> Value {
    json!({
        "clientToken": token,
        "releaseLabel": release_label,
        "type": engine_type,
    })
}

/// How many applications the store holds.
async fn application_count(router: &smithy_server_runtime::MultiServiceRouter) -> usize {
    list_applications(router, "")
        .await
        .body
        .get("applications")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// `String256`'s constraints on `Application$stateDetails`: length 1..256 and the
/// pattern `.*\S.*` (at least one non-whitespace character).
fn is_state_details(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| (1..=256).contains(&text.chars().count()) && text.trim() != "")
}

/// Which member of a create a refusal must point the caller at.
#[derive(Debug, Clone, Copy)]
enum Member {
    /// The create is accepted, so no member failed.
    None,
    /// `type` names no engine this service serves.
    Type,
    /// `releaseLabel` names no release that offers the engine asked for.
    ReleaseLabel,
}

/// `CreateApplication` refuses an engine type the named release does not support, and
/// mints nothing when it does; an engine the release serves is accepted whatever casing
/// the request used. A refusal names the MEMBER that failed: the engine type when it
/// names no engine this service serves, the release label when the engine is real but
/// the release named offers it — or does not exist at all.
#[tokio::test]
async fn create_application_refuses_an_engine_type_the_release_does_not_support() {
    let long_type = "a".repeat(64);
    // (case, type, releaseLabel, expected status, the member the refusal must name).
    // The rows differ along the value, its casing, its length and the release it is
    // asked for.
    let cases: Vec<(&str, &str, &str, u16, Member)> = vec![
        (
            "the first engine, canonical casing",
            "Spark",
            "emr-7.1.0",
            200,
            Member::None,
        ),
        (
            "the first engine, upper case",
            "SPARK",
            "emr-7.1.0",
            200,
            Member::None,
        ),
        (
            "the first engine, lower case",
            "spark",
            "emr-6.9.0",
            200,
            Member::None,
        ),
        (
            "the second engine, canonical casing",
            "Hive",
            "emr-7.1.0",
            200,
            Member::None,
        ),
        (
            "the second engine, mixed case",
            "hIvE",
            EARLIEST_RELEASE,
            200,
            Member::None,
        ),
        (
            "a release label carrying a build suffix",
            "Spark",
            "emr-6.6.0-20220101",
            200,
            Member::None,
        ),
        (
            "a release label with no readable version, which names no release",
            "Spark",
            "latest",
            400,
            Member::ReleaseLabel,
        ),
        (
            "an engine no release serves",
            "NOT_A_REAL_ENGINE",
            "emr-7.1.0",
            400,
            Member::Type,
        ),
        (
            "an engine named by a near miss",
            "Sparks",
            "emr-7.1.0",
            400,
            Member::Type,
        ),
        (
            "a one-character type (EngineType's minimum length)",
            "X",
            "emr-7.1.0",
            400,
            Member::Type,
        ),
        (
            "a 64-character type (EngineType's maximum length)",
            &long_type,
            "emr-7.1.0",
            400,
            Member::Type,
        ),
        (
            "a supported engine one release below the earliest that serves it",
            "Spark",
            BEFORE_EARLIEST_RELEASE,
            400,
            Member::ReleaseLabel,
        ),
        (
            "a supported engine at a much older release",
            "Hive",
            "emr-5.36.0",
            400,
            Member::ReleaseLabel,
        ),
    ];

    let router = emulator();
    let mut expected_applications = 0usize;
    for (index, (case, engine_type, release_label, expected_status, member)) in
        cases.into_iter().enumerate()
    {
        let token = format!("tok-engine-{index}");
        let answer =
            create_application(&router, create_with_engine(&token, engine_type, release_label))
                .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: CreateApplication({engine_type:?}, {release_label:?}) expected \
             {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );

        if expected_status == 200 {
            expected_applications += 1;
            let id = answer.at("/applicationId");
            assert!(
                id.as_str().is_some_and(|value| !value.is_empty()),
                "case {case:?}: an accepted create must name the application it minted, \
                 got {id:?}"
            );
        } else {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            let message = answer.at("/message");
            let text = message.as_str().unwrap_or_default();
            match member {
                Member::Type => assert!(
                    text.contains(engine_type) && text.contains("not supported for release label"),
                    "case {case:?}: the refusal must name the rejected type \
                     {engine_type:?}, got {message:?}"
                ),
                Member::ReleaseLabel => {
                    assert!(
                        text.contains("release label"),
                        "case {case:?}: the refusal must name the release label, got {message:?}"
                    );
                    assert!(
                        !text.contains(engine_type),
                        "case {case:?}: the engine type {engine_type:?} is valid, so the \
                         refusal must not blame it, got {message:?}"
                    );
                }
                Member::None => unreachable!("a refused create names a member"),
            }
            assert!(
                !text.contains(release_label),
                "case {case:?}: a refusal states the condition, never the submitted \
                 release label, got {message:?}"
            );
        }

        let held = application_count(&router).await;
        assert_eq!(
            held, expected_applications,
            "case {case:?}: a refused create must mint nothing — expected \
             {expected_applications} applications, got {held}"
        );
    }
}

/// The engine type a read reports is the service's own canonical value, whatever casing
/// the create asked in — and the summary `ListApplications` serves reports the same.
#[tokio::test]
async fn an_engine_type_reads_back_in_the_services_own_casing() {
    let router = emulator();
    for (index, canonical) in ENGINES.iter().enumerate() {
        // Every casing of the engine name: canonical, upper, lower and alternating.
        let alternating: String = canonical
            .chars()
            .enumerate()
            .map(|(position, letter)| {
                if position % 2 == 0 {
                    letter.to_ascii_lowercase()
                } else {
                    letter.to_ascii_uppercase()
                }
            })
            .collect();
        let casings = [
            canonical.to_string(),
            canonical.to_ascii_uppercase(),
            canonical.to_ascii_lowercase(),
            alternating,
        ];
        for (position, requested) in casings.iter().enumerate() {
            let case = format!("{canonical} asked for as {requested:?}");
            let token = format!("tok-casing-{index}-{position}");
            let created = create_application(
                &router,
                create_with_engine(&token, requested, "emr-7.1.0"),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateApplication expected 200, got {} with {:?}",
                created.status, created.body
            );
            let id = created
                .at("/applicationId")
                .as_str()
                .unwrap_or_default()
                .to_string();

            let fetched = get_application(&router, &id).await;
            assert_eq!(
                fetched.at("/application/type"),
                json!(canonical),
                "case {case:?}: GetApplication reports the canonical engine type, got {:?}",
                fetched.at("/application/type")
            );

            let listed = list_applications(&router, "").await;
            let summary = listed
                .body
                .get("applications")
                .and_then(Value::as_array)
                .and_then(|entries| entries.iter().find(|entry| entry["id"] == json!(id)))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                summary.get("type").cloned().unwrap_or(Value::Null),
                json!(canonical),
                "case {case:?}: the summary reports the canonical engine type, got {summary:?}"
            );
        }
    }
}

/// A read reports every member the application carries: the value the create gave, or
/// the one the service applied when the create gave none.
#[tokio::test]
async fn a_read_reports_the_members_the_application_carries() {
    // (case, request, expected tags, expected architecture, expected idle timeout)
    let cases: Vec<(&str, Value, Value, &str, i64)> = vec![
        (
            "only the required members",
            minimal_create("tok-defaults"),
            json!({}),
            DEFAULT_ARCHITECTURE,
            DEFAULT_IDLE_TIMEOUT_MINUTES,
        ),
        (
            "an empty tag map given explicitly",
            json!({
                "clientToken": "tok-empty-tags",
                "releaseLabel": "emr-7.1.0",
                "type": "Spark",
                "tags": {},
            }),
            json!({}),
            DEFAULT_ARCHITECTURE,
            DEFAULT_IDLE_TIMEOUT_MINUTES,
        ),
        (
            "two tags and the other architecture",
            json!({
                "clientToken": "tok-two-tags",
                "releaseLabel": "emr-7.1.0",
                "type": "Hive",
                "architecture": "ARM64",
                "tags": { "team": "data", "env": "test" },
            }),
            json!({ "team": "data", "env": "test" }),
            "ARM64",
            DEFAULT_IDLE_TIMEOUT_MINUTES,
        ),
        (
            "the idle timeout at its published maximum",
            json!({
                "clientToken": "tok-max-idle",
                "releaseLabel": "emr-7.1.0",
                "type": "Spark",
                "autoStopConfiguration": { "idleTimeoutMinutes": MAX_IDLE_TIMEOUT_MINUTES },
            }),
            json!({}),
            DEFAULT_ARCHITECTURE,
            MAX_IDLE_TIMEOUT_MINUTES,
        ),
        (
            "the idle timeout at its published minimum, with a monitoring configuration",
            json!({
                "clientToken": "tok-min-idle",
                "releaseLabel": "emr-7.1.0",
                "type": "Spark",
                "autoStopConfiguration": { "idleTimeoutMinutes": 1 },
                "monitoringConfiguration": {
                    "s3MonitoringConfiguration": { "logUri": "s3://bucket/logs" },
                },
            }),
            json!({}),
            DEFAULT_ARCHITECTURE,
            1,
        ),
        (
            "job level cost allocation asked for explicitly",
            json!({
                "clientToken": "tok-cost",
                "releaseLabel": "emr-7.1.0",
                "type": "Spark",
                "jobLevelCostAllocationConfiguration": { "enabled": true },
                "tags": { "only": "one" },
            }),
            json!({ "only": "one" }),
            DEFAULT_ARCHITECTURE,
            DEFAULT_IDLE_TIMEOUT_MINUTES,
        ),
    ];

    for (case, request, expected_tags, expected_architecture, expected_idle) in cases {
        let router = emulator();
        let created = create_application(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateApplication expected 200, got {} with {:?}",
            created.status, created.body
        );
        let id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();

        let fetched = get_application(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetApplication expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );

        for (member, expected) in [
            ("/application/tags", expected_tags),
            ("/application/architecture", json!(expected_architecture)),
            (
                "/application/autoStopConfiguration/idleTimeoutMinutes",
                json!(expected_idle),
            ),
            ("/application/autoStartConfiguration/enabled", json!(true)),
            ("/application/autoStopConfiguration/enabled", json!(true)),
        ] {
            assert_eq!(
                fetched.at(member),
                expected,
                "case {case:?}: GetApplication{member} expected {expected:?}, got {:?}",
                fetched.at(member)
            );
        }

        // Every configuration block an application carries is REPORTED, with whatever
        // the create put in it — never omitted for a caller to guess at.
        for member in [
            "/application/monitoringConfiguration",
            "/application/jobLevelCostAllocationConfiguration",
        ] {
            let actual = fetched.at(member);
            assert!(
                actual.is_object(),
                "case {case:?}: GetApplication{member} must report the configuration the \
                 application carries, got {actual:?}"
            );
            if let Some(given) = request.get(member.rsplit('/').next().unwrap_or_default()) {
                assert_eq!(
                    &actual, given,
                    "case {case:?}: GetApplication{member} must report what the create \
                     gave ({given:?}), got {actual:?}"
                );
            }
        }

        let details = fetched.at("/application/stateDetails");
        assert!(
            is_state_details(&details),
            "case {case:?}: stateDetails must satisfy String256, got {details:?}"
        );
    }
}

/// A deleted application is RETAINED in the terminal `TERMINATED` state: a delete the
/// settled application admits is accepted however soon it follows the create, and the
/// application it terminated stays readable, with the members it carried unchanged.
#[tokio::test]
async fn a_deleted_application_stays_readable_in_the_terminal_state() {
    // The rows differ along WHEN the delete arrives: inside the span the clock still
    // owes the create, and after it.
    for (case, settle_first) in [("inside the create transition", false), ("after it", true)] {
        let router = emulator();
        let created = create_application(&router, minimal_create("tok-terminate")).await;
        let id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if settle_first {
            wait_for_transition();
        }

        let before = get_application(&router, &id).await;
        let deleted = delete_application(&router, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteApplication expected 200, got {} with {:?}",
            deleted.status, deleted.body
        );

        let after = get_application(&router, &id).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: GetApplication after the delete expected 200, got {} with {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.at("/application/state"),
            json!("TERMINATED"),
            "case {case:?}: the terminated application reports TERMINATED, got {:?}",
            after.at("/application/state")
        );
        // The terminal state is where the application stays: a later read reports the
        // same thing, and nothing else about the record moved.
        let again = get_application(&router, &id).await;
        assert_eq!(
            again.at("/application/state"),
            json!("TERMINATED"),
            "case {case:?}: a second read must report TERMINATED too, got {:?}",
            again.at("/application/state")
        );
        for member in [
            "/application/applicationId",
            "/application/arn",
            "/application/releaseLabel",
            "/application/type",
            "/application/createdAt",
            "/application/stateDetails",
            "/application/tags",
            "/application/architecture",
        ] {
            assert_eq!(
                after.at(member),
                before.at(member),
                "case {case:?}: the delete must not change {member}, got {:?} after {:?}",
                after.at(member),
                before.at(member)
            );
        }

        let listed = list_applications(&router, "?states=TERMINATED").await;
        assert_eq!(
            listed.at("/applications/0/id"),
            json!(id),
            "case {case:?}: the terminated application is listed under its state, got {:?}",
            listed.body
        );

        // And it is past its lifecycle: nothing restarts it or runs on it.
        for (operation, uri) in [
            ("StartApplication", format!("/applications/{id}/start")),
            ("StopApplication", format!("/applications/{id}/stop")),
        ] {
            let answer =
                super::application_lifecycle::call(&router, "POST", &uri, None).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: {operation} on a terminated application expected 400, \
                 got {} with {:?}",
                answer.status, answer.body
            );
        }
        let submitted: Answer = super::application_lifecycle::call(
            &router,
            "POST",
            &format!("/applications/{id}/jobruns"),
            Some(json!({
                "clientToken": "job-after-delete",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRServerlessRole",
                "jobDriver": { "sparkSubmit": { "entryPoint": "s3://bucket/app.py" } },
            })),
        )
        .await;
        assert_eq!(
            submitted.status, 400,
            "case {case:?}: StartJobRun on a terminated application expected 400, got {} \
             with {:?}",
            submitted.status, submitted.body
        );
    }
}
