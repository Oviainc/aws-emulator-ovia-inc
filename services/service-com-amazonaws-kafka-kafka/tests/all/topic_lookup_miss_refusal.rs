// SPDX-License-Identifier: CC0-1.0

//! How a topic operation of a cluster this endpoint DOES hold reports a topic
//! the cluster does not hold, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from.
//!
//! The suite asserts the rule over a table rather than one recorded request: the
//! rows differ along the way the topic came to be absent (never created, created
//! and then removed once the clock owed it, a create the service refused) and
//! along the form of the name (plain, dotted, underscored, one character, the
//! longest Apache Kafka allows, all dots), with the two topic readers
//! (`DeleteTopic` and `DescribeTopic`) checked against each other on every row.
//!
//! The router, the `Reply` wrapper, the percent-encoder and the `CreateCluster`
//! request builder come from [`crate::kafka_harness`], the crate's shared
//! harness.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    MAX_TOPIC_NAME_LENGTH, RESOURCE_NOT_FOUND_MESSAGE, TOPIC_DELETION_DELAY,
};

use crate::kafka_harness::{pct, ClusterRequest, Emulator, Reply, REGION};

/// The sibling topic every cluster below holds, so a refusal is checked to leave
/// the topics the cluster really has alone.
const SIBLING: &str = "kept";

/// Stand a provisioned cluster of `brokers` nodes up and hand back its ARN.
async fn cluster(emulator: &Emulator, name: &str, brokers: i64) -> String {
    let mut request = ClusterRequest::named(name);
    request.brokers = brokers;
    emulator.create_ok(&request).await
}

async fn create_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    name: &str,
    partitions: i64,
    replication: i64,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            Some(json!({
                "topicName": name,
                "partitionCount": partitions,
                "replicationFactor": replication,
            })),
        )
        .await
}

async fn delete_topic(emulator: &Emulator, cluster_arn: &str, name: &str) -> Reply {
    emulator
        .call_in(
            REGION,
            "DELETE",
            &format!("/v1/clusters/{}/topics/{}", pct(cluster_arn), pct(name)),
            None,
        )
        .await
}

async fn describe_topic(emulator: &Emulator, cluster_arn: &str, name: &str) -> Reply {
    emulator
        .call_in(
            REGION,
            "GET",
            &format!("/v1/clusters/{}/topics/{}", pct(cluster_arn), pct(name)),
            None,
        )
        .await
}

/// Sleep past the span after which a `DELETING` topic is gone from every reader.
fn sleep_past_topic_deletion() {
    std::thread::sleep(TOPIC_DELETION_DELAY + Duration::from_millis(250));
}

/// Why the cluster holds no topic of the row's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Absence {
    /// Nothing ever created it.
    NeverCreated,
    /// It was created, deleted, and the clock owed its removal before the row's
    /// calls are made.
    CreatedThenRemoved,
    /// Its create was refused, so the name was never registered.
    CreateRefused,
}

/// The name forms the rule is checked over: plain, dotted, underscored and
/// digit-bearing, one character, the longest Apache Kafka allows
/// (`MAX_TOPIC_NAME_LENGTH`), and the all-dots name that is legal as long as it
/// is not "." or "..".
fn absent_names() -> Vec<String> {
    vec![
        "ghost".to_string(),
        "ghost.v2".to_string(),
        "ghost_v2-2025".to_string(),
        "g".to_string(),
        "x".repeat(MAX_TOPIC_NAME_LENGTH),
        "...".to_string(),
    ]
}

/// Assert `reply` is the service's resource-miss refusal: 404
/// `NotFoundException` carrying the fixed not-found sentence and nothing of the
/// request.
fn assert_resource_miss(reply: &Reply, case: &str, cluster_arn: &str, name: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (404, "NotFoundException"),
        "case {case}: expected a 404 NotFoundException for a topic the cluster does not hold, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        RESOURCE_NOT_FOUND_MESSAGE,
        "case {case}: expected the service's fixed not-found sentence, got {:?}",
        reply.error_message()
    );
    assert!(
        !reply.error_message().contains(name) && !reply.error_message().contains(cluster_arn),
        "case {case}: the message must carry nothing of the request, got {:?}",
        reply.error_message()
    );
}

/// RULE: a topic operation whose cluster this endpoint resolves but which names a
/// topic that cluster does not hold is refused as the missing resource it is —
/// HTTP 404 `NotFoundException` with the service's fixed not-found sentence —
/// however the name came to be absent and whatever its form, and the refusal
/// changes nothing the cluster holds. The model's second 404,
/// `UnknownTopicOrPartitionException`, stands for the Apache Kafka
/// broker-protocol condition and is not what a control-plane lookup miss
/// answers, so `DeleteTopic` and `DescribeTopic` report the same code for the
/// same absence.
#[tokio::test]
async fn a_topic_the_cluster_does_not_hold_is_a_not_found_exception() {
    let names = absent_names();
    for absence in [
        Absence::NeverCreated,
        Absence::CreatedThenRemoved,
        Absence::CreateRefused,
    ] {
        let emulator = Emulator::new();
        // Two brokers, so a replication factor of three is one past what this
        // cluster can place — the refused create of the row below.
        let cluster_arn = cluster(&emulator, "topic-miss", 2).await;
        let sibling = create_topic(&emulator, &cluster_arn, SIBLING, 3, 2).await;
        assert_eq!(
            sibling.status, 200,
            "case absence={absence:?}: expected the sibling topic to be created, got {} {}",
            sibling.status, sibling.body
        );

        for name in &names {
            let case = format!("absence={absence:?} name={name:?}");
            match absence {
                Absence::NeverCreated => {}
                Absence::CreatedThenRemoved => {
                    let created = create_topic(&emulator, &cluster_arn, name, 1, 1).await;
                    assert_eq!(
                        created.status, 200,
                        "case {case}: expected CreateTopic to return 200, got {} {}",
                        created.status, created.body
                    );
                    let deleted = delete_topic(&emulator, &cluster_arn, name).await;
                    assert_eq!(
                        (deleted.status, deleted.at("/status").as_str().unwrap_or_default()),
                        (200, "DELETING"),
                        "case {case}: expected the first DeleteTopic to answer 200 DELETING, got {} {}",
                        deleted.status,
                        deleted.body
                    );
                }
                Absence::CreateRefused => {
                    let refused = create_topic(&emulator, &cluster_arn, name, 1, 3).await;
                    assert_eq!(
                        (refused.status, refused.error_code()),
                        (400, "BadRequestException"),
                        "case {case}: expected a replication factor past the broker count to be refused, got {} {}",
                        refused.status,
                        refused.body
                    );
                }
            }
        }

        // One wait for the whole table: the removal each deleted topic was owed
        // falls due on the clock, so afterwards no row's name is held.
        if absence == Absence::CreatedThenRemoved {
            sleep_past_topic_deletion();
        }

        for name in &names {
            let case = format!("absence={absence:?} name={name:?}");
            let deleted = delete_topic(&emulator, &cluster_arn, name).await;
            assert_resource_miss(&deleted, &case, &cluster_arn, name);

            // The other topic reader of the same cluster agrees, which is what a
            // client dispatching on the error code relies on.
            let described = describe_topic(&emulator, &cluster_arn, name).await;
            assert_eq!(
                (described.status, described.error_code()),
                (deleted.status, deleted.error_code()),
                "case {case}: expected DescribeTopic to report the same miss as DeleteTopic, got {} {}",
                described.status,
                described.body
            );

            // Repeating the refused delete is the same refusal: it neither
            // registers the name nor remembers the attempt.
            let again = delete_topic(&emulator, &cluster_arn, name).await;
            assert_resource_miss(&again, &format!("{case} repeat"), &cluster_arn, name);
        }

        // Nothing the cluster really holds was touched by any of the refusals.
        let kept = describe_topic(&emulator, &cluster_arn, SIBLING).await;
        assert_eq!(
            (kept.status, kept.at("/topicName").clone()),
            (200, Value::String(SIBLING.to_string())),
            "case absence={absence:?}: expected the sibling topic to survive every refusal, got {} {}",
            kept.status,
            kept.body
        );
    }
}
