// SPDX-License-Identifier: CC0-1.0

//! A BATCHED publish is filtered ENTRY BY ENTRY: `PublishBatch` applies each
//! subscription's filter policy to every entry of the batch on its own, so a filtered
//! subscriber receives exactly the entries its policy admits — end to end over the wire,
//! with SQS mounted on the same engine so what was delivered is observed where a client
//! observes it: on the queue.
//!
//! The RULE asserted here is the one `Publish` holds, applied per entry: "The result of
//! publishing each message is reported individually in the response" (the `PublishBatch`
//! doc) and "FilterPolicy – The simple JSON object that lets your subscriber receive only
//! a subset of messages, rather than receiving every message published to the topic"
//! (`SubscribeInput$Attributes` doc) — so which subscriptions receive an entry is decided
//! from THAT entry's own attributes and body, not from the batch.
//!
//! The rows differ along what the rule generalises over: the entry's position in the
//! batch, whether it satisfies the policy, the condition kind (an exact value, `numeric`,
//! `exists`) and the scope the policy is matched under. An UNFILTERED subscription of the
//! same topic rides along, so an entry missing from the filtered queue is one the publish
//! itself did deliver.
//!
//! The policy language itself, and the policy-acceptance rules, are
//! `subscription_filter_policy_matching.rs`; this suite is about the BATCH path.

use super::sqs_subscription_delivery::{drain, element, emulator, given_queue, sns, Emulator};

/// One entry of the batch: its id, its body, the message attribute it carries (if any),
/// and whether the filtered subscription must receive it.
struct Entry {
    id: &'static str,
    body: &'static str,
    attribute: Option<(&'static str, &'static str, &'static str)>,
    admitted: bool,
}

/// Subscribe the queue `arn` to `topic` with raw delivery and the attributes in `extra`,
/// answering the subscription's ARN.
async fn subscribe_raw(
    emulator: &Emulator,
    topic: &str,
    arn: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut params: Vec<(String, String)> = vec![
        ("TopicArn".to_string(), topic.to_string()),
        ("Protocol".to_string(), "sqs".to_string()),
        ("Endpoint".to_string(), arn.to_string()),
        ("ReturnSubscriptionArn".to_string(), "true".to_string()),
        (
            "Attributes.entry.1.key".to_string(),
            "RawMessageDelivery".to_string(),
        ),
        ("Attributes.entry.1.value".to_string(), "true".to_string()),
    ];
    for (at, (name, value)) in extra.iter().enumerate() {
        let entry = at + 2;
        params.push((format!("Attributes.entry.{entry}.key"), (*name).to_string()));
        params.push((
            format!("Attributes.entry.{entry}.value"),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    element(
        &sns(emulator, "Subscribe", &borrowed).await,
        "SubscriptionArn",
    )
}

/// Publish `entries` as ONE batch to `topic`.
async fn publish_batch(emulator: &Emulator, topic: &str, entries: &[Entry]) {
    let mut params: Vec<(String, String)> = vec![("TopicArn".to_string(), topic.to_string())];
    for (at, entry) in entries.iter().enumerate() {
        let member = at + 1;
        let prefix = format!("PublishBatchRequestEntries.member.{member}");
        params.push((format!("{prefix}.Id"), entry.id.to_string()));
        params.push((format!("{prefix}.Message"), entry.body.to_string()));
        if let Some((name, data_type, value)) = entry.attribute {
            params.push((
                format!("{prefix}.MessageAttributes.entry.1.Name"),
                name.to_string(),
            ));
            params.push((
                format!("{prefix}.MessageAttributes.entry.1.Value.DataType"),
                data_type.to_string(),
            ));
            params.push((
                format!("{prefix}.MessageAttributes.entry.1.Value.StringValue"),
                value.to_string(),
            ));
        }
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns(emulator, "PublishBatch", &borrowed).await;
}

/// The bodies the filtered subscriber and the unfiltered one received, in delivery order.
async fn delivered(
    emulator: &Emulator,
    filtered_url: &str,
    open_url: &str,
) -> (Vec<String>, Vec<String>) {
    let bodies = |held: Vec<(String, serde_json::Value)>| {
        held.into_iter().map(|(body, _)| body).collect::<Vec<_>>()
    };
    let filtered = bodies(drain(emulator, filtered_url).await);
    let open = bodies(drain(emulator, open_url).await);
    (filtered, open)
}

/// An ATTRIBUTE-scoped policy is applied to each entry's own attributes.
#[tokio::test]
async fn a_batch_is_filtered_against_each_entrys_own_attributes() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "batch-filtered")]).await,
        "TopicArn",
    );
    let (filtered_url, filtered_arn) = given_queue(&emulator, "batch-filter-queue").await;
    let (open_url, open_arn) = given_queue(&emulator, "batch-filter-control-queue").await;
    let filtered = subscribe_raw(
        &emulator,
        &topic,
        &filtered_arn,
        &[("FilterPolicy", r#"{"tier":["gold","silver"]}"#)],
    )
    .await;
    assert!(
        filtered.contains(':'),
        "an SQS subscription is confirmed at once, got {filtered:?}"
    );
    subscribe_raw(&emulator, &topic, &open_arn, &[]).await;

    // The rows differ in position, in whether the attribute satisfies the policy, and in
    // whether the entry carries the property at all.
    let entries = [
        Entry {
            id: "first",
            body: "gold-one",
            attribute: Some(("tier", "String", "gold")),
            admitted: true,
        },
        Entry {
            id: "second",
            body: "bronze-one",
            attribute: Some(("tier", "String", "bronze")),
            admitted: false,
        },
        Entry {
            id: "third",
            body: "silver-one",
            attribute: Some(("tier", "String", "silver")),
            admitted: true,
        },
        Entry {
            id: "fourth",
            body: "no-attribute",
            attribute: None,
            admitted: false,
        },
        Entry {
            id: "fifth",
            body: "another-property",
            attribute: Some(("region", "String", "gold")),
            admitted: false,
        },
        Entry {
            id: "sixth",
            body: "gold-in-a-list",
            attribute: Some(("tier", "String.Array", r#"["bronze","gold"]"#)),
            admitted: true,
        },
    ];

    publish_batch(&emulator, &topic, &entries).await;
    let (filtered_bodies, open_bodies) = delivered(&emulator, &filtered_url, &open_url).await;

    let admitted: Vec<&str> = entries
        .iter()
        .filter(|entry| entry.admitted)
        .map(|entry| entry.body)
        .collect();
    let published: Vec<&str> = entries.iter().map(|entry| entry.body).collect();
    assert_eq!(
        filtered_bodies, admitted,
        "the filtered subscription must receive exactly the entries its policy admits, got \
         {filtered_bodies:?}"
    );
    assert_eq!(
        open_bodies, published,
        "the unfiltered subscription must receive every entry of the batch, got \
         {open_bodies:?}"
    );
}

/// A BODY-scoped policy is applied to each entry's own body, and a numeric condition and
/// an `exists` condition are judged per entry just the same.
#[tokio::test]
async fn a_batch_is_filtered_against_each_entrys_own_body() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "batch-body-filtered")]).await,
        "TopicArn",
    );
    let (filtered_url, filtered_arn) = given_queue(&emulator, "batch-body-filter-queue").await;
    let (open_url, open_arn) = given_queue(&emulator, "batch-body-control-queue").await;
    subscribe_raw(
        &emulator,
        &topic,
        &filtered_arn,
        &[
            ("FilterPolicyScope", "MessageBody"),
            (
                "FilterPolicy",
                r#"{"detail":{"price":[{"numeric":["<",100]}]}}"#,
            ),
        ],
    )
    .await;
    subscribe_raw(&emulator, &topic, &open_arn, &[]).await;

    let entries = [
        Entry {
            id: "cheap",
            body: r#"{"detail":{"price":42}}"#,
            attribute: None,
            admitted: true,
        },
        Entry {
            id: "dear",
            body: r#"{"detail":{"price":420}}"#,
            attribute: None,
            admitted: false,
        },
        Entry {
            id: "at-the-bound",
            body: r#"{"detail":{"price":100}}"#,
            attribute: None,
            admitted: false,
        },
        Entry {
            id: "no-property",
            body: r#"{"detail":{"other":1}}"#,
            attribute: None,
            admitted: false,
        },
        Entry {
            id: "not-a-document",
            body: "42",
            attribute: None,
            admitted: false,
        },
        Entry {
            id: "cheapest",
            body: r#"{"detail":{"price":1}}"#,
            attribute: None,
            admitted: true,
        },
    ];

    publish_batch(&emulator, &topic, &entries).await;
    let (filtered_bodies, open_bodies) = delivered(&emulator, &filtered_url, &open_url).await;

    let admitted: Vec<&str> = entries
        .iter()
        .filter(|entry| entry.admitted)
        .map(|entry| entry.body)
        .collect();
    let published: Vec<&str> = entries.iter().map(|entry| entry.body).collect();
    assert_eq!(
        filtered_bodies, admitted,
        "the body-scoped subscription must receive exactly the entries whose own body its \
         policy admits, got {filtered_bodies:?}"
    );
    assert_eq!(
        open_bodies, published,
        "the unfiltered subscription must receive every entry of the batch, got \
         {open_bodies:?}"
    );
}
