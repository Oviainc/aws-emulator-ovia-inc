// SPDX-License-Identifier: CC0-1.0

//! `PutEvents` INVOKES each target of every rule that matched, end to end over
//! the wire through the real `MultiServiceRouter`, with the services a target
//! lives in mounted beside EventBridge on one engine — so a delivery is observed
//! where a client observes it: on the queue, on the topic, on the state machine's
//! executions, and on the dead-letter queue and the `FailedInvocations` metric
//! when the delivery fails.
//!
//! The RULE asserted here: for every ENABLED rule whose `EventPattern` matches a
//! published event, EventBridge makes the API call that target's service is
//! invoked by — `sqs:SendMessage` for a queue, `sns:Publish` for a topic,
//! `states:StartExecution` for a state machine, `lambda:InvokeFunction` for a
//! function — carrying the whole event envelope. For the target kinds AWS invokes
//! UNDER the target's `RoleArn` ("for EC2 instances, Kinesis data streams, Step
//! Functions state machines and API Gateway APIs, EventBridge relies on IAM roles
//! that you specify in the `RoleARN` argument in `PutTargets`. For Lambda
//! functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
//! resource-based policies") a role that `events.amazonaws.com` cannot assume, or
//! whose policies withhold that call, is a delivery that fails: the event goes to
//! the target's dead-letter queue and is counted on the rule's
//! `FailedInvocations` metric instead.
//!
//! The tables span the class rather than one recorded case: the rows differ along
//! the target's SERVICE (which invocation is made and what the target then
//! holds), and along WHY the role bars the delivery — no trust for the service
//! principal, a policy naming another action, a policy stating nothing at all, a
//! policy that grants the call, a role the account does not hold (which is
//! evidence of nothing, so the delivery goes ahead), and a target kind whose
//! invocation the role does not govern at all.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    ComAmazonawsEventbridgeAwseventsState, InvocationOutcome, FAILED_INVOCATIONS_METRIC,
    METRIC_NAMESPACE, RULE_NAME_DIMENSION, SERVICE_PRINCIPAL,
};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// The source and detail type the rules below match on.
const SOURCE: &str = "com.demo.orders";
const DETAIL_TYPE: &str = "Order Created";
const DETAIL: &str = r#"{"orderId":"A-1","amount":7}"#;

/// A state machine definition that terminates on its own, so an execution
/// started by a delivery is a real execution rather than one waiting on a task.
const DEFINITION: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#;

/// The emulator: EventBridge and every service a target of these tests lives in,
/// on ONE engine — the shape `core/router/src/lib.rs` mounts them in.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router, engine }
}

/// One `awsJson` RPC call against the service whose signing name is
/// `signing_name` — the credential scope is what routes it.
async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

async fn events(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "events", "AWSEvents", "1.1", operation, body).await
}

async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "sqs", "AmazonSQS", "1.0", operation, body).await
}

async fn sfn(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        emulator,
        "states",
        "AWSStepFunctions",
        "1.0",
        operation,
        body,
    )
    .await
}

async fn cloudwatch(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "1.0",
        operation,
        body,
    )
    .await
}

/// Percent-encode one form value the way an AWS SDK encodes an `awsQuery` body.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One `awsQuery` call against the service whose signing name is `signing_name`.
async fn query(
    emulator: &Emulator,
    signing_name: &str,
    api_version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={api_version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

async fn iam(
    emulator: &Emulator,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    query(emulator, "iam", "2010-05-08", action, params).await
}

async fn sns(
    emulator: &Emulator,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    query(emulator, "sns", "2010-03-31", action, params).await
}

/// The text of the first `<tag>` of an XML body.
fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// A queue called `name`; answers its url and the ARN a rule names it by.
async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let (status, body) = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(status, 200, "case {name}: CreateQueue answered {status} {body}");
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    (url, arn)
}

/// Every message waiting on the queue at `url`, with its attributes.
async fn drain(emulator: &Emulator, url: &str) -> Vec<Value> {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "receive {url}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"].as_array().cloned().unwrap_or_default()
}

/// A rule called `name` matching the published event, carrying `targets`.
async fn given_rule(emulator: &Emulator, name: &str, targets: Value) {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern, "State": "ENABLED" }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutRule answered {status} {body}");
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({ "Rule": name, "Targets": targets }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutTargets answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {name}: PutTargets reported a failed entry: {body}"
    );
}

/// Publish the one event every test below routes, and answer its `EventId`.
async fn publish(emulator: &Emulator) -> String {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": DETAIL }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("no EventId in {body}"))
        .to_string()
}

/// What EventBridge recorded about each handover: `(rule, target arn, outcome)`.
fn invocations(emulator: &Emulator) -> Vec<(String, String, InvocationOutcome)> {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .invocations
        .iter()
        .map(|invocation| {
            (
                invocation.rule.clone(),
                invocation.target_arn.clone(),
                invocation.outcome.clone(),
            )
        })
        .collect()
}

/// The envelope the published event is delivered as, with the values this
/// publication fixes. Source: the EventBridge event-structure reference (and the
/// `PutEvents` entry members the event is built from).
fn assert_whole_envelope(case: &str, body: &str, event_id: &str) {
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case}: the delivered body is not JSON ({error})"));
    let rows = [
        ("version", json!("0")),
        ("id", json!(event_id)),
        ("detail-type", json!(DETAIL_TYPE)),
        ("source", json!(SOURCE)),
        ("account", json!(engine::emulator_account_id())),
        ("region", json!(REGION)),
        (
            "detail",
            serde_json::from_str::<Value>(DETAIL).expect("the detail is JSON"),
        ),
    ];
    for (member, expected) in rows {
        assert_eq!(
            parsed[member], expected,
            "case {case}: expected the envelope's {member} to be {expected}, got {parsed}"
        );
    }
}

/// The target kinds a rule's matched event is invoked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Queue,
    Topic,
    StateMachine,
}

/// A matched event is handed to the target's OWN API, for every target kind this
/// emulator invokes — and what the target then holds is the whole event
/// envelope.
#[tokio::test]
async fn a_matched_event_is_invoked_on_every_target_kind() {
    for kind in [Kind::Queue, Kind::Topic, Kind::StateMachine] {
        let emulator = emulator();
        let (target_arn, observe) = given_target(&emulator, kind).await;
        given_rule(
            &emulator,
            "dispatch",
            json!([{ "Id": "t1", "Arn": target_arn }]),
        )
        .await;

        let event_id = publish(&emulator).await;

        let recorded = invocations(&emulator);
        assert_eq!(
            recorded,
            vec![(
                "dispatch".to_string(),
                target_arn.clone(),
                InvocationOutcome::Invoked
            )],
            "case {kind:?}: expected one invoked handover, got {recorded:?}"
        );
        let delivered = observe.read(&emulator).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {kind:?}: expected the target to hold the event exactly once, got {delivered:?}"
        );
        assert_whole_envelope(&format!("{kind:?}"), &delivered[0], &event_id);
    }
}

/// How a row reads back what its target received.
enum Observe {
    /// The bodies of the messages on this queue.
    Queue(String),
    /// The messages SNS delivered to this topic's subscriptions.
    Topic(String),
    /// The inputs of this state machine's executions.
    StateMachine(String),
}

impl Observe {
    async fn read(&self, emulator: &Emulator) -> Vec<String> {
        match self {
            Self::Queue(url) => drain(emulator, url)
                .await
                .iter()
                .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
                .collect(),
            Self::Topic(arn) => emulator
                .engine
                .state::<service_com_amazonaws_sns_amazonsimplenotificationservice::ComAmazonawsSnsAmazonsimplenotificationserviceState>()
                .lock()
                .delivered
                .values()
                .flatten()
                .filter(|published| &published.topic_arn == arn)
                .map(|published| published.message.to_string())
                .collect(),
            Self::StateMachine(arn) => {
                let (status, listed) = sfn(
                    emulator,
                    "ListExecutions",
                    &json!({ "stateMachineArn": arn }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "ListExecutions answered {status} {listed}"
                );
                let mut inputs = Vec::new();
                for execution in listed["executions"].as_array().cloned().unwrap_or_default() {
                    let execution_arn = execution["executionArn"].as_str().unwrap_or_default();
                    let (status, described) = sfn(
                        emulator,
                        "DescribeExecution",
                        &json!({ "executionArn": execution_arn }),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "DescribeExecution answered {status} {described}"
                    );
                    inputs.push(described["input"].as_str().unwrap_or_default().to_string());
                }
                inputs
            }
        }
    }
}

/// Create the resource a row's target names, and answer its ARN plus how to read
/// back what it received.
async fn given_target(emulator: &Emulator, kind: Kind) -> (String, Observe) {
    match kind {
        Kind::Queue => {
            let (url, arn) = given_queue(emulator, "dispatch-queue").await;
            (arn, Observe::Queue(url))
        }
        Kind::Topic => {
            let (status, created) = sns(emulator, "CreateTopic", &[("Name", "dispatch-topic")]).await;
            assert_eq!(status, 200, "CreateTopic answered {status}: {created}");
            let arn = xml_text(&created, "TopicArn")
                .unwrap_or_else(|| panic!("no TopicArn in {created}"));
            // A delivery is observed on a SUBSCRIPTION, so the topic has one.
            let (_, queue_arn) = given_queue(emulator, "topic-subscriber").await;
            let (status, subscribed) = sns(
                emulator,
                "Subscribe",
                &[
                    ("TopicArn", &arn),
                    ("Protocol", "sqs"),
                    ("Endpoint", &queue_arn),
                ],
            )
            .await;
            assert_eq!(status, 200, "Subscribe answered {status}: {subscribed}");
            (arn.clone(), Observe::Topic(arn))
        }
        Kind::StateMachine => {
            let arn = given_state_machine(emulator, "dispatch-flow").await;
            (arn.clone(), Observe::StateMachine(arn))
        }
    }
}

/// A state machine called `name`, answered with its ARN.
async fn given_state_machine(emulator: &Emulator, name: &str) -> String {
    let role = format!(
        "arn:aws:iam::{}:role/state-machine-execution",
        engine::emulator_account_id()
    );
    let (status, created) = sfn(
        emulator,
        "CreateStateMachine",
        &json!({ "name": name, "definition": DEFINITION, "roleArn": role }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateStateMachine answered {status} {created}"
    );
    created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no stateMachineArn in {created}"))
        .to_string()
}

/// A trust policy with one statement, written as a client sends it.
fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// An identity policy allowing one action on every resource.
fn identity_policy(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": "*" }],
    })
    .to_string()
}

/// A role trusting `principal`, carrying an inline policy allowing `allowed`
/// when a row names one; answered with the ARN IAM persisted.
async fn given_role(
    emulator: &Emulator,
    name: &str,
    principal: &str,
    allowed: Option<&str>,
) -> String {
    let document = trust_policy(principal);
    let (status, created) = iam(
        emulator,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateRole answered {status}: {created}"
    );
    let arn =
        xml_text(&created, "Arn").unwrap_or_else(|| panic!("case {name}: no Arn in {created}"));
    if let Some(action) = allowed {
        let document = identity_policy(action);
        let (status, written) = iam(
            emulator,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "delivery"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: PutRolePolicy answered {status}: {written}"
        );
    }
    arn
}

/// Epoch seconds, for the metric window a statistics read asks about.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The `Sum` of the rule's `FailedInvocations` over a window around now, and the
/// series CloudWatch holds in EventBridge's namespace.
async fn failed_invocations(emulator: &Emulator, rule: &str) -> (f64, Vec<Value>) {
    let (status, listed) = cloudwatch(
        emulator,
        "ListMetrics",
        &json!({ "Namespace": METRIC_NAMESPACE }),
    )
    .await;
    assert_eq!(status, 200, "ListMetrics answered {status} {listed}");
    let series = listed["Metrics"].as_array().cloned().unwrap_or_default();
    let now = epoch_seconds();
    let (status, stats) = cloudwatch(
        emulator,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": FAILED_INVOCATIONS_METRIC,
            "Dimensions": [{ "Name": RULE_NAME_DIMENSION, "Value": rule }],
            "StartTime": now - 300.0,
            "EndTime": now + 300.0,
            "Period": 600,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    assert_eq!(status, 200, "GetMetricStatistics answered {status} {stats}");
    let sum = stats["Datapoints"]
        .as_array()
        .map(|points| {
            points
                .iter()
                .filter_map(|point| point["Sum"].as_f64())
                .sum::<f64>()
        })
        .unwrap_or_default();
    (sum, series)
}

/// One row of the role table: why the role does or does not bar the delivery.
struct RoleCase {
    case: &'static str,
    kind: Kind,
    /// The service principal the role's trust policy names, and the one action
    /// its inline policy allows — `None` for a role carrying no inline policy at
    /// all, and no role at all when `role` is `false`.
    principal: &'static str,
    allowed: Option<&'static str>,
    /// Whether the account holds the role the target names.
    role_exists: bool,
    expected: Expected,
}

/// What a row expects of the handover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    Invoked,
    Failed(&'static str),
}

/// A delivery the target's role bars is dead-lettered and counted; one the role
/// allows, and one the account holds no role for, go through.
#[tokio::test]
async fn a_delivery_the_targets_role_bars_is_dead_lettered_and_counted() {
    let rows = [
        RoleCase {
            case: "the role does not trust the EventBridge service principal",
            kind: Kind::StateMachine,
            principal: "lambda.amazonaws.com",
            allowed: Some("states:StartExecution"),
            role_exists: true,
            // The condition's own name, asserted as the LITERAL EventBridge
            // reports rather than through the constant the service words it
            // with, so a change to that constant's value fails this row.
            expected: Expected::Failed("NO_PERMISSIONS"),
        },
        RoleCase {
            case: "the role's policy names another action",
            kind: Kind::StateMachine,
            principal: SERVICE_PRINCIPAL,
            allowed: Some("sqs:SendMessage"),
            role_exists: true,
            expected: Expected::Failed("ACCESS_DENIED"),
        },
        RoleCase {
            case: "the role's policies state nothing about the call",
            kind: Kind::StateMachine,
            principal: SERVICE_PRINCIPAL,
            allowed: None,
            role_exists: true,
            expected: Expected::Failed("ACCESS_DENIED"),
        },
        RoleCase {
            // A queue target is invoked under the QUEUE's resource policy, not
            // under the role, so a role that neither trusts EventBridge nor
            // allows the call does not stop the delivery.
            case: "a target kind whose invocation the role does not govern",
            kind: Kind::Queue,
            principal: "lambda.amazonaws.com",
            allowed: None,
            role_exists: true,
            expected: Expected::Invoked,
        },
        RoleCase {
            case: "the role may start an execution",
            kind: Kind::StateMachine,
            principal: SERVICE_PRINCIPAL,
            allowed: Some("states:StartExecution"),
            role_exists: true,
            expected: Expected::Invoked,
        },
        RoleCase {
            case: "the account holds no such role",
            kind: Kind::Queue,
            principal: SERVICE_PRINCIPAL,
            allowed: None,
            role_exists: false,
            expected: Expected::Invoked,
        },
    ];

    for row in rows {
        let emulator = emulator();
        let (target_arn, observe) = given_target(&emulator, row.kind).await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "dead-letters").await;
        let role_arn = if row.role_exists {
            given_role(&emulator, "delivery-role", row.principal, row.allowed).await
        } else {
            format!(
                "arn:aws:iam::{}:role/not-in-this-account",
                engine::emulator_account_id()
            )
        };
        given_rule(
            &emulator,
            "guarded",
            json!([{
                "Id": "t1",
                "Arn": target_arn,
                "RoleArn": role_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            }]),
        )
        .await;

        let event_id = publish(&emulator).await;

        let recorded = invocations(&emulator);
        let outcome = recorded
            .first()
            .map(|(_, _, outcome)| outcome.clone())
            .unwrap_or_else(|| panic!("case {:?}: nothing was recorded", row.case));
        let dead_letters = drain(&emulator, &dead_letter_url).await;
        let (sum, series) = failed_invocations(&emulator, "guarded").await;
        match row.expected {
            Expected::Invoked => {
                assert_eq!(
                    outcome,
                    InvocationOutcome::Invoked,
                    "case {:?}: expected the delivery to go through, got {outcome:?}",
                    row.case
                );
                assert!(
                    dead_letters.is_empty(),
                    "case {:?}: a delivered event is not dead-lettered, got {dead_letters:?}",
                    row.case
                );
                assert_eq!(
                    sum, 0.0,
                    "case {:?}: a delivered event counts no failed invocation, got {sum}",
                    row.case
                );
                assert!(
                    series.is_empty(),
                    "case {:?}: no series is published for a delivery that worked, got {series:?}",
                    row.case
                );
                let delivered = observe.read(&emulator).await;
                assert_eq!(
                    delivered.len(),
                    1,
                    "case {:?}: expected the target to hold the event, got {delivered:?}",
                    row.case
                );
            }
            Expected::Failed(code) => {
                let InvocationOutcome::Failed { code: actual, .. } = &outcome else {
                    panic!(
                        "case {:?}: expected a failed invocation coded {code}, got {outcome:?}",
                        row.case
                    );
                };
                assert_eq!(
                    actual, code,
                    "case {:?}: expected the condition {code}, got {outcome:?}",
                    row.case
                );
                assert_eq!(
                    dead_letters.len(),
                    1,
                    "case {:?}: an undeliverable event goes to the dead-letter queue, \
                     got {dead_letters:?}",
                    row.case
                );
                let message = &dead_letters[0];
                assert_whole_envelope(
                    row.case,
                    message["Body"].as_str().unwrap_or_default(),
                    &event_id,
                );
                let attributes = &message["MessageAttributes"];
                assert_eq!(
                    attributes["TARGET_ARN"]["StringValue"],
                    json!(target_arn),
                    "case {:?}: the dead-letter message names the target, got {message}",
                    row.case
                );
                assert_eq!(
                    attributes["ERROR_CODE"]["StringValue"],
                    json!(code),
                    "case {:?}: the dead-letter message carries the condition, got {message}",
                    row.case
                );
                assert_eq!(
                    attributes["RULE_ARN"]["StringValue"],
                    json!(format!(
                        "arn:aws:events:{REGION}:{}:rule/guarded",
                        engine::emulator_account_id()
                    )),
                    "case {:?}: the dead-letter message names the rule, got {message}",
                    row.case
                );
                assert!(
                    attributes["ERROR_MESSAGE"]["StringValue"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()),
                    "case {:?}: the dead-letter message says why, got {message}",
                    row.case
                );
                assert_eq!(
                    sum, 1.0,
                    "case {:?}: one undeliverable event counts one failed invocation, got {sum} \
                     over the series {series:?}",
                    row.case
                );
                assert_eq!(
                    series,
                    vec![json!({
                        "Namespace": METRIC_NAMESPACE,
                        "MetricName": FAILED_INVOCATIONS_METRIC,
                        "Dimensions": [{ "Name": RULE_NAME_DIMENSION, "Value": "guarded" }],
                    })],
                    "case {:?}: expected the rule's own FailedInvocations series, got {series:?}",
                    row.case
                );
                if row.kind == Kind::StateMachine {
                    let delivered = observe.read(&emulator).await;
                    assert!(
                        delivered.is_empty(),
                        "case {:?}: a barred delivery reaches the target with nothing, \
                         got {delivered:?}",
                        row.case
                    );
                }
            }
        }
    }
}
