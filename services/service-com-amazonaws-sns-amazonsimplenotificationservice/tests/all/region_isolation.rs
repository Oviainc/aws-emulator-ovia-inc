// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the REGION an ARN-addressed SNS call is answered in, driven over
//! the wire: every call below is an awsQuery HTTP request addressed to one regional
//! endpoint and dispatched through the router the emulator serves in production.
//!
//! The rule under test: Amazon SNS is a regional service — a resource lives in the region
//! its own ARN carries — so an endpoint of one region neither reads nor acts on an ARN
//! naming another region: such a value is refused as a parameter fault (HTTP 400,
//! `InvalidParameter`, `Invalid parameter: <Member>`) BEFORE any record is looked up,
//! while the same call carrying an ARN of the endpoint's own region is served. That is one
//! rule over every operation that resolves an ARN of this service, whichever member
//! carries it and whether the call reads, writes, publishes or deletes.
//!
//! The rows differ along what the rule generalises over: the operation and its class (a
//! read, an attribute write, a listing, a publish, a batch publish, a policy write, a tag
//! read and write, a subscribe, a delete), the member the ARN arrives in (`TopicArn`,
//! `ResourceArn`, `SubscriptionArn`), the ARN KIND (a topic's, a subscription's), and the
//! region pairing — each endpoint answering its own region's ARN, another region's, and a
//! region of another partition. The mobile-push plane's two ARN members
//! (`PlatformApplicationArn`, `EndpointArn`) are covered by `platform_application_reads`
//! and `platform_endpoint_deletion`, whose stores are seeded per region.

use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The regions the table spans: three AWS regions of this partition, so every endpoint has
/// two foreign ones to be offered.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2"];

/// A region of ANOTHER partition, which no endpoint of the partition above serves either:
/// the rule is about the region field of the ARN, so a value naming a region that does not
/// exist in this partition at all is refused the same way.
const OTHER_PARTITION_REGION: &str = "us-gov-west-1";

/// The name every region's topic is created under: the same name in two regions names two
/// topics, so an ARN that differs ONLY in its region field still names another region's
/// resource.
const TOPIC_NAME: &str = "region-isolation";

/// The queue every region's subscription points at.
///
/// Deliberately ONE queue, in one region, for the subscriptions of all three: a subscriber
/// is reached in the region its OWN ARN names, which need not be the topic's, so the rule
/// under test is about the ARN of the SNS resource a call addresses and not about the
/// endpoint it delivers to. It is also what makes the table's `Subscribe` row idempotent —
/// one topic holds at most one subscription per (protocol, endpoint) pair — so the served
/// leg can be driven for every region without adding a subscription each time.
const QUEUE_ARN: &str = "arn:aws:sqs:us-east-1:000000000000:isolation-queue";

/// One operation of the table: how it is called with the ARN under test.
struct Operation {
    /// What this row exercises, named in every assertion.
    case: &'static str,
    /// The awsQuery action.
    action: &'static str,
    /// The input member the ARN under test arrives in.
    member: &'static str,
    /// Whether the ARN is the SUBSCRIPTION's rather than the topic's.
    subscription_arn: bool,
    /// The other parameters the call carries, all of them valid.
    extra: &'static [(&'static str, &'static str)],
}

/// Every operation that resolves an ARN of this service, bar the two destructive ones
/// (`Unsubscribe`, `DeleteTopic`), which are driven after this table so their served leg
/// does not take the records the other rows need.
const OPERATIONS: &[Operation] = &[
    Operation {
        case: "a topic attribute read",
        action: "GetTopicAttributes",
        member: "TopicArn",
        subscription_arn: false,
        extra: &[],
    },
    Operation {
        case: "a topic attribute write",
        action: "SetTopicAttributes",
        member: "TopicArn",
        subscription_arn: false,
        extra: &[("AttributeName", "DisplayName"), ("AttributeValue", "isolated")],
    },
    Operation {
        case: "a topic's subscription listing",
        action: "ListSubscriptionsByTopic",
        member: "TopicArn",
        subscription_arn: false,
        extra: &[],
    },
    Operation {
        case: "a publish",
        action: "Publish",
        member: "TopicArn",
        subscription_arn: false,
        extra: &[("Message", "hello")],
    },
    Operation {
        case: "a batch publish",
        action: "PublishBatch",
        member: "TopicArn",
        subscription_arn: false,
        extra: &[
            ("PublishBatchRequestEntries.member.1.Id", "one"),
            ("PublishBatchRequestEntries.member.1.Message", "hello"),
        ],
    },
    Operation {
        case: "a subscribe",
        action: "Subscribe",
        member: "TopicArn",
        subscription_arn: false,
        extra: &[("Protocol", "sqs"), ("Endpoint", QUEUE_ARN)],
    },
    Operation {
        case: "an access-policy grant",
        action: "AddPermission",
        member: "TopicArn",
        subscription_arn: false,
        extra: &[
            ("Label", "isolation"),
            ("AWSAccountId.member.1", "000000000000"),
            ("ActionName.member.1", "Publish"),
        ],
    },
    Operation {
        case: "an access-policy revoke",
        action: "RemovePermission",
        member: "TopicArn",
        subscription_arn: false,
        extra: &[("Label", "isolation")],
    },
    Operation {
        case: "a data-protection-policy read",
        action: "GetDataProtectionPolicy",
        member: "ResourceArn",
        subscription_arn: false,
        extra: &[],
    },
    Operation {
        case: "a tag read",
        action: "ListTagsForResource",
        member: "ResourceArn",
        subscription_arn: false,
        extra: &[],
    },
    Operation {
        case: "a tag write",
        action: "TagResource",
        member: "ResourceArn",
        subscription_arn: false,
        extra: &[
            ("Tags.member.1.Key", "team"),
            ("Tags.member.1.Value", "platform"),
        ],
    },
    Operation {
        case: "a tag removal",
        action: "UntagResource",
        member: "ResourceArn",
        subscription_arn: false,
        extra: &[("TagKeys.member.1", "team")],
    },
    Operation {
        case: "a subscription attribute read",
        action: "GetSubscriptionAttributes",
        member: "SubscriptionArn",
        subscription_arn: true,
        extra: &[],
    },
    Operation {
        case: "a subscription attribute write",
        action: "SetSubscriptionAttributes",
        member: "SubscriptionArn",
        subscription_arn: true,
        extra: &[
            ("AttributeName", "RawMessageDelivery"),
            ("AttributeValue", "true"),
        ],
    },
];

/// The topic and subscription one region holds.
struct Held {
    region: &'static str,
    topic: String,
    subscription: String,
}

impl Held {
    /// The ARN this row of the table addresses in `Held`'s region.
    fn arn(&self, operation: &Operation) -> String {
        if operation.subscription_arn {
            self.subscription.clone()
        } else {
            self.topic.clone()
        }
    }
}

/// The same ARN with its REGION field replaced: the fourth field of
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, so the value differs from the
/// one its owner holds in that one field alone.
fn in_region(arn: &str, region: &str) -> String {
    let mut fields: Vec<&str> = arn.splitn(6, ':').collect();
    fields[3] = region;
    fields.join(":")
}

/// The same ARN moved to another PARTITION's region, which also replaces the partition
/// field so the value is one a client of that partition would hold.
fn in_other_partition(arn: &str) -> String {
    let moved = in_region(arn, OTHER_PARTITION_REGION);
    let mut fields: Vec<&str> = moved.splitn(6, ':').collect();
    fields[1] = "aws-us-gov";
    fields.join(":")
}

/// Create one region's topic and one confirmed SQS subscription of it.
async fn given_region(sns: &Sns, region: &'static str) -> Held {
    let created = sns
        .call_in_region(region, "CreateTopic", &[("Name", TOPIC_NAME)])
        .await;
    assert_eq!(
        created.status, 200,
        "region {region}: CreateTopic expected 200, got {} / {}",
        created.status, created.body
    );
    let topic = created
        .element("TopicArn")
        .unwrap_or_else(|| panic!("region {region}: no TopicArn in {}", created.body));
    let queue = QUEUE_ARN.to_string();
    let subscribed = sns
        .call_in_region(
            region,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &queue),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    assert_eq!(
        subscribed.status, 200,
        "region {region}: Subscribe expected 200, got {} / {}",
        subscribed.status, subscribed.body
    );
    let subscription = subscribed
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("region {region}: no SubscriptionArn in {}", subscribed.body));
    Held {
        region,
        topic,
        subscription,
    }
}

/// Assert that `answer` is the parameter refusal this endpoint states for an ARN of
/// another region: HTTP 400, `InvalidParameter`, and the member's own bare sentence with
/// neither the region nor the ARN echoed back.
fn assert_refused(case: &str, member: &str, arn: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "InvalidParameter",
        "case {case:?}: expected the InvalidParameter code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(format!("Invalid parameter: {member}").as_str()),
        "case {case:?}: expected the member's own refusal sentence, got {}",
        answer.body
    );
    assert!(
        !answer.body.contains(arn),
        "case {case:?}: the refusal must not echo the ARN, got {}",
        answer.body
    );
}

/// Every ARN-addressed operation is served for an ARN of the endpoint's OWN region and
/// refused for an ARN of any other, with the records of each region left exactly as they
/// were.
#[tokio::test]
async fn an_endpoint_serves_its_own_regions_arns_and_refuses_every_other_regions() {
    let sns = Sns::new();
    let mut held: Vec<Held> = Vec::new();
    for region in REGIONS {
        held.push(given_region(&sns, region).await);
    }

    for owner in &held {
        for operation in OPERATIONS {
            let own = owner.arn(operation);

            // The endpoint of the ARN's own region serves the call.
            let mut params: Vec<(&str, &str)> = vec![(operation.member, own.as_str())];
            params.extend_from_slice(operation.extra);
            let served = sns
                .call_in_region(owner.region, operation.action, &params)
                .await;
            assert_eq!(
                served.status, 200,
                "case {:?} in its own region {}: expected HTTP 200, got {} / {}",
                operation.case, owner.region, served.status, served.body
            );

            // Every OTHER regional endpoint refuses that same ARN, and so does every
            // endpoint offered an ARN of another partition's region.
            for addressed in REGIONS.iter().filter(|region| **region != owner.region) {
                let case = format!(
                    "{} of {} addressed to {addressed}",
                    operation.case, owner.region
                );
                let mut params: Vec<(&str, &str)> = vec![(operation.member, own.as_str())];
                params.extend_from_slice(operation.extra);
                let answer = sns
                    .call_in_region(addressed, operation.action, &params)
                    .await;
                assert_refused(&case, operation.member, &own, &answer);
            }
            let elsewhere = in_other_partition(&own);
            let case = format!(
                "{} named in another partition's region, addressed to {}",
                operation.case, owner.region
            );
            let mut params: Vec<(&str, &str)> = vec![(operation.member, elsewhere.as_str())];
            params.extend_from_slice(operation.extra);
            let answer = sns
                .call_in_region(owner.region, operation.action, &params)
                .await;
            assert_refused(&case, operation.member, &elsewhere, &answer);
        }
    }

    // The refused calls wrote nothing into another region's state: every region still
    // holds its own topic with exactly the one subscription it was given, and the display
    // name only the served write set.
    for owner in &held {
        let read = sns
            .call_in_region(owner.region, "GetTopicAttributes", &[("TopicArn", &owner.topic)])
            .await;
        assert_eq!(
            read.status, 200,
            "region {}: the topic must still read, got {} / {}",
            owner.region, read.status, read.body
        );
        let attributes = attribute_map(&read);
        assert_eq!(
            attributes.get("DisplayName").map(String::as_str),
            Some("isolated"),
            "region {}: only the served write may have set the display name, got {:?}",
            owner.region,
            attributes.get("DisplayName")
        );
        assert_eq!(
            attributes.get("SubscriptionsConfirmed").map(String::as_str),
            Some("1"),
            "region {}: the topic must still hold exactly its own subscription, got {:?}",
            owner.region,
            attributes.get("SubscriptionsConfirmed")
        );
        let listed = sns
            .call_in_region(
                owner.region,
                "ListSubscriptionsByTopic",
                &[("TopicArn", &owner.topic)],
            )
            .await;
        assert_eq!(
            listed.elements("SubscriptionArn"),
            vec![owner.subscription.clone()],
            "region {}: the listing must report that region's subscription alone, got {}",
            owner.region,
            listed.body
        );
    }
}

/// The two DESTRUCTIVE operations follow the same rule: an `Unsubscribe` or a `DeleteTopic`
/// naming another region's ARN is refused rather than answered as the idempotent delete it
/// is for an ARN of its own region — so a client's region mistake cannot silently remove
/// nothing while reporting success, and the resource it named stays where it is.
#[tokio::test]
async fn a_delete_naming_another_regions_arn_is_refused_and_removes_nothing() {
    let sns = Sns::new();
    let home = given_region(&sns, "us-east-1").await;
    let away = given_region(&sns, "eu-west-1").await;

    for (case, action, member, arn) in [
        (
            "an unsubscribe of another region's subscription",
            "Unsubscribe",
            "SubscriptionArn",
            away.subscription.clone(),
        ),
        (
            "a delete of another region's topic",
            "DeleteTopic",
            "TopicArn",
            away.topic.clone(),
        ),
    ] {
        let answer = sns
            .call_in_region(home.region, action, &[(member, &arn)])
            .await;
        assert_refused(case, member, &arn, &answer);
    }

    // Nothing was removed: the other region still holds both records, which its own
    // endpoint reads.
    let read = sns
        .call_in_region(away.region, "GetTopicAttributes", &[("TopicArn", &away.topic)])
        .await;
    assert_eq!(
        read.status, 200,
        "the refused delete must leave the topic in place, got {} / {}",
        read.status, read.body
    );
    let attributes = sns
        .call_in_region(
            away.region,
            "GetSubscriptionAttributes",
            &[("SubscriptionArn", &away.subscription)],
        )
        .await;
    assert_eq!(
        attributes.status, 200,
        "the refused unsubscribe must leave the subscription in place, got {} / {}",
        attributes.status, attributes.body
    );

    // And each endpoint still performs the delete for its OWN region's ARNs.
    for (action, member, arn) in [
        ("Unsubscribe", "SubscriptionArn", away.subscription.clone()),
        ("DeleteTopic", "TopicArn", away.topic.clone()),
    ] {
        let answer = sns
            .call_in_region(away.region, action, &[(member, &arn)])
            .await;
        assert_eq!(
            answer.status, 200,
            "{action} in its own region: expected HTTP 200, got {} / {}",
            answer.status, answer.body
        );
    }
    assert!(
        sns.state()
            .lock()
            .topics
            .keys()
            .all(|arn| arn == &home.topic),
        "only the other region's topic was deleted, got {:?}",
        sns.state().lock().topics.keys().collect::<Vec<_>>()
    );
}
