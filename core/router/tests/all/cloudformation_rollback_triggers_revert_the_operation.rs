// SPDX-License-Identifier: CC0-1.0

//! A stack operation monitored by the rollback triggers its `RollbackConfiguration` names,
//! across CloudFormation, Amazon CloudWatch and Amazon SNS on ONE engine.
//!
//! RULE under test: **a create or an update whose `RollbackConfiguration` names rollback
//! triggers is rolled back when one of the CloudWatch alarms those triggers name is in
//! `ALARM` while the operation is in flight — "if any of the alarms you specify goes to ALARM
//! state during the stack operation or this monitoring period, CloudFormation rolls back the
//! entire stack operation" — so the stack reaches its rolled-back terminal status
//! (`ROLLBACK_COMPLETE` for a create, `UPDATE_ROLLBACK_COMPLETE` for an update) carrying the
//! triggers that fired as its `StackStatusReason`, and the configuration the operation had
//! already applied in the owning service is put back; a trigger whose alarm is in any other
//! state rolls nothing back and the operation completes, and a caller that asked for the
//! failed state to be kept (`DisableRollback`) gets `UPDATE_FAILED` with the change left
//! where it was.**
//!
//! Only a cross-service suite can pin it: what decides the rollback is an alarm's `StateValue`
//! in CLOUDWATCH's own records, read by CloudFormation in-process, and what says the rollback
//! really happened is the property SNS holds for the topic afterwards. The crate-local suite
//! `rollback_trigger_types.rs` pins the other half of the member — which trigger TYPES a
//! request may name — and could not reach this one, because CloudWatch is not mounted there.
//!
//! No alarm state is forced with `SetAlarmState`: each row's alarm reaches its state on
//! CloudWatch's own evaluation of a series nothing publishes a datapoint for, through
//! `TreatMissingData`, so what rolls a stack back here is a state CloudWatch really computed.
//!
//! The rows differ along what the rule generalises over: the alarm STATE the trigger is in
//! (each of the three `StateValue` values), the trigger TYPE (a metric alarm and a composite
//! alarm over one), how many triggers the configuration names and which of them fires, the
//! `MonitoringTimeInMinutes` stated, the operation that deploys the revision (`UpdateStack`
//! and `ExecuteChangeSet`), and whether the caller asked for the failed state to be kept.
//!
//! The awsQuery form encoder and the XML readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{decoded_entries, element, opt_element, physical_ids_of};
use service_com_amazonaws_cloudformation_cloudformation::state::rollback_triggers::{
    trigger_failure_summary, ALARM_STATE,
};

/// The Region every call below is signed for, and so the Region the stack, its topic and the
/// alarms all live in.
const REGION: &str = "us-west-2";

/// CloudWatch's `X-Amz-Target` prefix and its signing name (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The other two `StateValue` values, which are an alarm that is NOT firing
/// (`StateValue`: `ALARM | INSUFFICIENT_DATA | OK`).
const OK_STATE: &str = "OK";
const INSUFFICIENT_DATA_STATE: &str = "INSUFFICIENT_DATA";

/// The `TreatMissingData` behaviour that makes an alarm over a series with NO datapoints
/// settle in each state: "breaching — `ALARM`", "notBreaching — `OK`", "missing — the alarm
/// doesn't consider the missing data point" (so it stays `INSUFFICIENT_DATA`).
const TREAT_MISSING_BREACHING: &str = "breaching";
const TREAT_MISSING_NOT_BREACHING: &str = "notBreaching";
const TREAT_MISSING_MISSING: &str = "missing";

/// The topic property each row changes, and the two values it changes it between: the stack
/// deploys the first and the revision declares the second, so the value SNS holds afterwards
/// says whether the revision was kept or put back.
const WATCHED_ATTRIBUTE: &str = "DisplayName";
const BEFORE: &str = "deployed";
const AFTER: &str = "revised";

/// How long a row waits for something another service's clock owes — an alarm's evaluation,
/// a stack's terminal status — and how often it looks. Generous only so a failure is a
/// failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(30);
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service` in [`REGION`], which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Call one awsQuery operation of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(key));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One request member as the pair [`query`] takes.
fn member(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Call one CloudFormation operation (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// One CloudFormation call that must be answered.
async fn cfn_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(String, String)],
) -> String {
    let (status, body) = cloudformation(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudFormation {action}, got {status} with {body}"
    );
    body
}

/// Call one CloudWatch operation on the awsJson1_0 wire its model declares, which must be
/// answered.
async fn cw(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{CLOUDWATCH_SIGNING_NAME}.{REGION}.amazonaws.com"),
        )
        .header("authorization", authorization(CLOUDWATCH_SIGNING_NAME))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with {text}"
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// A metric alarm over a series NOTHING publishes a datapoint for, whose missing data is
/// treated as `treat_missing` — so CloudWatch's own evaluation settles it in the state that
/// behaviour documents. Answers the alarm's ARN, which is what a rollback trigger names.
async fn given_metric_alarm(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    treat_missing: &str,
) -> String {
    cw(
        router,
        case,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "Custom/Deploys",
            "MetricName": "Errors",
            "Statistic": "Maximum",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanOrEqualToThreshold",
            "TreatMissingData": treat_missing,
        }),
    )
    .await;
    alarm_arn(router, case, name).await
}

/// A COMPOSITE alarm whose rule is `ALARM("<child>")`, so its state follows that child's.
async fn given_composite_alarm(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    child: &str,
) -> String {
    cw(
        router,
        case,
        "PutCompositeAlarm",
        &json!({
            "AlarmName": name,
            "AlarmRule": format!("ALARM(\"{child}\")"),
        }),
    )
    .await;
    alarm_arn(router, case, name).await
}

/// The ARN CloudWatch publishes for the alarm `name`, metric or composite.
async fn alarm_arn(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let described = described_alarm(router, case, name).await;
    described["AlarmArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no AlarmArn for {name} in {described}"))
        .to_string()
}

/// The alarm `name`, as `DescribeAlarms` reports it — whichever of the two lists it is in.
async fn described_alarm(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let described = cw(
        router,
        case,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": ["MetricAlarm", "CompositeAlarm"]}),
    )
    .await;
    for list in ["MetricAlarms", "CompositeAlarms"] {
        if let Some(alarm) = described[list].as_array().and_then(|alarms| alarms.first()) {
            return alarm.clone();
        }
    }
    panic!("case {case:?}: CloudWatch reports no alarm {name}: {described}")
}

/// Wait until CloudWatch reports `name` in `wanted`, so a row's precondition is a state that
/// service really computed rather than one this suite assumed.
async fn alarm_settles_in(router: &MultiServiceRouter, case: &str, name: &str, wanted: &str) {
    let deadline = Instant::now() + DEADLINE;
    let mut seen = String::new();
    while Instant::now() < deadline {
        let alarm = described_alarm(router, case, name).await;
        seen = alarm["StateValue"].as_str().unwrap_or_default().to_string();
        if seen == wanted {
            return;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!(
        "case {case:?}: CloudWatch must settle the alarm {name} in {wanted}, got {seen:?} \
         within {DEADLINE:?}"
    );
}

/// The one-topic template, with `display_name` as the topic's `DisplayName`.
fn template(topic: &str, display_name: &str) -> String {
    json!({
        "Resources": {
            "Topic": {
                "Type": "AWS::SNS::Topic",
                "Properties": {"TopicName": topic, "DisplayName": display_name},
            }
        }
    })
    .to_string()
}

/// `RollbackConfiguration` members naming one trigger per entry of `arns`, with the
/// monitoring period `monitoring` states.
fn rollback_configuration(arns: &[String], monitoring: Option<i32>) -> Vec<(String, String)> {
    let mut members = Vec::new();
    if let Some(minutes) = monitoring {
        members.push(member(
            "RollbackConfiguration.MonitoringTimeInMinutes",
            &minutes.to_string(),
        ));
    }
    for (index, arn) in arns.iter().enumerate() {
        let position = index + 1;
        members.push(member(
            &format!("RollbackConfiguration.RollbackTriggers.member.{position}.Arn"),
            arn,
        ));
        members.push(member(
            &format!("RollbackConfiguration.RollbackTriggers.member.{position}.Type"),
            "AWS::CloudWatch::Alarm",
        ));
    }
    members
}

/// Wait until the stack reports a status that is not `*_IN_PROGRESS`, and answer the
/// `DescribeStacks` body it reported it in.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        last = cfn_ok(
            router,
            case,
            "DescribeStacks",
            &[member("StackName", stack)],
        )
        .await;
        let status = opt_element(&last, "StackStatus").unwrap_or_default();
        if !status.ends_with("_IN_PROGRESS") {
            return last;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case {case:?}: the stack {stack} never settled within {DEADLINE:?}: {last}");
}

/// The attributes SNS holds for the topic at `arn`, or `None` where SNS holds no such topic.
async fn topic_attributes(
    router: &MultiServiceRouter,
    arn: &str,
) -> Option<BTreeMap<String, String>> {
    let (status, body) = query(
        router,
        "sns",
        "2010-03-31",
        "GetTopicAttributes",
        &[member("TopicArn", arn)],
    )
    .await;
    if status != 200 {
        return None;
    }
    Some(decoded_entries(&body))
}

/// The topic the stack's `Topic` resource is, by the physical resource id the stack reports
/// for it — which is the topic's ARN.
async fn stack_topic_arn(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let body = cfn_ok(
        router,
        case,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    physical_ids_of(&body)
        .get("Topic")
        .unwrap_or_else(|| panic!("case {case:?}: no physical id for Topic in {body}"))
        .clone()
}

/// Which operation deploys the revision a row's rollback triggers monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deploy {
    /// `UpdateStack`, carrying the rollback configuration itself.
    UpdateStack,
    /// `CreateChangeSet` carrying it, then `ExecuteChangeSet` — the shape a client that
    /// reviews a change before deploying it drives.
    ExecuteChangeSet,
}

/// What a row's trigger alarms are: one alarm in the state its `TreatMissingData` settles it
/// in, or a composite alarm over a child in that state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// A metric alarm whose missing data is treated as the first field, settling it in the
    /// second.
    Metric(&'static str, &'static str),
    /// A COMPOSITE alarm over a metric alarm of the same shape, settling in the same state.
    Composite(&'static str, &'static str),
}

impl Trigger {
    /// The state CloudWatch settles this trigger's own alarm in.
    fn state(self) -> &'static str {
        match self {
            Self::Metric(_, state) | Self::Composite(_, state) => state,
        }
    }
}

/// One row: the triggers the operation is monitored by, how the revision is deployed, and
/// what must become of the stack and of the topic's property.
struct Row {
    case: &'static str,
    /// The stack name, which also seeds the topic and alarm names so rows cannot collide.
    stack: &'static str,
    triggers: &'static [Trigger],
    monitoring: Option<i32>,
    deploy: Deploy,
    /// `DisableRollback`, exactly as the request carries it (never sent when false).
    disable_rollback: bool,
    /// The terminal `StackStatus` the operation must reach.
    expected_status: &'static str,
    /// The `DisplayName` SNS must hold afterwards: [`BEFORE`] where the revision was put
    /// back, [`AFTER`] where it stands.
    expected_display_name: &'static str,
}

/// A metric alarm that fires, and the two that do not.
const FIRING: Trigger = Trigger::Metric(TREAT_MISSING_BREACHING, ALARM_STATE);
const QUIET: Trigger = Trigger::Metric(TREAT_MISSING_NOT_BREACHING, OK_STATE);
const UNKNOWN: Trigger = Trigger::Metric(TREAT_MISSING_MISSING, INSUFFICIENT_DATA_STATE);
const FIRING_COMPOSITE: Trigger = Trigger::Composite(TREAT_MISSING_BREACHING, ALARM_STATE);

#[tokio::test]
async fn an_alarm_in_alarm_rolls_the_update_it_monitors_back() {
    let rows: &[Row] = &[
        Row {
            case: "a metric alarm in ALARM rolls the update back",
            stack: "rbt-update-firing",
            triggers: &[FIRING],
            monitoring: Some(5),
            deploy: Deploy::UpdateStack,
            disable_rollback: false,
            expected_status: "UPDATE_ROLLBACK_COMPLETE",
            expected_display_name: BEFORE,
        },
        Row {
            case: "an alarm in OK rolls nothing back",
            stack: "rbt-update-quiet",
            triggers: &[QUIET],
            monitoring: Some(5),
            deploy: Deploy::UpdateStack,
            disable_rollback: false,
            expected_status: "UPDATE_COMPLETE",
            expected_display_name: AFTER,
        },
        Row {
            case: "an alarm with insufficient data rolls nothing back",
            stack: "rbt-update-unknown",
            triggers: &[UNKNOWN],
            monitoring: None,
            deploy: Deploy::UpdateStack,
            disable_rollback: false,
            expected_status: "UPDATE_COMPLETE",
            expected_display_name: AFTER,
        },
        Row {
            case: "a composite alarm in ALARM rolls the update back",
            stack: "rbt-update-composite",
            triggers: &[FIRING_COMPOSITE],
            monitoring: Some(0),
            deploy: Deploy::UpdateStack,
            disable_rollback: false,
            expected_status: "UPDATE_ROLLBACK_COMPLETE",
            expected_display_name: BEFORE,
        },
        Row {
            case: "one of two triggers firing is enough",
            stack: "rbt-update-one-of-two",
            triggers: &[QUIET, FIRING],
            monitoring: Some(1),
            deploy: Deploy::UpdateStack,
            disable_rollback: false,
            expected_status: "UPDATE_ROLLBACK_COMPLETE",
            expected_display_name: BEFORE,
        },
        Row {
            case: "neither of two triggers firing leaves the update alone",
            stack: "rbt-update-neither",
            triggers: &[QUIET, UNKNOWN],
            monitoring: Some(1),
            deploy: Deploy::UpdateStack,
            disable_rollback: false,
            expected_status: "UPDATE_COMPLETE",
            expected_display_name: AFTER,
        },
        Row {
            case: "a change set's own configuration monitors its execution",
            stack: "rbt-execute-firing",
            triggers: &[FIRING],
            monitoring: Some(5),
            deploy: Deploy::ExecuteChangeSet,
            disable_rollback: false,
            expected_status: "UPDATE_ROLLBACK_COMPLETE",
            expected_display_name: BEFORE,
        },
        // "DisableRollback: Preserve the state of previously provisioned resources when an
        // operation fails": the triggered failure is kept exactly as it failed, so the
        // revision the update applied stands.
        Row {
            case: "DisableRollback keeps the triggered failure as it failed",
            stack: "rbt-update-kept",
            triggers: &[FIRING],
            monitoring: Some(5),
            deploy: Deploy::UpdateStack,
            disable_rollback: true,
            expected_status: "UPDATE_FAILED",
            expected_display_name: AFTER,
        },
    ];

    for row in rows {
        let router = build_router();
        let case = row.case;
        let topic = format!("{}-topic", row.stack);

        // The triggers, each settled in the state its row says CloudWatch computes for it.
        let mut arns: Vec<String> = Vec::new();
        for (index, trigger) in row.triggers.iter().enumerate() {
            let child = format!("{}-alarm-{index}", row.stack);
            match trigger {
                Trigger::Metric(treat_missing, _) => {
                    arns.push(given_metric_alarm(&router, case, &child, treat_missing).await);
                    alarm_settles_in(&router, case, &child, trigger.state()).await;
                }
                Trigger::Composite(treat_missing, _) => {
                    given_metric_alarm(&router, case, &child, treat_missing).await;
                    alarm_settles_in(&router, case, &child, trigger.state()).await;
                    let composite = format!("{}-composite-{index}", row.stack);
                    arns.push(given_composite_alarm(&router, case, &composite, &child).await);
                    alarm_settles_in(&router, case, &composite, trigger.state()).await;
                }
            }
        }

        // The stack as it stands before the monitored revision: no triggers on the create, so
        // what this suite is about is the revision alone.
        cfn_ok(
            &router,
            case,
            "CreateStack",
            &[
                member("StackName", row.stack),
                member("TemplateBody", &template(&topic, BEFORE)),
            ],
        )
        .await;
        let created = settled_stack(&router, case, row.stack).await;
        assert_eq!(
            opt_element(&created, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: the stack under test must be created, got {created}"
        );
        let topic_arn = stack_topic_arn(&router, case, row.stack).await;
        let deployed = topic_attributes(&router, &topic_arn).await;
        assert_eq!(
            deployed
                .as_ref()
                .and_then(|attributes| attributes.get(WATCHED_ATTRIBUTE))
                .map(String::as_str),
            Some(BEFORE),
            "case {case:?}: SNS must hold the deployed {WATCHED_ATTRIBUTE}, got {deployed:?}"
        );

        // The revision, monitored by those triggers.
        let revision = template(&topic, AFTER);
        let mut members = vec![member("TemplateBody", &revision)];
        members.extend(rollback_configuration(&arns, row.monitoring));
        if row.disable_rollback {
            members.push(member("DisableRollback", "true"));
        }
        match row.deploy {
            Deploy::UpdateStack => {
                let mut update = vec![member("StackName", row.stack)];
                update.extend(members);
                cfn_ok(&router, case, "UpdateStack", &update).await;
            }
            Deploy::ExecuteChangeSet => {
                let change_set = format!("{}-change-set", row.stack);
                let mut create = vec![
                    member("StackName", row.stack),
                    member("ChangeSetName", &change_set),
                ];
                create.extend(members);
                cfn_ok(&router, case, "CreateChangeSet", &create).await;
                let planned = planned_change_set(&router, case, row.stack, &change_set).await;
                assert_eq!(
                    planned, "AVAILABLE",
                    "case {case:?}: the change set must be executable, got {planned:?}"
                );
                cfn_ok(
                    &router,
                    case,
                    "ExecuteChangeSet",
                    &[
                        member("StackName", row.stack),
                        member("ChangeSetName", &change_set),
                    ],
                )
                .await;
            }
        }

        let settled = settled_stack(&router, case, row.stack).await;
        let status = opt_element(&settled, "StackStatus").unwrap_or_default();
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected {}, got {status:?} in {settled}",
            row.expected_status
        );

        // The reason names the triggers that fired, and only those.
        let fired: Vec<&str> = row
            .triggers
            .iter()
            .zip(arns.iter())
            .filter(|(trigger, _)| trigger.state() == ALARM_STATE)
            .map(|(_, arn)| arn.as_str())
            .collect();
        let reason = opt_element(&settled, "StackStatusReason").unwrap_or_default();
        if fired.is_empty() {
            assert!(
                !reason.contains("rollback trigger"),
                "case {case:?}: an operation no trigger fired on reports no trigger failure, \
                 got {reason:?}"
            );
        } else {
            assert_eq!(
                reason,
                trigger_failure_summary(fired.iter().copied()),
                "case {case:?}: expected the reason to name the triggers that fired, got \
                 {reason:?}"
            );
        }

        // ...and what the owning service holds says whether the revision was put back.
        let held = topic_attributes(&router, &topic_arn).await;
        assert_eq!(
            held.as_ref()
                .and_then(|attributes| attributes.get(WATCHED_ATTRIBUTE))
                .map(String::as_str),
            Some(row.expected_display_name),
            "case {case:?}: expected SNS to hold {WATCHED_ATTRIBUTE} {:?}, got {held:?}",
            row.expected_display_name
        );
    }
}

/// The `ExecutionStatus` the change set `name` settles in.
async fn planned_change_set(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    name: &str,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        last = cfn_ok(
            router,
            case,
            "DescribeChangeSet",
            &[member("StackName", stack), member("ChangeSetName", name)],
        )
        .await;
        let execution = element(&last, "ExecutionStatus");
        if execution != "UNAVAILABLE" {
            return execution;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case {case:?}: the change set {name} never settled within {DEADLINE:?}: {last}");
}

/// A CREATE is monitored by the same triggers, and the rollback of a create removes what it
/// had made: the topic the create had created is gone from SNS, while a create whose trigger
/// is not firing completes and keeps it.
#[tokio::test]
async fn an_alarm_in_alarm_rolls_the_create_it_monitors_back() {
    // (case, the stack, the trigger, the terminal status, whether SNS still holds the topic)
    let rows: &[(&str, &str, Trigger, &str, bool)] = &[
        (
            "a create whose trigger is in ALARM is rolled back",
            "rbt-create-firing",
            FIRING,
            "ROLLBACK_COMPLETE",
            false,
        ),
        (
            "a create whose trigger is in OK completes",
            "rbt-create-quiet",
            QUIET,
            "CREATE_COMPLETE",
            true,
        ),
    ];

    for (case, stack, trigger, expected_status, topic_kept) in rows {
        let router = build_router();
        let alarm = format!("{stack}-alarm");
        let Trigger::Metric(treat_missing, _) = trigger else {
            unreachable!("every create row names a metric alarm")
        };
        let arn = given_metric_alarm(&router, case, &alarm, treat_missing).await;
        alarm_settles_in(&router, case, &alarm, trigger.state()).await;

        let topic = format!("{stack}-topic");
        let mut members = vec![
            member("StackName", stack),
            member("TemplateBody", &template(&topic, BEFORE)),
        ];
        members.extend(rollback_configuration(
            std::slice::from_ref(&arn),
            Some(5),
        ));
        cfn_ok(&router, case, "CreateStack", &members).await;

        let settled = settled_stack(&router, case, stack).await;
        let status = opt_element(&settled, "StackStatus").unwrap_or_default();
        assert_eq!(
            &status, expected_status,
            "case {case:?}: expected {expected_status}, got {status:?} in {settled}"
        );
        if trigger.state() == ALARM_STATE {
            let reason = opt_element(&settled, "StackStatusReason").unwrap_or_default();
            assert_eq!(
                reason,
                trigger_failure_summary([arn.as_str()]),
                "case {case:?}: expected the reason to name the trigger that fired, got \
                 {reason:?}"
            );
        }

        // The resource record survives the rollback, at the status the rollback left it in,
        // and it is what says which topic the create had made.
        let topic_arn = stack_topic_arn(&router, case, stack).await;
        let held = topic_attributes(&router, &topic_arn).await;
        assert_eq!(
            held.is_some(),
            *topic_kept,
            "case {case:?}: expected SNS to {} the topic {topic_arn}, got {held:?}",
            if *topic_kept { "hold" } else { "have removed" }
        );
    }
}
