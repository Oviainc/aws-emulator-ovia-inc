// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the region a queue belongs to, driven over the wire (`POST /`
//! with `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: **a queue belongs to the region the call that created it
//! addressed, so a queue is identified by that region together with its name — its ARN
//! carries that region, the same name created against a second region is a second,
//! independent queue, and every later reference (a url, a name, a dead-letter target ARN,
//! a message movement task's ARNs and handle) resolves only inside the region the request
//! addressed.**
//!
//! Each table spans what the rule generalises over rather than one example: the REGION
//! (five of them, from three continents, so nothing can be keyed to one label), the
//! OPERATION (every url-addressed one, plus the name-addressed lookup and the listing),
//! the FORM of the reference (a url, a `QueueName`, a `deadLetterTargetArn`, a `SourceArn`
//! and a `DestinationArn`, a `TaskHandle`) and the COUNT of messages each region's queue
//! holds. Every expected message comes from the constants
//! [`service_com_amazonaws_sqs_amazonsqs::state`] exports, never from a string copied out
//! of an answer.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    invalid_parameter_message, DEAD_LETTER_TARGET_NAMESPACE_REASON, QUEUE_DOES_NOT_EXIST_MESSAGE,
    REDRIVE_POLICY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    account, as_map, call_in, create_in, create_ok_in, emulator, error_code, expected_arn_in,
    expected_url_in, messages, ok_in, receive_until_in, OTHER_ACCOUNT, REGION,
};

/// The regions every table walks: the harness's own, two more commercial ones on other
/// continents, and two further apart again — so no row can be satisfied by a rule keyed to
/// one region label, and the ARNs of one name differ in exactly one field.
const REGIONS: &[&str] = &[REGION, "us-west-2", "eu-central-1", "ap-southeast-2", "sa-east-1"];

/// How long a receive loop waits for the messages a queue was sent. Generous, so a slow
/// machine cannot fail a row the emulator is still working through.
const RECEIVE_DEADLINE: Duration = Duration::from_secs(10);

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// One attribute the queue at `url` reports, read in `region`.
async fn attribute_in(
    router: &MultiServiceRouter,
    region: &str,
    url: &str,
    attribute: &str,
) -> String {
    let reported = ok_in(
        router,
        region,
        "GetQueueAttributes",
        json!({"QueueUrl": url, "AttributeNames": [attribute]}),
    )
    .await;
    as_map(&reported["Attributes"])
        .get(attribute)
        .cloned()
        .unwrap_or_else(|| panic!("case {attribute} of {url} in {region}: not reported"))
}

/// A `RedrivePolicy` value driving a queue's messages to `target_arn`.
fn redrive_policy(target_arn: &str) -> String {
    json!({"deadLetterTargetArn": target_arn, "maxReceiveCount": "3"}).to_string()
}

/// A queue created in `region` reports the ARN of that region, and the url it is reachable
/// at is that region's endpoint — so two queues of one name in two regions are two
/// resources whose ARNs differ only in the region field.
#[tokio::test]
async fn a_queues_arn_carries_the_region_its_create_addressed() {
    let router = emulator();
    let name = "regional-identity";

    let mut arns: Vec<String> = Vec::with_capacity(REGIONS.len());
    for region in REGIONS {
        let url = create_ok_in(&router, region, name, &json!({})).await;
        assert_eq!(
            url,
            expected_url_in(region, name),
            "case {region}: the url must be minted on the endpoint the create addressed"
        );

        let arn = attribute_in(&router, region, &url, "QueueArn").await;
        assert_eq!(
            arn,
            expected_arn_in(region, name),
            "case {region}: expected the ARN of this region, got {arn:?}"
        );

        // The name is the resource part of every one of them, so the ARNs differ in the
        // region field and in nothing else.
        assert_eq!(
            arn.rsplit(':').next(),
            Some(name),
            "case {region}: the ARN must name the queue, got {arn:?}"
        );
        arns.push(arn);
    }

    for (position, arn) in arns.iter().enumerate() {
        for other in arns.iter().skip(position + 1) {
            assert_ne!(
                arn, other,
                "case two regions: the same name in two regions must report two ARNs, \
                 got {arn:?} twice"
            );
        }
    }

    // A name lookup answers with the url of the region it was made in, for every region.
    for region in REGIONS {
        let resolved = ok_in(&router, region, "GetQueueUrl", json!({"QueueName": name})).await;
        assert_eq!(
            resolved["QueueUrl"].as_str(),
            Some(expected_url_in(region, name).as_str()),
            "case {region}: GetQueueUrl must answer with this region's queue, got {resolved}"
        );
    }
}

/// The same name created in several regions is several queues: each one delivers exactly
/// the messages sent to it, and reports exactly its own count.
#[tokio::test]
async fn the_same_name_in_two_regions_holds_separate_messages() {
    let router = emulator();
    let name = "regional-messages";

    // Each region gets a different NUMBER of messages, so a queue serving another region's
    // messages is visible in the count as well as in the bodies.
    let mut urls: Vec<(&str, String, Vec<String>)> = Vec::with_capacity(REGIONS.len());
    for (index, region) in REGIONS.iter().enumerate() {
        let url = create_ok_in(&router, region, name, &json!({})).await;
        let bodies: Vec<String> = (0..=index)
            .map(|number| format!("{region} body #{number}"))
            .collect();
        for body in &bodies {
            let sent = ok_in(
                &router,
                region,
                "SendMessage",
                json!({"QueueUrl": url, "MessageBody": body}),
            )
            .await;
            assert!(
                sent.get("MessageId").and_then(Value::as_str).is_some(),
                "case {region}: a send must mint a message id, got {sent}"
            );
        }
        urls.push((region, url, bodies));
    }

    for (region, url, bodies) in &urls {
        let expected_count = i64::try_from(bodies.len()).expect("a small count");
        let reported = attribute_in(&router, region, url, "ApproximateNumberOfMessages").await;
        assert_eq!(
            reported.parse::<i64>().ok(),
            Some(expected_count),
            "case {region}: expected {expected_count} messages of its own, got {reported:?}"
        );

        let mut delivered: Vec<String> = Vec::new();
        let deadline = Instant::now() + RECEIVE_DEADLINE;
        while delivered.len() < bodies.len() && Instant::now() < deadline {
            let reply = ok_in(
                &router,
                region,
                "ReceiveMessage",
                json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 1}),
            )
            .await;
            for message in messages(&reply) {
                delivered.push(
                    message["Body"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        delivered.sort();
        let mut expected = bodies.clone();
        expected.sort();
        assert_eq!(
            delivered, expected,
            "case {region}: a region's queue must deliver exactly its own messages, \
             got {delivered:?}"
        );
    }
}

/// Every url-addressed operation, and the name-addressed lookup, refuses a queue that
/// exists only in another region: the queue does not exist in the region the call
/// addressed.
#[tokio::test]
async fn a_queue_of_another_region_is_not_served() {
    let router = emulator();
    let home = REGION;
    let name = "regional-only-home";
    let url = create_ok_in(&router, home, name, &json!({})).await;

    // (operation, a request that is otherwise well formed, so the only thing a refusal can
    // be about is the queue).
    let requests: Vec<(&str, Value)> = vec![
        (
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": ["All"]}),
        ),
        (
            "SetQueueAttributes",
            json!({"QueueUrl": url, "Attributes": {"VisibilityTimeout": "10"}}),
        ),
        ("SendMessage", json!({"QueueUrl": url, "MessageBody": "hello"})),
        (
            "SendMessageBatch",
            json!({"QueueUrl": url, "Entries": [{"Id": "one", "MessageBody": "hello"}]}),
        ),
        ("ReceiveMessage", json!({"QueueUrl": url})),
        (
            "DeleteMessage",
            json!({"QueueUrl": url, "ReceiptHandle": "not-a-handle"}),
        ),
        (
            "ChangeMessageVisibility",
            json!({"QueueUrl": url, "ReceiptHandle": "not-a-handle", "VisibilityTimeout": 10}),
        ),
        ("PurgeQueue", json!({"QueueUrl": url})),
        ("ListQueueTags", json!({"QueueUrl": url})),
        ("ListDeadLetterSourceQueues", json!({"QueueUrl": url})),
        ("TagQueue", json!({"QueueUrl": url, "Tags": {"team": "platform"}})),
        ("UntagQueue", json!({"QueueUrl": url, "TagKeys": ["team"]})),
        (
            "AddPermission",
            json!({"QueueUrl": url, "Label": "shared",
                   "AWSAccountIds": [OTHER_ACCOUNT], "Actions": ["SendMessage"]}),
        ),
        ("RemovePermission", json!({"QueueUrl": url, "Label": "shared"})),
        ("GetQueueUrl", json!({"QueueName": name})),
        ("DeleteQueue", json!({"QueueUrl": url})),
    ];

    // Every region but the one the queue was created in, so no row depends on which other
    // region is asked.
    for region in REGIONS.iter().filter(|region| **region != home) {
        for (operation, request) in &requests {
            let (status, parsed) = call_in(&router, region, operation, request).await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "QueueDoesNotExist"),
                "case {operation} from {region}: a queue of {home} must not be served there, \
                 got {status} with {parsed}"
            );
            assert_eq!(
                message_of(&parsed),
                QUEUE_DOES_NOT_EXIST_MESSAGE,
                "case {operation} from {region}: expected Amazon SQS's fixed message"
            );
        }
    }

    // And the queue is still there, with nothing any of those calls did to it: the listing
    // of its own region reports it, and no other region's listing reports anything.
    let listed = ok_in(&router, home, "ListQueues", json!({})).await;
    assert_eq!(
        listed["QueueUrls"].as_array().map(Vec::len),
        Some(1),
        "case the home listing: the queue must have survived, got {listed}"
    );
    for region in REGIONS.iter().filter(|region| **region != home) {
        let empty = ok_in(&router, region, "ListQueues", json!({})).await;
        assert!(
            empty.get("QueueUrls").is_none(),
            "case the {region} listing: another region's queue must not be listed, got {empty}"
        );
    }
}

/// A url minted in one region, presented in another that has a queue of the SAME name,
/// addresses the queue of the region the call was made in — the endpoint the request
/// reached is what decides, exactly as the real service serves it.
#[tokio::test]
async fn a_url_is_served_by_the_queue_of_the_region_the_call_addressed() {
    let router = emulator();
    let name = "regional-same-name";
    let home = REGION;
    let far = "eu-central-1";
    let home_url = create_ok_in(&router, home, name, &json!({})).await;
    let far_url = create_ok_in(&router, far, name, &json!({})).await;

    // The home region's url, presented to the far region: the far region's own queue takes
    // the message.
    let _ = ok_in(
        &router,
        far,
        "SendMessage",
        json!({"QueueUrl": home_url, "MessageBody": "for the far region"}),
    )
    .await;

    let home_count = attribute_in(&router, home, &home_url, "ApproximateNumberOfMessages").await;
    let far_count = attribute_in(&router, far, &far_url, "ApproximateNumberOfMessages").await;
    assert_eq!(
        (home_count.as_str(), far_count.as_str()),
        ("0", "1"),
        "case a url of one region used in another: the call must land in the region it was \
         made in, got {home_count:?} at home and {far_count:?} away"
    );

    let delivered =
        receive_until_in(&router, far, &far_url, 1, Instant::now() + RECEIVE_DEADLINE).await;
    assert_eq!(
        delivered
            .iter()
            .map(|(_, body)| body.clone())
            .collect::<Vec<String>>(),
        vec!["for the far region".to_string()],
        "case the far region's queue: it must hold the message the call left there, \
         got {delivered:?}"
    );
}

/// A `deadLetterTargetArn` is refused unless it names a queue of the SAME account and
/// region as the queue carrying the policy — and a refusal leaves the attribute as it was.
#[tokio::test]
async fn a_dead_letter_target_outside_the_queues_own_region_is_refused() {
    assert_ne!(
        OTHER_ACCOUNT,
        account(),
        "case the other-account row: its account id must not be this emulator's"
    );
    let home = REGION;
    let target = "regional-dlq";

    // (case, the target ARN, whether the policy is refused). The rows walk every namespace
    // a target can be in: this one, each other region, and another account.
    let mut rows: Vec<(String, String, bool)> = vec![
        (
            "a target of this region".to_string(),
            expected_arn_in(home, target),
            false,
        ),
        (
            "a target of another account".to_string(),
            format!("arn:aws:sqs:{home}:{OTHER_ACCOUNT}:{target}"),
            true,
        ),
    ];
    for region in REGIONS.iter().filter(|region| **region != home) {
        rows.push((
            format!("a target of {region}"),
            expected_arn_in(region, target),
            true,
        ));
    }

    for (index, (case, target_arn, refused)) in rows.iter().enumerate() {
        let router = emulator();
        let _ = create_ok_in(&router, home, target, &json!({})).await;
        let policy = redrive_policy(target_arn);
        // Spelled out rather than composed from the constants the handler itself uses, so
        // this row fails if either the template or the reason changes: the fixed text is
        // Amazon SQS's own refused-parameter-value template and the rule the model states
        // for a dead-letter target, and the only variable part is the `RedrivePolicy` this
        // request supplied.
        let expected_message = refused.then(|| {
            format!(
                "Value {policy} for parameter RedrivePolicy is invalid. Reason: Only source \
                 queues in this Amazon Web Services account in the same Region can specify \
                 this queue as the dead-letter queue."
            )
        });
        assert_eq!(
            expected_message.as_deref(),
            refused
                .then(|| invalid_parameter_message(
                    &policy,
                    REDRIVE_POLICY,
                    DEAD_LETTER_TARGET_NAMESPACE_REASON
                ))
                .as_deref(),
            "case {case}: the expected message must be the one the service composes"
        );

        // 1. The create that would carry the policy.
        let name = format!("regional-source-{index}");
        let (status, parsed) =
            create_in(&router, home, &name, &json!({REDRIVE_POLICY: policy})).await;
        match &expected_message {
            None => assert_eq!(
                status, 200,
                "case {case} on CreateQueue: expected the policy to be stored, \
                 got {status} with {parsed}"
            ),
            Some(message) => {
                assert_eq!(
                    (status, error_code(&parsed).as_str()),
                    (400, "InvalidParameterValue"),
                    "case {case} on CreateQueue: expected a 400 InvalidParameterValue, \
                     got {status} with {parsed}"
                );
                assert_eq!(
                    message_of(&parsed),
                    *message,
                    "case {case} on CreateQueue: expected the refusal to state the rule"
                );
            }
        }

        // 2. The same policy uploaded onto a queue that already exists, which must leave
        //    the attribute exactly as it was when it is refused.
        let plain = format!("regional-plain-{index}");
        let url = create_ok_in(&router, home, &plain, &json!({})).await;
        let before = ok_in(
            &router,
            home,
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": ["All"]}),
        )
        .await;
        let (status, parsed) = call_in(
            &router,
            home,
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": {REDRIVE_POLICY: policy}}),
        )
        .await;
        match &expected_message {
            None => assert_eq!(
                status, 200,
                "case {case} on SetQueueAttributes: expected the policy to be stored, \
                 got {status} with {parsed}"
            ),
            Some(message) => {
                assert_eq!(
                    (status, error_code(&parsed).as_str()),
                    (400, "InvalidParameterValue"),
                    "case {case} on SetQueueAttributes: expected a 400 InvalidParameterValue, \
                     got {status} with {parsed}"
                );
                assert_eq!(
                    message_of(&parsed),
                    *message,
                    "case {case} on SetQueueAttributes: expected the refusal to state the rule"
                );
            }
        }

        let after = ok_in(
            &router,
            home,
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": ["All"]}),
        )
        .await;
        let stored = as_map(&after["Attributes"])
            .get(REDRIVE_POLICY)
            .cloned();
        match &expected_message {
            None => assert_eq!(
                stored.as_deref(),
                Some(policy.as_str()),
                "case {case}: an accepted policy must be what the queue reports"
            ),
            Some(_) => assert_eq!(
                stored,
                as_map(&before["Attributes"]).get(REDRIVE_POLICY).cloned(),
                "case {case}: a refused policy must leave the attribute as it was"
            ),
        }
    }
}

/// A message movement task belongs to one region: a start whose `SourceArn` or
/// `DestinationArn` names another region is refused, and the handle a started task
/// answered with names no task in any other region.
#[tokio::test]
async fn a_message_move_task_cannot_span_two_regions() {
    let home = REGION;
    let far = "us-west-2";

    // A dead-letter queue of the home region with one source queue, which is the only kind
    // of source a task supports.
    async fn build(router: &MultiServiceRouter, region: &str) {
        let dead_letter = create_ok_in(router, region, "move-dlq", &json!({})).await;
        let _ = create_ok_in(
            router,
            region,
            "move-source",
            &json!({REDRIVE_POLICY: redrive_policy(&expected_arn_in(region, "move-dlq"))}),
        )
        .await;
        let _ = ok_in(
            router,
            region,
            "SendMessage",
            json!({"QueueUrl": dead_letter, "MessageBody": "to be moved"}),
        )
        .await;
    }

    /// One refused start: the case, the region the call is made in, and the request it
    /// carries.
    struct Row {
        case: &'static str,
        region: &'static str,
        request: Value,
    }

    // Every way a start call can straddle a region boundary: the pair of the home region
    // addressed from elsewhere, a destination elsewhere, and a source elsewhere.
    let rows: Vec<Row> = vec![
        Row {
            case: "the home region's ARNs, started from another region",
            region: far,
            request: json!({"SourceArn": expected_arn_in(home, "move-dlq")}),
        },
        Row {
            case: "a destination of another region",
            region: home,
            request: json!({
                "SourceArn": expected_arn_in(home, "move-dlq"),
                "DestinationArn": expected_arn_in(far, "move-source"),
            }),
        },
        Row {
            case: "a source of another region",
            region: home,
            request: json!({"SourceArn": expected_arn_in("eu-central-1", "move-dlq")}),
        },
    ];

    for Row {
        case,
        region,
        request,
    } in &rows
    {
        let router = emulator();
        build(&router, home).await;
        let (status, parsed) = call_in(&router, region, "StartMessageMoveTask", request).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: a task cannot span two regions, got {status} with {parsed}"
        );
        assert!(
            parsed.get("TaskHandle").is_none(),
            "case {case}: a refused start must hand back no handle, got {parsed}"
        );
    }

    // The same pair INSIDE one region is served, and the task it starts belongs to that
    // region alone: another region neither lists it nor can cancel it.
    let router = emulator();
    build(&router, home).await;
    let started = ok_in(
        &router,
        home,
        "StartMessageMoveTask",
        json!({"SourceArn": expected_arn_in(home, "move-dlq")}),
    )
    .await;
    let handle = started["TaskHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case a task inside one region: no TaskHandle in {started}"))
        .to_string();

    let listed = ok_in(
        &router,
        home,
        "ListMessageMoveTasks",
        json!({"SourceArn": expected_arn_in(home, "move-dlq")}),
    )
    .await;
    assert_eq!(
        listed["Results"][0]["TaskHandle"].as_str(),
        Some(handle.as_str()),
        "case a task inside one region: its own region must list it, got {listed}"
    );
    assert_eq!(
        listed["Results"][0]["SourceArn"].as_str(),
        Some(expected_arn_in(home, "move-dlq").as_str()),
        "case a task inside one region: the ARNs it reports carry that region, got {listed}"
    );

    let (status, parsed) = call_in(
        &router,
        far,
        "ListMessageMoveTasks",
        &json!({"SourceArn": expected_arn_in(home, "move-dlq")}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (404, "ResourceNotFoundException"),
        "case listing from another region: the source queue is not there, \
         got {status} with {parsed}"
    );

    let (status, parsed) = call_in(
        &router,
        far,
        "CancelMessageMoveTask",
        &json!({"TaskHandle": handle}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (404, "ResourceNotFoundException"),
        "case cancelling from another region: the handle names no task there, \
         got {status} with {parsed}"
    );

    // And the task is still the home region's to cancel.
    let cancelled = ok_in(
        &router,
        home,
        "CancelMessageMoveTask",
        json!({"TaskHandle": handle}),
    )
    .await;
    assert!(
        cancelled
            .get("ApproximateNumberOfMessagesMoved")
            .is_some(),
        "case cancelling at home: the task's own region must cancel it, got {cancelled}"
    );
}

/// Deleting a queue defers its name in the region it was deleted in and nowhere else: the
/// same name is immediately available in every other region, while a re-create in its own
/// region waits for the deletion to propagate.
#[tokio::test]
async fn a_deleted_name_is_deferred_only_in_the_region_it_was_deleted_in() {
    let router = emulator();
    let home = REGION;
    let name = "regional-deleted";
    let url = create_ok_in(&router, home, name, &json!({})).await;
    let _ = ok_in(&router, home, "DeleteQueue", json!({"QueueUrl": url})).await;

    let (status, parsed) = create_in(&router, home, name, &json!({})).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "QueueDeletedRecently"),
        "case the region the delete was made in: the name is deferred there, \
         got {status} with {parsed}"
    );

    for region in REGIONS.iter().filter(|region| **region != home) {
        let created = create_ok_in(&router, region, name, &json!({})).await;
        assert_eq!(
            created,
            expected_url_in(region, name),
            "case {region}: a name another region deleted is available here"
        );
    }
}
