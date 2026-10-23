// SPDX-License-Identifier: CC0-1.0

//! `UpdateEnvironment` as a PATCH over the stored environment.
//!
//! The rule under test: every member the request carries replaces the stored one, every
//! member it omits leaves the stored one exactly as it was, and the answer — plus every
//! later read — comes out of the updated record. `Id` and `ApplicationId` are not
//! writable members, and `State` belongs to the clock, not to this call.
//!
//! The tables below span what the rule generalises over: WHICH subset of the writable
//! members a request carries, and the VALUE each one carries (both published length
//! bounds, one past each bound, and the modelled list length). Every assertion names its
//! case and prints the actual value.

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send, Answer,
};
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    ENVIRONMENT_DEPLOYING, ENVIRONMENT_READY_FOR_DEPLOYMENT, PREDEFINED_DEPLOYMENT_STRATEGIES,
};

/// One CloudWatch alarm monitor. The member names and the `AlarmRoleArn` pattern are the
/// model's own: `smithy_query(query="shape",shape="Monitor")` gives `AlarmArn`
/// (StringWithLengthBetween1And2048, required) and `AlarmRoleArn` (RoleArn, pattern
/// `^((arn):(aws|…):(iam)::\d{12}:role[/].*)$`).
fn monitor(index: usize) -> serde_json::Value {
    serde_json::json!({
        "AlarmArn": format!("arn:aws:cloudwatch:us-east-1:123456789012:alarm:appconfig-{index}"),
        "AlarmRoleArn": "arn:aws:iam::123456789012:role/appconfig-monitor",
    })
}

/// `monitors(n)` is the first `n` rows of that catalog.
fn monitors(count: usize) -> Vec<serde_json::Value> {
    (0..count).map(monitor).collect()
}

/// The environment a case expects to be able to read back.
#[derive(Clone, Debug)]
struct Expected {
    name: String,
    description: Option<String>,
    monitors: Vec<serde_json::Value>,
}

impl Expected {
    fn new(name: &str, description: Option<&str>, monitors: Vec<serde_json::Value>) -> Self {
        Self {
            name: name.to_string(),
            description: description.map(str::to_string),
            monitors,
        }
    }

    fn with_name(&self, name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..self.clone()
        }
    }

    fn with_description(&self, description: &str) -> Self {
        Self {
            description: Some(description.to_string()),
            ..self.clone()
        }
    }

    fn with_monitors(&self, monitors: Vec<serde_json::Value>) -> Self {
        Self {
            monitors,
            ..self.clone()
        }
    }
}

/// Assert that `answer` describes `expected`, under the identity `application`/`id` the
/// request named. `source` says which call produced the body.
fn assert_environment(
    case: &str,
    source: &str,
    answer: &Answer,
    application: &str,
    id: &str,
    expected: &Expected,
) {
    assert_eq!(
        answer.status, 200,
        "case {case:?} ({source}): expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    assert_eq!(
        body.get("Name").and_then(serde_json::Value::as_str),
        Some(expected.name.as_str()),
        "case {case:?} ({source}): expected Name {:?}, got {:?}",
        expected.name,
        body.get("Name")
    );
    assert_eq!(
        body.get("Description").and_then(serde_json::Value::as_str),
        expected.description.as_deref(),
        "case {case:?} ({source}): expected Description {:?}, got {:?}",
        expected.description,
        body.get("Description")
    );
    // An empty `Monitors` list is left off the wire, exactly as every other read of this
    // resource reports it.
    let reported = body
        .get("Monitors")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        reported, expected.monitors,
        "case {case:?} ({source}): expected Monitors {:?}, got {:?}",
        expected.monitors, reported
    );
    assert_eq!(
        body.get("Id").and_then(serde_json::Value::as_str),
        Some(id),
        "case {case:?} ({source}): Id is not writable, expected {id:?}, got {:?}",
        body.get("Id")
    );
    assert_eq!(
        body.get("ApplicationId").and_then(serde_json::Value::as_str),
        Some(application),
        "case {case:?} ({source}): ApplicationId is not writable, expected {application:?}, \
         got {:?}",
        body.get("ApplicationId")
    );
}

/// Read the environment back through `GetEnvironment` and assert the same expectation:
/// the update is persisted, not just reported.
async fn assert_round_trip(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    application: &str,
    id: &str,
    expected: &Expected,
) {
    let fetched = json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{id}"),
        &serde_json::Value::Null,
    )
    .await;
    assert_environment(case, "GetEnvironment", &fetched, application, id, expected);
}

/// Each patch replaces the members it carries and leaves the rest alone, and the stored
/// record it leaves behind is what `GetEnvironment` answers.
#[tokio::test]
async fn update_environment_replaces_only_the_members_the_request_carries() {
    let router = router();
    let application = create_application(&router, "patching").await;

    // The starting record carries every writable member, so a patch that omits one is
    // distinguishable from a patch that clears it.
    let created = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments"),
        &serde_json::json!({
            "Name": "baseline",
            "Description": "the original description",
            "Monitors": monitors(2),
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the environment under test must be created, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let id = created
        .string("Id")
        .expect("CreateEnvironment answered no Id");

    // `Name` is len 1..64 and `Description` is len 0..1024 (the model's `Name` and
    // `Description` shapes); `MonitorList` is len 0..5.
    let longest_name = "n".repeat(64);
    let longest_description = "d".repeat(1024);

    // Rows differ along which members the patch carries and the value each carries. Each
    // row's expectation is relative to the row before it, which is what makes "an
    // omitted member is unchanged" observable.
    let mut expected = Expected::new("baseline", Some("the original description"), monitors(2));
    let cases: Vec<(&str, serde_json::Value, Expected)> = vec![
        (
            "name only",
            serde_json::json!({ "Name": "renamed" }),
            expected.with_name("renamed"),
        ),
        (
            "description only",
            serde_json::json!({ "Description": "a replacement description" }),
            expected
                .with_name("renamed")
                .with_description("a replacement description"),
        ),
        (
            "no writable member at all",
            serde_json::json!({}),
            expected
                .with_name("renamed")
                .with_description("a replacement description"),
        ),
        (
            "monitors at the modelled maximum of five",
            serde_json::json!({ "Monitors": monitors(5) }),
            expected
                .with_name("renamed")
                .with_description("a replacement description")
                .with_monitors(monitors(5)),
        ),
        (
            "monitors cleared by an empty list",
            serde_json::json!({ "Monitors": [] }),
            expected
                .with_name("renamed")
                .with_description("a replacement description")
                .with_monitors(monitors(0)),
        ),
        (
            "description cleared by the empty string at the lower length bound",
            serde_json::json!({ "Description": "" }),
            expected
                .with_name("renamed")
                .with_description("")
                .with_monitors(monitors(0)),
        ),
        (
            "name at the one-character lower bound",
            serde_json::json!({ "Name": "a" }),
            expected
                .with_name("a")
                .with_description("")
                .with_monitors(monitors(0)),
        ),
        (
            "every member at once, at the upper length bounds",
            serde_json::json!({
                "Name": longest_name,
                "Description": longest_description,
                "Monitors": monitors(1),
            }),
            Expected::new(
                &longest_name,
                Some(&longest_description),
                monitors(1),
            ),
        ),
    ];

    for (case, patch, after) in cases {
        let updated = json_call(
            &router,
            "PATCH",
            &format!("/applications/{application}/environments/{id}"),
            &patch,
        )
        .await;
        assert_environment(
            case,
            "UpdateEnvironment",
            &updated,
            &application,
            &id,
            &after,
        );
        // `State` is never a member the caller writes; nothing has been deployed here,
        // so the environment is ready for one.
        assert_eq!(
            updated.string("State").as_deref(),
            Some(ENVIRONMENT_READY_FOR_DEPLOYMENT),
            "case {case:?}: an environment with no deployment is ReadyForDeployment, got {:?}",
            updated.string("State")
        );
        assert_round_trip(&router, case, &application, &id, &after).await;
        expected = after;
    }

    // The last row is still what the record holds once the table is done.
    assert_round_trip(&router, "final state", &application, &id, &expected).await;
}

/// A value past a published bound is refused with 400 and writes nothing: the record the
/// refused request named is unchanged afterwards.
#[tokio::test]
async fn update_environment_refuses_a_value_past_a_modelled_bound_without_writing() {
    let router = router();
    let application = create_application(&router, "bounds").await;
    let created = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments"),
        &serde_json::json!({
            "Name": "intact",
            "Description": "untouched",
            "Monitors": monitors(1),
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the environment under test must be created, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let id = created
        .string("Id")
        .expect("CreateEnvironment answered no Id");
    let expected = Expected::new("intact", Some("untouched"), monitors(1));

    // One past each bound the model publishes: `Name` max 64, `Description` max 1024,
    // `MonitorList` max 5, and `Name` min 1.
    for (case, patch) in [
        (
            "name one character past the upper bound",
            serde_json::json!({ "Name": "n".repeat(65) }),
        ),
        (
            "name one character below the lower bound",
            serde_json::json!({ "Name": "" }),
        ),
        (
            "description one character past the upper bound",
            serde_json::json!({ "Description": "d".repeat(1025) }),
        ),
        (
            "one monitor past the list maximum",
            serde_json::json!({ "Monitors": monitors(6) }),
        ),
    ] {
        let refused = json_call(
            &router,
            "PATCH",
            &format!("/applications/{application}/environments/{id}"),
            &patch,
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        // The boundary validator refuses a modelled constraint before the handler runs,
        // with the error code `crate::scaffolded` documents for that refusal.
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_round_trip(&router, case, &application, &id, &expected).await;
    }
}

/// `UpdateEnvironment` resolves both levels of the path and answers 404
/// `ResourceNotFoundException` for whichever one does not exist.
#[tokio::test]
async fn update_environment_is_not_found_when_the_application_or_environment_is_unknown() {
    let router = router();
    let application = create_application(&router, "lookups").await;
    let environment = create_environment(&router, &application, "known").await;

    for (case, uri) in [
        (
            "unknown application",
            format!("/applications/nosuchapp/environments/{environment}"),
        ),
        (
            "unknown environment",
            format!("/applications/{application}/environments/nosuchenv"),
        ),
        (
            "known ids crossed over",
            format!("/applications/{environment}/environments/{application}"),
        ),
    ] {
        let answer = json_call(&router, "PATCH", &uri, &serde_json::json!({ "Name": "x" })).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
    }
}

/// Two updates in a row are not a conflict — the operation models none — and the second
/// one wins: the record holds the last write, and a read agrees with it. `State` stays
/// the clock's: while a deployment is in flight the update reports `DEPLOYING` without
/// the patch touching it.
#[tokio::test]
async fn the_last_update_wins_and_state_stays_the_clocks() {
    let router = router();
    let application = create_application(&router, "last-write").await;
    let profile = create_profile(&router, &application, "deployed-config").await;
    let hosted = send(
        &router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        hosted.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        hosted.status,
        String::from_utf8_lossy(&hosted.body)
    );

    let environment = create_environment(&router, &application, "contended").await;

    // Successive writes of the same member: whichever landed last is what the record
    // holds, for every row of the sequence.
    for case in ["first writer", "second writer", "third writer"] {
        let updated = json_call(
            &router,
            "PATCH",
            &format!("/applications/{application}/environments/{environment}"),
            &serde_json::json!({ "Name": case, "Description": format!("set by {case}") }),
        )
        .await;
        let expected = Expected::new(case, Some(&format!("set by {case}")), monitors(0));
        assert_environment(
            case,
            "UpdateEnvironment",
            &updated,
            &application,
            &environment,
            &expected,
        );
        assert_round_trip(&router, case, &application, &environment, &expected).await;
    }

    // A deployment in flight is what makes an environment DEPLOYING. The strategy is the
    // longest-running row of the predefined catalog, so the rollout is still in progress
    // when the update runs.
    let slowest = PREDEFINED_DEPLOYMENT_STRATEGIES
        .iter()
        .max_by_key(|strategy| {
            strategy.deployment_duration_in_minutes + strategy.final_bake_time_in_minutes
        })
        .expect("the predefined catalog is not empty");
    let started = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &serde_json::json!({
            "DeploymentStrategyId": slowest.id,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "the deployment must start, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );

    let during = json_call(
        &router,
        "PATCH",
        &format!("/applications/{application}/environments/{environment}"),
        &serde_json::json!({ "Name": "renamed-mid-deployment" }),
    )
    .await;
    let expected = Expected::new(
        "renamed-mid-deployment",
        Some("set by third writer"),
        monitors(0),
    );
    assert_environment(
        "update during a deployment",
        "UpdateEnvironment",
        &during,
        &application,
        &environment,
        &expected,
    );
    assert_eq!(
        during.string("State").as_deref(),
        Some(ENVIRONMENT_DEPLOYING),
        "case \"update during a deployment\": an environment with a deployment in flight is \
         Deploying, got {:?}",
        during.string("State")
    );
    assert_round_trip(
        &router,
        "update during a deployment",
        &application,
        &environment,
        &expected,
    )
    .await;
}
