// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the REGION an SNS account-wide listing is scoped to, driven
//! over the wire: every call below is an awsQuery HTTP request signed for one region
//! and dispatched through the router the emulator serves in production, so nothing but
//! persisted state carries information between calls.
//!
//! The rule under test: SNS is a regional service — a resource lives in the region its
//! own ARN carries, and a subscription lives in the region of the topic it extends —
//! so an account-wide listing answered by a regional endpoint reports that region's
//! records and no others. `ListTopics`, `ListSubscriptions` and
//! `ListPlatformApplications` take no scoping input but their pagination token
//! (`smithy_query(query="operation", operation="ListTopics")` and the same for the
//! other two: input is `NextToken` alone), so the endpoint a request reached IS the
//! scope of its listing.
//!
//! The rows differ along the REGION answering, the number of records that region holds
//! (none, one, several), the record KIND (topic, subscription, platform application),
//! whether the same resource NAME is held in two regions at once, and whether a
//! subscription's endpoint names a resource of ANOTHER region — which is what the rule
//! generalises over. Platform applications are written straight into persisted state
//! because `CreatePlatformApplication` is still a scaffold stub with an issue of its
//! own, through the same writer the platform-application suite uses.

use std::collections::HashSet;

use super::platform_applications::seed_in_region;
use super::topics_subscriptions_publish::{Answer, Sns};

/// One region of the table: the topics it holds, the region of the SQS queue each of
/// its subscriptions points at, and the mobile-push applications it holds.
struct RegionCase {
    region: &'static str,
    topics: &'static [&'static str],
    /// The region of the queue ARN each of this region's subscriptions names. A
    /// subscription's endpoint is in no way bound to its topic's region, so an
    /// endpoint of another region must not move the subscription out of the region of
    /// the topic it extends.
    endpoint_region: &'static str,
    applications: &'static [&'static str],
}

/// The regions the table spans, with the records each of them holds.
///
/// `shared-topic` is created in two of them, so a listing that reported another
/// region's records would report two topics of that one name; `ap-southeast-2` holds
/// nothing, so its listings must be empty however much the other regions hold.
const CASES: &[RegionCase] = &[
    RegionCase {
        region: "us-east-1",
        topics: &["shared-topic", "east-only-topic"],
        endpoint_region: "us-east-1",
        applications: &["push-east"],
    },
    RegionCase {
        region: "us-west-2",
        topics: &["shared-topic"],
        endpoint_region: "us-east-1",
        applications: &[],
    },
    RegionCase {
        region: "eu-west-1",
        topics: &["eu-one-topic", "eu-two-topic", "eu-three-topic"],
        endpoint_region: "ap-southeast-2",
        applications: &["push-eu-one", "push-eu-two"],
    },
    RegionCase {
        region: "ap-southeast-2",
        topics: &[],
        endpoint_region: "ap-southeast-2",
        applications: &[],
    },
];

/// What one region must hold, collected while the records are created.
#[derive(Default)]
struct Expected {
    topics: Vec<String>,
    subscriptions: Vec<String>,
    applications: Vec<String>,
}

/// The ARNs an answer carries under `tag`, as a set.
fn reported(answer: &Answer, tag: &str) -> HashSet<String> {
    answer.elements(tag).into_iter().collect()
}

/// `expected` as a set, for comparison with what a listing reported.
fn wanted(expected: &[String]) -> HashSet<String> {
    expected.iter().cloned().collect()
}

/// An account-wide SNS listing reports the records of the region that answered it, and
/// none of another region's — whatever the other regions hold.
#[tokio::test]
async fn an_account_wide_listing_reports_only_the_answering_regions_records() {
    let sns = Sns::new();
    let mut expected: Vec<Expected> = Vec::new();
    let mut seeded_applications = 0u64;

    for case in CASES {
        let mut held = Expected::default();
        for name in case.topics {
            let created = sns
                .call_in_region(case.region, "CreateTopic", &[("Name", name)])
                .await;
            assert_eq!(
                created.status, 200,
                "case {:?} topic {name:?}: expected 200, got {} / {}",
                case.region, created.status, created.body
            );
            let topic = created.element("TopicArn").unwrap_or_else(|| {
                panic!(
                    "case {:?} topic {name:?}: no TopicArn in {}",
                    case.region, created.body
                )
            });
            assert!(
                topic.contains(&format!(":{}:", case.region)),
                "case {:?} topic {name:?}: the ARN must carry the request's region, got {topic:?}",
                case.region
            );

            let endpoint = format!(
                "arn:aws:sqs:{}:000000000000:{name}-queue",
                case.endpoint_region
            );
            let subscribed = sns
                .call_in_region(
                    case.region,
                    "Subscribe",
                    &[
                        ("TopicArn", &topic),
                        ("Protocol", "sqs"),
                        ("Endpoint", &endpoint),
                        ("ReturnSubscriptionArn", "true"),
                    ],
                )
                .await;
            assert_eq!(
                subscribed.status, 200,
                "case {:?} subscription to {endpoint:?}: expected 200, got {} / {}",
                case.region, subscribed.status, subscribed.body
            );
            let subscription = subscribed.element("SubscriptionArn").unwrap_or_else(|| {
                panic!(
                    "case {:?} subscription to {endpoint:?}: no SubscriptionArn in {}",
                    case.region, subscribed.body
                )
            });
            assert!(
                subscription.starts_with(&format!("{topic}:")),
                "case {:?} subscription to {endpoint:?}: expected the topic's ARN plus an id, \
                 got {subscription:?}",
                case.region
            );
            held.topics.push(topic);
            held.subscriptions.push(subscription);
        }
        for name in case.applications {
            held.applications.push(seed_in_region(
                &sns,
                case.region,
                seeded_applications,
                "GCM",
                name,
                &[],
            ));
            seeded_applications += 1;
        }
        expected.push(held);
    }

    for (case, held) in CASES.iter().zip(&expected) {
        let listings: &[(&str, &str, &[String])] = &[
            ("ListTopics", "TopicArn", &held.topics),
            ("ListSubscriptions", "SubscriptionArn", &held.subscriptions),
            (
                "ListPlatformApplications",
                "PlatformApplicationArn",
                &held.applications,
            ),
        ];
        for (action, tag, want) in listings {
            let answer = sns.call_in_region(case.region, action, &[]).await;
            assert_eq!(
                answer.status, 200,
                "case {:?} {action}: expected 200, got {} / {}",
                case.region, answer.status, answer.body
            );
            assert_eq!(
                reported(&answer, tag),
                wanted(want),
                "case {:?} {action}: expected exactly that region's records {want:?}, got {}",
                case.region,
                answer.body
            );
            assert_eq!(
                answer.element("NextToken"),
                None,
                "case {:?} {action}: one page holds every record of the region, got {}",
                case.region,
                answer.body
            );
        }

        // A subscription is reported by the region of the TOPIC it extends, whatever
        // region its endpoint names, so every listed topic ARN carries the answering
        // region too.
        let answer = sns
            .call_in_region(case.region, "ListSubscriptions", &[])
            .await;
        for topic in answer.elements("TopicArn") {
            assert!(
                topic.contains(&format!(":{}:", case.region)),
                "case {:?}: a listed subscription must belong to a topic of that region, \
                 got {topic:?}",
                case.region
            );
        }
    }
}

/// The topic's OWN subscription listing is scoped by the topic it names, so it reports
/// the subscription the account-wide listing of the topic's region reports — and the
/// endpoint's region changes neither.
#[tokio::test]
async fn a_topics_own_subscription_listing_follows_the_topics_region() {
    let sns = Sns::new();
    let home = "us-west-2";
    let away = "us-east-1";
    let created = sns
        .call_in_region(home, "CreateTopic", &[("Name", "cross-region-topic")])
        .await;
    let topic = created
        .element("TopicArn")
        .unwrap_or_else(|| panic!("no TopicArn in {}", created.body));
    let endpoint = format!("arn:aws:sqs:{away}:000000000000:cross-region-queue");
    let subscribed = sns
        .call_in_region(
            home,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &endpoint),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    let subscription = subscribed
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("no SubscriptionArn in {}", subscribed.body));

    let by_topic = sns
        .call_in_region(home, "ListSubscriptionsByTopic", &[("TopicArn", &topic)])
        .await;
    assert_eq!(
        reported(&by_topic, "SubscriptionArn"),
        HashSet::from([subscription.clone()]),
        "case \"the topic's own listing\": expected exactly {subscription:?}, got {}",
        by_topic.body
    );

    let at_home = sns.call_in_region(home, "ListSubscriptions", &[]).await;
    assert_eq!(
        reported(&at_home, "SubscriptionArn"),
        HashSet::from([subscription.clone()]),
        "case \"the topic's region\": expected exactly {subscription:?}, got {}",
        at_home.body
    );

    let at_away = sns.call_in_region(away, "ListSubscriptions", &[]).await;
    assert!(
        !reported(&at_away, "SubscriptionArn").contains(&subscription),
        "case \"the endpoint's region\": {subscription:?} belongs to {home}, got {}",
        at_away.body
    );
}
