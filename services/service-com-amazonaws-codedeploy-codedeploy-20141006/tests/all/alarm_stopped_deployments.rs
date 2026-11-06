// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch alarm gate a CodeDeploy deployment runs under, end to end over
//! the wire with CloudWatch mounted beside CodeDeploy on one engine.
//!
//! The rule under test: a deployment in flight is POLLED against the alarms its
//! effective `alarmConfiguration` names — the deployment's own
//! `overrideAlarmConfiguration` when it carries one, else its deployment group's
//! — and one that any of those alarms is sounding for is stopped (`Stopped`, with
//! `errorInformation.code` `ALARM_ACTIVE` naming the activated alarms) instead of
//! completing; when the effective `autoRollbackConfiguration` is enabled for
//! `DEPLOYMENT_STOP_ON_ALARM`, CodeDeploy also creates the companion rollback
//! deployment attributed to `codeDeployRollback`, and both records carry the
//! `rollbackInfo` that links them.
//!
//! So the tables below drive the rule over a SET of inputs rather than the one
//! case: the alarm's state (`ALARM`, `OK`, `INSUFFICIENT_DATA`, and an alarm whose
//! state cannot be retrieved at all), how many alarms the configuration names and
//! which of them sound, whether polling is enabled, whether
//! `ignorePollAlarmFailure` is set, where the configuration comes from (the group,
//! or the deployment's override of it), the compute platform the deployment runs
//! on, and — for the rollback — which `autoRollbackEvent`s the configuration
//! names and whether the group has a revision to go back to.
//!
//! Every alarm here is created through `PutMetricAlarm` and driven through
//! `SetAlarmState`, the operations a client uses, so nothing reaches into either
//! service's store.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{s3_revision, terminal_deployment, Emulator};

/// The service role every deployment group here is created with.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The `ErrorCode` a deployment its alarms stopped reports
/// (`smithy_query(query="shape",shape="ErrorCode")` carries `ALARM_ACTIVE`).
const ALARM_ACTIVE: &str = "ALARM_ACTIVE";

/// The `DeploymentCreator` of a deployment CodeDeploy created to roll another one
/// back (`smithy_query(query="shape",shape="DeploymentCreator")`).
const CREATOR_ROLLBACK: &str = "codeDeployRollback";

/// The `StateValue` values a CloudWatch alarm carries, as that service's own
/// model lists them (`smithy_query(query="shape",shape="StateValue")`: OK |
/// ALARM | INSUFFICIENT_DATA).
const STATE_ALARM: &str = "ALARM";
/// See [`STATE_ALARM`].
const STATE_OK: &str = "OK";
/// See [`STATE_ALARM`].
const STATE_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// Create an application and one deployment group of it carrying `extras`,
/// answering the group's name.
async fn group_with(
    emulator: &Emulator,
    application: &str,
    platform: &str,
    extras: &Value,
) -> String {
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
        "serviceRoleArn": SERVICE_ROLE,
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emulator.ok("CreateDeploymentGroup", request).await;
    group
}

/// Create a CloudWatch alarm named `name` and force it into `state_value`.
///
/// The alarm watches a metric nothing publishes into, so its state is whatever
/// `SetAlarmState` last forced — which is how the alarm gate is driven without
/// depending on any metric data.
async fn alarm_at(emulator: &Emulator, name: &str, state_value: &str) {
    emulator
        .cloudwatch_ok(
            "PutMetricAlarm",
            json!({
                "AlarmName": name,
                "Namespace": "Sim/CodeDeployGate",
                "MetricName": "Errors",
                "Statistic": "Sum",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanOrEqualToThreshold",
                "TreatMissingData": "notBreaching",
            }),
        )
        .await;
    emulator
        .cloudwatch_ok(
            "SetAlarmState",
            json!({
                "AlarmName": name,
                "StateValue": state_value,
                "StateReason": "driven by the suite",
            }),
        )
        .await;
}

/// An `alarmConfiguration` naming `alarms`.
fn alarm_configuration(enabled: bool, ignore_poll_alarm_failure: bool, alarms: &[&str]) -> Value {
    json!({
        "enabled": enabled,
        "ignorePollAlarmFailure": ignore_poll_alarm_failure,
        "alarms": alarms.iter().map(|name| json!({"name": name})).collect::<Vec<Value>>(),
    })
}

/// Create a deployment of `revision`, answering the id CodeDeploy minted.
async fn create_deployment(
    emulator: &Emulator,
    application: &str,
    group: &str,
    revision: &Value,
    extras: &Value,
) -> String {
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "revision": revision,
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emulator.ok("CreateDeployment", request).await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `deploymentInfo` of one deployment.
async fn deployment_info(emulator: &Emulator, deployment_id: &str) -> Value {
    emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await["deploymentInfo"]
        .clone()
}

/// The ids `ListDeployments` answers for one deployment group.
async fn listed_deployments(emulator: &Emulator, application: &str, group: &str) -> Vec<String> {
    emulator
        .ok(
            "ListDeployments",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await["deployments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// Which deployments the alarms stop
// ---------------------------------------------------------------------------

/// One row of the alarm table.
struct Case {
    case: &'static str,
    /// The compute platform the deployment runs on.
    platform: &'static str,
    /// Extra members the deployment group is created with, beside its alarm
    /// configuration.
    group_extras: Value,
    /// The group's `alarmConfiguration`, absent when `None`.
    group_alarms: Option<Value>,
    /// The deployment's `overrideAlarmConfiguration`, absent when `None`.
    override_alarms: Option<Value>,
    /// The alarms to create, with the state each is forced into. An alarm the
    /// configuration names and this list does not is one the account does not
    /// hold, so its state cannot be retrieved.
    held_alarms: &'static [(&'static str, &'static str)],
    /// The terminal status the deployment must reach.
    expected_status: &'static str,
    /// The alarms the stop must name in `errorInformation.message`, in order;
    /// empty for a deployment the alarms do not stop.
    expected_named: &'static [&'static str],
}

/// RULE: a deployment in flight is stopped, with `ALARM_ACTIVE` naming the
/// alarms, exactly when its effective alarm configuration is enabled and one of
/// the alarms it names is in the `ALARM` state — or, for an alarm whose state
/// cannot be retrieved, unless the configuration says to ignore that; otherwise
/// the deployment completes as it would with no alarm configuration at all.
#[tokio::test]
async fn the_alarms_a_deployment_polls_decide_whether_it_is_stopped() {
    // Rows differ along the alarm's state, how many alarms are named and which of
    // them sound, whether polling is enabled, `ignorePollAlarmFailure`, where the
    // configuration comes from, and the compute platform.
    let cases: &[Case] = &[
        Case {
            case: "one alarm in ALARM",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(true, false, &["gate-alarm"])),
            override_alarms: None,
            held_alarms: &[("gate-alarm", STATE_ALARM)],
            expected_status: "Stopped",
            expected_named: &["gate-alarm"],
        },
        Case {
            case: "one alarm at OK",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(true, false, &["ok-alarm"])),
            override_alarms: None,
            held_alarms: &[("ok-alarm", STATE_OK)],
            expected_status: "Succeeded",
            expected_named: &[],
        },
        Case {
            case: "one alarm with insufficient data",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(true, false, &["quiet-alarm"])),
            override_alarms: None,
            held_alarms: &[("quiet-alarm", STATE_INSUFFICIENT_DATA)],
            expected_status: "Succeeded",
            expected_named: &[],
        },
        Case {
            case: "the second of three alarms in ALARM",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(
                true,
                false,
                &["three-first", "three-second", "three-third"],
            )),
            override_alarms: None,
            held_alarms: &[
                ("three-first", STATE_OK),
                ("three-second", STATE_ALARM),
                ("three-third", STATE_OK),
            ],
            expected_status: "Stopped",
            expected_named: &["three-second"],
        },
        Case {
            case: "two of three alarms in ALARM",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(
                true,
                false,
                &["pair-first", "pair-second", "pair-third"],
            )),
            override_alarms: None,
            held_alarms: &[
                ("pair-first", STATE_ALARM),
                ("pair-second", STATE_OK),
                ("pair-third", STATE_ALARM),
            ],
            expected_status: "Stopped",
            expected_named: &["pair-first", "pair-third"],
        },
        Case {
            case: "polling is not enabled",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(false, false, &["unwatched-alarm"])),
            override_alarms: None,
            held_alarms: &[("unwatched-alarm", STATE_ALARM)],
            expected_status: "Succeeded",
            expected_named: &[],
        },
        Case {
            case: "no alarm configuration at all",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: None,
            override_alarms: None,
            held_alarms: &[],
            expected_status: "Succeeded",
            expected_named: &[],
        },
        Case {
            case: "an alarm the account does not hold",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(true, false, &["absent-alarm"])),
            override_alarms: None,
            held_alarms: &[],
            expected_status: "Stopped",
            expected_named: &["absent-alarm"],
        },
        Case {
            case: "an alarm the account does not hold, ignoring poll failures",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(true, true, &["ignored-absent-alarm"])),
            override_alarms: None,
            held_alarms: &[],
            expected_status: "Succeeded",
            expected_named: &[],
        },
        Case {
            case: "an alarm in ALARM, ignoring poll failures",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(true, true, &["sounding-alarm"])),
            override_alarms: None,
            held_alarms: &[("sounding-alarm", STATE_ALARM)],
            expected_status: "Stopped",
            expected_named: &["sounding-alarm"],
        },
        Case {
            case: "the deployment's override turns polling off",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(true, false, &["overridden-alarm"])),
            override_alarms: Some(json!({"enabled": false})),
            held_alarms: &[("overridden-alarm", STATE_ALARM)],
            expected_status: "Succeeded",
            expected_named: &[],
        },
        Case {
            case: "the deployment's override names its own alarm",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: None,
            override_alarms: Some(alarm_configuration(true, false, &["override-alarm"])),
            held_alarms: &[("override-alarm", STATE_ALARM)],
            expected_status: "Stopped",
            expected_named: &["override-alarm"],
        },
        Case {
            case: "the deployment's override replaces the group's alarms",
            platform: "Lambda",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(true, false, &["group-side-alarm"])),
            override_alarms: Some(alarm_configuration(true, false, &["deployment-side-alarm"])),
            held_alarms: &[
                ("group-side-alarm", STATE_ALARM),
                ("deployment-side-alarm", STATE_OK),
            ],
            expected_status: "Succeeded",
            expected_named: &[],
        },
        Case {
            case: "a server deployment",
            platform: "Server",
            group_extras: json!({}),
            group_alarms: Some(alarm_configuration(true, false, &["server-alarm"])),
            override_alarms: None,
            held_alarms: &[("server-alarm", STATE_ALARM)],
            expected_status: "Stopped",
            expected_named: &["server-alarm"],
        },
        Case {
            case: "an ecs deployment",
            platform: "ECS",
            group_extras: json!({"ecsServices": [{"clusterName": "cluster", "serviceName": "web"}]}),
            group_alarms: Some(alarm_configuration(true, false, &["ecs-alarm"])),
            override_alarms: None,
            held_alarms: &[("ecs-alarm", STATE_ALARM)],
            expected_status: "Stopped",
            expected_named: &["ecs-alarm"],
        },
    ];

    for case in cases {
        let name = case.case;
        let emulator = Emulator::with_cloudwatch();
        let application = format!(
            "gate-{}",
            name.replace([' ', ','], "-").replace('\'', "")
        );
        for (alarm, state_value) in case.held_alarms {
            alarm_at(&emulator, alarm, state_value).await;
        }
        let mut extras = case.group_extras.clone();
        if let Some(configuration) = &case.group_alarms {
            extras["alarmConfiguration"] = configuration.clone();
        }
        let group = group_with(&emulator, &application, case.platform, &extras).await;

        let mut deployment_extras = json!({});
        if let Some(configuration) = &case.override_alarms {
            deployment_extras["overrideAlarmConfiguration"] = configuration.clone();
        }
        let deployment_id = create_deployment(
            &emulator,
            &application,
            &group,
            &s3_revision("gate-bucket", "app.zip"),
            &deployment_extras,
        )
        .await;

        let info = terminal_deployment(&emulator, &deployment_id, name).await;
        assert_eq!(
            info["status"].as_str(),
            Some(case.expected_status),
            "case {name:?}: expected the deployment to end {:?}, got {info:?}",
            case.expected_status
        );
        let error = &info["errorInformation"];
        if case.expected_named.is_empty() {
            assert!(
                error.is_null(),
                "case {name:?}: expected no errorInformation from the alarm gate, got {error:?}"
            );
        } else {
            assert_eq!(
                error["code"].as_str(),
                Some(ALARM_ACTIVE),
                "case {name:?}: expected {ALARM_ACTIVE:?} as the error code, got {error:?}"
            );
            let message = error["message"].as_str().unwrap_or_default().to_string();
            for alarm in case.expected_named {
                assert!(
                    message.contains(alarm),
                    "case {name:?}: expected the message to name {alarm:?}, got {message:?}"
                );
            }
            // Only the alarms that stopped it are named: an alarm of the same
            // configuration that was quiet is not.
            for (alarm, state_value) in case.held_alarms {
                if *state_value != STATE_ALARM {
                    assert!(
                        !message.contains(alarm),
                        "case {name:?}: expected the message not to name the quiet alarm \
                         {alarm:?}, got {message:?}"
                    );
                }
            }
        }

        // Reading a deployment is a report: the second read says what the first
        // said, so the gate cannot move a deployment a caller has already seen
        // terminal.
        let again = deployment_info(&emulator, &deployment_id).await;
        assert_eq!(
            again["status"], info["status"],
            "case {name:?}: expected a stable status across reads, got {again:?} after {info:?}"
        );
        assert_eq!(
            again["errorInformation"], info["errorInformation"],
            "case {name:?}: expected stable errorInformation across reads, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rollback a stop on alarm creates
// ---------------------------------------------------------------------------

/// One row of the rollback table.
struct RollbackCase {
    case: &'static str,
    /// The group's `autoRollbackConfiguration`, absent when `None`.
    group_rollback: Option<Value>,
    /// The deployment's own `autoRollbackConfiguration`, absent when `None`.
    deployment_rollback: Option<Value>,
    /// Whether the stop creates the companion rollback deployment.
    expect_rollback: bool,
}

/// RULE: a deployment the alarms stopped is rolled back — a second deployment of
/// the revision the group was on before it, attributed to `codeDeployRollback`,
/// with `rollbackInfo` linking the two records in both directions — exactly when
/// the effective `autoRollbackConfiguration` (the deployment's own, else its
/// group's) is enabled and names the `DEPLOYMENT_STOP_ON_ALARM` event.
#[tokio::test]
async fn a_stop_on_alarm_rolls_the_deployment_back_when_its_configuration_asks() {
    // Rows differ along where the automatic rollback configuration lives, whether
    // it is enabled, and which events it names.
    let cases: &[RollbackCase] = &[
        RollbackCase {
            case: "the group asks for a rollback on alarm",
            group_rollback: Some(json!({"enabled": true, "events": ["DEPLOYMENT_STOP_ON_ALARM"]})),
            deployment_rollback: None,
            expect_rollback: true,
        },
        RollbackCase {
            case: "the group names several events including the alarm one",
            group_rollback: Some(json!({
                "enabled": true,
                "events": ["DEPLOYMENT_FAILURE", "DEPLOYMENT_STOP_ON_ALARM"],
            })),
            deployment_rollback: None,
            expect_rollback: true,
        },
        RollbackCase {
            case: "the group names only the failure event",
            group_rollback: Some(json!({"enabled": true, "events": ["DEPLOYMENT_FAILURE"]})),
            deployment_rollback: None,
            expect_rollback: false,
        },
        RollbackCase {
            case: "the group's configuration is not enabled",
            group_rollback: Some(json!({"enabled": false, "events": ["DEPLOYMENT_STOP_ON_ALARM"]})),
            deployment_rollback: None,
            expect_rollback: false,
        },
        RollbackCase {
            case: "no automatic rollback anywhere",
            group_rollback: None,
            deployment_rollback: None,
            expect_rollback: false,
        },
        RollbackCase {
            case: "the deployment asks for a rollback on alarm",
            group_rollback: None,
            deployment_rollback: Some(
                json!({"enabled": true, "events": ["DEPLOYMENT_STOP_ON_ALARM"]}),
            ),
            expect_rollback: true,
        },
        RollbackCase {
            case: "the deployment's configuration replaces the group's",
            group_rollback: Some(json!({"enabled": true, "events": ["DEPLOYMENT_STOP_ON_ALARM"]})),
            deployment_rollback: Some(json!({"enabled": true, "events": ["DEPLOYMENT_FAILURE"]})),
            expect_rollback: false,
        },
    ];

    for case in cases {
        let name = case.case;
        let emulator = Emulator::with_cloudwatch();
        let application = format!("rollback-{}", name.replace(' ', "-").replace('\'', ""));        let alarm = format!("{application}-alarm");
        // The alarm is quiet while the group's first deployment runs, so the
        // group has a revision to go back to before the alarm sounds.
        alarm_at(&emulator, &alarm, STATE_OK).await;
        let mut extras = json!({"alarmConfiguration": alarm_configuration(true, false, &[&alarm])});
        if let Some(configuration) = &case.group_rollback {
            extras["autoRollbackConfiguration"] = configuration.clone();
        }
        let group = group_with(&emulator, &application, "Lambda", &extras).await;

        let first_revision = s3_revision("rollback-bucket", "first.zip");
        let stopped_revision = s3_revision("rollback-bucket", "second.zip");
        let first_id = create_deployment(
            &emulator,
            &application,
            &group,
            &first_revision,
            &json!({}),
        )
        .await;
        let first = terminal_deployment(&emulator, &first_id, name).await;
        assert_eq!(
            first["status"].as_str(),
            Some("Succeeded"),
            "case {name:?}: expected the deployment under a quiet alarm to succeed, got {first:?}"
        );

        alarm_at(&emulator, &alarm, STATE_ALARM).await;
        let mut deployment_extras = json!({});
        if let Some(configuration) = &case.deployment_rollback {
            deployment_extras["autoRollbackConfiguration"] = configuration.clone();
        }
        let stopped_id = create_deployment(
            &emulator,
            &application,
            &group,
            &stopped_revision,
            &deployment_extras,
        )
        .await;
        let stopped = terminal_deployment(&emulator, &stopped_id, name).await;
        assert_eq!(
            stopped["status"].as_str(),
            Some("Stopped"),
            "case {name:?}: expected the sounding alarm to stop the deployment, got {stopped:?}"
        );

        let rollback = &stopped["rollbackInfo"];
        assert_eq!(
            !rollback.is_null(),
            case.expect_rollback,
            "case {name:?}: expected rollbackInfo present={}, got {stopped:?}",
            case.expect_rollback
        );
        let listed = listed_deployments(&emulator, &application, &group).await;
        if !case.expect_rollback {
            assert_eq!(
                listed.len(),
                2,
                "case {name:?}: expected only the two client deployments, got {listed:?}"
            );
            continue;
        }

        let rollback_id = rollback["rollbackDeploymentId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !rollback_id.is_empty(),
            "case {name:?}: expected a rollback deployment id, got {rollback:?}"
        );
        assert_eq!(
            rollback["rollbackTriggeringDeploymentId"].as_str(),
            Some(stopped_id.as_str()),
            "case {name:?}: expected the stopped deployment as the trigger, got {rollback:?}"
        );
        assert!(
            rollback["rollbackMessage"]
                .as_str()
                .is_some_and(|message| message.contains(&rollback_id)),
            "case {name:?}: expected the rollback message to name the rollback, got {rollback:?}"
        );
        assert!(
            listed.contains(&rollback_id),
            "case {name:?}: expected {rollback_id:?} among the group's deployments, got {listed:?}"
        );

        // The rollback is a deployment in its own right: the service's own, of the
        // revision the group was on before the stopped one, pointing back at it —
        // and it is CodeDeploy's remediation for the alarm, so the same sounding
        // alarm does not stop it in turn.
        let rolled_back = terminal_deployment(&emulator, &rollback_id, name).await;
        assert_eq!(
            rolled_back["creator"].as_str(),
            Some(CREATOR_ROLLBACK),
            "case {name:?}: expected the service as the rollback's creator, got {rolled_back:?}"
        );
        assert_eq!(
            rolled_back["revision"], first_revision,
            "case {name:?}: expected the rollback to redeploy the previous revision, got \
             {rolled_back:?}"
        );
        assert_eq!(
            rolled_back["previousRevision"], stopped_revision,
            "case {name:?}: expected the stopped revision as the rollback's previous one, got \
             {rolled_back:?}"
        );
        assert_eq!(
            rolled_back["rollbackInfo"]["rollbackTriggeringDeploymentId"].as_str(),
            Some(stopped_id.as_str()),
            "case {name:?}: expected the rollback to point back at the stopped deployment, got \
             {rolled_back:?}"
        );
        assert_eq!(
            rolled_back["status"].as_str(),
            Some("Succeeded"),
            "case {name:?}: expected the rollback itself to run ungated, got {rolled_back:?}"
        );
    }
}

/// RULE: a deployment group whose FIRST deployment the alarms stop has no
/// revision to go back to, so the stop records why it cannot be rolled back and
/// creates no second deployment.
#[tokio::test]
async fn a_first_deployment_stopped_by_an_alarm_has_nothing_to_roll_back_to() {
    let case = "first deployment";
    let emulator = Emulator::with_cloudwatch();
    let application = "rollback-first-deployment";
    let alarm = "rollback-first-alarm";
    alarm_at(&emulator, alarm, STATE_ALARM).await;
    let group = group_with(
        &emulator,
        application,
        "Lambda",
        &json!({
            "alarmConfiguration": alarm_configuration(true, false, &[alarm]),
            "autoRollbackConfiguration": {"enabled": true, "events": ["DEPLOYMENT_STOP_ON_ALARM"]},
        }),
    )
    .await;

    let deployment_id = create_deployment(
        &emulator,
        application,
        &group,
        &s3_revision("rollback-bucket", "only.zip"),
        &json!({}),
    )
    .await;
    let stopped = terminal_deployment(&emulator, &deployment_id, case).await;
    assert_eq!(
        stopped["status"].as_str(),
        Some("Stopped"),
        "case {case:?}: expected the sounding alarm to stop the deployment, got {stopped:?}"
    );
    let rollback = &stopped["rollbackInfo"];
    assert!(
        rollback["rollbackDeploymentId"].is_null(),
        "case {case:?}: expected no rollback deployment for a group with no previous revision, \
         got {rollback:?}"
    );
    assert_eq!(
        rollback["rollbackTriggeringDeploymentId"].as_str(),
        Some(deployment_id.as_str()),
        "case {case:?}: expected the stopped deployment as the trigger, got {rollback:?}"
    );
    assert!(
        rollback["rollbackMessage"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: expected a message saying why, got {rollback:?}"
    );
    let listed = listed_deployments(&emulator, application, &group).await;
    assert_eq!(
        listed,
        vec![deployment_id],
        "case {case:?}: expected the stopped deployment alone, got {listed:?}"
    );
}
