// SPDX-License-Identifier: CC0-1.0

//! The KEY an Amazon SNS topic target is encrypted with decides whether a rule's
//! delivery to that topic is published at all, end to end over the wire through
//! the real `MultiServiceRouter` with Amazon SNS, Amazon SQS and KMS mounted on
//! one engine — so the key is read where its own handlers wrote it and the
//! outcome is asserted where a client sees it: on the queue subscribed to the
//! topic, and on the target's dead-letter queue.
//!
//! The RULE asserted here: a topic carrying `KmsMasterKeyId` has every message
//! published to it encrypted under that key, and Amazon SNS performs that
//! encryption by calling `kms:GenerateDataKey` for the publisher — so the KEY's
//! own policy is a second document in force over such a delivery, beside the
//! topic's own access policy (`tests/all/topic_target_policy_authorization.rs`),
//! and a key policy that does not admit the EventBridge service principal is a
//! publish that does not happen however freely the topic's policy grants
//! `sns:Publish`. Nothing reaches a subscriber of the topic; the delivery FAILED,
//! so the event goes to the target's dead-letter queue under the condition
//! `NO_PERMISSIONS`, carrying the two nested refusals the call produced — KMS's
//! denial of the `kms:GenerateDataKey` inside Amazon SNS's `KMSAccessDenied` — and
//! it is not retried. A key policy written afterwards lets the next delivery
//! through with nothing else changed.
//!
//! The table spans the class rather than the one recorded case: the rows differ
//! along what the key's policy states — whether the topic names a key at all,
//! whether a statement about the service principal exists, which action and
//! principal it is about, which `Effect` it states, and which condition it is
//! scoped by — and along whether the key is one this account holds.
//!
//! The harness (the router with EventBridge, Amazon SQS, Amazon SNS, KMS, IAM and
//! CloudWatch on one engine, the queue fixtures and the rule fixture) is
//! `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, NO_PERMISSIONS, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    drain, emulator, given_queue, given_queue_with_policy, given_rule, publish, rpc, sns,
    xml_text, Emulator, REGION,
};

/// The rule every test here publishes through.
const RULE: &str = "encrypted-topic-rule";

/// The attribute Amazon SNS reports an encrypted topic's key under.
const KMS_MASTER_KEY_ID: &str = "KmsMasterKeyId";

/// The dead-letter attributes EventBridge stamps on an undeliverable event — the
/// EXACT set a refusal that is not retried carries.
const DEAD_LETTER_ATTRIBUTES: [&str; 4] =
    ["ERROR_CODE", "ERROR_MESSAGE", "RULE_ARN", "TARGET_ARN"];

/// What the two nested refusals read with every request id normalized away, which
/// is the whole of the requirement: the innermost sentence is KMS's for a denial
/// that can report nothing about the resource, wrapped in the metadata block of
/// the refused `kms:GenerateDataKey` and then in Amazon SNS's own
/// `KMSAccessDenied`.
fn expected_message() -> String {
    format!(
        "User: {SERVICE_PRINCIPAL} is not authorized to perform: kms:GenerateDataKey on this \
         resource because the resource does not exist in this Region, no resource-based \
         policies allow access, or a resource-based policy explicitly denies access (Service: \
         AWSKMS; Status Code: 400; Error Code: AccessDeniedException; Request ID: <id>; Proxy: \
         null) (Service: AmazonSNS; Status Code: 400; Error Code: KMSAccessDenied; Request ID: \
         <id>; Proxy: null)"
    )
}

/// `message` with each request id it reports replaced by `<id>`, asserting on the
/// way that each one has the 8-4-4-4-12 form an AWS request id carries: the ids
/// identify the two calls that were refused, so they differ per delivery and are
/// no part of the sentence.
fn normalized(case: &str, message: &str) -> String {
    const MARKER: &str = "Request ID: ";

    let mut rest = message;
    let mut out = String::with_capacity(message.len());
    let mut found = 0usize;
    while let Some(at) = rest.find(MARKER) {
        let (before, after) = rest.split_at(at + MARKER.len());
        out.push_str(before);
        let end = after
            .find(';')
            .unwrap_or_else(|| panic!("case {case:?}: an unterminated request id in {message:?}"));
        let id = &after[..end];
        let widths: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            widths,
            vec![8, 4, 4, 4, 12],
            "case {case:?}: expected a request id in the 8-4-4-4-12 form, got {id:?}"
        );
        out.push_str("<id>");
        rest = &after[end..];
        found += 1;
    }
    out.push_str(rest);
    assert_eq!(
        found, 2,
        "case {case:?}: expected one request id per nested refusal, got {message:?}"
    );
    out
}

/// The account every resource below belongs to.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The statement every key policy carries beside what a row states: the account
/// root's own authority over the key, which is what `CreateKey` writes and what
/// the lockout safety check of `PutKeyPolicy` requires of a replacement.
fn root_statement() -> Value {
    json!({
        "Sid": "Enable IAM policies",
        "Effect": "Allow",
        "Principal": { "AWS": format!("arn:aws:iam::{}:root", account()) },
        "Action": "kms:*",
        "Resource": "*",
    })
}

/// A key policy document carrying the root statement and whatever `statements`
/// adds to it.
fn key_policy(statements: Vec<Value>) -> String {
    let mut all = vec![root_statement()];
    all.extend(statements);
    json!({ "Version": "2012-10-17", "Id": "key-default", "Statement": all }).to_string()
}

/// A statement about the EventBridge service principal: `effect` of `actions`,
/// scoped by `condition` when the row states one.
fn about_events(effect: &str, actions: Value, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Principal": { "Service": SERVICE_PRINCIPAL },
        "Action": actions,
        "Resource": "*",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// The policy a queue SUBSCRIBED to a topic needs for the notification to reach
/// it: Amazon SNS's own send, which that service decides against this document.
fn granting_sns(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// One KMS call (`awsJson1_1`, `TrentService.<Operation>`) that must answer 200.
async fn kms(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = rpc(emulator, "kms", "TrentService", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: KMS {operation} answered {status}: {answered}"
    );
    answered
}

/// A customer managed key carrying `policy` when a row states one, answered as its
/// ARN.
async fn given_key(emulator: &Emulator, case: &str, policy: Option<String>) -> String {
    let created = kms(emulator, case, "CreateKey", &json!({})).await;
    let arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no KeyMetadata.Arn in {created}"))
        .to_string();
    if let Some(policy) = policy {
        kms(
            emulator,
            case,
            "PutKeyPolicy",
            &json!({ "KeyId": arn, "PolicyName": "default", "Policy": policy }),
        )
        .await;
    }
    arn
}

/// A topic called `name`, encrypted with `key` when the row names one, whose own
/// access policy grants this service the publish, with `subscriber_arn` subscribed
/// to it; answers the topic's ARN.
async fn given_encrypted_topic(
    emulator: &Emulator,
    case: &str,
    name: &str,
    key: Option<&str>,
    subscriber_arn: &str,
) -> String {
    let (status, created) = sns(emulator, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTopic answered {status}: {created}"
    );
    let arn = xml_text(&created, "TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TopicArn in {created}"));
    // The topic's OWN policy grants the publish in every row: what is under test
    // is the key, so nothing about the topic's access policy may withhold the
    // delivery.
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": SERVICE_PRINCIPAL },
            "Action": "sns:Publish",
            "Resource": arn,
        }],
    })
    .to_string();
    set_attribute(emulator, case, &arn, "Policy", &policy).await;
    if let Some(key) = key {
        set_attribute(emulator, case, &arn, KMS_MASTER_KEY_ID, key).await;
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

/// Write one attribute onto a topic, as a client does.
async fn set_attribute(
    emulator: &Emulator,
    case: &str,
    topic_arn: &str,
    name: &str,
    value: &str,
) {
    let (status, written) = sns(
        emulator,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic_arn),
            ("AttributeName", name),
            ("AttributeValue", value),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetTopicAttributes {name} answered {status}: {written}"
    );
}

/// Write the rule carrying the encrypted topic as its target, with the
/// dead-letter queue and the retry allowance the recorded case uses.
async fn given_topic_rule(emulator: &Emulator, topic_arn: &str, dead_letter_arn: &str) {
    given_rule(
        emulator,
        RULE,
        json!([{
            "Id": "topic",
            "Arn": topic_arn,
            // No retry at all: a refusal the key's policy decided is permanent,
            // so the dead letter must carry no retry attributes either.
            "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
            "DeadLetterConfig": { "Arn": dead_letter_arn },
        }]),
    )
    .await;
}

/// What a row's key is.
enum Key {
    /// The topic names no key at all.
    None,
    /// A key this account holds, carrying the policy the row states — `None`
    /// leaving the one `CreateKey` writes.
    Own(Option<Vec<Value>>),
    /// A key ARN of this Region that no key exists for.
    Unknown,
}

/// One row: the case, the key the topic is encrypted with, and whether
/// EventBridge publishes to it.
struct Row {
    case: &'static str,
    key: Key,
    published: bool,
}

/// The key's own policy decides each delivery to an encrypted topic target, over a
/// table of key policies.
#[tokio::test]
async fn an_encrypted_topic_target_is_published_to_only_where_its_key_allows() {
    let rows = vec![
        Row {
            case: "a topic on no customer managed key",
            key: Key::None,
            published: true,
        },
        Row {
            case: "a key carrying only the policy CreateKey writes",
            key: Key::Own(None),
            published: false,
        },
        Row {
            case: "a key granting the service principal the data-key actions",
            key: Key::Own(Some(vec![about_events(
                "Allow",
                json!(["kms:GenerateDataKey*", "kms:Decrypt"]),
                None,
            )])),
            published: true,
        },
        Row {
            case: "a key granting the service principal every action of the service",
            key: Key::Own(Some(vec![about_events("Allow", json!("kms:*"), None)])),
            published: true,
        },
        Row {
            case: "a key granting the service principal another action only",
            key: Key::Own(Some(vec![about_events("Allow", json!("kms:Decrypt"), None)])),
            published: false,
        },
        Row {
            case: "a key whose grant to the service principal is denied beside it",
            key: Key::Own(Some(vec![
                about_events("Allow", json!("kms:GenerateDataKey*"), None),
                about_events("Deny", json!("kms:GenerateDataKey"), None),
            ])),
            published: false,
        },
        Row {
            case: "a key granting another service principal",
            key: Key::Own(Some(vec![json!({
                "Effect": "Allow",
                "Principal": { "Service": "s3.amazonaws.com" },
                "Action": "kms:GenerateDataKey*",
                "Resource": "*",
            })])),
            published: false,
        },
        Row {
            case: "a grant scoped by kms:ViaService to Amazon SNS in this Region",
            key: Key::Own(Some(vec![about_events(
                "Allow",
                json!("kms:GenerateDataKey*"),
                Some(json!({
                    "StringEquals": { "kms:ViaService": format!("sns.{REGION}.amazonaws.com") },
                })),
            )])),
            published: true,
        },
        Row {
            case: "a grant scoped by kms:ViaService to another service",
            key: Key::Own(Some(vec![about_events(
                "Allow",
                json!("kms:GenerateDataKey*"),
                Some(json!({
                    "StringEquals": { "kms:ViaService": format!("sqs.{REGION}.amazonaws.com") },
                })),
            )])),
            published: false,
        },
        Row {
            // A key this account holds no record of is evidence of nothing: there
            // is no policy to weigh, so the delivery is not withheld on one.
            case: "a key ARN this account does not hold",
            key: Key::Unknown,
            published: true,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (subscriber_url, subscriber_arn) = given_queue_with_policy(
            &emulator,
            &format!("encrypted-topic-subscriber-{index}"),
            Some(&granting_sns),
        )
        .await;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("encrypted-topic-dlq-{index}")).await;
        let key = match row.key {
            Key::None => None,
            Key::Own(statements) => {
                Some(given_key(&emulator, case, statements.map(key_policy)).await)
            }
            Key::Unknown => Some(format!(
                "arn:aws:kms:{REGION}:{}:key/00000000-0000-0000-0000-000000000000",
                account()
            )),
        };
        let topic_arn = given_encrypted_topic(
            &emulator,
            case,
            &format!("encrypted-topic-{index}"),
            key.as_deref(),
            &subscriber_arn,
        )
        .await;
        given_topic_rule(&emulator, &topic_arn, &dead_letter_arn).await;

        publish(&emulator).await;

        let notified = drain(&emulator, &subscriber_url).await;
        let expected_notifications = usize::from(row.published);
        assert_eq!(
            notified.len(),
            expected_notifications,
            "case {case:?}: expected {expected_notifications} notification(s) on the subscribed \
             queue, got {notified:?}"
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
            let reported = attributes["ERROR_MESSAGE"]["StringValue"]
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                normalized(case, reported),
                expected_message(),
                "case {case:?}: expected the nested KMS and Amazon SNS refusal, got {reported:?}"
            );
        }
    }
}

/// Each delivery is decided against the key policy in force AT THAT INSTANT: a
/// grant written after a refused delivery lets the next one through, with nothing
/// about the topic, the rule or the target changed in between — which is the
/// signal a client repairing a mis-scoped key policy watches for.
#[tokio::test]
async fn a_key_policy_written_after_a_refusal_lets_the_next_delivery_through() {
    let case = "a key policy granting the service principal after a refused delivery";
    let emulator = emulator();
    let (subscriber_url, subscriber_arn) =
        given_queue_with_policy(&emulator, "repaired-subscriber", Some(&granting_sns)).await;
    let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "repaired-dlq").await;
    let key = given_key(&emulator, case, None).await;
    let topic_arn = given_encrypted_topic(
        &emulator,
        case,
        "repaired-topic",
        Some(&key),
        &subscriber_arn,
    )
    .await;
    given_topic_rule(&emulator, &topic_arn, &dead_letter_arn).await;

    publish(&emulator).await;
    let withheld = drain(&emulator, &subscriber_url).await;
    assert!(
        withheld.is_empty(),
        "case {case:?}: the key's policy admits this service nothing, so nothing must be \
         published; got {withheld:?}"
    );
    let dead_lettered = drain(&emulator, &dead_letter_url).await;
    assert_eq!(
        dead_lettered.len(),
        1,
        "case {case:?}: expected the refused delivery on the dead-letter queue, got \
         {dead_lettered:?}"
    );

    kms(
        &emulator,
        case,
        "PutKeyPolicy",
        &json!({
            "KeyId": key,
            "PolicyName": "default",
            "Policy": key_policy(vec![about_events(
                "Allow",
                json!(["kms:GenerateDataKey*", "kms:Decrypt"]),
                None,
            )]),
        }),
    )
    .await;

    publish(&emulator).await;
    let notified = drain(&emulator, &subscriber_url).await;
    assert_eq!(
        notified.len(),
        1,
        "case {case:?}: expected the notification once the key policy granted the data key, \
         got {notified:?}"
    );
    let after = drain(&emulator, &dead_letter_url).await;
    assert!(
        after.is_empty(),
        "case {case:?}: nothing may be dead-lettered once the delivery goes through, got \
         {after:?}"
    );
}
