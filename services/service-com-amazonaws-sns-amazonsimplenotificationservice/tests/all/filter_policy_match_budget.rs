// SPDX-License-Identifier: CC0-1.0

//! The matching ONE publish may spend on filter policies is bounded by what each condition
//! READS, not only by how many conditions it visits — end to end over the wire, with SQS
//! mounted on the same engine so what was delivered is observed where a client observes it:
//! on the queue.
//!
//! The RULE asserted here: the cost of applying a filter policy is a PRODUCT of factors the
//! caller chooses — the values a published property holds (a `String.Array` attribute, a JSON
//! array in the body), the options or relations one condition's operator argument lists, and
//! the bytes a string condition compares — so a single request's matching is capped across
//! all of them: how many comparisons it makes, and how many bytes those comparisons read. A
//! subscription whose filter the request could not finish applying is WITHHELD from, never
//! delivered to: a filter that was not applied must not be read as a filter that matched, and
//! that holds for a NEGATED condition (`anything-but`) too, where an unfinished match would
//! otherwise invert into an admission. A message that is merely LARGE is not withheld: the
//! byte allowance is far above what one message judged by one policy reads. An unfiltered
//! subscription of the same topic rides along, so a withheld message is one the publish
//! itself did deliver.
//!
//! The rows differ along which factor is multiplied out (the options an `anything-but` lists,
//! the conditions a property lists, the bytes a `wildcard` pattern and value compare), along
//! how many values the property holds and how long each one is, and along which side of each
//! cap the product lands on.

use crate::sqs_subscription_delivery::{drain, element, emulator, given_queue, sns, Emulator};

/// The property the policies below match on, and the attribute every publish carries.
const PROPERTY: &str = "region";

/// The value every policy below admits, and the only member of a published array that is
/// not one of its filler members.
const WANTED: &str = "us-west-2";

/// Subscribe the queue `queue_arn` to `topic` with raw delivery, and answer the
/// subscription's ARN.
async fn subscribe(emulator: &Emulator, topic: &str, queue_arn: &str) -> String {
    element(
        &sns(
            emulator,
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", "sqs"),
                ("Endpoint", queue_arn),
                ("ReturnSubscriptionArn", "true"),
                ("Attributes.entry.1.key", "RawMessageDelivery"),
                ("Attributes.entry.1.value", "true"),
            ],
        )
        .await,
        "SubscriptionArn",
    )
}

/// The `index`th filler member of a published array, `width` characters wide.
fn filler(index: usize, width: usize) -> String {
    let member = format!("other-{index}");
    format!("{member}{}", "x".repeat(width.saturating_sub(member.len())))
}

/// A `String.Array` attribute of `count` filler members plus [`WANTED`], which sits LAST
/// when `wanted_last` so that every filler member is judged before the admitting one.
fn array_attribute(count: usize, width: usize, wanted_last: bool) -> String {
    let mut members: Vec<String> = (0..count).map(|index| filler(index, width)).collect();
    if wanted_last {
        members.push(WANTED.to_string());
    } else {
        members.insert(0, WANTED.to_string());
    }
    let quoted: Vec<String> = members.iter().map(|member| format!("\"{member}\"")).collect();
    format!("[{}]", quoted.join(","))
}

/// An `anything-but` condition naming the first `count` filler members of width `width`, so
/// every one of those members is excluded and only [`WANTED`] is admitted.
fn excluding_filler(count: usize, width: usize) -> String {
    let options: Vec<String> = (0..count)
        .map(|index| format!("\"{}\"", filler(index, width)))
        .collect();
    format!(
        r#"{{"{PROPERTY}":[{{"anything-but":[{}]}}]}}"#,
        options.join(",")
    )
}

/// A policy listing `count` `wildcard` conditions no value matches, followed by one that
/// matches [`WANTED`] — so every condition before the last is judged against every value
/// the property holds before the admitting pair is reached.
fn wildcards_before_the_match(count: usize) -> String {
    let mut conditions: Vec<String> = (0..count)
        .map(|index| format!(r#"{{"wildcard":"*-none-{index}-*"}}"#))
        .collect();
    conditions.push(r#"{"wildcard":"*-west-*"}"#.to_string());
    format!(r#"{{"{PROPERTY}":[{}]}}"#, conditions.join(","))
}

/// One row: the case, the condition the policy holds, the attribute value published, and
/// whether the filtered subscriber must receive the message.
type BudgetCase = (&'static str, String, String, bool);

#[tokio::test]
async fn a_condition_the_request_cannot_finish_applying_withholds_the_message() {
    let rows: &[BudgetCase] = &[
        (
            // The admitting value comes first, so the walk stops at once however long the
            // excluded list is: the cost is what is READ, not what is written.
            "an anything-but satisfied by the first value of many",
            excluding_filler(400, 12),
            array_attribute(200, 12, false),
            true,
        ),
        (
            "an anything-but whose options times the values it judges stays under the cap",
            excluding_filler(20, 12),
            array_attribute(20, 12, true),
            true,
        ),
        (
            "an anything-but whose options times the values it judges passes the cap",
            excluding_filler(200, 12),
            array_attribute(200, 12, true),
            false,
        ),
        (
            "a wildcard matched by the first value of many",
            format!(r#"{{"{PROPERTY}":[{{"wildcard":"*-west-*"}}]}}"#),
            array_attribute(200, 12, false),
            true,
        ),
        (
            // Large, but nothing near the byte allowance: a message SNS accepts, judged
            // by one policy, is delivered rather than withheld for its size.
            "a wildcard against many long values, which the byte allowance covers",
            format!(r#"{{"{PROPERTY}":[{{"wildcard":"*-west-*"}}]}}"#),
            array_attribute(200, 400, true),
            true,
        ),
        (
            // Every condition before the last reads all nine long values, so the bytes
            // compared pass the allowance before the admitting condition is reached.
            "conditions times the long values they read passes the byte allowance",
            wildcards_before_the_match(45),
            array_attribute(9, 25_000, true),
            false,
        ),
        (
            "the same shape within the byte allowance",
            wildcards_before_the_match(4),
            array_attribute(9, 25_000, true),
            true,
        ),
    ];

    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "match-budget")]).await,
        "TopicArn",
    );
    let (filtered_url, filtered_arn) = given_queue(&emulator, "match-budget-queue").await;
    let (open_url, open_arn) = given_queue(&emulator, "match-budget-control-queue").await;
    let subscription = subscribe(&emulator, &topic, &filtered_arn).await;
    // No policy at all, so this one receives every publish whatever the matching cost: a
    // filtered queue that stayed empty is the matching's doing, not a publish that
    // delivered nothing.
    subscribe(&emulator, &topic, &open_arn).await;

    for (case, policy, attribute, delivered) in rows {
        sns(
            &emulator,
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &subscription),
                ("AttributeName", "FilterPolicy"),
                ("AttributeValue", policy),
            ],
        )
        .await;
        sns(
            &emulator,
            "Publish",
            &[
                ("TopicArn", &topic),
                ("Message", "a filtered message"),
                ("MessageAttributes.entry.1.Name", PROPERTY),
                ("MessageAttributes.entry.1.Value.DataType", "String.Array"),
                ("MessageAttributes.entry.1.Value.StringValue", attribute),
            ],
        )
        .await;

        let received = drain(&emulator, &filtered_url).await;
        let expected = usize::from(*delivered);
        assert_eq!(
            received.len(),
            expected,
            "case {case:?}: expected {expected} delivered message(s), got {}",
            received.len()
        );
        let open = drain(&emulator, &open_url).await;
        assert_eq!(
            open.len(),
            1,
            "case {case:?}: the unfiltered subscription must receive the publish whatever the \
             matching cost, got {} message(s)",
            open.len()
        );
    }
}
