// SPDX-License-Identifier: CC0-1.0

//! `GetDeploymentInstance` over the wire.
//!
//! RULE: the call answers the `InstanceSummary` of the one instance target the
//! deployment's own record holds under the id the request names — the snapshot
//! its deployment group resolved when it was created — carrying the deployment's
//! id, that instance's id, the `InstanceStatus` the deployment's own clock-owed
//! progress gives it, the instant it last moved, the environment it belongs to
//! for a blue/green deployment and the lifecycle event results reported against
//! the deployment; a deployment on a compute platform that has no instances is
//! refused with `InvalidComputePlatformException` and an id the deployment holds
//! no instance under with `InstanceDoesNotExistException`.
//!
//! This is the deprecated, instance-shaped view of the records
//! `GetDeploymentTarget` answers ("This operation is deprecated, use
//! GetDeploymentTarget instead"), so the cases here pin the two readers against
//! each other member for member rather than restating what
//! [`super::deployment_targets`] and [`super::get_deployment_target`] already
//! pin about the records themselves.
//!
//! The harness is the one [`super::codedeploy_lifecycle`] drives the rest of this
//! service with, and the fleet helpers are [`super::list_deployment_targets`]';
//! only the cases here are new.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006 as codedeploy;

use crate::codedeploy_lifecycle::{error_code, terminal_deployment, Emulator};
use crate::deployment_targets::deployable;
use crate::list_deployment_targets::{deploy, force_status, group_with, register};

/// The same on-premises selector, deployed blue/green — the only deployment
/// style whose instances belong to an environment at all.
fn deployable_blue_green() -> Value {
    let mut extras = deployable();
    extras["deploymentStyle"] =
        json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"});
    extras
}

/// The `instanceSummary` of a successful `GetDeploymentInstance` answer.
async fn summary_of(emulator: &Emulator, deployment_id: &str, instance_id: &str) -> Value {
    emulator
        .ok(
            "GetDeploymentInstance",
            json!({"deploymentId": deployment_id, "instanceId": instance_id}),
        )
        .await["instanceSummary"]
        .clone()
}

/// The `instanceTarget` the newer `GetDeploymentTarget` answers for the same
/// record, which is what this operation is the older view of.
async fn instance_target_of(emulator: &Emulator, deployment_id: &str, target_id: &str) -> Value {
    emulator
        .ok(
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": target_id}),
        )
        .await["deploymentTarget"]["instanceTarget"]
        .clone()
}

/// The `instancesList` the deprecated listing answers for the same deployment.
async fn listed_instances(emulator: &Emulator, deployment_id: &str) -> Vec<String> {
    emulator
        .ok(
            "ListDeploymentInstances",
            json!({"deploymentId": deployment_id}),
        )
        .await["instancesList"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// What an instance reads back as
// ---------------------------------------------------------------------------

/// RULE: the answered instance is the deployment's own target record, projected
/// into `InstanceSummary` — so every member it shares with the `InstanceTarget`
/// the newer reader answers for that record carries the same value, for every
/// instance the deployment resolved.
#[tokio::test]
async fn an_instance_reads_back_the_same_way_as_the_target_reader() {
    // Rows differ along the deployment style (so along the environment an
    // instance belongs to), along how many of the registered instances the
    // group's selector takes in, and along the FORM of the instance names —
    // every character class the service's resource-name set admits.
    struct Case<'a> {
        case: &'a str,
        application: &'a str,
        extras: Value,
        fleet: &'a [(&'a str, &'a [(&'a str, &'a str)])],
        /// The instance ids expected, in resolution order.
        expected: &'a [&'a str],
        /// The `InstanceType` each answered instance carries, if any.
        expected_type: Option<&'a str>,
    }
    let mixed_fleet: &[(&str, &[(&str, &str)])] = &[
        ("host-a", &[("Fleet", "deployable")]),
        ("host-b", &[("Fleet", "deployable")]),
        ("host-c", &[("Fleet", "retired")]),
    ];
    // Names spanning the service's resource-name character set
    // (`smithy_query(query="shape",shape="ApplicationName")`: pattern
    // `^[A-Za-z0-9+=,.@_-]*$`), none of them the one the issue cited.
    let varied_fleet: &[(&str, &[(&str, &str)])] = &[
        ("web.01", &[("Fleet", "deployable")]),
        ("HOST_B", &[("Fleet", "deployable")]),
        ("node+1=2,3@eu", &[("Fleet", "deployable")]),
    ];
    let single_fleet: &[(&str, &[(&str, &str)])] = &[("solo", &[("Fleet", "deployable")])];
    let cases: &[Case] = &[
        Case {
            case: "an in-place deployment to two of three registered instances",
            application: "in-place-app",
            extras: deployable(),
            fleet: mixed_fleet,
            expected: &["host-a", "host-b"],
            expected_type: None,
        },
        Case {
            case: "a blue/green deployment to two of three registered instances",
            application: "blue-green-app",
            extras: deployable_blue_green(),
            fleet: mixed_fleet,
            // A blue/green deployment's resolved instances are the ORIGINAL
            // environment, which `InstanceType` calls Blue
            // (`smithy_query(query="shape",shape="InstanceType")`: Blue | Green).
            expected: &["host-a", "host-b"],
            expected_type: Some("Blue"),
        },
        Case {
            case: "an in-place deployment to instances whose names span the name set",
            application: "varied-names-app",
            extras: deployable(),
            fleet: varied_fleet,
            expected: &["web.01", "HOST_B", "node+1=2,3@eu"],
            expected_type: None,
        },
        Case {
            case: "a blue/green deployment to one instance",
            application: "single-instance-app",
            extras: deployable_blue_green(),
            fleet: single_fleet,
            expected: &["solo"],
            expected_type: Some("Blue"),
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        for (instance, tags) in case.fleet {
            register(&emulator, instance, tags).await;
        }
        group_with(&emulator, case.application, "Server", &case.extras).await;
        let deployment_id = deploy(&emulator, case.application).await;
        // The deployment is held mid-flight so the two readers are compared on
        // one state of one record, whatever the machine's own timing: what the
        // clock owes the record is the next case's subject.
        force_status(&emulator, &deployment_id, "InProgress", true);

        assert_eq!(
            listed_instances(&emulator, &deployment_id).await,
            case.expected,
            "case {:?}: expected the deployment to have resolved {:?}",
            case.case,
            case.expected
        );

        for instance_id in case.expected {
            let summary = summary_of(&emulator, &deployment_id, instance_id).await;
            let target = instance_target_of(&emulator, &deployment_id, instance_id).await;
            assert_eq!(
                summary["deploymentId"].as_str(),
                Some(deployment_id.as_str()),
                "case {:?}: expected {instance_id} to name the deployment it belongs to, got \
                 {summary:?}",
                case.case
            );
            assert_eq!(
                summary["instanceId"].as_str(),
                Some(*instance_id),
                "case {:?}: expected the instance asked for, got {summary:?}",
                case.case
            );
            assert_eq!(
                summary["status"].as_str(),
                target["status"].as_str(),
                "case {:?}: expected {instance_id} to carry the status the target reader \
                 answers ({:?}), got {summary:?}",
                case.case,
                target["status"]
            );
            assert_eq!(
                summary["lastUpdatedAt"].as_f64(),
                target["lastUpdatedAt"].as_f64(),
                "case {:?}: expected {instance_id} to carry the instant the target reader \
                 answers ({:?}), got {summary:?}",
                case.case,
                target["lastUpdatedAt"]
            );
            assert_eq!(
                summary["instanceType"].as_str(),
                case.expected_type,
                "case {:?}: expected {instance_id} to belong to {:?}, got {summary:?}",
                case.case,
                case.expected_type
            );
            assert_eq!(
                summary["instanceType"].as_str(),
                target["instanceLabel"].as_str(),
                "case {:?}: expected {instance_id} to carry the environment the target reader \
                 answers ({:?}), got {summary:?}",
                case.case,
                target["instanceLabel"]
            );
            // EC2/on-premises lifecycle events are run by the CodeDeploy agent
            // rather than by a validation function this service holds a report
            // from, so neither reader has an event to answer for such an
            // instance — and whatever either holds, they hold the same.
            assert_eq!(
                summary["lifecycleEvents"],
                target["lifecycleEvents"],
                "case {:?}: expected {instance_id} to carry the lifecycle events the target \
                 reader answers, got {summary:?}",
                case.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The status an instance carries
// ---------------------------------------------------------------------------

/// RULE: an instance's `InstanceStatus` is the one its deployment's own progress
/// implies — pending until the deployment has started, in progress while it
/// runs, and the outcome the deployment ended with once it has, with a stopped
/// deployment's instances skipped rather than failed.
#[tokio::test]
async fn an_instances_status_is_the_one_its_deployments_progress_implies() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    group_with(&emulator, "status-app", "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, "status-app").await;

    // Rows span the model's whole `DeploymentStatus` enum
    // (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued
    // | InProgress | Baking | Succeeded | Failed | Stopped | Ready), including
    // the statuses this emulator reaches by no call of its own, and both sides
    // of whether the deployment has started: `(case, deployment status,
    // started, expected instance status)`.
    let cases: &[(&str, &str, bool, &str)] = &[
        ("a deployment the clock has not started", "Created", false, "Pending"),
        ("a queued deployment", "Queued", false, "Pending"),
        ("a deployment under way", "InProgress", true, "InProgress"),
        ("a deployment baking", "Baking", true, "InProgress"),
        ("a deployment ready for traffic", "Ready", true, "InProgress"),
        ("a deployment that succeeded", "Succeeded", true, "Succeeded"),
        ("a deployment that failed", "Failed", true, "Failed"),
        // A caller stopped the deployment, so the revision was never finished
        // on its instances: skipped, not failed.
        ("a deployment a caller stopped", "Stopped", true, "Skipped"),
    ];

    for (case, deployment_status, started, expected) in cases {
        force_status(&emulator, &deployment_id, deployment_status, *started);
        let summary = summary_of(&emulator, &deployment_id, "host-a").await;
        assert_eq!(
            summary["status"].as_str(),
            Some(*expected),
            "case {case:?}: expected the instance status {expected}, got {summary:?}"
        );
        assert!(
            codedeploy::ops::types::INSTANCE_STATUS_VALUES.contains(expected),
            "case {case:?}: expected {expected} to be a modelled InstanceStatus"
        );
        assert!(
            summary["lastUpdatedAt"].as_f64().is_some(),
            "case {case:?}: expected the instant the instance last moved, got {summary:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a request this operation cannot answer is refused with the modelled
/// error the model gives it for that cause, and HTTP 400 — the id's FORM before
/// the lookup, and whether the deployment reports instances at all before
/// whether it holds this one.
#[tokio::test]
async fn a_request_that_cannot_be_answered_is_refused_with_its_modelled_error() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;

    group_with(&emulator, "refusal-app", "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, "refusal-app").await;
    terminal_deployment(&emulator, &deployment_id, "refusals").await;

    group_with(
        &emulator,
        "refusal-ecs-app",
        "ECS",
        &json!({"ecsServices": [{"clusterName": "prod", "serviceName": "web"}]}),
    )
    .await;
    let ecs_deployment_id = deploy(&emulator, "refusal-ecs-app").await;
    terminal_deployment(&emulator, &ecs_deployment_id, "refusals").await;

    group_with(&emulator, "refusal-lambda-app", "Lambda", &json!({})).await;
    let lambda_deployment_id = deploy(&emulator, "refusal-lambda-app").await;
    terminal_deployment(&emulator, &lambda_deployment_id, "refusals").await;

    // A name one character past the longest the service accepts
    // (`state::MAX_NAME_LENGTH`).
    let too_long = "n".repeat(codedeploy::state::MAX_NAME_LENGTH + 1);
    // Rows differ along WHICH part of the request is unanswerable: the
    // deployment id (absent, malformed, or naming no deployment), the instance
    // id (absent, malformed, or naming no instance of the deployment), and the
    // compute platform the deployment is on.
    let cases: &[(&str, Value, &str)] = &[
        (
            "an empty deployment id",
            json!({"deploymentId": "", "instanceId": "host-a"}),
            "DeploymentIdRequiredException",
        ),
        (
            "a blank deployment id",
            json!({"deploymentId": "   ", "instanceId": "host-a"}),
            "DeploymentIdRequiredException",
        ),
        (
            "a deployment id of no recognisable form",
            json!({"deploymentId": "not-an-id", "instanceId": "host-a"}),
            "InvalidDeploymentIdException",
        ),
        (
            "a deployment id whose suffix is lowercase",
            json!({"deploymentId": "d-abcdefghi", "instanceId": "host-a"}),
            "InvalidDeploymentIdException",
        ),
        (
            "a well-formed deployment id no deployment has",
            json!({"deploymentId": "d-ZZZZZZZZZ", "instanceId": "host-a"}),
            "DeploymentDoesNotExistException",
        ),
        (
            "an empty instance id",
            json!({"deploymentId": deployment_id, "instanceId": ""}),
            "InstanceIdRequiredException",
        ),
        (
            "a blank instance id",
            json!({"deploymentId": deployment_id, "instanceId": "  "}),
            "InstanceIdRequiredException",
        ),
        (
            "an instance id outside the service's name set",
            json!({"deploymentId": deployment_id, "instanceId": "prod:web"}),
            "InvalidInstanceNameException",
        ),
        (
            "an instance id one character too long",
            json!({"deploymentId": deployment_id, "instanceId": too_long}),
            "InvalidInstanceNameException",
        ),
        (
            "a well-formed instance id the deployment resolved no instance under",
            json!({"deploymentId": deployment_id, "instanceId": "host-z"}),
            "InstanceDoesNotExistException",
        ),
        // The platform the model's own sentence excludes: an Amazon ECS
        // deployment is a perfectly good deployment that simply has no
        // instances.
        (
            "an Amazon ECS deployment, which has no instances",
            json!({"deploymentId": ecs_deployment_id, "instanceId": "host-a"}),
            "InvalidComputePlatformException",
        ),
        // A Lambda deployment DOES report instances, but its one target is the
        // function its revision's AppSpec names, which CodeDeploy's state holds
        // no content of — so no id reaches it.
        (
            "a Lambda deployment, whose one instance is named inside the revision",
            json!({"deploymentId": lambda_deployment_id, "instanceId": "host-a"}),
            "InstanceDoesNotExistException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("GetDeploymentInstance", request.clone())
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case:?}: expected a 400 {expected}, got {status} {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected the refusal to carry a message, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Reading changes nothing
// ---------------------------------------------------------------------------

/// RULE: reading an instance reports the stored record and moves nothing — two
/// reads of one state answer the same summary, and what every other reader of
/// that deployment answers is unchanged by the read.
#[tokio::test]
async fn reading_an_instance_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    register(&emulator, "host-b", &[("Fleet", "deployable")]).await;
    group_with(&emulator, "stable-app", "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, "stable-app").await;
    // Read on a deployment the clock has nothing left to owe, so a difference
    // between two reads can only be this operation's doing.
    let before = terminal_deployment(&emulator, &deployment_id, "stable reads").await;
    let listed_before = listed_instances(&emulator, &deployment_id).await;

    let first = summary_of(&emulator, &deployment_id, "host-a").await;
    // Reading one instance must not move another, so both are read in between.
    let other = summary_of(&emulator, &deployment_id, "host-b").await;
    let second = summary_of(&emulator, &deployment_id, "host-a").await;
    assert_eq!(
        first, second,
        "case \"two reads of one instance\": expected the same summary, got {first:?} then \
         {second:?}"
    );
    assert_eq!(
        other["status"].as_str(),
        first["status"].as_str(),
        "case \"the other instance of the deployment\": expected the same status as {first:?}, \
         got {other:?}"
    );

    let after = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await["deploymentInfo"]
        .clone();
    assert_eq!(
        after["status"].as_str(),
        before["status"].as_str(),
        "case \"the deployment behind the instance\": expected the read to leave its status at \
         {:?}, got {after:?}",
        before["status"]
    );
    assert_eq!(
        listed_instances(&emulator, &deployment_id).await,
        listed_before,
        "case \"the listing behind the instance\": expected the read to leave it at \
         {listed_before:?}"
    );
}
