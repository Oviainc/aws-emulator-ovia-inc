// SPDX-License-Identifier: CC0-1.0

//! The STACK SET LIFECYCLE EVENTS CloudFormation publishes onto the account's default
//! EventBridge bus, observed where a client observes them: on the Amazon SQS queue a rule on
//! that bus delivers to.
//!
//! The rule pinned here: **every status change of a stack set's lifecycle is published onto
//! the account's default event bus with `source` `aws.cloudformation`, in the stack set's own
//! account and Region, naming the stack set's ARN both in the event's `resources` and in
//! `detail.stack-set-arn` — the stack set reaching a status (`detail-type` "CloudFormation
//! StackSet Status Change", `detail.status-details.status`), a stack-set OPERATION starting
//! and then concluding ("CloudFormation StackSet Operation Status Change", carrying the
//! `OperationId` the API call answered and the operation's `action`, going RUNNING then
//! SUCCEEDED, the RUNNING event's own `time` no later than the conclusion's), and each stack
//! INSTANCE that operation acted on reaching its own status ("CloudFormation StackSet
//! StackInstance Status Change", carrying that instance's stack id and its
//! `status`/`detailed-status`).** Each event reports the record a read of the same instant
//! answers from, so `DescribeStackSetOperation`, `ListStackInstances` and the events cannot
//! disagree.
//!
//! Four crates have to agree for that to hold — CloudFormation publishes, EventBridge matches
//! the event against the rules of the bus and hands it to their targets, Amazon SQS holds what
//! was delivered, and Systems Manager holds the parameter the instance's stack really
//! deployed — so the suite lives here rather than in any one of them. Nothing is asserted on
//! an emulator internal: every observation is a client call.
//!
//! The rows differ along what the rule generalises over: the template spelling (JSON and
//! YAML), how many instances one operation acts on and therefore how many instance events it
//! owes (one, two), whether the target Region is the stack set's own or another, and which
//! parameter name and value the deployed template carries. The recorded case — one instance in
//! the caller's own Region — is the first row.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element};

/// The Region every administrator call below is signed for, which is the Region the stack
/// sets of this suite belong to and the Region their events are published in.
const REGION: &str = "us-west-2";

/// A second Region, so a row can deploy an instance outside the stack set's own.
const OTHER_REGION: &str = "us-east-1";

/// The account this emulator serves, which is the account the events carry.
const ACCOUNT: &str = "000000000000";

/// The `source` every event CloudFormation publishes carries.
const SOURCE: &str = "aws.cloudformation";

/// The three `detail-type` values a stack set's lifecycle publishes under.
const STACK_SET_CHANGE: &str = "CloudFormation StackSet Status Change";
const OPERATION_CHANGE: &str = "CloudFormation StackSet Operation Status Change";
const INSTANCE_CHANGE: &str = "CloudFormation StackSet StackInstance Status Change";

/// The statuses asserted on, as the model spells them.
const ACTIVE: &str = "ACTIVE";
const DELETED: &str = "DELETED";
const RUNNING: &str = "RUNNING";
const SUCCEEDED: &str = "SUCCEEDED";
const CURRENT: &str = "CURRENT";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `detail.action` of the two operations each row drives.
const CREATE: &str = "CREATE";
const DELETE: &str = "DELETE";

/// How long a poll or a drain waits for something the clock owes, and how often it looks.
const BUDGET: Duration = Duration::from_secs(20);
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// The whole emulator over one engine, exactly as the binary assembles it: which service
/// claims each call — CloudFormation's own in-process `PutEvents` and EventBridge's delivery
/// into Amazon SQS included — is decided by the production mount.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service` and `region`, which is what
/// routes the request. The signature is never checked here.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery call, as its status and XML body.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    region: &str,
    version: &str,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name, region))
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    region: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("authorization", authorization(signing_name, region))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `(key, value)` request member, owned so a row can build names like
/// `Regions.member.2`.
fn member(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One CloudFormation call in `region` that must answer 200.
async fn cloudformation(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    action: &str,
    members: &[(String, String)],
) -> String {
    let (status, body) = query(router, "cloudformation", region, "2010-05-15", action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudFormation {action}, got {status} with {body}"
    );
    body
}

/// One EventBridge call that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", REGION, "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon SQS call that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", REGION, "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Systems Manager call in `region`, as its status and parsed body.
async fn ssm(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(router, "ssm", region, "AmazonSSM", "1.1", operation, body).await
}

/// A queue whose own policy admits the delivering service, and a rule on the default bus that
/// delivers every event of this suite's three `detail-type`s to it: the queue's url.
///
/// "For Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
/// resource-based policies" (`PutTargets`), so the queue carries the statement that admits
/// `events.amazonaws.com`; the rule is created BEFORE the stack set, since an event published
/// while the bus carries no matching rule reaches no target.
async fn wired_queue(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    });
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
    events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": format!("{name}-rule"),
            "EventPattern": json!({
                "source": [SOURCE],
                "detail-type": [STACK_SET_CHANGE, OPERATION_CHANGE, INSTANCE_CHANGE],
            })
            .to_string(),
        }),
    )
    .await;
    let targeted = events(
        router,
        case,
        "PutTargets",
        &json!({
            "Rule": format!("{name}-rule"),
            "Targets": [{ "Id": "queue", "Arn": arn }],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "case {case:?}: the queue target must be accepted, got {targeted}"
    );
    url
}

/// Every event body waiting on the queue at `url`, parsed, and taken off the queue.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|message| message["Body"].as_str())
        .map(|body| serde_json::from_str(body).unwrap_or(Value::Null))
        .collect()
}

/// Drain the queue at `url` into `seen` until `enough` is satisfied or [`BUDGET`] runs out.
async fn collect_until(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    seen: &mut Vec<Value>,
    enough: impl Fn(&[Value]) -> bool,
) {
    let deadline = Instant::now() + BUDGET;
    loop {
        seen.extend(drain(router, case, url).await);
        if enough(seen) || Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Every event of `detail_type` among `seen`.
fn of_type<'a>(seen: &'a [Value], detail_type: &str) -> Vec<&'a Value> {
    seen.iter()
        .filter(|event| event["detail-type"] == detail_type)
        .collect()
}

/// The status whatever the event is about reached.
fn status_of(event: &Value) -> &str {
    event["detail"]["status-details"]["status"]
        .as_str()
        .unwrap_or_default()
}

/// The instant the event carries.
///
/// Compared as TEXT where this suite orders two events: the envelope's `time` is an instant
/// at whole-second precision closed by `Z` (`2026-01-01T00:00:00Z`), one fixed-width form for
/// every event, so lexicographic order over it is chronological order.
fn time_of(event: &Value) -> &str {
    event["time"].as_str().unwrap_or_default()
}

/// The one event of `detail_type` whose status is `status`, or `None` for none.
fn one_with_status<'a>(seen: &'a [Value], detail_type: &str, status: &str) -> Option<&'a Value> {
    of_type(seen, detail_type)
        .into_iter()
        .find(|event| status_of(event) == status)
}

/// The operation events of `operation_id` and `action` with status `status`.
fn operation_events<'a>(
    seen: &'a [Value],
    operation_id: &str,
    action: &str,
    status: &str,
) -> Vec<&'a Value> {
    of_type(seen, OPERATION_CHANGE)
        .into_iter()
        .filter(|event| {
            event["detail"]["stack-set-operation-id"] == operation_id
                && event["detail"]["action"] == action
                && status_of(event) == status
        })
        .collect()
}

/// What every event about one stack set must carry, whichever change it reports.
fn assert_envelope(case: &str, event: &Value, stack_set_arn: &str) {
    assert_eq!(
        event["source"], SOURCE,
        "case {case:?}: expected source {SOURCE:?}, got {event}"
    );
    assert_eq!(
        event["account"], ACCOUNT,
        "case {case:?}: expected account {ACCOUNT:?}, got {event}"
    );
    assert_eq!(
        event["region"], REGION,
        "case {case:?}: expected region {REGION:?}, got {event}"
    );
    assert_eq!(
        event["resources"],
        json!([stack_set_arn]),
        "case {case:?}: expected resources [{stack_set_arn:?}], got {event}"
    );
    assert_eq!(
        event["detail"]["stack-set-arn"], stack_set_arn,
        "case {case:?}: expected detail.stack-set-arn {stack_set_arn:?}, got {event}"
    );
}

/// Poll `DescribeStackSetOperation` until it reports a terminal status, and answer what it
/// reported last.
async fn operation_settles(
    router: &MultiServiceRouter,
    case: &str,
    stack_set: &str,
    operation_id: &str,
) -> String {
    let deadline = Instant::now() + BUDGET;
    loop {
        let body = cloudformation(
            router,
            case,
            REGION,
            "DescribeStackSetOperation",
            &[
                member("StackSetName", stack_set),
                member("OperationId", operation_id),
            ],
        )
        .await;
        let reported = opt_element(&body, "Status").unwrap_or_default();
        let in_flight = matches!(reported.as_str(), RUNNING | "QUEUED" | "STOPPING");
        if !in_flight || Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Poll `DescribeStacks` in the stack's own Region until it reports a status that is not in
/// progress, and answer it.
async fn stack_settles(router: &MultiServiceRouter, case: &str, stack_id: &str) -> String {
    let region = region_of(stack_id);
    let deadline = Instant::now() + BUDGET;
    loop {
        let body = cloudformation(
            router,
            case,
            &region,
            "DescribeStacks",
            &[member("StackName", stack_id)],
        )
        .await;
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") || Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The Region an ARN belongs to: its fourth colon-separated field.
fn region_of(arn: &str) -> String {
    arn.split(':').nth(3).unwrap_or(REGION).to_string()
}

/// One row of the table: a stack set to deploy, and the instances one operation acts on.
struct Row {
    case: &'static str,
    /// The stack set's name, which also names this row's queue and rule.
    stack_set: &'static str,
    /// The template body, in either spelling, declaring ONE `AWS::SSM::Parameter`.
    template: String,
    /// The parameter that template declares, which the deployed instance really creates in
    /// Systems Manager.
    parameter_name: &'static str,
    parameter_value: &'static str,
    /// The Regions the instances are deployed in: one event per instance.
    regions: Vec<&'static str>,
}

/// The class the rule covers: any template, in either spelling, deployed to any number of
/// target Regions.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one instance in the stack set's own Region, JSON template",
            stack_set: "events-one-instance",
            template: json!({
                "Resources": {
                    "Probe": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/stackset/events/one",
                            "Type": "String",
                            "Value": "sim2real",
                        },
                    },
                },
            })
            .to_string(),
            parameter_name: "/stackset/events/one",
            parameter_value: "sim2real",
            regions: vec![REGION],
        },
        Row {
            case: "two instances, in two Regions, JSON template",
            stack_set: "events-two-instances",
            template: json!({
                "Resources": {
                    "Probe": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/stackset/events/two",
                            "Type": "String",
                            "Value": "both-regions",
                        },
                    },
                },
            })
            .to_string(),
            parameter_name: "/stackset/events/two",
            parameter_value: "both-regions",
            regions: vec![REGION, OTHER_REGION],
        },
        Row {
            case: "one instance outside the stack set's own Region, YAML template",
            stack_set: "events-yaml-instance",
            template: concat!(
                "Resources:\n",
                "  Probe:\n",
                "    Type: AWS::SSM::Parameter\n",
                "    Properties:\n",
                "      Name: /stackset/events/yaml\n",
                "      Type: String\n",
                "      Value: yaml-spelling\n",
            )
            .to_string(),
            parameter_name: "/stackset/events/yaml",
            parameter_value: "yaml-spelling",
            regions: vec![OTHER_REGION],
        },
    ]
}

/// The whole lifecycle of a SELF_MANAGED stack set, with every status change it owes observed
/// on the queue a rule on the default bus delivers to.
#[tokio::test]
async fn a_stack_set_lifecycle_publishes_every_status_change_onto_the_default_bus() {
    for row in rows() {
        let case = row.case;
        // A fresh emulator per row, so one row's events never decide another's.
        let router = router();
        let queue = wired_queue(&router, case, row.stack_set).await;

        // The stack set itself: SELF_MANAGED, with the administration and execution roles
        // such a stack set names.
        let created = cloudformation(
            &router,
            case,
            REGION,
            "CreateStackSet",
            &[
                member("StackSetName", row.stack_set),
                member("TemplateBody", &row.template),
                member("PermissionModel", "SELF_MANAGED"),
                member(
                    "AdministrationRoleARN",
                    &format!("arn:aws:iam::{ACCOUNT}:role/{}-admin", row.stack_set),
                ),
                member("ExecutionRoleName", "StackSetExecutionRole"),
            ],
        )
        .await;
        let stack_set_id = opt_element(&created, "StackSetId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateStackSet answered no id: {created}"));
        let stack_set_arn =
            format!("arn:aws:cloudformation:{REGION}:{ACCOUNT}:stackset/{stack_set_id}");

        // 1. The stack set reaching ACTIVE.
        let mut seen: Vec<Value> = Vec::new();
        collect_until(&router, case, &queue, &mut seen, |seen| {
            one_with_status(seen, STACK_SET_CHANGE, ACTIVE).is_some()
        })
        .await;
        let active = one_with_status(&seen, STACK_SET_CHANGE, ACTIVE).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {STACK_SET_CHANGE:?} event with status {ACTIVE:?} \
                 after CreateStackSet, got {seen:?}"
            )
        });
        assert_envelope(case, active, &stack_set_arn);
        let active_time = time_of(active).to_string();

        // 2. One CREATE operation over this row's Regions, which must settle and publish its
        //    own start and conclusion plus one event per instance it acted on.
        let mut members = vec![
            member("StackSetName", row.stack_set),
            member("Accounts.member.1", ACCOUNT),
        ];
        for (index, region) in row.regions.iter().enumerate() {
            members.push(member(&format!("Regions.member.{}", index + 1), region));
        }
        members.push(member("OperationPreferences.FailureToleranceCount", "0"));
        members.push(member("OperationPreferences.MaxConcurrentCount", "1"));
        let deployed = cloudformation(&router, case, REGION, "CreateStackInstances", &members).await;
        let create_operation = opt_element(&deployed, "OperationId").unwrap_or_else(|| {
            panic!("case {case:?}: CreateStackInstances answered no OperationId: {deployed}")
        });
        let settled = operation_settles(&router, case, row.stack_set, &create_operation).await;
        assert_eq!(
            settled, SUCCEEDED,
            "case {case:?}: expected the CREATE operation to reach {SUCCEEDED:?}, got {settled:?}"
        );

        let listed = cloudformation(
            &router,
            case,
            REGION,
            "ListStackInstances",
            &[member("StackSetName", row.stack_set)],
        )
        .await;
        let instance_stacks = elements(&listed, "StackId");
        assert_eq!(
            instance_stacks.len(),
            row.regions.len(),
            "case {case:?}: expected one instance stack per target Region, got \
             {instance_stacks:?} from {listed}"
        );

        let wanted_instances = instance_stacks.clone();
        let awaited_operation = create_operation.clone();
        collect_until(&router, case, &queue, &mut seen, move |seen| {
            let paired = !operation_events(seen, &awaited_operation, CREATE, RUNNING).is_empty()
                && !operation_events(seen, &awaited_operation, CREATE, SUCCEEDED).is_empty();
            let instances = of_type(seen, INSTANCE_CHANGE);
            paired
                && wanted_instances.iter().all(|stack_id| {
                    instances
                        .iter()
                        .any(|event| event["detail"]["stack-id"] == stack_id.as_str())
                })
        })
        .await;

        let running = operation_events(&seen, &create_operation, CREATE, RUNNING);
        let succeeded = operation_events(&seen, &create_operation, CREATE, SUCCEEDED);
        assert_eq!(
            running.len(),
            1,
            "case {case:?}: expected exactly one {OPERATION_CHANGE:?} {RUNNING:?} event for \
             {create_operation:?}, got {seen:?}"
        );
        assert_eq!(
            succeeded.len(),
            1,
            "case {case:?}: expected exactly one {OPERATION_CHANGE:?} {SUCCEEDED:?} event for \
             {create_operation:?}, got {seen:?}"
        );
        assert_envelope(case, running[0], &stack_set_arn);
        assert_envelope(case, succeeded[0], &stack_set_arn);
        assert!(
            time_of(running[0]) <= time_of(succeeded[0]),
            "case {case:?}: expected the {RUNNING:?} event not to be later than the \
             {SUCCEEDED:?} one, got {:?} then {:?}",
            time_of(running[0]),
            time_of(succeeded[0])
        );

        // 3. One instance event per instance the operation acted on, carrying that instance's
        //    own stack id and the status the instance reached.
        for stack_id in &instance_stacks {
            let instance = of_type(&seen, INSTANCE_CHANGE)
                .into_iter()
                .find(|event| event["detail"]["stack-id"] == stack_id.as_str())
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected an {INSTANCE_CHANGE:?} event for {stack_id:?}, \
                         got {seen:?}"
                    )
                });
            assert_envelope(case, instance, &stack_set_arn);
            assert_eq!(
                status_of(instance),
                CURRENT,
                "case {case:?}: expected the deployed instance {stack_id:?} to report status \
                 {CURRENT:?}, got {instance}"
            );
            assert_eq!(
                instance["detail"]["status-details"]["detailed-status"], SUCCEEDED,
                "case {case:?}: expected the deployed instance {stack_id:?} to report \
                 detailed-status {SUCCEEDED:?}, got {instance}"
            );

            // ...and the instance's own stack really deployed the declared parameter, in the
            // Region the instance belongs to.
            let stack_status = stack_settles(&router, case, stack_id).await;
            assert_eq!(
                stack_status, CREATE_COMPLETE,
                "case {case:?}: expected the instance stack {stack_id:?} to reach \
                 {CREATE_COMPLETE:?}, got {stack_status:?}"
            );
            let (status, read) = ssm(
                &router,
                &region_of(stack_id),
                "GetParameter",
                &json!({ "Name": row.parameter_name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the deployed parameter to be readable in \
                 {:?}, got {status} with {read}",
                region_of(stack_id)
            );
            assert_eq!(
                read["Parameter"]["Value"], row.parameter_value,
                "case {case:?}: expected the deployed parameter to carry {:?}, got {read}",
                row.parameter_value
            );
        }

        // 4. The DELETE operation over the same instances, and the stack set's own deletion.
        let mut members = vec![
            member("StackSetName", row.stack_set),
            member("Accounts.member.1", ACCOUNT),
            member("RetainStacks", "false"),
        ];
        for (index, region) in row.regions.iter().enumerate() {
            members.push(member(&format!("Regions.member.{}", index + 1), region));
        }
        let removed = cloudformation(&router, case, REGION, "DeleteStackInstances", &members).await;
        let delete_operation = opt_element(&removed, "OperationId").unwrap_or_else(|| {
            panic!("case {case:?}: DeleteStackInstances answered no OperationId: {removed}")
        });
        let settled = operation_settles(&router, case, row.stack_set, &delete_operation).await;
        assert_eq!(
            settled, SUCCEEDED,
            "case {case:?}: expected the DELETE operation to reach {SUCCEEDED:?}, got {settled:?}"
        );
        let awaited_delete = delete_operation.clone();
        collect_until(&router, case, &queue, &mut seen, move |seen| {
            !operation_events(seen, &awaited_delete, DELETE, RUNNING).is_empty()
                && !operation_events(seen, &awaited_delete, DELETE, SUCCEEDED).is_empty()
        })
        .await;
        for status in [RUNNING, SUCCEEDED] {
            let published = operation_events(&seen, &delete_operation, DELETE, status);
            assert_eq!(
                published.len(),
                1,
                "case {case:?}: expected exactly one {OPERATION_CHANGE:?} {status:?} event with \
                 action {DELETE:?} for {delete_operation:?}, got {seen:?}"
            );
            assert_envelope(case, published[0], &stack_set_arn);
        }

        cloudformation(
            &router,
            case,
            REGION,
            "DeleteStackSet",
            &[member("StackSetName", row.stack_set)],
        )
        .await;
        collect_until(&router, case, &queue, &mut seen, |seen| {
            one_with_status(seen, STACK_SET_CHANGE, DELETED).is_some()
        })
        .await;
        let deleted = one_with_status(&seen, STACK_SET_CHANGE, DELETED).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {STACK_SET_CHANGE:?} event with status {DELETED:?} \
                 after DeleteStackSet, got {seen:?}"
            )
        });
        assert_envelope(case, deleted, &stack_set_arn);
        assert!(
            active_time.as_str() <= time_of(deleted),
            "case {case:?}: expected the {ACTIVE:?} event not to be later than the {DELETED:?} \
             one, got {active_time:?} then {:?}",
            time_of(deleted)
        );
        assert_eq!(
            of_type(&seen, STACK_SET_CHANGE).len(),
            2,
            "case {case:?}: expected exactly the {ACTIVE:?} and {DELETED:?} stack-set events, \
             got {seen:?}"
        );
    }
}
