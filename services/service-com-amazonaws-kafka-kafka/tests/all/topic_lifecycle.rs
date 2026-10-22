// SPDX-License-Identifier: CC0-1.0

//! The topic lifecycle of a provisioned cluster (`CreateTopic`, `DescribeTopic`,
//! `DeleteTopic`), driven end to end over the same `MultiServiceRouter` the
//! emulator serves from.
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded
//! request: the rows differ along whatever the rule generalises over (topic name
//! form and length, partition count, replication factor, `Configs` presence,
//! broker count, which topic of a set is addressed, which way an ARN is
//! malformed, signing region). Where the rule is an invariant — a created topic
//! reading back exactly as created, a delete being visible to every later reader,
//! a refusal leaving the registry alone — the input space is walked exhaustively.
//!
//! The router, the `Reply` wrapper, the percent-encoder and the `CreateCluster`
//! request builder come from [`crate::kafka_harness`], the crate's shared
//! harness; only the topic calls and the topic table row live here.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    MAX_TOPIC_NAME_LENGTH, TOPIC_ACTIVATION_DELAY, TOPIC_DELETION_DELAY,
};

use crate::kafka_harness::{pct, ClusterRequest, Emulator, Reply, ACCOUNT, REGION};

/// The topic calls, written as methods on the shared harness's [`Emulator`] so
/// the suites below read the same way as this crate's cluster suites do. They go
/// through the harness's own `call` / `call_in`, so every request in this crate
/// is dispatched by one code path.
impl Emulator {
    /// Create a cluster of `brokers` nodes in the suite's region and hand back
    /// its ARN. The cluster is only the context a topic needs, so the row is the
    /// harness's minimal `CreateCluster` request with the broker count set.
    async fn cluster(&self, name: &str, brokers: i64) -> String {
        let mut request = ClusterRequest::named(name);
        request.brokers = brokers;
        self.create_ok(&request).await
    }

    async fn create_topic(&self, cluster_arn: &str, request: &TopicRequest) -> Reply {
        self.call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            Some(request.to_json()),
        )
        .await
    }

    async fn describe_topic(&self, cluster_arn: &str, topic: &str) -> Reply {
        self.describe_topic_in(REGION, cluster_arn, topic).await
    }

    async fn describe_topic_in(&self, region: &str, cluster_arn: &str, topic: &str) -> Reply {
        self.call_in(
            region,
            "GET",
            &format!("/v1/clusters/{}/topics/{}", pct(cluster_arn), pct(topic)),
            None,
        )
        .await
    }

    async fn delete_topic(&self, cluster_arn: &str, topic: &str) -> Reply {
        self.delete_topic_in(REGION, cluster_arn, topic).await
    }

    async fn delete_topic_in(&self, region: &str, cluster_arn: &str, topic: &str) -> Reply {
        self.call_in(
            region,
            "DELETE",
            &format!("/v1/clusters/{}/topics/{}", pct(cluster_arn), pct(topic)),
            None,
        )
        .await
    }

    /// Create `request` on `cluster_arn` and assert it succeeded.
    async fn create_topic_ok(&self, cluster_arn: &str, request: &TopicRequest) -> Reply {
        let reply = self.create_topic(cluster_arn, request).await;
        assert_eq!(
            reply.status, 200,
            "case {request:?}: expected CreateTopic to return 200, got {} {}",
            reply.status, reply.body
        );
        reply
    }
}

/// One row of a `CreateTopic` table. `configs` is `None` when the request omits
/// the member, which is how a row asserts that the reader reports it absent.
#[derive(Debug, Clone)]
struct TopicRequest {
    name: String,
    partitions: i64,
    replication: i64,
    configs: Option<&'static str>,
}

impl TopicRequest {
    fn new(name: &str, partitions: i64, replication: i64) -> Self {
        Self {
            name: name.to_string(),
            partitions,
            replication,
            configs: None,
        }
    }

    fn with_configs(mut self, configs: &'static str) -> Self {
        self.configs = Some(configs);
        self
    }

    fn to_json(&self) -> Value {
        let mut body = json!({
            "topicName": self.name,
            "partitionCount": self.partitions,
            "replicationFactor": self.replication,
        });
        if let Some(configs) = self.configs {
            body["configs"] = json!(configs);
        }
        body
    }
}

/// The ARN the service must mint for `topic` on `cluster_arn`: the cluster's own
/// identity under the `topic` resource type, which is how Amazon MSK names a
/// topic in an IAM policy.
fn expected_topic_arn(cluster_arn: &str, topic: &str) -> String {
    let (prefix, identity) = cluster_arn
        .split_once(":cluster/")
        .unwrap_or_else(|| panic!("case cluster={cluster_arn:?}: not a cluster ARN"));
    format!("{prefix}:topic/{identity}/{topic}")
}

/// Sleep past the emulator's own `CREATING` -> `ACTIVE` span for a topic, so a
/// later read sees a settled topic. `std::thread::sleep` because the workspace's
/// tokio carries no timer feature; nothing else has to make progress here.
fn sleep_past_topic_activation() {
    std::thread::sleep(TOPIC_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// Sleep past the span after which a `DELETING` topic is gone.
fn sleep_past_topic_deletion() {
    std::thread::sleep(TOPIC_DELETION_DELAY + Duration::from_millis(250));
}

/// RULE: a created topic is persisted whole, and `DescribeTopic` reports every
/// member from that record — the name and ARN the create minted, the partition
/// count and replication factor it was created with, and the `Configs` string it
/// was given (absent when the request omitted it). A topic starts `CREATING` and
/// becomes `ACTIVE` once the clock owes it that transition.
#[tokio::test]
async fn a_created_topic_reads_back_from_the_record() {
    let emulator = Emulator::new();
    let cluster = emulator.cluster("round-trip", 3).await;

    // The rows differ along everything the rule generalises over: name form
    // (plain, dotted, underscored, single character, the maximum length Apache
    // Kafka allows, a leading-underscore name), partition count, replication
    // factor within the cluster's three brokers, and whether `Configs` is sent.
    let rows = vec![
        TopicRequest::new("orders", 1, 1),
        TopicRequest::new("orders.v2", 3, 2).with_configs("cmV0ZW50aW9uLm1zPTg2NDAwMDAw"),
        TopicRequest::new("orders_v2-2025", 12, 3).with_configs(""),
        TopicRequest::new("a", 1, 3),
        // 249 characters: Apache Kafka's own maximum topic-name length
        // (`Topic.MAX_NAME_LENGTH`), which `MAX_TOPIC_NAME_LENGTH` names.
        TopicRequest::new(&"x".repeat(MAX_TOPIC_NAME_LENGTH), 2, 1),
        TopicRequest::new("__audit.trail", 6, 2).with_configs("Y2xlYW51cC5wb2xpY3k9Y29tcGFjdA=="),
        TopicRequest::new("...", 4, 1),
    ];

    for case in &rows {
        let created = emulator.create_topic_ok(&cluster, case).await;
        let arn = expected_topic_arn(&cluster, &case.name);
        assert_eq!(
            created.at("/topicArn").as_str().unwrap_or_default(),
            arn,
            "case {case:?}: expected CreateTopic to mint {arn}, got {}",
            created.body
        );
        assert!(
            arn.starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:topic/")),
            "case {case:?}: expected a topic ARN in {REGION} for {ACCOUNT}, got {arn}"
        );
        assert_eq!(
            created.at("/topicName").as_str().unwrap_or_default(),
            case.name,
            "case {case:?}: expected CreateTopic to echo the topic name, got {}",
            created.body
        );
        assert_eq!(
            created.at("/status").as_str().unwrap_or_default(),
            "CREATING",
            "case {case:?}: expected a new topic to be CREATING, got {}",
            created.body
        );

        let described = emulator.describe_topic(&cluster, &case.name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeTopic to return 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.at("/topicArn").as_str().unwrap_or_default(),
            arn,
            "case {case:?}: expected DescribeTopic to report {arn}, got {}",
            described.body
        );
        assert_eq!(
            described.at("/partitionCount").as_i64(),
            Some(case.partitions),
            "case {case:?}: expected the stored partition count, got {}",
            described.body
        );
        assert_eq!(
            described.at("/replicationFactor").as_i64(),
            Some(case.replication),
            "case {case:?}: expected the stored replication factor, got {}",
            described.body
        );
        match case.configs {
            Some(configs) => assert_eq!(
                described.at("/configs").as_str(),
                Some(configs),
                "case {case:?}: expected the stored configs, got {}",
                described.body
            ),
            None => assert_eq!(
                described.at("/configs"),
                &Value::Null,
                "case {case:?}: expected no configs for a request that sent none, got {}",
                described.body
            ),
        }
        assert_eq!(
            described.at("/status").as_str().unwrap_or_default(),
            "CREATING",
            "case {case:?}: expected a topic read back at once to still be CREATING, got {}",
            described.body
        );
    }

    // One wait for the whole table: the activation each topic was owed falls due
    // whether or not anybody read it, so every row is ACTIVE afterwards.
    sleep_past_topic_activation();
    for case in &rows {
        let described = emulator.describe_topic(&cluster, &case.name).await;
        assert_eq!(
            described.at("/status").as_str().unwrap_or_default(),
            "ACTIVE",
            "case {case:?}: expected the topic to be ACTIVE once the clock owed it, got {}",
            described.body
        );
    }
}

/// RULE: deleting a topic the cluster holds answers the non-terminal `DELETING`
/// status and leaves that topic readable in `DELETING` until the clock owes its
/// removal; afterwards every reader reports it gone (`DescribeTopic` with its
/// modelled `NotFoundException`, `DeleteTopic` with its modelled
/// `UnknownTopicOrPartitionException`), its name is free again, and no sibling
/// topic of the cluster is touched.
#[tokio::test]
async fn delete_moves_the_topic_to_deleting_and_then_removes_it() {
    // Exhaustive over which topic of the set is deleted rather than sampled: the
    // set is small enough to walk, and which one is addressed is exactly what the
    // rule generalises over.
    let topics = ["alpha", "beta.1", "gamma_2"];
    for target in &topics {
        let emulator = Emulator::new();
        let cluster = emulator.cluster("delete-span", 2).await;
        for name in &topics {
            emulator
                .create_topic_ok(&cluster, &TopicRequest::new(name, 3, 2))
                .await;
        }

        let deleted = emulator.delete_topic(&cluster, target).await;
        assert_eq!(
            (deleted.status, deleted.at("/status").as_str().unwrap_or_default()),
            (200, "DELETING"),
            "case topic={target:?}: expected DeleteTopic to answer 200 DELETING, got {} {}",
            deleted.status,
            deleted.body
        );
        assert_eq!(
            deleted.at("/topicArn").as_str().unwrap_or_default(),
            expected_topic_arn(&cluster, target),
            "case topic={target:?}: expected DeleteTopic to report the topic's ARN, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.at("/topicName").as_str().unwrap_or_default(),
            *target,
            "case topic={target:?}: expected DeleteTopic to report the topic's name, got {}",
            deleted.body
        );

        // The accepted deletion is observable: the record is still there, in the
        // state the delete reported.
        let described = emulator.describe_topic(&cluster, target).await;
        assert_eq!(
            (described.status, described.at("/status").as_str().unwrap_or_default()),
            (200, "DELETING"),
            "case topic={target:?}: expected a deleting topic to read back as DELETING, got {} {}",
            described.status,
            described.body
        );
        // Repeating the call reports the same pending deletion; it does not
        // postpone it, which the wait below proves.
        let again = emulator.delete_topic(&cluster, target).await;
        assert_eq!(
            (again.status, again.at("/status").as_str().unwrap_or_default()),
            (200, "DELETING"),
            "case topic={target:?}: expected a repeated DeleteTopic to report DELETING, got {} {}",
            again.status,
            again.body
        );

        for sibling in topics.iter().filter(|name| *name != target) {
            let reply = emulator.describe_topic(&cluster, sibling).await;
            assert_eq!(
                reply.status, 200,
                "case topic={target:?}: expected the sibling {sibling:?} to be untouched, got {} {}",
                reply.status, reply.body
            );
            assert_ne!(
                reply.at("/status").as_str().unwrap_or_default(),
                "DELETING",
                "case topic={target:?}: deleting it must not mark {sibling:?} DELETING, got {}",
                reply.body
            );
        }

        sleep_past_topic_deletion();

        let described = emulator.describe_topic(&cluster, target).await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "NotFoundException"),
            "case topic={target:?}: expected a removed topic to be a 404 NotFoundException from DescribeTopic, got {} {}",
            described.status,
            described.body
        );
        let deleted_again = emulator.delete_topic(&cluster, target).await;
        assert_eq!(
            (deleted_again.status, deleted_again.error_code()),
            (404, "UnknownTopicOrPartitionException"),
            "case topic={target:?}: expected a removed topic to be a 404 UnknownTopicOrPartitionException from DeleteTopic, got {} {}",
            deleted_again.status,
            deleted_again.body
        );
        for sibling in topics.iter().filter(|name| *name != target) {
            let reply = emulator.describe_topic(&cluster, sibling).await;
            assert_eq!(
                (reply.status, reply.at("/status").as_str().unwrap_or_default()),
                (200, "ACTIVE"),
                "case topic={target:?}: expected the sibling {sibling:?} to be ACTIVE, got {} {}",
                reply.status,
                reply.body
            );
        }

        // The name is free again, so the same topic can be created afresh.
        let recreated = emulator
            .create_topic_ok(&cluster, &TopicRequest::new(target, 1, 1))
            .await;
        let recreated_status = recreated
            .at("/status")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            matches!(recreated_status.as_str(), "CREATING" | "ACTIVE"),
            "case topic={target:?}: expected the freed name to be creatable again, got {}",
            recreated.body
        );
    }
}

/// RULE: a topic operation resolves the CLUSTER first and the TOPIC second — a
/// `ClusterArn` that is not an MSK cluster ARN is a `BadRequestException`, a
/// well-formed one this endpoint's region holds no cluster for is a
/// `NotFoundException`, and a legal topic name an existing cluster holds no topic
/// for is an `UnknownTopicOrPartitionException` — and a refusal leaves the
/// registry exactly as it was.
#[tokio::test]
async fn delete_rejects_an_unknown_cluster_or_topic() {
    let emulator = Emulator::new();
    let cluster = emulator.cluster("present", 2).await;
    emulator
        .create_topic_ok(&cluster, &TopicRequest::new("kept", 3, 2))
        .await;

    let absent = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
    );
    let other_region = format!(
        "arn:aws:kafka:us-east-1:{ACCOUNT}:cluster/present/11111111-2222-4333-8444-555555555555-1"
    );

    // `(cluster ARN, topic name, expected status, expected error code)`. Rows: one
    // per way the ARN grammar can fail, then a well-formed ARN no cluster answers
    // for (absent, and belonging to another region), then the cluster that does
    // exist with a topic it does not hold.
    let rows: Vec<(String, &str, u16, &str)> = vec![
        ("not-an-arn".to_string(), "kept", 400, "BadRequestException"),
        (
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster/present/abc-1"),
            "kept",
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/present/abc"),
            "kept",
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/present/abc/extra"),
            "kept",
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka::{ACCOUNT}:cluster/present/abc-1"),
            "kept",
            400,
            "BadRequestException",
        ),
        (absent.clone(), "kept", 404, "NotFoundException"),
        (other_region, "kept", 404, "NotFoundException"),
        (
            cluster.clone(),
            "absent-topic",
            404,
            "UnknownTopicOrPartitionException",
        ),
        (
            cluster.clone(),
            "absent.topic-2",
            404,
            "UnknownTopicOrPartitionException",
        ),
    ];

    for (arn, topic, status, code) in &rows {
        let deleted = emulator.delete_topic(arn, topic).await;
        assert_eq!(
            (deleted.status, deleted.error_code()),
            (*status, *code),
            "case arn={arn:?} topic={topic:?}: expected {status} {code} from DeleteTopic, got {} {}",
            deleted.status,
            deleted.body
        );
    }

    // A cluster is served only by its own region's endpoint, so the very same
    // cluster ARN resolves to nothing under another signing region.
    let elsewhere = emulator.delete_topic_in("eu-west-1", &cluster, "kept").await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code()),
        (404, "NotFoundException"),
        "case region=eu-west-1: expected a 404 NotFoundException for another region's cluster, got {} {}",
        elsewhere.status,
        elsewhere.body
    );
    let read_elsewhere = emulator
        .describe_topic_in("eu-west-1", &cluster, "kept")
        .await;
    assert_eq!(
        (read_elsewhere.status, read_elsewhere.error_code()),
        (404, "NotFoundException"),
        "case region=eu-west-1: expected a 404 NotFoundException from DescribeTopic, got {} {}",
        read_elsewhere.status,
        read_elsewhere.body
    );

    // None of those refusals disturbed the topic that does exist. Its status is
    // whatever the clock owes it by now — the point is that it is still there
    // and not `DELETING`.
    let kept = emulator.describe_topic(&cluster, "kept").await;
    let kept_status = kept.at("/status").as_str().unwrap_or_default().to_string();
    assert!(
        kept.status == 200 && matches!(kept_status.as_str(), "CREATING" | "ACTIVE"),
        "a refused read or delete must leave the topic alone, got {} {}",
        kept.status,
        kept.body
    );
}

/// RULE: a topic name every topic operation accepts is one Apache Kafka accepts —
/// at most [`MAX_TOPIC_NAME_LENGTH`] characters, neither `.` nor `..`, and built
/// only from `[a-zA-Z0-9._-]`; anything else is a `BadRequestException` naming
/// `TopicName`, and nothing is created for it.
#[tokio::test]
async fn an_illegal_topic_name_is_refused() {
    let emulator = Emulator::new();
    let cluster = emulator.cluster("names", 1).await;

    // The catalog and the bound come from Apache Kafka's own topic-name rule
    // (`org.apache.kafka.common.internals.Topic.validate`): the length bound, the
    // two reserved names, and the legal character set. Rows cover the empty name,
    // both reserved names, each bound of the length range and one past it, and
    // one illegal character of each kind (space, '/', ':', '+', non-ASCII).
    let illegal = vec![
        String::new(),
        ".".to_string(),
        "..".to_string(),
        "x".repeat(MAX_TOPIC_NAME_LENGTH + 1),
        "x".repeat(MAX_TOPIC_NAME_LENGTH * 2),
        "bad topic".to_string(),
        "bad:topic".to_string(),
        "bad+topic".to_string(),
        "tópico".to_string(),
    ];
    for name in &illegal {
        let created = emulator
            .create_topic(&cluster, &TopicRequest::new(name, 1, 1))
            .await;
        assert_eq!(
            (created.status, created.error_code()),
            (400, "BadRequestException"),
            "case name={name:?}: expected a 400 BadRequestException from CreateTopic, got {} {}",
            created.status,
            created.body
        );
        assert_eq!(
            created.at("/invalidParameter").as_str(),
            Some("TopicName"),
            "case name={name:?}: expected the refusal to name TopicName, got {}",
            created.body
        );
    }

    // The legal rows, including both ends of the length range and the reserved
    // names' nearest legal neighbours, are accepted and readable.
    let legal = vec![
        "a".to_string(),
        "...".to_string(),
        "_".to_string(),
        "-".to_string(),
        "9".to_string(),
        "x".repeat(MAX_TOPIC_NAME_LENGTH),
    ];
    for name in &legal {
        let created = emulator
            .create_topic(&cluster, &TopicRequest::new(name, 1, 1))
            .await;
        assert_eq!(
            created.status, 200,
            "case name={name:?}: expected CreateTopic to accept a legal Kafka topic name, got {} {}",
            created.status, created.body
        );
        let described = emulator.describe_topic(&cluster, name).await;
        assert_eq!(
            described.status, 200,
            "case name={name:?}: expected the created topic to read back, got {} {}",
            described.status, described.body
        );
    }

    // An illegal name is refused by the delete path too, and the names it holds
    // are untouched. The empty name is omitted here: it is not addressable as a
    // URI path segment, which the router refuses before the operation runs.
    for name in illegal.iter().filter(|name| !name.is_empty()) {
        let deleted = emulator.delete_topic(&cluster, name).await;
        assert_eq!(
            (deleted.status, deleted.error_code()),
            (400, "BadRequestException"),
            "case name={name:?}: expected a 400 BadRequestException from DeleteTopic, got {} {}",
            deleted.status,
            deleted.body
        );
    }
    for name in &legal {
        let described = emulator.describe_topic(&cluster, name).await;
        assert_eq!(
            described.status, 200,
            "case name={name:?}: a refusal must leave an existing topic alone, got {} {}",
            described.status, described.body
        );
    }
}

/// RULE: a topic's replication factor is bounded by the number of brokers of the
/// cluster it is created on — Kafka cannot place more replicas of a partition
/// than it has brokers — so the bound moves with the cluster, and a request past
/// it is a `BadRequestException` that creates nothing.
#[tokio::test]
async fn create_bounds_the_replication_factor_by_the_broker_count() {
    let emulator = Emulator::new();

    // The broker counts span the modelled 1..15 range of
    // `CreateClusterRequest$NumberOfBrokerNodes`, including both bounds.
    for brokers in [1_i64, 2, 3, 15] {
        let cluster = emulator
            .cluster(&format!("rf-{brokers}"), brokers)
            .await;

        // At the bound: a replication factor equal to the broker count is legal.
        let at_bound = TopicRequest::new("at-bound", 2, brokers);
        let created = emulator.create_topic(&cluster, &at_bound).await;
        assert_eq!(
            created.status, 200,
            "case brokers={brokers}: expected a replication factor of {brokers} to be accepted, got {} {}",
            created.status, created.body
        );

        // One past it: refused, and nothing is registered for the name.
        let past_bound = TopicRequest::new("past-bound", 2, brokers + 1);
        let refused = emulator.create_topic(&cluster, &past_bound).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "BadRequestException"),
            "case brokers={brokers}: expected a replication factor of {} to be refused, got {} {}",
            brokers + 1,
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.at("/invalidParameter").as_str(),
            Some("ReplicationFactor"),
            "case brokers={brokers}: expected the refusal to name ReplicationFactor, got {}",
            refused.body
        );
        let described = emulator.describe_topic(&cluster, "past-bound").await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "NotFoundException"),
            "case brokers={brokers}: a refused create must register nothing, got {} {}",
            described.status,
            described.body
        );
    }
}

/// RULE: a topic name identifies a topic WITHIN its cluster — the same name on
/// two clusters is two topics with their own ARNs, a repeat on one cluster is a
/// `TopicExistsException` that leaves the stored topic as it was (including while
/// it is `DELETING`), and deleting one cluster's topic leaves the other's alone.
#[tokio::test]
async fn a_topic_name_is_unique_within_its_cluster() {
    let emulator = Emulator::new();
    let first = emulator.cluster("first", 2).await;
    let second = emulator.cluster("second", 2).await;

    // The same name on two clusters: two topics, each ARN rooted at its own
    // cluster.
    for cluster in [&first, &second] {
        let created = emulator
            .create_topic_ok(cluster, &TopicRequest::new("shared", 3, 2))
            .await;
        assert_eq!(
            created.at("/topicArn").as_str().unwrap_or_default(),
            expected_topic_arn(cluster, "shared"),
            "case cluster={cluster:?}: expected the topic ARN to be rooted at its own cluster, got {}",
            created.body
        );
    }

    // A repeat conflicts, whatever else the request changes, and the stored topic
    // keeps the values it was created with.
    let repeats = vec![
        TopicRequest::new("shared", 3, 2),
        TopicRequest::new("shared", 1, 1),
        TopicRequest::new("shared", 48, 2).with_configs("cmV0ZW50aW9uLm1zPTE="),
    ];
    for case in &repeats {
        let reply = emulator.create_topic(&first, case).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (409, "TopicExistsException"),
            "case {case:?}: expected a 409 TopicExistsException for a name already in use, got {} {}",
            reply.status,
            reply.body
        );
        let described = emulator.describe_topic(&first, "shared").await;
        assert_eq!(
            (
                described.at("/partitionCount").as_i64(),
                described.at("/configs").clone()
            ),
            (Some(3), Value::Null),
            "case {case:?}: a refused create must leave the stored topic alone, got {}",
            described.body
        );
    }

    // Deleting one cluster's topic is invisible to the other's, and the name is
    // still taken while the deletion is pending.
    let deleted = emulator.delete_topic(&first, "shared").await;
    assert_eq!(
        (deleted.status, deleted.at("/status").as_str().unwrap_or_default()),
        (200, "DELETING"),
        "expected DeleteTopic to answer 200 DELETING, got {} {}",
        deleted.status,
        deleted.body
    );
    let while_deleting = emulator
        .create_topic(&first, &TopicRequest::new("shared", 3, 2))
        .await;
    assert_eq!(
        (while_deleting.status, while_deleting.error_code()),
        (409, "TopicExistsException"),
        "expected a name whose topic is still DELETING to be taken, got {} {}",
        while_deleting.status,
        while_deleting.body
    );
    let other = emulator.describe_topic(&second, "shared").await;
    let other_status = other.at("/status").as_str().unwrap_or_default().to_string();
    assert!(
        other.status == 200 && matches!(other_status.as_str(), "CREATING" | "ACTIVE"),
        "expected the other cluster's topic to be untouched, got {} {}",
        other.status,
        other.body
    );

    sleep_past_topic_deletion();
    let gone = emulator.describe_topic(&first, "shared").await;
    assert_eq!(
        (gone.status, gone.error_code()),
        (404, "NotFoundException"),
        "expected the deleted topic to be gone from its cluster, got {} {}",
        gone.status,
        gone.body
    );
    let survivor = emulator.describe_topic(&second, "shared").await;
    assert_eq!(
        survivor.status, 200,
        "expected the other cluster's topic to survive, got {} {}",
        survivor.status, survivor.body
    );
}

/// RULE: the removal a delete scheduled falls due on the CLOCK, not on being
/// observed — so once that instant passes the name is free for a fresh create
/// even when nothing read the topic in between.
#[tokio::test]
async fn a_name_the_clock_freed_is_creatable_without_an_intervening_read() {
    // The rows differ along the partition count and replication factor the
    // recreate asks for, so the fresh topic is the one the second request
    // describes rather than a survivor of the first.
    let rows = vec![
        (TopicRequest::new("recycled", 3, 2), TopicRequest::new("recycled", 1, 1)),
        (
            TopicRequest::new("recycled.2", 1, 1),
            TopicRequest::new("recycled.2", 24, 2).with_configs("cmV0ZW50aW9uLm1zPTE="),
        ),
    ];
    for (first, second) in &rows {
        let emulator = Emulator::new();
        let cluster = emulator.cluster("recycle", 2).await;
        emulator.create_topic_ok(&cluster, first).await;
        let deleted = emulator.delete_topic(&cluster, &first.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {first:?}: expected DeleteTopic to return 200, got {} {}",
            deleted.status, deleted.body
        );

        // Nothing reads the topic across the wait: the registry must free the
        // name on its own.
        sleep_past_topic_deletion();

        let recreated = emulator.create_topic(&cluster, second).await;
        assert_eq!(
            recreated.status, 200,
            "case {second:?}: expected the clock-freed name to be creatable, got {} {}",
            recreated.status, recreated.body
        );
        let described = emulator.describe_topic(&cluster, &second.name).await;
        assert_eq!(
            (
                described.status,
                described.at("/partitionCount").as_i64(),
                described.at("/replicationFactor").as_i64()
            ),
            (200, Some(second.partitions), Some(second.replication)),
            "case {second:?}: expected the fresh topic's own values, got {} {}",
            described.status,
            described.body
        );
    }
}

/// RULE: a topic lives on the cluster that holds it, so deleting the cluster
/// takes its topics with it: every topic operation on that cluster afterwards
/// reports the cluster as the missing resource.
#[tokio::test]
async fn deleting_a_cluster_takes_its_topics_with_it() {
    let emulator = Emulator::new();
    let cluster = emulator.cluster("doomed", 2).await;
    for name in ["one", "two.2", "three_3"] {
        emulator
            .create_topic_ok(&cluster, &TopicRequest::new(name, 1, 1))
            .await;
    }

    let deleted = emulator
        .call("DELETE", &format!("/v1/clusters/{}", pct(&cluster)), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteCluster to return 200, got {} {}",
        deleted.status, deleted.body
    );

    for name in ["one", "two.2", "three_3"] {
        let described = emulator.describe_topic(&cluster, name).await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "NotFoundException"),
            "case topic={name:?}: expected a 404 NotFoundException once the cluster is gone, got {} {}",
            described.status,
            described.body
        );
        let delete_again = emulator.delete_topic(&cluster, name).await;
        assert_eq!(
            (delete_again.status, delete_again.error_code()),
            (404, "NotFoundException"),
            "case topic={name:?}: expected DeleteTopic to report the cluster missing, got {} {}",
            delete_again.status,
            delete_again.body
        );
    }
}
