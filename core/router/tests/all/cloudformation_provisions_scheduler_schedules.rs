// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Scheduler::ScheduleGroup` or an
//! `AWS::Scheduler::Schedule` really creates them in Amazon EventBridge Scheduler.
//!
//! The rule under test: **a stack reaches a successful terminal state only once it has
//! created every declared resource of a type this emulator provisions through the OWNING
//! service's own create API, so each one is readable through that service's own read right
//! after the stack completes, carrying the configuration the template declared — and
//! deleting the stack deletes it there.** Four crates have to agree for that to hold
//! (CloudFormation makes the calls; EventBridge Scheduler, IAM and Amazon SQS serve them),
//! so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! group and the schedule or leaves the name to CloudFormation, whether the schedule names
//! a declared group or none at all (the account's `default` group), how many schedules one
//! group holds (one, two), which form the `ScheduleExpression` takes (`rate`, `cron`),
//! which `State` the schedule declares (and whether it declares one at all), which
//! `FlexibleTimeWindow` mode it asks for, whether it carries the optional `Description`,
//! `ScheduleExpressionTimezone`, `StartDate`/`EndDate`, `Target.Input` and
//! `Target.RetryPolicy`, whether the template is JSON or YAML, and which Region the stack
//! is deployed in.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none,
//! which is CloudFormation's to compute: what is checked about it is that the group
//! EventBridge Scheduler holds under that name is the stack's, and that the schedules the
//! row declares are in it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`).
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the template it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// Long enough for a deleted schedule group's own clock-owed span
/// (`GROUP_DELETION_DELAY`, 500 ms) to come due, so the group is gone rather than
/// `DELETING`.
const PAST_GROUP_DELETION_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// The group a schedule that names none lands in (`GetSchedule$GroupName`: "if you omit
/// this, the default schedule group is used"), which AWS creates with the account.
const DEFAULT_GROUP: &str = "default";

/// `2030-01-01T00:00:00Z` and `2031-01-01T00:00:00Z` as the seconds since the epoch the
/// `restJson1` wire carries a timestamp member as — the two instants the dated row
/// declares, written in the template as the ISO-8601 text a CloudFormation template spells
/// a timestamp property with.
const START_DATE: &str = "2030-01-01T00:00:00Z";
const START_DATE_EPOCH_SECONDS: f64 = 1_893_456_000.0;
const END_DATE: &str = "2031-01-01T00:00:00Z";
const END_DATE_EPOCH_SECONDS: f64 = 1_924_992_000.0;

/// The emulator with the four services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request either
/// way (the credential scope is what routes it).
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
        service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one EventBridge Scheduler operation over its `restJson1` wire, on the URI that
/// operation's `@http` binding declares, signed for `region`.
async fn scheduler(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("scheduler.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("scheduler", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The ARN a schedule group of `name` carries in `region`: the same total function of the
/// Region and the name both services compute it from.
fn group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:scheduler:{region}:{ACCOUNT_ID}:schedule-group/{name}")
}

/// The ARN a schedule of `name` in `group` carries in `region`.
fn schedule_arn(region: &str, group: &str, name: &str) -> String {
    format!("arn:aws:scheduler:{region}:{ACCOUNT_ID}:schedule/{group}/{name}")
}

/// The trust policy every row's execution role carries: EventBridge Scheduler resolves a
/// schedule's `Target.RoleArn` against IAM when the schedule is written, so the role the
/// stack creates has to admit the Scheduler service principal.
fn scheduler_trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "scheduler.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// One schedule a row declares, and what the rule says EventBridge Scheduler must then
/// report about it.
struct ExpectedSchedule {
    /// The output key whose value is the schedule's name (its `Ref`).
    name_output: &'static str,
    /// The output key whose value is the schedule's ARN (`Fn::GetAtt … Arn`).
    arn_output: &'static str,
    /// The name the template declared for it.
    name: &'static str,
    /// The `ScheduleState` the read must report — the declared one, or `ENABLED` for a
    /// declaration that names none.
    state: &'static str,
    /// The `ScheduleExpression` the template declared.
    expression: &'static str,
    /// The `FlexibleTimeWindow.Mode` it declared, and the window it asked for.
    flexible_mode: &'static str,
    maximum_window_in_minutes: Option<u64>,
    /// The optional members the declaration carries, each of which the read must report
    /// back; `None` is a member the declaration leaves out, which the read must not report.
    description: Option<&'static str>,
    timezone: Option<&'static str>,
    input: Option<&'static str>,
    /// Whether the declaration dates the schedule ([`START_DATE`], [`END_DATE`]).
    dated: bool,
}

/// One row of the table: a template, and the group and schedules the rule says it must
/// leave in EventBridge Scheduler.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the resources are created in and
    /// the one their ARNs carry.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The output key whose value is the declared group's name, or `None` for a row that
    /// declares no group — whose schedules land in the account's `default` group.
    group_name_output: Option<&'static str>,
    /// The output key whose value is the declared group's ARN.
    group_arn_output: Option<&'static str>,
    /// The tags `scheduler ListTagsForResource` must report for the group, beside the three
    /// CloudFormation records on every resource it provisions.
    group_tags: &'static [(&'static str, &'static str)],
    /// The schedules the template declares in that group.
    schedules: &'static [ExpectedSchedule],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named group with one named schedule, a queue and a role, in JSON",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an EventBridge Scheduler group and schedule",
                "Resources": {
                    "ScheduleGroup": {
                        "Type": "AWS::Scheduler::ScheduleGroup",
                        "Properties": {
                            "Name": "orders-group",
                            "Tags": [{ "Key": "Owner", "Value": "orders" }]
                        }
                    },
                    "Queue": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": { "QueueName": "orders-queue" }
                    },
                    "SchedulerRole": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "RoleName": "orders-scheduler-role",
                            "AssumeRolePolicyDocument": scheduler_trust_policy()
                        }
                    },
                    "Schedule": {
                        "Type": "AWS::Scheduler::Schedule",
                        "Properties": {
                            "Name": "orders-schedule",
                            "GroupName": { "Ref": "ScheduleGroup" },
                            "ScheduleExpression": "rate(1 day)",
                            "FlexibleTimeWindow": { "Mode": "OFF" },
                            "State": "ENABLED",
                            "Target": {
                                "Arn": { "Fn::GetAtt": ["Queue", "Arn"] },
                                "RoleArn": { "Fn::GetAtt": ["SchedulerRole", "Arn"] }
                            }
                        }
                    }
                },
                "Outputs": {
                    "GroupRef": { "Value": { "Ref": "ScheduleGroup" } },
                    "GroupArn": { "Value": { "Fn::GetAtt": ["ScheduleGroup", "Arn"] } },
                    "ScheduleRef": { "Value": { "Ref": "Schedule" } },
                    "ScheduleArn": { "Value": { "Fn::GetAtt": ["Schedule", "Arn"] } }
                }
            })
            .to_string()
        },
        group_name_output: Some("GroupRef"),
        group_arn_output: Some("GroupArn"),
        group_tags: &[("Owner", "orders")],
        schedules: &[ExpectedSchedule {
            name_output: "ScheduleRef",
            arn_output: "ScheduleArn",
            name: "orders-schedule",
            state: "ENABLED",
            expression: "rate(1 day)",
            flexible_mode: "OFF",
            maximum_window_in_minutes: None,
            description: None,
            timezone: None,
            input: None,
            dated: false,
        }],
    },
    Row {
        case: "a group CloudFormation names, holding two schedules, in YAML",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Group:",
                "    Type: AWS::Scheduler::ScheduleGroup",
                "  Queue:",
                "    Type: AWS::SQS::Queue",
                "  Role:",
                "    Type: AWS::IAM::Role",
                "    Properties:",
                "      AssumeRolePolicyDocument:",
                "        Version: \"2012-10-17\"",
                "        Statement:",
                "          - Effect: Allow",
                "            Principal:",
                "              Service: scheduler.amazonaws.com",
                "            Action: sts:AssumeRole",
                "  Nightly:",
                "    Type: AWS::Scheduler::Schedule",
                "    Properties:",
                "      Name: nightly-report",
                "      GroupName: !Ref Group",
                "      Description: the nightly report",
                "      ScheduleExpression: cron(0 3 * * ? *)",
                "      ScheduleExpressionTimezone: Europe/Berlin",
                "      State: DISABLED",
                "      FlexibleTimeWindow:",
                "        Mode: FLEXIBLE",
                "        MaximumWindowInMinutes: 15",
                "      Target:",
                "        Arn: !GetAtt Queue.Arn",
                "        RoleArn: !GetAtt Role.Arn",
                "        Input: '{\"report\":\"nightly\"}'",
                "        RetryPolicy:",
                "          MaximumEventAgeInSeconds: 120",
                "          MaximumRetryAttempts: 3",
                "  Hourly:",
                "    Type: AWS::Scheduler::Schedule",
                "    Properties:",
                "      Name: hourly-sweep",
                "      GroupName: !Ref Group",
                "      ScheduleExpression: rate(1 hour)",
                "      State: ENABLED",
                "      FlexibleTimeWindow:",
                "        Mode: 'OFF'",
                "      Target:",
                "        Arn: !GetAtt Queue.Arn",
                "        RoleArn: !GetAtt Role.Arn",
                "Outputs:",
                "  GroupRef:",
                "    Value: !Ref Group",
                "  GroupArn:",
                "    Value: !GetAtt Group.Arn",
                "  NightlyRef:",
                "    Value: !Ref Nightly",
                "  NightlyArn:",
                "    Value: !GetAtt Nightly.Arn",
                "  HourlyRef:",
                "    Value: !Ref Hourly",
                "  HourlyArn:",
                "    Value: !GetAtt Hourly.Arn",
                "",
            ]
            .join("\n")
        },
        group_name_output: Some("GroupRef"),
        group_arn_output: Some("GroupArn"),
        group_tags: &[],
        schedules: &[
            ExpectedSchedule {
                name_output: "HourlyRef",
                arn_output: "HourlyArn",
                name: "hourly-sweep",
                state: "ENABLED",
                expression: "rate(1 hour)",
                flexible_mode: "OFF",
                maximum_window_in_minutes: None,
                description: None,
                timezone: None,
                input: None,
                dated: false,
            },
            ExpectedSchedule {
                name_output: "NightlyRef",
                arn_output: "NightlyArn",
                name: "nightly-report",
                state: "DISABLED",
                expression: "cron(0 3 * * ? *)",
                flexible_mode: "FLEXIBLE",
                maximum_window_in_minutes: Some(15),
                description: Some("the nightly report"),
                timezone: Some("Europe/Berlin"),
                input: Some("{\"report\":\"nightly\"}"),
                dated: false,
            },
        ],
    },
    Row {
        case: "a dated schedule naming no group, so it lands in the default group",
        region: "ap-southeast-2",
        template: || {
            json!({
                "Resources": {
                    "Queue": { "Type": "AWS::SQS::Queue" },
                    "Role": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "AssumeRolePolicyDocument": scheduler_trust_policy()
                        }
                    },
                    "Sweep": {
                        "Type": "AWS::Scheduler::Schedule",
                        "Properties": {
                            "Name": "default-group-sweep",
                            "ScheduleExpression": "rate(5 minutes)",
                            "StartDate": START_DATE,
                            "EndDate": END_DATE,
                            "FlexibleTimeWindow": { "Mode": "OFF" },
                            "Target": {
                                "Arn": { "Fn::GetAtt": ["Queue", "Arn"] },
                                "RoleArn": { "Fn::GetAtt": ["Role", "Arn"] }
                            }
                        }
                    }
                },
                "Outputs": {
                    "SweepRef": { "Value": { "Ref": "Sweep" } },
                    "SweepArn": { "Value": { "Fn::GetAtt": ["Sweep", "Arn"] } }
                }
            })
            .to_string()
        },
        group_name_output: None,
        group_arn_output: None,
        group_tags: &[],
        schedules: &[ExpectedSchedule {
            name_output: "SweepRef",
            arn_output: "SweepArn",
            name: "default-group-sweep",
            // The declaration names no `State`, and `CreateSchedule` defaults it.
            state: "ENABLED",
            expression: "rate(5 minutes)",
            flexible_mode: "OFF",
            maximum_window_in_minutes: None,
            description: None,
            timezone: None,
            input: None,
            dated: true,
        }],
    },
];

/// Every row: the stack reaches `CREATE_COMPLETE`, and the very next read through
/// EventBridge Scheduler's own API resolves the group and the schedules the template
/// declared — the group `ACTIVE` under the ARN the stack published, and `ListSchedules`
/// over that group reporting exactly the declared schedules with their declared state and
/// configuration. Deleting the stack deletes them there.
#[tokio::test]
async fn a_stack_provisions_the_scheduler_resources_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("scheduler-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                // The named-role row declares its role's `RoleName`, which is the
                // acknowledgement CloudFormation asks for separately.
                ("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The stack reports the template it deployed once the create COMPLETES.
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );
        let outputs = outputs_of(&described);

        // The group the schedules belong to: the one the template declared, under the name
        // it asked for or the one CloudFormation generated, else the account's `default`.
        let group = match row.group_name_output {
            Some(key) => outputs
                .get(key)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no output {key:?} in {described}")),
            None => DEFAULT_GROUP.to_string(),
        };
        let expected_group_arn = group_arn(region, &group);
        if let Some(key) = row.group_arn_output {
            assert_eq!(
                outputs.get(key),
                Some(&expected_group_arn),
                "case {case:?}: expected output {key:?} to be the group ARN \
                 {expected_group_arn:?}, got {:?}",
                outputs.get(key)
            );
        }

        // The read the issue reported as a 404: the group the stack says it created is one
        // EventBridge Scheduler holds, ACTIVE, under that ARN.
        let (status, reported) = scheduler(
            &router,
            region,
            "GET",
            &format!("/schedule-groups/{group}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetScheduleGroup({group}) right after \
             CREATE_COMPLETE, got {status} with body {reported}"
        );
        assert_eq!(
            reported.get("State").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: expected the group to be ACTIVE, got {reported}"
        );
        assert_eq!(
            reported.get("Name").and_then(Value::as_str),
            Some(group.as_str()),
            "case {case:?}: expected the group to report its own name, got {reported}"
        );
        assert_eq!(
            reported.get("Arn").and_then(Value::as_str),
            Some(expected_group_arn.as_str()),
            "case {case:?}: expected the group ARN {expected_group_arn:?}, got {reported}"
        );

        // And `ListSchedules` over that group reports exactly the declared schedules.
        let (status, listed) = scheduler(
            &router,
            region,
            "GET",
            &format!("/schedules?ScheduleGroup={group}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListSchedules(GroupName={group}), got \
             {status} with body {listed}"
        );
        let summaries = listed
            .get("Schedules")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no 'Schedules' array in {listed}"));
        assert_eq!(
            summaries.len(),
            row.schedules.len(),
            "case {case:?}: expected exactly the {} declared schedule(s) in {group}, got \
             {listed}",
            row.schedules.len()
        );

        for expected in row.schedules {
            let name = outputs.get(expected.name_output).cloned().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no output {:?} in {described}",
                    expected.name_output
                )
            });
            assert_eq!(
                name, expected.name,
                "case {case:?}: expected Ref of the schedule to be its name {:?}, got {name:?}",
                expected.name
            );
            let arn = schedule_arn(region, &group, expected.name);
            assert_eq!(
                outputs.get(expected.arn_output),
                Some(&arn),
                "case {case:?}: expected output {:?} to be the schedule ARN {arn:?}, got {:?}",
                expected.arn_output,
                outputs.get(expected.arn_output)
            );

            let summary = summaries
                .iter()
                .find(|summary| {
                    summary.get("Name").and_then(Value::as_str) == Some(expected.name)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {:?} is not among the schedules of {group}: {listed}",
                        expected.name
                    )
                });
            assert_eq!(
                summary.get("GroupName").and_then(Value::as_str),
                Some(group.as_str()),
                "case {case:?}: expected the schedule to be in {group:?}, got {summary}"
            );
            assert_eq!(
                summary.get("State").and_then(Value::as_str),
                Some(expected.state),
                "case {case:?}: expected the listed state {:?}, got {summary}",
                expected.state
            );
            assert_eq!(
                summary.get("Arn").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?}: expected the listed ARN {arn:?}, got {summary}"
            );

            // The schedule's own read: the configuration the template declared, through
            // the pair EventBridge Scheduler keys a schedule by.
            let (status, schedule) = scheduler(
                &router,
                region,
                "GET",
                &format!("/schedules/{}?groupName={group}", expected.name),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetSchedule({:?}), got {status} with \
                 body {schedule}",
                expected.name
            );
            assert_eq!(
                schedule.get("ScheduleExpression").and_then(Value::as_str),
                Some(expected.expression),
                "case {case:?}: expected the declared expression {:?}, got {schedule}",
                expected.expression
            );
            assert_eq!(
                schedule
                    .get("FlexibleTimeWindow")
                    .and_then(|window| window.get("Mode"))
                    .and_then(Value::as_str),
                Some(expected.flexible_mode),
                "case {case:?}: expected the declared flexible-window mode {:?}, got \
                 {schedule}",
                expected.flexible_mode
            );
            assert_eq!(
                schedule
                    .get("FlexibleTimeWindow")
                    .and_then(|window| window.get("MaximumWindowInMinutes"))
                    .and_then(Value::as_u64),
                expected.maximum_window_in_minutes,
                "case {case:?}: expected the declared window {:?}, got {schedule}",
                expected.maximum_window_in_minutes
            );
            assert_eq!(
                schedule.get("Description").and_then(Value::as_str),
                expected.description,
                "case {case:?}: expected the declared description {:?}, got {schedule}",
                expected.description
            );
            assert_eq!(
                schedule
                    .get("ScheduleExpressionTimezone")
                    .and_then(Value::as_str),
                expected.timezone,
                "case {case:?}: expected the declared timezone {:?}, got {schedule}",
                expected.timezone
            );
            let target = schedule.get("Target").cloned().unwrap_or(Value::Null);
            assert_eq!(
                target.get("Input").and_then(Value::as_str),
                expected.input,
                "case {case:?}: expected the declared target input {:?}, got {schedule}",
                expected.input
            );
            // The target is the queue and the role the SAME stack created, reached through
            // the intrinsics the template read them with.
            let target_arn = target
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                target_arn.starts_with(&format!("arn:aws:sqs:{region}:{ACCOUNT_ID}:")),
                "case {case:?}: expected the target to be the stack's own queue, got \
                 {target_arn:?}"
            );
            let role_arn = target
                .get("RoleArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                role_arn.starts_with(&format!("arn:aws:iam::{ACCOUNT_ID}:role/")),
                "case {case:?}: expected the target to carry the stack's own role, got \
                 {role_arn:?}"
            );
            let dates = [
                ("StartDate", START_DATE_EPOCH_SECONDS),
                ("EndDate", END_DATE_EPOCH_SECONDS),
            ];
            for (member, expected_seconds) in dates {
                let reported = schedule.get(member).and_then(Value::as_f64);
                let wanted = expected.dated.then_some(expected_seconds);
                assert_eq!(
                    reported, wanted,
                    "case {case:?}: expected {member} to be {wanted:?}, got {schedule}"
                );
            }
        }

        // Deleting the stack deletes what it created there: the schedules go, and a group
        // the stack owns goes with them.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        for expected in row.schedules {
            let (status, gone) = scheduler(
                &router,
                region,
                "GET",
                &format!("/schedules/{}?groupName={group}", expected.name),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 from GetSchedule once the stack deleted \
                 {:?}, got {status} with body {gone}",
                expected.name
            );
        }
        if row.group_name_output.is_some() {
            // A group whose deletion is under way is `DELETING` until its own clock-owed
            // span comes due, and gone after it.
            tokio::time::sleep(PAST_GROUP_DELETION_SPAN).await;
            let (status, gone) = scheduler(
                &router,
                region,
                "GET",
                &format!("/schedule-groups/{group}"),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 from GetScheduleGroup once the stack \
                 deleted {group:?}, got {status} with body {gone}"
            );
        } else {
            // The account's own `default` group is not the stack's to delete.
            let (status, still_there) = scheduler(
                &router,
                region,
                "GET",
                &format!("/schedule-groups/{DEFAULT_GROUP}"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the account's default group to survive the stack, \
                 got {status} with body {still_there}"
            );
        }
    }
}

/// A stack whose declaration EventBridge Scheduler — or this mapping onto its API — REFUSES
/// does not report a resource nothing created: the stack fails, naming the resource, and no
/// schedule is created.
///
/// The two rows differ along where the refusal comes from: the owning service's own
/// (`CreateScheduleGroup` refuses the `default` group, which is AWS's, created with the
/// account) and the request this mapping builds (a `StartDate` that is not an instant is
/// refused naming the member rather than sent as text a timestamp member cannot carry).
#[tokio::test]
async fn a_declaration_the_owning_service_refuses_fails_the_stack() {
    /// A template declaring a group, a queue, a role and one schedule, with `group` as the
    /// schedule's group and `extra` merged into the schedule's own properties.
    fn template(group: Value, extra: Value) -> String {
        let mut properties = json!({
            "Name": "never-created",
            "GroupName": group,
            "ScheduleExpression": "rate(1 day)",
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "Target": {
                "Arn": { "Fn::GetAtt": ["Queue", "Arn"] },
                "RoleArn": { "Fn::GetAtt": ["Role", "Arn"] }
            }
        });
        if let (Some(properties), Some(extra)) = (properties.as_object_mut(), extra.as_object())
        {
            for (member, value) in extra {
                properties.insert(member.clone(), value.clone());
            }
        }
        json!({
            "Resources": {
                "Group": {
                    "Type": "AWS::Scheduler::ScheduleGroup",
                    "Properties": { "Name": { "Ref": "GroupNameParameter" } }
                },
                "Queue": { "Type": "AWS::SQS::Queue" },
                "Role": {
                    "Type": "AWS::IAM::Role",
                    "Properties": { "AssumeRolePolicyDocument": scheduler_trust_policy() }
                },
                "Schedule": {
                    "Type": "AWS::Scheduler::Schedule",
                    "Properties": properties
                }
            },
            "Parameters": {
                "GroupNameParameter": { "Type": "String" }
            }
        })
        .to_string()
    }

    let router = build_router();
    let region = "us-east-2";
    let cases = [
        (
            "the reserved default group, which that service refuses",
            "reserved-group",
            DEFAULT_GROUP.to_string(),
            template(json!({ "Ref": "Group" }), Value::Null),
            "Group",
        ),
        (
            "a StartDate that is not an instant",
            "unparsable-date",
            "dated-group".to_string(),
            template(
                json!({ "Ref": "Group" }),
                json!({ "StartDate": "the first of never" }),
            ),
            "Schedule",
        ),
    ];

    for (case, stack_name, group, body, failing) in cases {
        let (status, answered) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", stack_name),
                ("TemplateBody", &body),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Parameters.member.1.ParameterKey", "GroupNameParameter"),
                ("Parameters.member.1.ParameterValue", &group),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body \
             {answered}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back rather than report \
             a resource nothing created, got {described}"
        );
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains(failing),
            "case {case:?}: expected the failure to name {failing:?}, got {reason:?}"
        );

        // Nothing was put in the group the declaration pointed at.
        let (status, listed) = scheduler(
            &router,
            region,
            "GET",
            &format!("/schedules?ScheduleGroup={group}"),
        )
        .await;
        let schedules = match status {
            // The group the first row named is AWS's own and is there; the one the second
            // row declared was created, and is empty.
            200 => listed
                .get("Schedules")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            // A group that was never created holds nothing either.
            404 => Vec::new(),
            other => panic!(
                "case {case:?}: expected 200 or 404 from ListSchedules, got {other} with body \
                 {listed}"
            ),
        };
        assert!(
            schedules.is_empty(),
            "case {case:?}: expected no schedule to have been created, got {listed}"
        );
    }
}

/// The tags CloudFormation propagates to a schedule group it provisions — the stack-level
/// `Tags` — are on the group EventBridge Scheduler holds, beside the ones the declaration
/// itself asked for. The three keys CloudFormation records of its own accord are in AWS's
/// reserved `aws:` namespace, which this service's own tag validation refuses to a caller,
/// so they are left off rather than sent to be refused: a stack must not fail over tags
/// CloudFormation adds itself.
#[tokio::test]
async fn a_provisioned_schedule_group_carries_the_tags_cloudformation_gives_it() {
    let router = build_router();
    let region = "us-west-2";

    for (index, row) in ROWS.iter().enumerate() {
        let Some(group_output) = row.group_name_output else {
            continue;
        };
        let case = row.case;
        let stack_name = format!("scheduler-group-tags-{index}");
        let template = (row.template)();
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
                ("Tags.member.1.Key", "Service"),
                ("Tags.member.1.Value", "scheduling"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );
        let outputs = outputs_of(&described);
        let group = outputs
            .get(group_output)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no output {group_output:?}"));
        let arn = group_arn(region, &group);

        // `GET /tags/{ResourceArn}`: the ARN is a non-greedy `@httpLabel`, so it travels as
        // ONE path segment with everything but the unreserved characters escaped — the same
        // encoding a form component takes, which is why it rides on the one encoder this
        // crate writes rather than a second copy of it.
        let (status, listed) =
            scheduler(&router, region, "GET", &format!("/tags/{}", form_encode(&arn))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource({arn}), got {status} \
             with body {listed}"
        );
        let reported: BTreeMap<String, String> = listed
            .get("Tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no 'Tags' array in {listed}"))
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("Key").and_then(Value::as_str)?.to_string(),
                    tag.get("Value").and_then(Value::as_str)?.to_string(),
                ))
            })
            .collect();
        let mut expected: Vec<(String, String)> = row
            .group_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected.push(("Service".to_string(), "scheduling".to_string()));
        for (key, value) in &expected {
            assert_eq!(
                reported.get(key).map(String::as_str),
                Some(value.as_str()),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case:?}: expected exactly the {} tag(s) the declaration and the stack ask \
             for, got {reported:?}",
            expected.len()
        );
    }
}

/// A `ScheduleGroupNamePrefix` (modelled `length 1..64`, pattern `^[0-9a-zA-Z-_.]+$`) no group
/// in these Regions starts with: neither a name a row declares or leaves to CloudFormation
/// (`<stack name>-<logical id>-<unique id>`, and every stack here is named
/// `scheduler-group-listing-…`) nor the account's own `default` group begins with it.
const UNMATCHED_PREFIX: &str = "zz";

/// How many leading characters the partial-prefix probe filters by — a PROPER prefix of
/// every group name a row in [`ROWS`] ends up with.
const PARTIAL_PREFIX_CHARS: usize = 4;

/// Most pages [`list_schedule_groups`] follows before it calls a `NextToken` chain
/// non-terminating: these Regions hold a handful of groups, so even a page size of one is
/// well inside it.
const MAX_LISTING_PAGES: usize = 16;

/// One form the paginated read takes, and whether the group the stack declared has to be
/// among the entries that form enumerates.
struct ListingProbe {
    case: &'static str,
    /// The `NamePrefix` the read filters by, as a function of the group name the stack
    /// published; `None` leaves the filter off, so the whole account/Region is enumerated.
    prefix: fn(&str) -> Option<String>,
    /// The `MaxResults` page size the read asks for, `None` for one that names none. Both
    /// values are inside the modelled `range 1..100`
    /// (`smithy_query(operation="ListScheduleGroups")`); the bounds themselves, and one past
    /// each, are the owning crate's own suite.
    max_results: Option<u32>,
    /// Whether the declared group is among the enumerated entries.
    enumerates_group: bool,
}

/// The rows the listing is probed along: whether the read filters at all, by the group's
/// whole name, by a proper prefix of it or by a prefix nothing starts with, and how many
/// entries it asks for per page — so the group is reached both in one page and only by
/// following a `NextToken`.
const PROBES: &[ListingProbe] = &[
    ListingProbe {
        case: "no filter, the page size the service chooses",
        prefix: |_| None,
        max_results: None,
        enumerates_group: true,
    },
    ListingProbe {
        case: "no filter, one group per page, so the group is reached through NextToken",
        prefix: |_| None,
        max_results: Some(1),
        enumerates_group: true,
    },
    ListingProbe {
        case: "the group's whole name as the prefix",
        prefix: |group| Some(group.to_string()),
        max_results: None,
        enumerates_group: true,
    },
    ListingProbe {
        case: "a proper prefix of the group's name",
        prefix: |group| Some(group.chars().take(PARTIAL_PREFIX_CHARS).collect()),
        max_results: None,
        enumerates_group: true,
    },
    ListingProbe {
        case: "the whole name, asking for the largest page the model admits",
        prefix: |group| Some(group.to_string()),
        max_results: Some(100),
        enumerates_group: true,
    },
    ListingProbe {
        case: "a prefix no group starts with",
        prefix: |_| Some(UNMATCHED_PREFIX.to_string()),
        max_results: None,
        enumerates_group: false,
    },
];

/// Page through `ListScheduleGroups` (`GET /schedule-groups`, whose `NamePrefix`,
/// `MaxResults` and `NextToken` members are `@httpQuery`-bound under those names) with the
/// filter and page size asked for, following the token to the end, and answer every summary
/// the whole enumeration carried.
///
/// Each page is checked where it is read: a 200 carrying the required `ScheduleGroups` list,
/// no page longer than the size asked for, and the enumeration as a whole in ascending name
/// order with no name twice — a paginated read is one walk over one total order, so a client
/// that follows the token sees each group exactly once and terminates.
async fn list_schedule_groups(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    prefix: Option<&str>,
    max_results: Option<u32>,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    let mut pages = 0usize;
    loop {
        pages += 1;
        assert!(
            pages <= MAX_LISTING_PAGES,
            "case {case:?}: expected the NextToken chain to terminate within \
             {MAX_LISTING_PAGES} pages, collected {collected:?}"
        );
        let mut query: Vec<String> = Vec::new();
        if let Some(prefix) = prefix {
            query.push(format!("NamePrefix={}", form_encode(prefix)));
        }
        if let Some(max) = max_results {
            query.push(format!("MaxResults={max}"));
        }
        if let Some(token) = &next {
            query.push(format!("NextToken={}", form_encode(token)));
        }
        let uri = if query.is_empty() {
            "/schedule-groups".to_string()
        } else {
            format!("/schedule-groups?{}", query.join("&"))
        };

        let (status, answered) = scheduler(router, region, "GET", &uri).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListScheduleGroups({uri}), got {status} \
             with body {answered}"
        );
        let page = answered
            .get("ScheduleGroups")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: no required 'ScheduleGroups' list in {answered}")
            });
        if let Some(limit) = max_results {
            assert!(
                page.len() <= limit as usize,
                "case {case:?}: expected at most {limit} group(s) in a page, got {} in \
                 {answered}",
                page.len()
            );
        }
        collected.extend(page);
        next = answered
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next.is_none() {
            break;
        }
    }

    let names = listed_names(&collected);
    let mut in_order = names.clone();
    in_order.sort();
    in_order.dedup();
    assert_eq!(
        names, in_order,
        "case {case:?}: expected the enumeration in ascending name order with each group \
         once, got {names:?}"
    );
    collected
}

/// The `Name` of every summary, in the order the enumeration carried them.
fn listed_names(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .filter_map(|summary| summary.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Every row: the group a `CREATE_COMPLETE` stack declared is ENUMERATED by EventBridge
/// Scheduler's own paginated listing — `State` `ACTIVE`, under the ARN the stack published —
/// in every form the read takes, and it is gone from the listing once the stack that
/// declared it is deleted.
///
/// The same rule the sibling suites check through `GetScheduleGroup` and `ListSchedules`, at
/// the DISCOVERY observation point: a resource a stack provisioned through the owning
/// service's create API is one that service's own list operation reports, so a client that
/// finds its resources by listing finds them. The probes vary what the rule generalises
/// over — whether the read filters by name at all, by the whole name, by a proper prefix or
/// by a prefix nothing matches, and how many entries it asks for per page — on top of the
/// rows' own variation in whether the template names the group, which Region it deploys in
/// and which form it is written in.
#[tokio::test]
async fn a_declared_group_is_enumerated_by_list_schedule_groups() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let deployed = row.case;
        let region = row.region;
        let stack_name = format!("scheduler-group-listing-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {deployed:?}: expected HTTP 200 from CreateStack, got {status} with body \
             {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {deployed:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {deployed:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );
        let outputs = outputs_of(&described);
        let group = match row.group_name_output {
            Some(key) => outputs
                .get(key)
                .cloned()
                .unwrap_or_else(|| panic!("case {deployed:?}: no output {key:?} in {described}")),
            None => DEFAULT_GROUP.to_string(),
        };
        let expected_arn = group_arn(region, &group);

        for probe in PROBES {
            let case = format!("{deployed} / {}", probe.case);
            let prefix = (probe.prefix)(&group);
            let listed =
                list_schedule_groups(&router, region, &case, prefix.as_deref(), probe.max_results)
                    .await;
            let matching: Vec<&Value> = listed
                .iter()
                .filter(|summary| {
                    summary.get("Name").and_then(Value::as_str) == Some(group.as_str())
                })
                .collect();

            if probe.enumerates_group {
                assert_eq!(
                    matching.len(),
                    1,
                    "case {case:?}: expected the declared group {group:?} exactly once among \
                     the enumerated groups, got {listed:?}"
                );
                let summary = matching[0];
                assert_eq!(
                    summary.get("State").and_then(Value::as_str),
                    Some("ACTIVE"),
                    "case {case:?}: expected the enumerated group to be ACTIVE, got {summary}"
                );
                assert_eq!(
                    summary.get("Arn").and_then(Value::as_str),
                    Some(expected_arn.as_str()),
                    "case {case:?}: expected the enumerated ARN {expected_arn:?}, got {summary}"
                );
                for dated in ["CreationDate", "LastModificationDate"] {
                    let reported = summary.get(dated).and_then(Value::as_f64);
                    assert!(
                        reported.is_some_and(|seconds| seconds > 0.0),
                        "case {case:?}: expected the enumerated group to carry a server-set \
                         {dated}, got {reported:?} in {summary}"
                    );
                }
            } else {
                assert!(
                    listed.is_empty(),
                    "case {case:?}: expected a prefix no group starts with to be an empty page \
                     rather than a refusal or a match, got {listed:?}"
                );
            }

            // An unfiltered read enumerates the whole account/Region, so the group AWS
            // creates with the account is in it beside the stack's.
            if prefix.is_none() {
                let names = listed_names(&listed);
                assert!(
                    names.iter().any(|name| name == DEFAULT_GROUP),
                    "case {case:?}: expected the account's {DEFAULT_GROUP:?} group among the \
                     enumerated groups, got {names:?}"
                );
            }
        }

        // Deleting the stack takes the group it created out of the listing, while the
        // account's own `default` group — never the stack's to delete — stays in it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {deployed:?}: expected HTTP 200 from DeleteStack, got {status} with body \
             {body}"
        );
        tokio::time::sleep(PAST_GROUP_DELETION_SPAN).await;
        let case = format!("{deployed} / once the stack is deleted");
        let names = listed_names(&list_schedule_groups(&router, region, &case, None, None).await);
        if row.group_name_output.is_some() {
            assert!(
                !names.iter().any(|name| name == &group),
                "case {case:?}: expected {group:?} to be gone from the listing once the stack \
                 that declared it was deleted, got {names:?}"
            );
        } else {
            assert!(
                names.iter().any(|name| name == DEFAULT_GROUP),
                "case {case:?}: expected the account's {DEFAULT_GROUP:?} group to survive the \
                 stack, got {names:?}"
            );
        }
    }
}

/// The names one update row's stack declares, each carrying the row's index so rows deployed
/// onto the one emulator never name the same resource.
struct Declared {
    group: String,
    schedule: String,
    primary_queue: String,
    secondary_queue: String,
    role: String,
}

impl Declared {
    fn for_row(index: usize) -> Self {
        Self {
            group: format!("update-group-{index}"),
            schedule: format!("update-schedule-{index}"),
            primary_queue: format!("update-primary-{index}"),
            secondary_queue: format!("update-secondary-{index}"),
            role: format!("update-role-{index}"),
        }
    }
}

/// A template declaring a schedule group, the two queues a schedule's target can name, a
/// scheduler-trusted role and ONE schedule carrying `properties` — the row's declaration,
/// with the name CloudFormation is to give the schedule written in.
///
/// `group_tags` is the group's declared `Tags` property, left out entirely when it is
/// `Value::Null`.
fn update_template(declared: &Declared, group_tags: Value, properties: Value) -> String {
    let mut properties = properties;
    if let Some(object) = properties.as_object_mut() {
        object.insert("Name".to_string(), json!(declared.schedule));
    }
    let mut group_properties = json!({ "Name": declared.group });
    if let (Some(object), false) = (group_properties.as_object_mut(), group_tags.is_null()) {
        object.insert("Tags".to_string(), group_tags);
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Group": {
                "Type": "AWS::Scheduler::ScheduleGroup",
                "Properties": group_properties
            },
            "Primary": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": declared.primary_queue }
            },
            "Secondary": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": declared.secondary_queue }
            },
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": declared.role,
                    "AssumeRolePolicyDocument": scheduler_trust_policy()
                }
            },
            "Schedule": {
                "Type": "AWS::Scheduler::Schedule",
                "Properties": properties
            }
        },
        "Outputs": {
            "GroupRef": { "Value": { "Ref": "Group" } },
            "ScheduleRef": { "Value": { "Ref": "Schedule" } },
            "ScheduleArn": { "Value": { "Fn::GetAtt": ["Schedule", "Arn"] } }
        }
    })
    .to_string()
}

/// One stack operation (a create or an update) on an update row's stack, waited past the
/// stack's clock-owed span and answered with the `DescribeStacks` body.
async fn deploy(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    action: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        region,
        action,
        &[
            ("StackName", stack_name),
            ("TemplateBody", template),
            // Every row's template declares a role it NAMES, which is the acknowledgement
            // CloudFormation asks for separately.
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ("Tags.member.1.Key", STACK_TAG.0),
            ("Tags.member.1.Value", STACK_TAG.1),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (status, described) =
        cloudformation(router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {described}"
    );
    described
}

/// The one stack-level tag every update row deploys with, which CloudFormation propagates to
/// the resources in the stack and which therefore survives every change to a declaration's
/// own tags.
const STACK_TAG: (&str, &str) = ("Service", "scheduling");

/// The schedule group an `AWS::Scheduler::Schedule` declaration puts its schedule in: the
/// `GroupName` it names, else the account's `default` group.
fn declared_group(properties: &Value, group: &str) -> String {
    match properties.get("GroupName") {
        Some(_) => group.to_string(),
        None => DEFAULT_GROUP.to_string(),
    }
}

/// Assert EventBridge Scheduler holds, in `group`, exactly the schedule `declared` describes.
///
/// The expectation is the DECLARATION itself, member by member: a member it states must be
/// reported with that value, and one it does not state must not be reported at all — which
/// is what makes a declaration that drops a member a real assertion rather than a weaker
/// one. `State` is the one member with a system default (`ENABLED`), so a declaration that
/// drops it expects that default.
async fn assert_holds_declared_schedule(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    group: &str,
    name: &str,
    declared: &Value,
    target_queue: &str,
) {
    let wanted_target_arn = format!("arn:aws:sqs:{region}:{ACCOUNT_ID}:{target_queue}");
    let (status, schedule) = scheduler(
        router,
        region,
        "GET",
        &format!("/schedules/{name}?groupName={group}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetSchedule({name:?}) in {group:?}, got {status} \
         with body {schedule}"
    );
    assert_eq!(
        schedule.get("GroupName").and_then(Value::as_str),
        Some(group),
        "case {case:?}: expected the schedule to be in {group:?}, got {schedule}"
    );
    assert_eq!(
        schedule.get("Arn").and_then(Value::as_str),
        Some(schedule_arn(region, group, name).as_str()),
        "case {case:?}: expected the ARN of {name:?} in {group:?}, got {schedule}"
    );
    for member in ["Description", "ScheduleExpression", "ScheduleExpressionTimezone"] {
        let wanted = declared.get(member).and_then(Value::as_str);
        assert_eq!(
            schedule.get(member).and_then(Value::as_str),
            wanted,
            "case {case:?}: expected {member} to be the declared {wanted:?}, got {schedule}"
        );
    }
    let wanted_state = declared
        .get("State")
        .and_then(Value::as_str)
        .unwrap_or("ENABLED");
    assert_eq!(
        schedule.get("State").and_then(Value::as_str),
        Some(wanted_state),
        "case {case:?}: expected State to be {wanted_state:?}, got {schedule}"
    );
    let wanted_window = declared
        .get("FlexibleTimeWindow")
        .cloned()
        .unwrap_or(Value::Null);
    let window = schedule
        .get("FlexibleTimeWindow")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        window.get("Mode").and_then(Value::as_str),
        wanted_window.get("Mode").and_then(Value::as_str),
        "case {case:?}: expected the declared flexible-window mode {:?}, got {schedule}",
        wanted_window.get("Mode").and_then(Value::as_str)
    );
    assert_eq!(
        window.get("MaximumWindowInMinutes").and_then(Value::as_u64),
        wanted_window
            .get("MaximumWindowInMinutes")
            .and_then(Value::as_u64),
        "case {case:?}: expected the declared window {:?}, got {schedule}",
        wanted_window
            .get("MaximumWindowInMinutes")
            .and_then(Value::as_u64)
    );
    let declared_target = declared.get("Target").cloned().unwrap_or(Value::Null);
    let target = schedule.get("Target").cloned().unwrap_or(Value::Null);
    let wanted_input = declared_target.get("Input").and_then(Value::as_str);
    assert_eq!(
        target.get("Input").and_then(Value::as_str),
        wanted_input,
        "case {case:?}: expected the declared target input {wanted_input:?}, got {schedule}"
    );
    assert_eq!(
        target.get("Arn").and_then(Value::as_str),
        Some(wanted_target_arn.as_str()),
        "case {case:?}: expected the target to be {wanted_target_arn:?}, got {schedule}"
    );
    let wanted_role = format!("arn:aws:iam::{ACCOUNT_ID}:role/");
    let role = target
        .get("RoleArn")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        role.starts_with(&wanted_role),
        "case {case:?}: expected the target to carry the stack's own role, got {role:?}"
    );
}

/// One row of the stack-update table: the declaration the stack is created with, the one it
/// is then updated to, and which of the two queues each names as its target.
struct UpdateRow {
    case: &'static str,
    /// The schedule's property document before the update, and the queue its target names.
    before: fn() -> Value,
    before_queue: TargetQueue,
    /// The document the update declares, and the queue it names.
    after: fn() -> Value,
    after_queue: TargetQueue,
}

/// Which of the two queues a row's template declares its target names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetQueue {
    Primary,
    Secondary,
}

impl TargetQueue {
    /// The queue name the stack gives that queue, which is what its ARN carries.
    fn name_in(self, declared: &Declared) -> String {
        match self {
            Self::Primary => declared.primary_queue.clone(),
            Self::Secondary => declared.secondary_queue.clone(),
        }
    }
}

/// The base of every row's declaration: the two required members beside the target, with the
/// target naming the stack's `Primary` queue and its role.
fn primary_target() -> Value {
    json!({
        "Arn": { "Fn::GetAtt": ["Primary", "Arn"] },
        "RoleArn": { "Fn::GetAtt": ["Role", "Arn"] }
    })
}

/// The same target over the stack's OTHER queue, with the input the row sends to it.
fn secondary_target(input: &str) -> Value {
    json!({
        "Arn": { "Fn::GetAtt": ["Secondary", "Arn"] },
        "RoleArn": { "Fn::GetAtt": ["Role", "Arn"] },
        "Input": input
    })
}

/// The rows differ along what the rule generalises over: which FORM the changed expression
/// takes, whether an optional member is added, re-valued or DROPPED, whether the target
/// moves to another resource and gains an input, whether the flexible-time window opens or
/// closes, and whether the schedule moves between the group the stack declares and the
/// account's own `default` group.
const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "the expression changes from a rate to a cron, in a named timezone",
        before: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "ScheduleExpression": "rate(1 day)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        before_queue: TargetQueue::Primary,
        after: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "ScheduleExpression": "cron(0 3 * * ? *)",
                "ScheduleExpressionTimezone": "Europe/Berlin",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        after_queue: TargetQueue::Primary,
    },
    UpdateRow {
        case: "the state flips and the description the first declaration carried is dropped",
        before: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "Description": "the first pass",
                "State": "ENABLED",
                "ScheduleExpression": "rate(1 hour)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        before_queue: TargetQueue::Primary,
        after: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "State": "DISABLED",
                "ScheduleExpression": "rate(1 hour)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        after_queue: TargetQueue::Primary,
    },
    UpdateRow {
        case: "the target becomes the stack's other queue, with an input",
        before: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "ScheduleExpression": "rate(30 minutes)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        before_queue: TargetQueue::Primary,
        after: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "ScheduleExpression": "rate(30 minutes)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": secondary_target("{\"pass\":2}")
            })
        },
        after_queue: TargetQueue::Secondary,
    },
    UpdateRow {
        case: "the flexible time window opens",
        before: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "ScheduleExpression": "rate(7 days)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        before_queue: TargetQueue::Primary,
        after: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "ScheduleExpression": "rate(7 days)",
                "FlexibleTimeWindow": { "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 10 },
                "Target": primary_target()
            })
        },
        after_queue: TargetQueue::Primary,
    },
    UpdateRow {
        case: "the schedule moves out of the account's default group into the declared one",
        before: || {
            json!({
                "ScheduleExpression": "rate(2 hours)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        before_queue: TargetQueue::Primary,
        after: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "ScheduleExpression": "rate(2 hours)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        after_queue: TargetQueue::Primary,
    },
    UpdateRow {
        case: "the schedule moves out of the declared group into the account's default",
        before: || {
            json!({
                "GroupName": { "Ref": "Group" },
                "ScheduleExpression": "rate(3 hours)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        before_queue: TargetQueue::Primary,
        after: || {
            json!({
                "ScheduleExpression": "rate(3 hours)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": primary_target()
            })
        },
        after_queue: TargetQueue::Primary,
    },
];

/// Every row: a stack update that re-declares its `AWS::Scheduler::Schedule` differently
/// reaches `UPDATE_COMPLETE`, and the very next read through EventBridge Scheduler's own API
/// reports the schedule the NEW declaration describes — never the one the first declaration
/// did, and never a stack that failed over a change its owning service can make.
#[tokio::test]
async fn a_stack_update_applies_a_changed_schedule_declaration() {
    let router = build_router();
    let region = "eu-central-1";

    for (index, row) in UPDATE_ROWS.iter().enumerate() {
        let case = row.case;
        let declared = Declared::for_row(index);
        let stack_name = format!("scheduler-update-{index}");
        let before = (row.before)();
        let after = (row.after)();

        let described = deploy(
            &router,
            region,
            case,
            "CreateStack",
            &stack_name,
            &update_template(&declared, Value::Null, before.clone()),
        )
        .await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );
        let group_before = declared_group(&before, &declared.group);
        assert_holds_declared_schedule(
            &router,
            region,
            case,
            &group_before,
            &declared.schedule,
            &before,
            &row.before_queue.name_in(&declared),
        )
        .await;

        let described = deploy(
            &router,
            region,
            case,
            "UpdateStack",
            &stack_name,
            &update_template(&declared, Value::Null, after.clone()),
        )
        .await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the update to reach UPDATE_COMPLETE, got {described}"
        );
        let group_after = declared_group(&after, &declared.group);
        assert_holds_declared_schedule(
            &router,
            region,
            case,
            &group_after,
            &declared.schedule,
            &after,
            &row.after_queue.name_in(&declared),
        )
        .await;
        // The stack still publishes the ARN of the schedule that really exists, which for a
        // row that moved it between groups is a different ARN from the one it published
        // before.
        let outputs = outputs_of(&described);
        let arn = schedule_arn(region, &group_after, &declared.schedule);
        assert_eq!(
            outputs.get("ScheduleArn").map(String::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the stack to publish {arn:?}, got {outputs:?}"
        );

        // A row that moved the schedule left nothing of it behind in the group it left.
        if group_after != group_before {
            let (status, gone) = scheduler(
                &router,
                region,
                "GET",
                &format!("/schedules/{}?groupName={group_before}", declared.schedule),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 from GetSchedule in the group the schedule \
                 left ({group_before:?}), got {status} with body {gone}"
            );
        }
    }
}

/// One row of the group-tagging table: the `Tags` a group is declared with, the ones it is
/// re-declared with, and the set the rule says EventBridge Scheduler must then hold.
struct TagRow {
    case: &'static str,
    before: fn() -> Value,
    after: fn() -> Value,
    /// The declared tags the group must end up carrying, beside [`STACK_TAG`].
    expected: &'static [(&'static str, &'static str)],
}

/// The rows differ along what a re-declaration can do to a tag set: add a key, re-value one,
/// drop one, and drop the declared set entirely — the stack's own propagated tag surviving
/// every one of them.
const TAG_ROWS: &[TagRow] = &[
    TagRow {
        case: "a key is added",
        before: || json!([{ "Key": "Owner", "Value": "orders" }]),
        after: || {
            json!([
                { "Key": "Owner", "Value": "orders" },
                { "Key": "Tier", "Value": "gold" }
            ])
        },
        expected: &[("Owner", "orders"), ("Tier", "gold")],
    },
    TagRow {
        case: "a key is re-valued",
        before: || json!([{ "Key": "Owner", "Value": "orders" }]),
        after: || json!([{ "Key": "Owner", "Value": "payments" }]),
        expected: &[("Owner", "payments")],
    },
    TagRow {
        case: "a key is dropped",
        before: || {
            json!([
                { "Key": "Owner", "Value": "orders" },
                { "Key": "Tier", "Value": "gold" }
            ])
        },
        after: || json!([{ "Key": "Owner", "Value": "orders" }]),
        expected: &[("Owner", "orders")],
    },
    TagRow {
        case: "the declared tag set goes away entirely",
        before: || json!([{ "Key": "Owner", "Value": "orders" }]),
        after: || json!([]),
        expected: &[],
    },
];

/// Every row: a stack update that re-declares its `AWS::Scheduler::ScheduleGroup`'s `Tags`
/// reaches `UPDATE_COMPLETE`, and `ListTagsForResource` on the group then reports exactly
/// the set the new declaration asks for beside the stack's own propagated tag — a key the
/// re-declaration dropped is gone, and one it re-valued carries the new value.
#[tokio::test]
async fn a_stack_update_applies_a_changed_schedule_group_tag_set() {
    let router = build_router();
    let region = "us-west-1";

    for (index, row) in TAG_ROWS.iter().enumerate() {
        let case = row.case;
        // Offset past the schedule table's rows, so the two suites never name one resource.
        let declared = Declared::for_row(UPDATE_ROWS.len() + index);
        let stack_name = format!("scheduler-group-update-{index}");
        let properties = json!({
            "GroupName": { "Ref": "Group" },
            "ScheduleExpression": "rate(1 day)",
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "Target": primary_target()
        });

        let described = deploy(
            &router,
            region,
            case,
            "CreateStack",
            &stack_name,
            &update_template(&declared, (row.before)(), properties.clone()),
        )
        .await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let described = deploy(
            &router,
            region,
            case,
            "UpdateStack",
            &stack_name,
            &update_template(&declared, (row.after)(), properties.clone()),
        )
        .await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the update to reach UPDATE_COMPLETE, got {described}"
        );

        let arn = group_arn(region, &declared.group);
        let (status, listed) =
            scheduler(&router, region, "GET", &format!("/tags/{}", form_encode(&arn))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource({arn}), got {status} with \
             body {listed}"
        );
        let reported: BTreeMap<String, String> = listed
            .get("Tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no 'Tags' array in {listed}"))
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("Key").and_then(Value::as_str)?.to_string(),
                    tag.get("Value").and_then(Value::as_str)?.to_string(),
                ))
            })
            .collect();
        let mut expected: BTreeMap<String, String> = row
            .expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected.insert(STACK_TAG.0.to_string(), STACK_TAG.1.to_string());
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the group to carry exactly {expected:?}, got {reported:?}"
        );

        // The schedule the group holds is untouched by a change to the group's own tags.
        assert_holds_declared_schedule(
            &router,
            region,
            case,
            &declared.group,
            &declared.schedule,
            &properties,
            &declared.primary_queue,
        )
        .await;
    }
}
