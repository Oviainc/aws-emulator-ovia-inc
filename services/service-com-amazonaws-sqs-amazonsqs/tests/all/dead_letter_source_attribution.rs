// SPDX-License-Identifier: CC0-1.0

//! Where a dead-lettered message came FROM, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in
//! [`super::support`].
//!
//! The rule pinned here: a message a `RedrivePolicy` moved to a dead-letter queue carries
//! the ARN of the queue it was moved OUT OF as its `DeadLetterQueueSourceArn` message
//! system attribute, so a receive on the dead-letter queue selecting that name — or `All`,
//! which "returns all values" — reports it, whichever queue of the store drove the message
//! there. A message no redrive moved carries none and reports none, and the attribute
//! travels with the message when a movement task drives it out of the dead-letter queue
//! again.
//!
//! The tables span what the rule generalises over rather than one example: the number of
//! source queues sharing one dead-letter queue (one, two and three — a dead-letter queue is
//! routinely a fan-in point), both queue types, the delivery allowance the policy spends
//! before the move, and both members a request may select system attribute names on
//! crossed with the ways a name is selected (`All`, the name itself, another name only).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::support::{
    as_map, attribute_count as count, create_ok, emulator, expected_arn, fifo_attributes,
    messages, ok,
};
use smithy_server_runtime::MultiServiceRouter;

/// How long a loop waits for the moves the redrive owes before giving up. Far longer than
/// any row's own schedule, so a slow machine cannot fail a row the emulator is still
/// working through.
const DEADLINE: Duration = Duration::from_secs(20);

/// The attribute whose presence and value this suite is about. One of the values
/// `MessageSystemAttributeName` publishes — source: `smithy_query(query="shape",
/// shape="MessageSystemAttributeName")` on this crate's model.
const SOURCE_ARN: &str = "DeadLetterQueueSourceArn";

/// The `RedrivePolicy` attribute value driving a queue's messages to `target_arn` after
/// `max_receive_count` deliveries.
fn redrive_policy(target_arn: &str, max_receive_count: i64) -> String {
    json!({
        "deadLetterTargetArn": target_arn,
        "maxReceiveCount": max_receive_count.to_string(),
    })
    .to_string()
}

/// One message's system `Attributes` map, empty when the answer carries none.
fn attributes(message: &Value) -> BTreeMap<String, String> {
    message.get("Attributes").map(as_map).unwrap_or_default()
}

/// Send `body` to the queue at `url`, as a FIFO send under `group` when one is given —
/// a FIFO send needs a group and a deduplication token, and giving each message a group of
/// its own lets a FIFO dead-letter queue hand every one of them over at once.
///
/// A `MessageGroupId` "can only include alphanumeric and punctuation characters", so the
/// token is passed rather than derived from a body carrying spaces.
async fn send(router: &MultiServiceRouter, url: &str, body: &str, group: Option<&str>) {
    let mut request = json!({"QueueUrl": url, "MessageBody": body});
    if let Some(group) = group {
        request["MessageGroupId"] = json!(group);
        request["MessageDeduplicationId"] = json!(group);
    }
    ok(router, "SendMessage", request).await;
}

/// Receive from the queue at `url` without keeping anything: how a source queue's delivery
/// allowance is spent, which is what makes the redrive due.
async fn poll(router: &MultiServiceRouter, url: &str) {
    ok(
        router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 0}),
    )
    .await;
}

/// Poll each queue in `source_urls` until the dead-letter queue at `dead_letter_url` holds
/// `wanted` messages.
async fn drive_to_the_dead_letter_queue(
    router: &MultiServiceRouter,
    case: &str,
    source_urls: &[String],
    dead_letter_url: &str,
    wanted: i64,
) {
    let deadline = Instant::now() + DEADLINE;
    while count(router, dead_letter_url, "ApproximateNumberOfMessages").await < wanted {
        assert!(
            Instant::now() < deadline,
            "case {case}: expected {wanted} messages to reach the dead-letter queue within \
             {DEADLINE:?}"
        );
        for url in source_urls {
            poll(router, url).await;
        }
    }
}

/// Receive from the queue at `url` until `wanted` messages have come over, reporting each
/// one's `(Body, Attributes)` — with the system attributes `names` selects on `member`.
///
/// Each delivery is made with `VisibilityTimeout: 0`, so a row may read the same queue
/// again under a different selection.
async fn collect(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
    member: &str,
    names: &[&str],
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut collected: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let deadline = Instant::now() + DEADLINE;
    while collected.len() < wanted {
        assert!(
            Instant::now() < deadline,
            "case {case}: expected {wanted} messages from {url} within {DEADLINE:?}, got \
             {collected:?}"
        );
        let mut request = json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "WaitTimeSeconds": 1,
            "VisibilityTimeout": 0,
        });
        if !names.is_empty() {
            request[member] = json!(names);
        }
        let reply = ok(router, "ReceiveMessage", request).await;
        for message in messages(&reply) {
            let body = message["Body"].as_str().unwrap_or_default().to_string();
            collected.insert(body, attributes(&message));
        }
    }
    collected
}

/// A message a redrive moved to a dead-letter queue names the queue it was moved out of,
/// whichever of the queues sharing that dead-letter queue drove it there.
#[tokio::test]
async fn a_dead_lettered_message_names_the_queue_it_was_redriven_from() {
    // (case, FIFO queues, how many source queues share the dead-letter queue, the
    // deliveries each source queue's RedrivePolicy allows before the move)
    let rows: &[(&str, bool, usize, i64)] = &[
        ("one standard source, one delivery allowed", false, 1, 1),
        ("two standard sources sharing a dead-letter queue", false, 2, 1),
        ("three standard sources, two deliveries allowed", false, 3, 2),
        ("two FIFO sources sharing a FIFO dead-letter queue", true, 2, 1),
    ];

    for (index, (case, fifo, sources, allowed)) in rows.iter().copied().enumerate() {
        let router = emulator();
        let suffix = if fifo { ".fifo" } else { "" };
        let base = if fifo { fifo_attributes(&[]) } else { json!({}) };

        let dead_letter_name = format!("attributed-{index}-dlq{suffix}");
        let dead_letter_url = create_ok(&router, &dead_letter_name, &base).await;
        let dead_letter_arn = expected_arn(&dead_letter_name);

        // Every source drives its messages to the one dead-letter queue, and a visibility
        // timeout of zero hands each message straight back to the next receive, so the
        // allowance is spent without waiting a timeout out.
        let mut source_names: Vec<String> = Vec::with_capacity(sources);
        let mut source_urls: Vec<String> = Vec::with_capacity(sources);
        for source in 0..sources {
            let name = format!("attributed-{index}-source-{source}{suffix}");
            let mut source_attributes = base.clone();
            source_attributes["VisibilityTimeout"] = json!("0");
            source_attributes["RedrivePolicy"] =
                json!(redrive_policy(&dead_letter_arn, allowed));
            let url = create_ok(&router, &name, &source_attributes).await;
            let group = fifo.then(|| name.clone());
            send(
                &router,
                &url,
                &format!("{case} from {name}"),
                group.as_deref(),
            )
            .await;
            source_names.push(name);
            source_urls.push(url);
        }

        let wanted = i64::try_from(sources).expect("a small count");
        drive_to_the_dead_letter_queue(&router, case, &source_urls, &dead_letter_url, wanted)
            .await;

        // `All` "returns all values", so the attribute is among them for every message the
        // redrive moved — each naming its OWN source queue, which is the whole point of a
        // dead-letter queue several queues fan into.
        let collected = collect(
            &router,
            case,
            &dead_letter_url,
            sources,
            "MessageSystemAttributeNames",
            &["All"],
        )
        .await;
        for name in &source_names {
            let body = format!("{case} from {name}");
            let reported = collected.get(&body).unwrap_or_else(|| {
                panic!(
                    "case {case}: {body:?} never reached the dead-letter queue, got \
                     {collected:?}"
                )
            });
            let expected = expected_arn(name);
            let actual = reported.get(SOURCE_ARN).map(String::as_str);
            assert_eq!(
                actual,
                Some(expected.as_str()),
                "case {case}: expected {body:?} to name {expected} as the queue it was \
                 redriven from, got {actual:?} in {reported:?}"
            );
        }
    }
}

/// The attribute is reported only where the request selects it, and only for a message a
/// redrive moved: one sent straight to the same queue names no source at all.
#[tokio::test]
async fn only_a_redriven_message_names_a_source_and_only_where_the_request_selects_it() {
    let router = emulator();
    let dead_letter_name = "selection-dlq";
    let source_name = "selection-source";
    let dead_letter_url = create_ok(&router, dead_letter_name, &json!({})).await;
    let source_url = create_ok(
        &router,
        source_name,
        &json!({
            "VisibilityTimeout": "0",
            "RedrivePolicy": redrive_policy(&expected_arn(dead_letter_name), 1),
        }),
    )
    .await;

    let redriven = "redriven into the dead-letter queue".to_string();
    let sent_directly = "sent straight to the dead-letter queue".to_string();
    send(&router, &source_url, &redriven, None).await;
    send(&router, &dead_letter_url, &sent_directly, None).await;
    drive_to_the_dead_letter_queue(
        &router,
        "the selection table",
        std::slice::from_ref(&source_url),
        &dead_letter_url,
        2,
    )
    .await;
    let expected = expected_arn(source_name);

    // (case, the member the names are supplied on, the names it carries, whether the
    // redriven message must report the attribute). Both members carry the same value set —
    // `AttributeNames` is the discontinued alias — so each selection is driven on both.
    let rows: &[(&str, &[&str], bool)] = &[
        ("All selects it", &["All"], true),
        ("the name itself selects it", &[SOURCE_ARN], true),
        (
            "the name beside another selects it",
            &["SentTimestamp", SOURCE_ARN],
            true,
        ),
        ("another name alone selects it not", &["SentTimestamp"], false),
        (
            "two other names select it not",
            &["SenderId", "ApproximateReceiveCount"],
            false,
        ),
        ("no selection at all reports nothing", &[], false),
    ];

    for (case, names, reported) in rows.iter().copied() {
        for member in ["MessageSystemAttributeNames", "AttributeNames"] {
            let case = format!("{member}: {case}");
            let collected = collect(&router, &case, &dead_letter_url, 2, member, names).await;

            let moved = collected
                .get(&redriven)
                .unwrap_or_else(|| panic!("case {case}: {redriven:?} was not handed over"));
            let actual = moved.get(SOURCE_ARN).map(String::as_str);
            let wanted = reported.then_some(expected.as_str());
            assert_eq!(
                actual, wanted,
                "case {case}: expected the redriven message to report {wanted:?} under \
                 {SOURCE_ARN}, got {actual:?} in {moved:?}"
            );

            // Nothing moved this one: it was sent to this queue, so it has no source queue
            // to name whatever the request asks for.
            let direct = collected
                .get(&sent_directly)
                .unwrap_or_else(|| panic!("case {case}: {sent_directly:?} was not handed over"));
            assert_eq!(
                direct.get(SOURCE_ARN),
                None,
                "case {case}: a message no redrive moved must name no source, got {direct:?}"
            );
        }
    }
}

/// A message movement task driving a dead-lettered message out of the dead-letter queue
/// moves what the message carries with it: on the destination it still names the queue it
/// was dead-lettered FROM.
#[tokio::test]
async fn a_message_driven_out_of_a_dead_letter_queue_still_names_where_it_was_dead_lettered_from()
{
    let router = emulator();
    let dead_letter_name = "travelling-dlq";
    let source_name = "travelling-source";
    let destination_name = "travelling-destination";
    let dead_letter_url = create_ok(&router, dead_letter_name, &json!({})).await;
    let dead_letter_arn = expected_arn(dead_letter_name);
    let source_url = create_ok(
        &router,
        source_name,
        &json!({
            "VisibilityTimeout": "0",
            "RedrivePolicy": redrive_policy(&dead_letter_arn, 1),
        }),
    )
    .await;
    // A destination of its own, so what the attribute names is distinguishable from the
    // queue the task delivered the message to.
    let destination_url = create_ok(&router, destination_name, &json!({})).await;

    let body = "poison, redriven and driven back".to_string();
    send(&router, &source_url, &body, None).await;
    drive_to_the_dead_letter_queue(
        &router,
        "the travelling message",
        std::slice::from_ref(&source_url),
        &dead_letter_url,
        1,
    )
    .await;

    ok(
        &router,
        "StartMessageMoveTask",
        json!({"SourceArn": dead_letter_arn, "DestinationArn": expected_arn(destination_name)}),
    )
    .await;

    let collected = collect(
        &router,
        "the travelling message",
        &destination_url,
        1,
        "MessageSystemAttributeNames",
        &["All"],
    )
    .await;
    let reported = collected.get(&body).unwrap_or_else(|| {
        panic!(
            "case the travelling message: {body:?} never reached the destination, got \
             {collected:?}"
        )
    });
    let expected = expected_arn(source_name);
    let actual = reported.get(SOURCE_ARN).map(String::as_str);
    assert_eq!(
        actual,
        Some(expected.as_str()),
        "case the travelling message: expected it to keep naming {expected} as the queue it \
         was dead-lettered from, got {actual:?} in {reported:?}"
    );
}
