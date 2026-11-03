// SPDX-License-Identifier: CC0-1.0

//! What a successful CodeDeploy answer carries for an output member whose value
//! the call already knows, driven over the wire.
//!
//! RULE under test: a success SERIALIZES every output member whose value the
//! call knows, the known-empty value included, rather than leaving the member
//! out — so a client reads one member instead of telling absent from empty:
//!
//! * a batch reader's batch-level `errorMessage`
//!   (`smithy_query(query="shape",shape="BatchGetApplicationRevisionsOutput")`
//!   and `...shape="BatchGetDeploymentInstancesOutput"`: "Information about
//!   errors that might have occurred during the API call") is the EMPTY string
//!   on a batch no part of which failed;
//! * the `hooksNotCleanedUp` list of the two deployment-group writers that
//!   report it (`smithy_query(query="shape",shape="DeleteDeploymentGroupOutput")`
//!   and `...shape="UpdateDeploymentGroupOutput"`) is the EMPTY list when no
//!   Auto Scaling lifecycle hook was left behind;
//! * a deployment group reports its bake time and its managed-hook
//!   configuration (`DeploymentGroupInfo$bakeTime`, `$managedHookConfig`)
//!   whether or not it configured either: no bake time is reported as no wait
//!   at all, and a group with no managed-hook execution role reports the
//!   sentinel CodeDeploy answers with.
//!
//! The rows of each table differ along what the rule generalises over — the
//! SHAPE and SIZE of the batch, which members the group was created with, and
//! the compute platform of the application — and every case goes through the
//! same front door a client uses, the shared [`Emulator`] harness of
//! [`super::codedeploy_lifecycle`].

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006 as codedeploy;

use crate::codedeploy_lifecycle::{s3_revision, Emulator};
use crate::deployment_targets::deployable;
use crate::list_deployment_targets::{deploy, group_with, register};

/// The service role every deployment group here is created with.
const ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The value a deployment group's managed-hook execution role is reported as
/// when the group carries none: the sentinel the recorded service answers with,
/// not an absent member.
const NO_MANAGED_HOOK_ROLE: &str = "NONE";

/// The batch-level no-error text: the member is present and empty whenever the
/// batch itself was answered.
const NO_BATCH_ERROR: &str = "";

/// Assert that `body` reports the no-error `errorMessage`, as a string rather
/// than an absent member.
fn assert_no_batch_error(body: &Value, case: &str) {
    assert_eq!(
        body["errorMessage"], NO_BATCH_ERROR,
        "case {case:?}: expected errorMessage {NO_BATCH_ERROR:?}, got {:?} in {body:?}",
        body["errorMessage"]
    );
    assert!(
        body["errorMessage"].is_string(),
        "case {case:?}: expected errorMessage to be reported as a string, got {:?} in {body:?}",
        body["errorMessage"]
    );
}

/// Assert that `body` reports `hooksNotCleanedUp` as the empty list.
fn assert_no_hooks_left(body: &Value, case: &str) {
    assert_eq!(
        body["hooksNotCleanedUp"],
        json!([]),
        "case {case:?}: expected hooksNotCleanedUp [], got {:?} in {body:?}",
        body["hooksNotCleanedUp"]
    );
}

/// RULE: a `BatchGetApplicationRevisions` that was answered reports the
/// batch-level `errorMessage` as the empty string.
///
/// Rows differ along the SHAPE and SIZE of the batch — one revision, several, a
/// repeated one, one the application never registered, a mix, and a batch right
/// at the request-size quota — which is what the rule generalises over: a batch
/// that was answered carries the no-error text whatever it asked for.
#[tokio::test]
async fn an_answered_revision_batch_reports_the_no_error_message() {
    let emulator = Emulator::new();
    let application = "batch-error-message";
    emulator
        .ok("CreateApplication", json!({"applicationName": application}))
        .await;

    let registered = s3_revision("artifacts", "app/one.zip");
    let also_registered = s3_revision("artifacts", "app/two.zip");
    for revision in [&registered, &also_registered] {
        emulator
            .ok(
                "RegisterApplicationRevision",
                json!({"applicationName": application, "revision": revision}),
            )
            .await;
    }
    let unregistered = s3_revision("artifacts", "app/never-registered.zip");
    // The largest batch this service answers, from the quota its
    // `BatchLimitExceededException` states (`codedeploy::state::MAX_BATCH_SIZE`).
    let at_the_quota: Vec<Value> = (0..codedeploy::state::MAX_BATCH_SIZE)
        .map(|index| s3_revision("artifacts", &format!("app/bulk-{index:02}.zip")))
        .collect();

    let cases: &[(&str, Vec<Value>)] = &[
        ("one registered revision", vec![registered.clone()]),
        (
            "two registered revisions",
            vec![registered.clone(), also_registered.clone()],
        ),
        (
            "the same revision twice",
            vec![registered.clone(), registered.clone()],
        ),
        ("a revision never registered", vec![unregistered.clone()]),
        (
            "a registered and an unregistered revision",
            vec![registered.clone(), unregistered.clone()],
        ),
        ("a batch at the request-size quota", at_the_quota),
    ];

    for (case, revisions) in cases {
        let body = emulator
            .ok(
                "BatchGetApplicationRevisions",
                json!({"applicationName": application, "revisions": revisions}),
            )
            .await;
        assert_no_batch_error(&body, case);
        // The answer still carries one entry per revision asked about, so the
        // no-error text is not standing in for a batch that answered nothing.
        let answered = body["revisions"].as_array().map(Vec::len).unwrap_or_default();
        assert_eq!(
            answered,
            revisions.len(),
            "case {case:?}: expected {} revision entries, got {answered} in {body:?}",
            revisions.len()
        );
    }
}

/// RULE: the other batch reader of the same member,
/// `BatchGetDeploymentInstances`, reports it the same way.
///
/// Rows differ along the SHAPE of the batch over one deployment's instances:
/// every instance, a subset, reordered, repeated, and mixed with an id the
/// deployment holds nothing under.
#[tokio::test]
async fn an_answered_instance_batch_reports_the_no_error_message() {
    let emulator = Emulator::new();
    let application = "batch-instance-error-message";
    for instance in ["host-one", "host-two"] {
        register(&emulator, instance, &[("Fleet", "deployable")]).await;
    }
    register(&emulator, "host-three", &[("Fleet", "retired")]).await;
    group_with(&emulator, application, "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, application).await;

    let cases: &[(&str, &[&str])] = &[
        ("every resolved instance", &["host-one", "host-two"]),
        ("one of them", &["host-two"]),
        ("reordered", &["host-two", "host-one"]),
        ("the same instance twice", &["host-one", "host-one"]),
        (
            "mixed with an instance the deployment holds nothing under",
            &["host-one", "host-three"],
        ),
    ];

    for (case, instance_ids) in cases {
        let body = emulator
            .ok(
                "BatchGetDeploymentInstances",
                json!({"deploymentId": deployment_id, "instanceIds": instance_ids}),
            )
            .await;
        assert_no_batch_error(&body, case);
        assert!(
            body["instancesSummary"].is_array(),
            "case {case:?}: expected instancesSummary to be reported as a list, got {body:?}"
        );
    }
}

/// One deployment group to write and read: the case name, the application's
/// compute platform, and the members `CreateDeploymentGroup` is given beyond the
/// name and the service role.
type GroupCase<'a> = (&'a str, &'a str, Value);

/// The group rows every table below spans: the platform and the members a group
/// was created with are what the rules generalise over.
fn group_cases() -> Vec<GroupCase<'static>> {
    vec![
        ("nothing beyond the required members, Server", "Server", json!({})),
        ("nothing beyond the required members, Lambda", "Lambda", json!({})),
        ("nothing beyond the required members, ECS", "ECS", json!({})),
        (
            "on-premises tag filters",
            "Server",
            json!({"onPremisesInstanceTagFilters": [{"Key": "Fleet", "Value": "blue", "Type": "KEY_AND_VALUE"}]}),
        ),
        (
            "EC2 tag filters and a trigger",
            "Server",
            json!({
                "ec2TagFilters": [{"Key": "Stage", "Value": "prod", "Type": "KEY_AND_VALUE"}],
                "triggerConfigurations": [{
                    "triggerName": "all-events",
                    "triggerTargetArn": "arn:aws:sns:us-east-1:123456789012:codedeploy",
                    "triggerEvents": ["DeploymentSuccess"],
                }],
            }),
        ),
        (
            "Auto Scaling groups",
            "Server",
            json!({"autoScalingGroups": ["asg-one", "asg-two"]}),
        ),
        (
            "a blue/green style with traffic control",
            "Server",
            json!({
                "deploymentStyle": {
                    "deploymentType": "BLUE_GREEN",
                    "deploymentOption": "WITH_TRAFFIC_CONTROL",
                },
            }),
        ),
    ]
}

/// Create the application and the deployment group of one row, answering the
/// group's name.
async fn seed_group(emulator: &Emulator, application: &str, platform: &str, extras: &Value) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let group = format!("{application}-dg");
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": ROLE,
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emulator.ok("CreateDeploymentGroup", request).await;
    group
}

/// RULE: both writers that report `hooksNotCleanedUp` answer it as the empty
/// list when no Auto Scaling lifecycle hook was left behind — whatever the group
/// they acted on carried.
#[tokio::test]
async fn a_group_write_reports_an_empty_left_behind_hook_list() {
    let emulator = Emulator::new();
    for (index, (case, platform, extras)) in group_cases().into_iter().enumerate() {
        let application = format!("hook-list-{index:02}");
        let group = seed_group(&emulator, &application, platform, &extras).await;

        let updated = emulator
            .ok(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "serviceRoleArn": ROLE,
                }),
            )
            .await;
        assert_no_hooks_left(&updated, case);

        let deleted = emulator
            .ok(
                "DeleteDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_no_hooks_left(&deleted, case);

        // Deleting a group nothing holds is the same success with the same
        // member: the empty list is about hooks, not about what was removed.
        let again = emulator
            .ok(
                "DeleteDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_no_hooks_left(&again, case);
    }
}

/// RULE: every deployment group a read answers reports its bake time and its
/// managed-hook configuration — a group that configured no bake time reports no
/// wait at all, and a group with no managed-hook execution role reports the
/// sentinel for "none" — and it keeps reporting them after an update.
#[tokio::test]
async fn every_deployment_group_reports_its_bake_time_and_managed_hook_config() {
    let emulator = Emulator::new();

    /// The `deploymentGroupInfo` `GetDeploymentGroup` answers.
    async fn read(emulator: &Emulator, application: &str, group: &str) -> Value {
        emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await["deploymentGroupInfo"]
            .clone()
    }

    fn assert_reported(info: &Value, case: &str) {
        assert_eq!(
            info["bakeTime"],
            json!(codedeploy::state::NO_BAKE_TIME),
            "case {case:?}: expected bakeTime {}, got {:?} in {info:?}",
            codedeploy::state::NO_BAKE_TIME,
            info["bakeTime"]
        );
        assert!(
            info["bakeTime"].is_number(),
            "case {case:?}: expected bakeTime to be reported as a number, got {info:?}"
        );
        assert_eq!(
            info["managedHookConfig"],
            json!({"executionRoleArn": NO_MANAGED_HOOK_ROLE}),
            "case {case:?}: expected managedHookConfig {{executionRoleArn: \
             {NO_MANAGED_HOOK_ROLE:?}}}, got {:?} in {info:?}",
            info["managedHookConfig"]
        );
    }

    for (index, (case, platform, extras)) in group_cases().into_iter().enumerate() {
        let application = format!("bake-time-{index:02}");
        let group = seed_group(&emulator, &application, platform, &extras).await;

        let created = read(&emulator, &application, &group).await;
        assert_reported(&created, case);

        // A renaming update rewrites the stored group; the two members survive
        // it, because they are carried on the group and not minted by the read.
        let renamed = format!("{group}-renamed");
        emulator
            .ok(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "newDeploymentGroupName": renamed,
                }),
            )
            .await;
        let after_update = read(&emulator, &application, &renamed).await;
        assert_reported(&after_update, case);
    }
}
