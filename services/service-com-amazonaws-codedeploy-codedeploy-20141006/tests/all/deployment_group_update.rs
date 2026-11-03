// SPDX-License-Identifier: CC0-1.0

//! `UpdateDeploymentGroup`, driven over the wire through the same front door a
//! client uses (the harness in [`crate::codedeploy_lifecycle`]).
//!
//! The rule every case here checks: an update REPLACES exactly the members the
//! request names — a non-null empty list clears the one it names — KEEPS every
//! member the request omits, and leaves the group's identity
//! (`deploymentGroupId`) alone; what `GetDeploymentGroup` answers afterwards is
//! the stored record. A refused update changes nothing at all.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, s3_revision, Emulator};

/// The deployment group name every case starts from.
const GROUP: &str = "dg";

/// A deployment group carrying one value for every member an update can
/// replace, so a case can check both what it asked to change and what it did
/// not.
fn baseline(application: &str) -> Value {
    json!({
        "applicationName": application,
        "deploymentGroupName": GROUP,
        "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployOriginal",
        "deploymentConfigName": "CodeDeployDefault.OneAtATime",
        "ec2TagFilters": [{"Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE"}],
        "onPremisesInstanceTagFilters": [
            {"Key": "Stage", "Value": "prod", "Type": "KEY_AND_VALUE"}
        ],
        "autoScalingGroups": ["asg-original"],
        "triggerConfigurations": [{
            "triggerName": "trigger-original",
            "triggerTargetArn": "arn:aws:sns:us-east-1:123456789012:codedeploy",
            "triggerEvents": ["DeploymentStart"],
        }],
        "alarmConfiguration": {"enabled": true, "alarms": [{"name": "alarm-original"}]},
        "autoRollbackConfiguration": {"enabled": true, "events": ["DEPLOYMENT_FAILURE"]},
        "outdatedInstancesStrategy": "UPDATE",
        "terminationHookEnabled": true,
    })
}

/// Create `application` with a deployment group built from [`baseline`].
async fn seed_baseline_group(emulator: &Emulator, application: &str) {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": "Server"}),
        )
        .await;
    emulator
        .ok("CreateDeploymentGroup", baseline(application))
        .await;
}

/// An `UpdateDeploymentGroup` request for `group` of `application`, with
/// `patch`'s members written over it — so a case can also override the two
/// required names themselves.
fn update_request(application: &str, group: &str, patch: &Value) -> Value {
    let mut body = json!({
        "applicationName": application,
        "currentDeploymentGroupName": group,
    });
    for (name, value) in patch.as_object().into_iter().flatten() {
        body[name] = value.clone();
    }
    body
}

/// The `deploymentGroupInfo` `GetDeploymentGroup` answers for `group`.
async fn read_group(emulator: &Emulator, application: &str, group: &str) -> Value {
    emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await["deploymentGroupInfo"]
        .clone()
}

/// The value at a dotted `path` through a response: a numeric segment indexes a
/// list, anything else names a member. A member that is not there reads as
/// `null`.
fn member<'a>(body: &'a Value, path: &str) -> &'a Value {
    let mut cursor = body;
    for segment in path.split('.') {
        cursor = match segment.parse::<usize>() {
            Ok(index) => &cursor[index],
            Err(_) => &cursor[segment],
        };
    }
    cursor
}

/// The `DeploymentGroupInfo` members an update carrying `patch` is ALLOWED to
/// change: the ones it names, plus the alternative form of a tag filter list
/// (a group is described by either a filter list or a tag set, never both — see
/// `InvalidEC2TagCombinationException`).
fn changeable_members(patch: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in patch.as_object().into_iter().flatten().map(|(name, _)| name) {
        match name.as_str() {
            // The two required names identify the group; neither replaces a
            // member of it.
            "applicationName" | "currentDeploymentGroupName" => {}
            "newDeploymentGroupName" => names.push("deploymentGroupName".to_string()),
            "ec2TagFilters" | "ec2TagSet" => {
                names.push("ec2TagFilters".to_string());
                names.push("ec2TagSet".to_string());
            }
            "onPremisesInstanceTagFilters" | "onPremisesTagSet" => {
                names.push("onPremisesInstanceTagFilters".to_string());
                names.push("onPremisesTagSet".to_string());
            }
            other => names.push(other.to_string()),
        }
    }
    names
}

/// One row of an update table: the case name, the members the update names, and
/// the response paths the stored group must answer afterwards.
type UpdateCase<'a> = (&'a str, Value, &'a [(&'a str, Value)]);

/// An alarm configuration carrying `count` distinctly named alarms.
fn alarm_configuration(count: usize) -> Value {
    let alarms: Vec<Value> = (0..count)
        .map(|index| json!({"name": format!("alarm-{index}")}))
        .collect();
    json!({"enabled": true, "alarms": alarms})
}

/// `count` tag filters, for the per-list bound. The shape is the same for an
/// `EC2TagFilter` and a `TagFilter`: both carry `Key` / `Value` / `Type`.
fn ec2_tag_filters(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": "on", "Type": "KEY_AND_VALUE"}))
            .collect(),
    )
}

/// An `ec2TagSet` carrying `count` tag groups.
fn ec2_tag_set(count: usize) -> Value {
    let groups: Vec<Value> = (0..count)
        .map(|index| json!([{"Key": format!("group-{index}"), "Value": "on", "Type": "KEY_AND_VALUE"}]))
        .collect();
    json!({"ec2TagSetList": groups})
}

/// An `onPremisesTagSet` carrying `count` tag groups.
fn on_premises_tag_set(count: usize) -> Value {
    let groups: Vec<Value> = (0..count)
        .map(|index| json!([{"Key": format!("group-{index}"), "Value": "on", "Type": "KEY_AND_VALUE"}]))
        .collect();
    json!({"onPremisesTagSetList": groups})
}

// ---------------------------------------------------------------------------
// The replacement rule
// ---------------------------------------------------------------------------

/// RULE: an update replaces exactly the members it names — a non-null empty
/// list clears the one it names — and keeps every member it does not name.
///
/// Each row is one update over the same baseline group. Besides the row's own
/// expectations, EVERY member the row's request does not name is required to
/// come back byte-identical to what it was before the call, which is the half of
/// the rule a per-member assertion cannot state.
#[tokio::test]
async fn an_update_replaces_only_the_members_it_names() {
    let emulator = Emulator::new();
    // The rows differ along what the rule generalises over: which member is
    // replaced, and of what kind — a list (replaced, grown, cleared), a string,
    // an enum, a boolean, a structure, a nested list-of-lists — and whether the
    // member had a value before.
    let cases: &[UpdateCase<'_>] = &[
        (
            "a list member replaced in place",
            json!({"ec2TagFilters": [{"Key": "Name", "Value": "api", "Type": "KEY_AND_VALUE"}]}),
            &[("ec2TagFilters.0.Value", json!("api"))],
        ),
        (
            "a list member grown to two entries",
            json!({"ec2TagFilters": [
                {"Key": "Name", "Value": "api", "Type": "KEY_AND_VALUE"},
                {"Key": "Tier", "Type": "KEY_ONLY"},
            ]}),
            &[
                ("ec2TagFilters.1.Key", json!("Tier")),
                ("ec2TagFilters.1.Type", json!("KEY_ONLY")),
            ],
        ),
        (
            "a list member cleared by a non-null empty list",
            json!({"ec2TagFilters": []}),
            // A selection list the group holds is reported as the list it holds,
            // empty included, so a cleared one reads back as `[]`.
            &[("ec2TagFilters", json!([]))],
        ),
        (
            "the on-premises filter list replaced",
            json!({"onPremisesInstanceTagFilters": [
                {"Key": "Stage", "Value": "staging", "Type": "KEY_AND_VALUE"},
                {"Key": "Owner", "Value": "team", "Type": "KEY_AND_VALUE"},
            ]}),
            &[
                ("onPremisesInstanceTagFilters.0.Value", json!("staging")),
                ("onPremisesInstanceTagFilters.1.Key", json!("Owner")),
            ],
        ),
        (
            "the on-premises filter list cleared",
            json!({"onPremisesInstanceTagFilters": []}),
            &[("onPremisesInstanceTagFilters", json!([]))],
        ),
        (
            "the Auto Scaling groups replaced",
            json!({"autoScalingGroups": ["asg-first", "asg-second"]}),
            &[
                ("autoScalingGroups.0.name", json!("asg-first")),
                ("autoScalingGroups.1.name", json!("asg-second")),
            ],
        ),
        (
            "the Auto Scaling groups detached by a non-null empty list",
            json!({"autoScalingGroups": []}),
            &[("autoScalingGroups", json!([]))],
        ),
        (
            "a string member replaced",
            json!({"serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployReplacement"}),
            &[(
                "serviceRoleArn",
                json!("arn:aws:iam::123456789012:role/CodeDeployReplacement"),
            )],
        ),
        (
            "the deployment configuration replaced",
            json!({"deploymentConfigName": "CodeDeployDefault.HalfAtATime"}),
            &[(
                "deploymentConfigName",
                json!("CodeDeployDefault.HalfAtATime"),
            )],
        ),
        (
            "an enum member replaced",
            json!({"outdatedInstancesStrategy": "IGNORE"}),
            &[("outdatedInstancesStrategy", json!("IGNORE"))],
        ),
        (
            "a boolean member replaced",
            json!({"terminationHookEnabled": false}),
            &[("terminationHookEnabled", json!(false))],
        ),
        (
            "a structure member cleared by an information-free value",
            json!({"autoRollbackConfiguration": {"enabled": false, "events": []}}),
            // A disabled rollback listing no events says nothing an omitted
            // member does not, so the store normalises it away — the same
            // normalisation `CreateDeploymentGroup` applies.
            &[("autoRollbackConfiguration", Value::Null)],
        ),
        (
            "a structure member replaced",
            json!({"autoRollbackConfiguration": {
                "enabled": true,
                "events": ["DEPLOYMENT_STOP_ON_ALARM", "DEPLOYMENT_STOP_ON_REQUEST"],
            }}),
            &[
                ("autoRollbackConfiguration.enabled", json!(true)),
                (
                    "autoRollbackConfiguration.events.1",
                    json!("DEPLOYMENT_STOP_ON_REQUEST"),
                ),
            ],
        ),
        (
            "the alarm configuration replaced",
            json!({"alarmConfiguration": {
                "enabled": true,
                "ignorePollAlarmFailure": true,
                "alarms": [{"name": "alarm-first"}, {"name": "alarm-second"}],
            }}),
            &[
                ("alarmConfiguration.alarms.0.name", json!("alarm-first")),
                ("alarmConfiguration.alarms.1.name", json!("alarm-second")),
                ("alarmConfiguration.ignorePollAlarmFailure", json!(true)),
            ],
        ),
        (
            "the triggers replaced",
            json!({"triggerConfigurations": [{
                "triggerName": "trigger-replacement",
                "triggerTargetArn": "arn:aws:sns:us-east-1:123456789012:other",
                "triggerEvents": ["DeploymentSuccess", "DeploymentRollback"],
            }]}),
            &[
                (
                    "triggerConfigurations.0.triggerName",
                    json!("trigger-replacement"),
                ),
                (
                    "triggerConfigurations.0.triggerEvents.1",
                    json!("DeploymentRollback"),
                ),
            ],
        ),
        (
            "a member the group did not have before",
            json!({"deploymentStyle": {
                "deploymentType": "BLUE_GREEN",
                "deploymentOption": "WITH_TRAFFIC_CONTROL",
            }}),
            &[
                ("deploymentStyle.deploymentType", json!("BLUE_GREEN")),
                (
                    "deploymentStyle.deploymentOption",
                    json!("WITH_TRAFFIC_CONTROL"),
                ),
            ],
        ),
        (
            "the ECS services added",
            json!({"ecsServices": [{"serviceName": "svc", "clusterName": "cluster"}]}),
            &[
                ("ecsServices.0.serviceName", json!("svc")),
                ("ecsServices.0.clusterName", json!("cluster")),
            ],
        ),
        (
            "the EC2 deployment options added",
            json!({"ec2DeploymentOptions": {"autoScalingGroupProcessesToSuspend": ["Terminate"]}}),
            &[(
                "ec2DeploymentOptions.autoScalingGroupProcessesToSuspend.0",
                json!("Terminate"),
            )],
        ),
        (
            "a nested tag set replacing the EC2 filter list",
            json!({"ec2TagSet": ec2_tag_set(2)}),
            &[
                ("ec2TagSet.ec2TagSetList.0.0.Key", json!("group-0")),
                ("ec2TagSet.ec2TagSetList.1.0.Key", json!("group-1")),
                ("ec2TagFilters", json!([])),
            ],
        ),
        (
            "a nested tag set replacing the on-premises filter list",
            json!({"onPremisesTagSet": on_premises_tag_set(1)}),
            &[
                (
                    "onPremisesTagSet.onPremisesTagSetList.0.0.Key",
                    json!("group-0"),
                ),
                ("onPremisesInstanceTagFilters", json!([])),
            ],
        ),
        (
            "a request naming no replacement at all",
            json!({}),
            &[("ec2TagFilters.0.Value", json!("web"))],
        ),
    ];

    for (index, (case, patch, expectations)) in cases.iter().enumerate() {
        let application = format!("app-replace-{index}");
        seed_baseline_group(&emulator, &application).await;
        let before = read_group(&emulator, &application, GROUP).await;

        let answered = emulator
            .ok(
                "UpdateDeploymentGroup",
                update_request(&application, GROUP, patch),
            )
            .await;
        assert_eq!(
            answered["hooksNotCleanedUp"],
            json!([]),
            "case {case:?}: expected no hooks left behind, got {answered:?}"
        );

        let after = read_group(&emulator, &application, GROUP).await;
        for (path, expected) in *expectations {
            assert_eq!(
                member(&after, path),
                expected,
                "case {case:?}: expected {path} to be {expected:?}, got {:?}",
                member(&after, path)
            );
        }

        // The other half of the rule: nothing the request did not name moved.
        let changeable = changeable_members(patch);
        let mut names: Vec<String> = before
            .as_object()
            .into_iter()
            .flat_map(|members| members.keys().cloned())
            .collect();
        for name in after
            .as_object()
            .into_iter()
            .flat_map(|members| members.keys())
        {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        for name in names {
            if changeable.contains(&name) {
                continue;
            }
            assert_eq!(
                after[&name], before[&name],
                "case {case:?}: expected {name} to be kept as {:?}, got {:?}",
                before[&name], after[&name]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Renaming
// ---------------------------------------------------------------------------

/// RULE: a rename moves the group to the new name without creating a new
/// resource — the id it was created with, its members and its deployment
/// history all follow it, and the old name no longer names a group.
#[tokio::test]
async fn a_rename_moves_the_group_and_keeps_its_identity() {
    let emulator = Emulator::new();
    // The rows differ along how many renames are applied, and whether a rename
    // names the group's current name (which asks for no move at all).
    let cases: &[(&str, &[&str], &str)] = &[
        ("no rename", &[], GROUP),
        ("one rename", &["dg-renamed"], "dg-renamed"),
        ("two renames in a row", &["dg-second", "dg-third"], "dg-third"),
        ("a rename to the group's current name", &[GROUP], GROUP),
    ];

    for (index, (case, renames, expected_name)) in cases.iter().enumerate() {
        let application = format!("app-rename-{index}");
        seed_baseline_group(&emulator, &application).await;
        let created_id = read_group(&emulator, &application, GROUP).await["deploymentGroupId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let deployment_id = emulator
            .ok(
                "CreateDeployment",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": GROUP,
                    "revision": s3_revision("bundles", "app.zip"),
                }),
            )
            .await["deploymentId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let mut current = GROUP.to_string();
        for rename in *renames {
            emulator
                .ok(
                    "UpdateDeploymentGroup",
                    json!({
                        "applicationName": application,
                        "currentDeploymentGroupName": current,
                        "newDeploymentGroupName": rename,
                    }),
                )
                .await;
            current = (*rename).to_string();
        }
        assert_eq!(
            current, *expected_name,
            "case {case:?}: expected the group to end up named {expected_name}, got {current}"
        );

        let moved = read_group(&emulator, &application, expected_name).await;
        assert_eq!(
            moved["deploymentGroupId"].as_str(),
            Some(created_id.as_str()),
            "case {case:?}: expected the created id {created_id} to follow the rename, got {moved:?}"
        );
        assert_eq!(
            moved["deploymentGroupName"].as_str(),
            Some(*expected_name),
            "case {case:?}: expected the group to report its new name, got {moved:?}"
        );
        assert_eq!(
            moved["serviceRoleArn"].as_str(),
            Some("arn:aws:iam::123456789012:role/CodeDeployOriginal"),
            "case {case:?}: expected the stored members to survive the rename, got {moved:?}"
        );
        assert_eq!(
            member(&moved, "lastAttemptedDeployment.deploymentId").as_str(),
            Some(deployment_id.as_str()),
            "case {case:?}: expected the deployment history to follow the rename, got {moved:?}"
        );

        if *expected_name != GROUP {
            let (status, body) = emulator
                .call(
                    "GetDeploymentGroup",
                    json!({"applicationName": application, "deploymentGroupName": GROUP}),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "DeploymentGroupDoesNotExistException"),
                "case {case:?}: expected the old name to name nothing, got {status} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a request the service refuses is answered with the modelled error for
/// what is wrong with it, HTTP 400, and leaves the stored group exactly as it
/// was.
#[tokio::test]
async fn a_refused_update_answers_its_modelled_error_and_changes_nothing() {
    let emulator = Emulator::new();
    // A name 256 characters long: one past the 255 the model's `Alarm$name`
    // publishes ("Maximum length is 255 characters").
    let over_long_alarm_name = "a".repeat(256);
    // The rows differ along which rule the request breaks, and the kind of
    // breach: an unknown reference, a name collision, a quota, a malformed
    // member, and two members that cannot be used together.
    let cases: &[(&str, Value, &str)] = &[
        (
            "an application that does not exist",
            json!({"applicationName": "app-absent"}),
            "ApplicationDoesNotExistException",
        ),
        (
            "a deployment group that does not exist",
            json!({"currentDeploymentGroupName": "dg-absent"}),
            "DeploymentGroupDoesNotExistException",
        ),
        (
            "a deployment configuration that does not exist",
            json!({"deploymentConfigName": "CodeDeployDefault.NotAConfig"}),
            "DeploymentConfigDoesNotExistException",
        ),
        (
            "a new name another group already has",
            json!({"newDeploymentGroupName": "dg-taken"}),
            "DeploymentGroupAlreadyExistsException",
        ),
        (
            "an enabled alarm configuration with an empty alarm list",
            json!({"alarmConfiguration": {"enabled": true, "alarms": []}}),
            "InvalidAlarmConfigException",
        ),
        (
            "two alarms with the same name",
            json!({"alarmConfiguration": {
                "enabled": true,
                "alarms": [{"name": "same"}, {"name": "same"}],
            }}),
            "InvalidAlarmConfigException",
        ),
        (
            "an alarm with an empty name",
            json!({"alarmConfiguration": {"enabled": true, "alarms": [{"name": ""}]}}),
            "InvalidAlarmConfigException",
        ),
        (
            "an alarm name one character past the published 255",
            json!({"alarmConfiguration": {
                "enabled": true,
                "alarms": [{"name": over_long_alarm_name}],
            }}),
            "InvalidAlarmConfigException",
        ),
        (
            "both an EC2 filter list and an EC2 tag set",
            json!({
                "ec2TagFilters": [{"Key": "Name", "Value": "api", "Type": "KEY_AND_VALUE"}],
                "ec2TagSet": ec2_tag_set(1),
            }),
            "InvalidEC2TagCombinationException",
        ),
        (
            "both an on-premises filter list and an on-premises tag set",
            json!({
                "onPremisesInstanceTagFilters": [
                    {"Key": "Stage", "Value": "prod", "Type": "KEY_AND_VALUE"}
                ],
                "onPremisesTagSet": on_premises_tag_set(1),
            }),
            "InvalidOnPremisesTagCombinationException",
        ),
        (
            "one EC2 tag group past the published maximum of three",
            json!({"ec2TagSet": ec2_tag_set(4)}),
            "TagSetListLimitExceededException",
        ),
        (
            "one on-premises tag group past the published maximum of three",
            json!({"onPremisesTagSet": on_premises_tag_set(4)}),
            "TagSetListLimitExceededException",
        ),
        (
            "a suspendable process in the wrong case",
            json!({"ec2DeploymentOptions": {"autoScalingGroupProcessesToSuspend": ["terminate"]}}),
            // The member is a modelled enum (Terminate | AZRebalance |
            // InstanceRefresh | ScheduledActions, case sensitive), so the
            // boundary validator refuses it before the handler runs.
            "ValidationException",
        ),
        (
            "a process CodeDeploy cannot suspend",
            json!({"ec2DeploymentOptions": {
                "autoScalingGroupProcessesToSuspend": ["Terminate", "Reboot"],
            }}),
            "ValidationException",
        ),
        (
            "a blank replacement service role",
            json!({"serviceRoleArn": "   "}),
            "InvalidRoleException",
        ),
        (
            "an enabled automatic rollback that names no triggering event",
            json!({"autoRollbackConfiguration": {"enabled": true, "events": []}}),
            "InvalidAutoRollbackConfigException",
        ),
        (
            "one EC2 tag filter past the maximum of ten per list",
            json!({"ec2TagFilters": ec2_tag_filters(11)}),
            "InvalidEC2TagException",
        ),
        (
            "one on-premises tag filter past the maximum of ten per list",
            json!({"onPremisesInstanceTagFilters": ec2_tag_filters(11)}),
            "InvalidTagException",
        ),
        (
            "a tag group holding one filter past the maximum of ten",
            json!({"ec2TagSet": {"ec2TagSetList": [ec2_tag_filters(11)]}}),
            "InvalidEC2TagException",
        ),
    ];

    for (index, (case, patch, expected_code)) in cases.iter().enumerate() {
        let application = format!("app-refuse-{index}");
        seed_baseline_group(&emulator, &application).await;
        // The collision row needs a second group to collide with; every other
        // row is unaffected by it.
        emulator
            .ok(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": "dg-taken",
                    "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployOther",
                }),
            )
            .await;
        let before = read_group(&emulator, &application, GROUP).await;

        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                update_request(&application, GROUP, patch),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} {body:?}"
        );

        let after = read_group(&emulator, &application, GROUP).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused update to change nothing, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The published bounds
// ---------------------------------------------------------------------------

/// RULE: a value AT a published bound is accepted and stored — the bound is the
/// last accepted value, not the first refused one.
#[tokio::test]
async fn an_update_accepts_every_published_bound() {
    let emulator = Emulator::new();
    // 255 characters: the maximum the model's `Alarm$name` publishes.
    let longest_alarm_name = "n".repeat(255);
    // The rows sit exactly ON each published bound, and on each entry of the
    // process catalogue the model's `AutoScalingProcess` enum publishes
    // (Terminate | AZRebalance | InstanceRefresh | ScheduledActions).
    let cases: &[UpdateCase<'_>] = &[
        (
            "ten alarms, the published maximum",
            json!({"alarmConfiguration": alarm_configuration(10)}),
            &[("alarmConfiguration.alarms.9.name", json!("alarm-9"))],
        ),
        (
            "one alarm",
            json!({"alarmConfiguration": alarm_configuration(1)}),
            &[("alarmConfiguration.alarms.0.name", json!("alarm-0"))],
        ),
        (
            "a disabled alarm configuration with no alarms at all",
            json!({"alarmConfiguration": {"enabled": false}}),
            &[("alarmConfiguration.enabled", json!(false))],
        ),
        (
            "an alarm name of exactly the published 255 characters",
            json!({"alarmConfiguration": {
                "enabled": true,
                "alarms": [{"name": longest_alarm_name.clone()}],
            }}),
            &[(
                "alarmConfiguration.alarms.0.name",
                json!(longest_alarm_name),
            )],
        ),
        (
            "ten EC2 tag filters, the maximum per list",
            json!({"ec2TagFilters": ec2_tag_filters(10)}),
            &[("ec2TagFilters.9.Key", json!("key-9"))],
        ),
        (
            "ten on-premises tag filters, the maximum per list",
            json!({"onPremisesInstanceTagFilters": ec2_tag_filters(10)}),
            &[("onPremisesInstanceTagFilters.9.Key", json!("key-9"))],
        ),
        (
            "three EC2 tag groups, the published maximum",
            json!({"ec2TagSet": ec2_tag_set(3)}),
            &[("ec2TagSet.ec2TagSetList.2.0.Key", json!("group-2"))],
        ),
        (
            "three on-premises tag groups, the published maximum",
            json!({"onPremisesTagSet": on_premises_tag_set(3)}),
            &[(
                "onPremisesTagSet.onPremisesTagSetList.2.0.Key",
                json!("group-2"),
            )],
        ),
        (
            "the Terminate process",
            json!({"ec2DeploymentOptions": {"autoScalingGroupProcessesToSuspend": ["Terminate"]}}),
            &[(
                "ec2DeploymentOptions.autoScalingGroupProcessesToSuspend.0",
                json!("Terminate"),
            )],
        ),
        (
            "the AZRebalance process",
            json!({"ec2DeploymentOptions": {
                "autoScalingGroupProcessesToSuspend": ["AZRebalance"],
            }}),
            &[(
                "ec2DeploymentOptions.autoScalingGroupProcessesToSuspend.0",
                json!("AZRebalance"),
            )],
        ),
        (
            "the InstanceRefresh process",
            json!({"ec2DeploymentOptions": {
                "autoScalingGroupProcessesToSuspend": ["InstanceRefresh"],
            }}),
            &[(
                "ec2DeploymentOptions.autoScalingGroupProcessesToSuspend.0",
                json!("InstanceRefresh"),
            )],
        ),
        (
            "the ScheduledActions process",
            json!({"ec2DeploymentOptions": {
                "autoScalingGroupProcessesToSuspend": ["ScheduledActions"],
            }}),
            &[(
                "ec2DeploymentOptions.autoScalingGroupProcessesToSuspend.0",
                json!("ScheduledActions"),
            )],
        ),
        (
            "every publishable process at once",
            json!({"ec2DeploymentOptions": {"autoScalingGroupProcessesToSuspend": [
                "Terminate", "AZRebalance", "InstanceRefresh", "ScheduledActions",
            ]}}),
            &[(
                "ec2DeploymentOptions.autoScalingGroupProcessesToSuspend.3",
                json!("ScheduledActions"),
            )],
        ),
    ];

    for (index, (case, patch, expectations)) in cases.iter().enumerate() {
        let application = format!("app-bound-{index}");
        seed_baseline_group(&emulator, &application).await;
        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                update_request(&application, GROUP, patch),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the published bound to be accepted, got {status} {body:?}"
        );

        let after = read_group(&emulator, &application, GROUP).await;
        for (path, expected) in *expectations {
            assert_eq!(
                member(&after, path),
                expected,
                "case {case:?}: expected {path} to be {expected:?}, got {:?}",
                member(&after, path)
            );
        }
    }
}

/// RULE: the documented alarm figure is what a NEW deployment group is admitted
/// under — `CreateDeploymentGroup` refuses a configuration past it — which is a
/// gate on that admission and not on every write of a group's alarms; what an
/// update of an existing group's configuration is admitted with is tabulated in
/// [`super::deployment_group_alarm_counts`].
#[tokio::test]
async fn the_alarm_bound_holds_wherever_an_alarm_configuration_is_set() {
    let emulator = Emulator::new();
    // Rows: the published maximum, one past it, and far past it.
    let cases: &[(&str, usize, Option<&str>)] = &[
        ("ten alarms, the published maximum", 10, None),
        ("eleven alarms", 11, Some("AlarmsLimitExceededException")),
        ("fifty alarms", 50, Some("AlarmsLimitExceededException")),
    ];

    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": "app-create-alarms"}),
        )
        .await;
    for (index, (case, count, expected_code)) in cases.iter().enumerate() {
        let group = format!("dg-create-{index}");
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": "app-create-alarms",
                    "deploymentGroupName": group,
                    "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
                    "alarmConfiguration": alarm_configuration(*count),
                }),
            )
            .await;
        match expected_code {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be accepted, got {status} {body:?}"
                );
                let stored = read_group(&emulator, "app-create-alarms", &group).await;
                assert_eq!(
                    member(&stored, "alarmConfiguration.alarms")
                        .as_array()
                        .map(Vec::len),
                    Some(*count),
                    "case {case:?}: expected {count} stored alarms, got {stored:?}"
                );
            }
            Some(code) => assert_eq!(
                (status, error_code(&body).as_str()),
                (400, *code),
                "case {case:?}: expected 400 {code}, got {status} {body:?}"
            ),
        }
    }
}
