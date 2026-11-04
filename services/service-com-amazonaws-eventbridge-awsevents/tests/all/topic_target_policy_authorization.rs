// SPDX-License-Identifier: CC0-1.0

//! The TOPIC's own access policy decides whether a rule's SNS target is published
//! to, end to end over the wire through the real `MultiServiceRouter`, with Amazon
//! SNS and Amazon SQS mounted on the same engine so the outcome is asserted where a
//! client observes it: on a queue subscribed to the topic, on the target's
//! dead-letter queue, and on the healthy SIBLING target of the same rule.
//!
//! The RULE asserted here: "for Lambda functions, Amazon SNS topics and Amazon SQS
//! queues, EventBridge relies on resource-based policies" (`PutTargets`), and the
//! call EventBridge makes on a topic target is `sns:Publish` as the service
//! principal `events.amazonaws.com`. A service principal holds no identity-based
//! policies, so the topic's own `Policy` attribute is the ONLY document in force:
//! nothing is published unless that document states an `Allow` about this principal,
//! this action and this topic — a topic carrying only the policy Amazon SNS
//! generates for its owner has granted nothing — an explicit `Deny` about it refuses
//! the publish whatever is granted beside it, and a statement conditioned on
//! `aws:SourceArn` or `aws:SourceAccount` authorizes the deliveries of the rule and
//! the account it names and no others.
//!
//! A withheld publish is a delivery that FAILED, so it goes to the target's
//! dead-letter queue carrying what Amazon SNS answers a caller it has granted
//! nothing: that service says WHICH of the two it was — an `AuthorizationError`
//! naming the principal, the action and the topic — so the condition names the
//! PERMISSION (`NO_PERMISSIONS`) where a queue's refusal names the resource. It is
//! not retried, so the dead letter carries exactly the four attributes EventBridge
//! stamps on one and no retry attributes, even for a target whose `RetryPolicy`
//! allows many attempts. The rule's OTHER targets are unaffected: each handover is
//! decided on its own, so the healthy sibling receives its copy either way.
//!
//! The table spans the class rather than the one recorded case: the rows differ along
//! what the topic's document states — whether a statement exists at all, which
//! principal, action and resource it is about, which `Effect` it states and in which
//! order, and which condition it is scoped by.
//!
//! The harness (the router with EventBridge, Amazon SQS, Amazon SNS, Step Functions,
//! IAM and CloudWatch on one engine, the queue fixtures and the rule fixture) is
//! `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, NO_PERMISSIONS, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    drain, emulator, given_queue, given_queue_with_policy, given_rule, publish, sns, xml_text,
    Emulator, REGION,
};

/// The IAM action a delivery to a topic target performs on that topic.
const PUBLISH: &str = "sns:Publish";

/// The rule every test here publishes through, named so a row's condition can scope a
/// grant to it.
const RULE: &str = "topic-policy-rule";

/// The dead-letter attributes EventBridge stamps on an undeliverable event, as the
/// EXACT set a refused publish carries — stated here rather than read from the
/// service.
const DEAD_LETTER_ATTRIBUTES: [&str; 4] =
    ["ERROR_CODE", "ERROR_MESSAGE", "RULE_ARN", "TARGET_ARN"];

/// The metadata block Amazon SNS's own refusal closes with, as the caller of
/// `sns:Publish` reads it: the service, the status and the error code of the refused
/// call ([`AuthorizationErrorException`]'s `awsQueryError` code and `httpError`,
/// `smithy_query(query="shape",shape="AuthorizationErrorException",model_path="services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json")`).
const SNS_REFUSAL_METADATA: &str =
    " (Service: AmazonSNS; Status Code: 403; Error Code: AuthorizationError; Request ID: ";

/// The account every resource below belongs to.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// An `Allow` of [`PUBLISH`] to the EventBridge service principal on `topic_arn`,
/// scoped by `condition` when the row states one.
fn grant(topic_arn: &str, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": {"Service": SERVICE_PRINCIPAL},
        "Action": PUBLISH,
        "Resource": topic_arn,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A `Deny` of [`PUBLISH`] to the EventBridge service principal on `topic_arn`.
fn deny(topic_arn: &str) -> Value {
    json!({
        "Effect": "Deny",
        "Principal": {"Service": SERVICE_PRINCIPAL},
        "Action": PUBLISH,
        "Resource": topic_arn,
    })
}

/// The ARN of a rule of this account and Region that is NOT the one publishing below.
fn another_rule_arn() -> String {
    rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, "somebody-elses-rule")
}

/// The policy a queue SUBSCRIBED to a topic needs for the notification to reach it:
/// Amazon SNS's own send, which that service decides against this document.
fn granting_sns(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "sns.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// How a row writes the document its topic carries: from the topic's own ARN, so a
/// row can state something about the very topic the document is installed on.
type PolicyFor = Box<dyn Fn(&str) -> String>;

/// One row: the case, the document the topic carries beside the one Amazon SNS
/// generates, and whether EventBridge publishes to it.
struct Row {
    case: &'static str,
    /// `None` leaves the topic on the policy Amazon SNS generates for its owner,
    /// which is what a `CreateTopic` that names no policy answers with.
    policy: Option<PolicyFor>,
    published: bool,
}

/// A topic called `name` carrying the document `policy` builds from its own ARN, with
/// `subscriber_arn` subscribed to it; answers the topic's ARN.
async fn given_topic(
    emulator: &Emulator,
    case: &str,
    name: &str,
    subscriber_arn: &str,
    policy: Option<&PolicyFor>,
) -> String {
    let (status, created) = sns(emulator, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTopic answered {status}: {created}"
    );
    let arn = xml_text(&created, "TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TopicArn in {created}"));
    if let Some(policy) = policy {
        let document = policy(&arn);
        let (status, written) = sns(
            emulator,
            "SetTopicAttributes",
            &[
                ("TopicArn", &arn),
                ("AttributeName", "Policy"),
                ("AttributeValue", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: SetTopicAttributes answered {status}: {written}"
        );
    }
    let (status, subscribed) = sns(
        emulator,
        "Subscribe",
        &[
            ("TopicArn", &arn),
            ("Protocol", "sqs"),
            ("Endpoint", subscriber_arn),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: Subscribe answered {status}: {subscribed}"
    );
    arn
}

/// The topic's own `Policy` decides each publish, over a table of documents — and the
/// healthy sibling target of the same rule receives its copy whichever way the topic
/// goes.
#[tokio::test]
async fn a_topic_target_is_published_to_only_where_its_own_policy_allows() {
    let rows: Vec<Row> = vec![
        Row {
            case: "the policy Amazon SNS generates for the topic's owner",
            policy: None,
            published: false,
        },
        Row {
            case: "an unconditional grant to the service principal",
            policy: Some(Box::new(|arn: &str| document(json!([grant(arn, None)])))),
            published: true,
        },
        Row {
            case: "a grant whose Action names every action of the service",
            policy: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": "sns:*",
                    "Resource": arn,
                }]))
            })),
            published: true,
        },
        Row {
            // The spelling Amazon SNS itself writes its actions in, which IAM folds.
            case: "a grant naming the action in the casing Amazon SNS publishes",
            policy: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": "SNS:Publish",
                    "Resource": arn,
                }]))
            })),
            published: true,
        },
        Row {
            case: "a grant of another action of the same service",
            policy: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": "sns:Subscribe",
                    "Resource": arn,
                }]))
            })),
            published: false,
        },
        Row {
            case: "a grant naming another service principal",
            policy: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"Service": "s3.amazonaws.com"},
                    "Action": PUBLISH,
                    "Resource": arn,
                }]))
            })),
            published: false,
        },
        Row {
            case: "a grant whose Resource names another topic",
            policy: Some(Box::new(|_arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": PUBLISH,
                    "Resource": format!("arn:aws:sns:{REGION}:{}:another-topic", account()),
                }]))
            })),
            published: false,
        },
        Row {
            case: "an explicit deny written after the grant",
            policy: Some(Box::new(|arn: &str| {
                document(json!([grant(arn, None), deny(arn)]))
            })),
            published: false,
        },
        Row {
            case: "an explicit deny written before the grant",
            policy: Some(Box::new(|arn: &str| {
                document(json!([deny(arn), grant(arn, None)]))
            })),
            published: false,
        },
        Row {
            case: "a grant scoped by ArnEquals to the publishing rule",
            policy: Some(Box::new(|arn: &str| {
                let mine = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE);
                document(json!([grant(
                    arn,
                    Some(json!({"ArnEquals": {"aws:SourceArn": mine}}))
                )]))
            })),
            published: true,
        },
        Row {
            case: "a grant scoped by ArnEquals to another rule",
            policy: Some(Box::new(|arn: &str| {
                document(json!([grant(
                    arn,
                    Some(json!({"ArnEquals": {"aws:SourceArn": another_rule_arn()}}))
                )]))
            })),
            published: false,
        },
        Row {
            case: "a grant scoped by StringEquals to this account",
            policy: Some(Box::new(|arn: &str| {
                document(json!([grant(
                    arn,
                    Some(json!({"StringEquals": {"aws:SourceAccount": account()}}))
                )]))
            })),
            published: true,
        },
        Row {
            case: "a grant scoped by StringEquals to another account",
            policy: Some(Box::new(|arn: &str| {
                document(json!([grant(
                    arn,
                    Some(json!({"StringEquals": {"aws:SourceAccount": "210987654321"}}))
                )]))
            })),
            published: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        // The three queues of the row: the one subscribed to the topic (where a
        // published notification arrives), the target's dead-letter queue, and the
        // HEALTHY sibling target of the same rule.
        let (subscriber_url, subscriber_arn) = given_queue_with_policy(
            &emulator,
            &format!("topic-subscriber-{index}"),
            Some(&granting_sns),
        )
        .await;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("topic-dlq-{index}")).await;
        let (healthy_url, healthy_arn) =
            given_queue(&emulator, &format!("topic-healthy-{index}")).await;
        let topic_arn = given_topic(
            &emulator,
            case,
            &format!("policy-topic-{index}"),
            &subscriber_arn,
            row.policy.as_ref(),
        )
        .await;
        given_rule(
            &emulator,
            RULE,
            json!([
                {
                    "Id": "denied",
                    "Arn": topic_arn,
                    // A generous allowance, so a refusal that is NOT retried is
                    // observable as one: AWS does not retry an authorization
                    // refusal, however many attempts the target allows.
                    "RetryPolicy": { "MaximumRetryAttempts": 180, "MaximumEventAgeInSeconds": 86400 },
                    "DeadLetterConfig": { "Arn": dead_letter_arn },
                },
                { "Id": "healthy", "Arn": healthy_arn },
            ]),
        )
        .await;

        publish(&emulator).await;

        // The sibling target is decided on its own, so it receives its copy in
        // every row.
        let healthy = drain(&emulator, &healthy_url).await;
        assert_eq!(
            healthy.len(),
            1,
            "case {case:?}: the healthy sibling target must receive the event, got {healthy:?}"
        );

        let notified = drain(&emulator, &subscriber_url).await;
        let expected_notifications = usize::from(row.published);
        assert_eq!(
            notified.len(),
            expected_notifications,
            "case {case:?}: expected {expected_notifications} notification(s) on the \
             subscribed queue, got {notified:?}"
        );

        let dead_lettered = drain(&emulator, &dead_letter_url).await;
        let expected_dead_letters = usize::from(!row.published);
        assert_eq!(
            dead_lettered.len(),
            expected_dead_letters,
            "case {case:?}: expected {expected_dead_letters} dead letter(s), got \
             {dead_lettered:?}"
        );
        for message in &dead_lettered {
            // The attribute set is EXACTLY the four EventBridge stamps on a
            // dead-lettered event: an authorization refusal is not retried, so
            // nothing reports retries.
            let attributes = message["MessageAttributes"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: no MessageAttributes in {message}"));
            let mut names: Vec<&str> = attributes.keys().map(String::as_str).collect();
            names.sort_unstable();
            let mut expected = DEAD_LETTER_ATTRIBUTES.to_vec();
            expected.sort_unstable();
            assert_eq!(
                names, expected,
                "case {case:?}: expected exactly {expected:?} on the dead letter, got {message}"
            );
            assert_eq!(
                attributes["ERROR_CODE"]["StringValue"],
                json!(NO_PERMISSIONS),
                "case {case:?}: expected the dead letter to name the permission that stopped \
                 the publish, got {message}"
            );
            assert_eq!(
                attributes["RULE_ARN"]["StringValue"],
                json!(rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE)),
                "case {case:?}: expected the dead letter to name the publishing rule, got \
                 {message}"
            );
            assert_eq!(
                attributes["TARGET_ARN"]["StringValue"],
                json!(topic_arn),
                "case {case:?}: expected the dead letter to name the topic, got {message}"
            );
            // The sentence Amazon SNS answers a caller its resource policy does not
            // permit: the principal the call was made as, the action and the topic,
            // then the metadata block of the refused call. Only the request id
            // varies.
            let reported = attributes["ERROR_MESSAGE"]["StringValue"]
                .as_str()
                .unwrap_or_default();
            let expected_sentence = format!(
                "User: {SERVICE_PRINCIPAL} is not authorized to perform: SNS:Publish on \
                 resource: {topic_arn} because no resource-based policy allows the SNS:Publish \
                 action{SNS_REFUSAL_METADATA}"
            );
            assert!(
                reported.starts_with(&expected_sentence)
                    && reported.ends_with("; Proxy: null)"),
                "case {case:?}: expected the Amazon SNS refusal sentence \
                 {expected_sentence:?}, got {reported:?}"
            );
            let request_id = reported
                .trim_start_matches(&expected_sentence)
                .trim_end_matches("; Proxy: null)");
            // A UUID in the canonical 8-4-4-4-12 form, which is what an AWS request
            // id is.
            let widths: Vec<usize> = request_id.split('-').map(str::len).collect();
            assert_eq!(
                widths,
                vec![8, 4, 4, 4, 12],
                "case {case:?}: expected a request id in the 8-4-4-4-12 form, got \
                 {request_id:?}"
            );
        }
    }
}

/// Each publish is decided against the document the topic carries AT THAT INSTANT: a
/// grant written after a refused delivery lets the next one through, which is the
/// signal a client repairing a topic policy watches for.
#[tokio::test]
async fn a_topic_policy_decides_each_publish_as_it_stands_then() {
    let case = "a grant written after a refused publish";
    let emulator = emulator();
    let (subscriber_url, subscriber_arn) =
        given_queue_with_policy(&emulator, "late-subscriber", Some(&granting_sns)).await;
    let topic_arn = given_topic(&emulator, case, "late-topic", &subscriber_arn, None).await;
    given_rule(
        &emulator,
        RULE,
        json!([{ "Id": "denied", "Arn": topic_arn }]),
    )
    .await;

    publish(&emulator).await;
    let before = drain(&emulator, &subscriber_url).await;
    assert!(
        before.is_empty(),
        "case {case:?}: the topic's own policy granted nothing, so nothing must be published; \
         got {before:?}"
    );

    let granted = document(json!([grant(&topic_arn, None)]));
    let (status, written) = sns(
        &emulator,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic_arn.as_str()),
            ("AttributeName", "Policy"),
            ("AttributeValue", granted.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetTopicAttributes answered {status}: {written}"
    );

    publish(&emulator).await;
    let after = drain(&emulator, &subscriber_url).await;
    assert_eq!(
        after.len(),
        1,
        "case {case:?}: expected the notification once the grant was written, got {after:?}"
    );
}
