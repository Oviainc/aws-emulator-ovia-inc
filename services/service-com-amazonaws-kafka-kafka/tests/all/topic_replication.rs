// SPDX-License-Identifier: CC0-1.0

//! What a replicating MSK Replicator makes its TARGET cluster report: the topics
//! of the flow's source cluster that the flow selects, under the name the flow
//! gives them there.
//!
//! The rule these suites assert, over a TABLE rather than one recorded request:
//! while a replicator is replicating, every topic of a replication flow's source
//! cluster that the flow's `TopicsToReplicate` patterns select and its
//! `TopicsToExclude` patterns do not is reported by the flow's target cluster —
//! named after the source topic, prefixed with the source cluster's alias unless
//! the flow asks for identical names — from the instant the clock owes the
//! propagation until the source topic or the replicator is gone.
//!
//! The rows differ along what that rule generalises over: the pattern form (an
//! exact name, a wildcard, a prefix, an alternation, none at all), the exclusion
//! patterns, the topic names matched against them, the naming configuration, the
//! detect-and-copy and copy-configuration settings, when the source topic was
//! created relative to the replicator, the partition count and replication factor
//! carried over, the broker count of the target cluster, and the length of the
//! name the flow would have to mint.
//!
//! The HTTP harness — the router, the `Reply` wrapper, the percent-encoder and the
//! `CreateCluster` request builder — comes from [`crate::kafka_harness`], the same
//! one every other suite in this crate drives the service through.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    MAX_TOPIC_NAME_LENGTH, MAX_TOPIC_PATTERNS_PER_FLOW, MAX_TOPIC_PATTERN_LENGTH,
    TOPIC_ACTIVATION_DELAY, TOPIC_DELETION_DELAY, TOPIC_REPLICATION_DELAY,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The IAM role every replicator below is created with. `ServiceExecutionRoleArn`
/// is a plain `__string` in the model, so the service stores it verbatim.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// The `ReplicationTopicNameConfigurationType` member that names a replicated
/// topic identically to its source topic.
const IDENTICAL: &str = "IDENTICAL";

/// The member that prefixes a replicated topic's name with the source cluster's
/// alias — the naming a flow that configures nothing gets.
const PREFIXED: &str = "PREFIXED_WITH_SOURCE_CLUSTER_ALIAS";

/// Wait past the span the clock owes a replicated topic: the source topic's own
/// activation plus the propagation onto the target. `std::thread::sleep` because
/// the workspace's tokio carries no timer feature; nothing else has to make
/// progress here.
fn sleep_past_replication() {
    std::thread::sleep(TOPIC_ACTIVATION_DELAY + TOPIC_REPLICATION_DELAY + Duration::from_millis(250));
}

/// Wait past the span the clock owes a deleted topic's removal.
fn sleep_past_topic_deletion() {
    std::thread::sleep(TOPIC_DELETION_DELAY + Duration::from_millis(250));
}

/// A provisioned cluster of `brokers` nodes, named `name`, and its ARN. The
/// cluster is only the context replication needs, so the row is the harness's
/// minimal `CreateCluster` request with the broker count set.
async fn cluster(emulator: &Emulator, name: &str, brokers: i64) -> String {
    let mut request = ClusterRequest::named(name);
    request.brokers = brokers;
    emulator.create_ok(&request).await
}

/// The alias MSK derives for the cluster `cluster_arn` names: the cluster NAME
/// truncated to 28 characters, a hyphen, and the final group of the uuid in the
/// ARN's id segment (12 hex digits). That is what `KafkaClusterAlias` reports and
/// what prefixes the names of topics replicated FROM that cluster. Spelled out
/// here rather than built from the service's own helper, so the form is pinned by
/// the suite instead of by the code under test.
fn alias_of(cluster_arn: &str) -> String {
    let mut segments = cluster_arn
        .splitn(6, ':')
        .nth(5)
        .unwrap_or_default()
        .split('/')
        .skip(1);
    let name: String = segments
        .next()
        .unwrap_or_default()
        .chars()
        .take(28)
        .collect();
    let id = segments.next().unwrap_or_default();
    let tail = id.split('-').nth(4).unwrap_or_default();
    format!("{name}-{tail}")
}

/// The ARN a topic named `topic` has on the cluster `cluster_arn`: that cluster's
/// own identity under the `topic` resource type, which is the resource an MSK IAM
/// policy grants topic actions on.
fn topic_arn(cluster_arn: &str, topic: &str) -> String {
    let (prefix, identity) = cluster_arn
        .split_once(":cluster/")
        .unwrap_or_else(|| panic!("{cluster_arn} is not a cluster ARN"));
    format!("{prefix}:topic/{identity}/{topic}")
}

/// `CreateTopic`, which the model binds to `POST /v1/clusters/{ClusterArn}/topics`.
async fn create_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    name: &str,
    partitions: i64,
    replication: i64,
    configs: Option<&str>,
) -> Reply {
    let mut body = json!({
        "topicName": name,
        "partitionCount": partitions,
        "replicationFactor": replication,
    });
    if let Some(configs) = configs {
        body["configs"] = json!(configs);
    }
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            Some(body),
        )
        .await
}

/// The same create, asserted to have succeeded.
async fn create_topic_ok(
    emulator: &Emulator,
    cluster_arn: &str,
    name: &str,
    partitions: i64,
    replication: i64,
    configs: Option<&str>,
    case: &str,
) {
    let reply = create_topic(emulator, cluster_arn, name, partitions, replication, configs).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected CreateTopic({name}) to return 200, got {} {}",
        reply.status, reply.body
    );
}

/// `DeleteTopic`, which the model binds to
/// `DELETE /v1/clusters/{ClusterArn}/topics/{TopicName}`.
async fn delete_topic(emulator: &Emulator, cluster_arn: &str, name: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!(
                "/v1/clusters/{}/topics/{}",
                pct(cluster_arn),
                pct(name)
            ),
            None,
        )
        .await
}

/// `DescribeTopic`, on the same resource URI.
async fn describe_topic(emulator: &Emulator, cluster_arn: &str, name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/v1/clusters/{}/topics/{}",
                pct(cluster_arn),
                pct(name)
            ),
            None,
        )
        .await
}

/// The topic names `ListTopics` reports for `cluster_arn`, in the order it pages
/// them.
async fn topic_names(emulator: &Emulator, cluster_arn: &str, case: &str) -> Vec<String> {
    let reply = emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListTopics to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/topics")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListTopics reported no topics list"))
        .iter()
        .map(|topic| {
            topic["topicName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a listed topic carries no topicName"))
                .to_string()
        })
        .collect()
}

/// A `CreateReplicator` body with one flow from `source` to `target`, carrying
/// `topic_replication` as the row wrote it.
fn replicator_body(name: &str, source: &str, target: &str, topic_replication: Value) -> Value {
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": ROLE_ARN,
        "kafkaClusters": [
            {
                "amazonMskCluster": { "mskClusterArn": source },
                "vpcConfig": { "subnetIds": ["subnet-a", "subnet-b", "subnet-c"] },
            },
            {
                "amazonMskCluster": { "mskClusterArn": target },
                "vpcConfig": { "subnetIds": ["subnet-a", "subnet-b", "subnet-c"] },
            },
        ],
        "replicationInfoList": [{
            "sourceKafkaClusterArn": source,
            "targetKafkaClusterArn": target,
            "targetCompressionType": "NONE",
            "topicReplication": topic_replication,
            "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
        }],
    })
}

/// `CreateReplicator`, which the model binds to `POST /replication/v1/replicators`.
async fn create_replicator(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .call("POST", "/replication/v1/replicators", Some(body))
        .await
}

/// Create a replicator for one flow and hand back its ARN.
async fn create_replicator_ok(
    emulator: &Emulator,
    name: &str,
    source: &str,
    target: &str,
    topic_replication: Value,
    case: &str,
) -> String {
    let reply =
        create_replicator(emulator, replicator_body(name, source, target, topic_replication)).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected CreateReplicator to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/replicatorArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateReplicator returned no replicatorArn"))
        .to_string()
}

/// `DeleteReplicator`, on the replicator's own resource URI.
async fn delete_replicator(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/replication/v1/replicators/{}", pct(arn)),
            None,
        )
        .await
}

/// `DescribeReplicator`, on the same resource URI.
async fn describe_replicator(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(arn)),
            None,
        )
        .await
}

/// `UpdateReplicationInfo`, which the model binds to
/// `PUT /replication/v1/replicators/{ReplicatorArn}/replication-info`.
async fn update_replication_info(emulator: &Emulator, arn: &str, body: Value) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/replication/v1/replicators/{}/replication-info", pct(arn)),
            Some(body),
        )
        .await
}

/// One row of the filter-and-naming table: the flow's topic filter, how it names
/// what it replicates, and the source cluster's topics with whether the flow
/// replicates each one.
#[derive(Debug, Clone)]
struct FilterRow {
    case: &'static str,
    replicate: &'static [&'static str],
    exclude: &'static [&'static str],
    /// `ReplicationTopicNameConfiguration$Type`, or `None` where the flow omits
    /// the member — which is how a row tests the naming a flow gets by default.
    naming: Option<&'static str>,
    topics: &'static [(&'static str, bool)],
}

impl FilterRow {
    /// The `TopicReplication` the flow carries: only the members the row
    /// exercises, so an omitted one tests the setting's default.
    fn topic_replication(&self) -> Value {
        let mut settings = json!({ "topicsToReplicate": self.replicate });
        if !self.exclude.is_empty() {
            settings["topicsToExclude"] = json!(self.exclude);
        }
        if let Some(naming) = self.naming {
            settings["topicNameConfiguration"] = json!({ "type": naming });
        }
        settings
    }

    /// The name the target cluster must report the source topic `topic` under.
    fn replicated_name(&self, source_alias: &str, topic: &str) -> String {
        if self.naming == Some(IDENTICAL) {
            topic.to_string()
        } else {
            format!("{source_alias}.{topic}")
        }
    }
}

/// The filter-and-naming table. The patterns are regular expressions
/// (`TopicReplication$TopicsToReplicate`: "List of regular expression patterns
/// indicating the topics to copy"), each describing a whole topic name, so a row
/// pairs every pattern form with a topic it does and one it does not select.
fn filter_rows() -> Vec<FilterRow> {
    vec![
        FilterRow {
            case: "an exact pattern selects that topic and no longer name",
            replicate: &["orders"],
            exclude: &[],
            naming: None,
            topics: &[("orders", true), ("orders-eu", false), ("the-orders", false)],
        },
        FilterRow {
            case: "a wildcard selects every topic the exclusions leave",
            replicate: &[".*"],
            exclude: &["__.*"],
            naming: None,
            topics: &[("orders", true), ("payments", true), ("__internal", false)],
        },
        FilterRow {
            case: "a prefix pattern selects the topics under that prefix",
            replicate: &["orders-.*"],
            exclude: &[],
            naming: None,
            topics: &[("orders-eu", true), ("orders-us", true), ("payments", false)],
        },
        FilterRow {
            case: "an alternation selects either branch and nothing around it",
            replicate: &["orders|payments"],
            exclude: &[],
            naming: None,
            topics: &[
                ("orders", true),
                ("payments", true),
                ("payments-eu", false),
                ("audit", false),
            ],
        },
        FilterRow {
            case: "two patterns select the union of what each selects",
            replicate: &["orders", "audit-.*"],
            exclude: &[],
            naming: None,
            topics: &[("orders", true), ("audit-eu", true), ("payments", false)],
        },
        FilterRow {
            case: "an exclusion pattern withholds the topic it names",
            replicate: &[".*"],
            exclude: &["payments"],
            naming: None,
            topics: &[("orders", true), ("payments", false)],
        },
        FilterRow {
            case: "no pattern selects nothing",
            replicate: &[],
            exclude: &[],
            naming: None,
            topics: &[("orders", false), ("payments", false)],
        },
        FilterRow {
            case: "an explicitly prefixed flow names what the default does",
            replicate: &[".*"],
            exclude: &[],
            naming: Some(PREFIXED),
            topics: &[("orders", true)],
        },
        FilterRow {
            case: "an identical-naming flow reports the source topic's own name",
            replicate: &[".*"],
            exclude: &[],
            naming: Some(IDENTICAL),
            topics: &[("orders", true), ("payments", true)],
        },
    ]
}

#[tokio::test]
async fn a_replicating_flow_makes_the_target_report_the_source_topics_it_selects() {
    let emulator = Emulator::new();
    let rows = filter_rows();
    // Every row's setup is stood up first and every assertion made after one
    // wait, so the suite waits through the emulated spans once rather than per
    // row.
    let mut prepared = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let source = cluster(&emulator, &format!("replsource{index}"), 3).await;
        let target = cluster(&emulator, &format!("repltarget{index}"), 3).await;
        create_replicator_ok(
            &emulator,
            &format!("replflow{index}"),
            &source,
            &target,
            row.topic_replication(),
            row.case,
        )
        .await;
        prepared.push((row, source, target));
    }
    sleep_past_activation();
    for (row, source, _) in &prepared {
        for (topic, _) in row.topics {
            create_topic_ok(&emulator, source, topic, 2, 2, None, row.case).await;
        }
    }
    sleep_past_replication();

    for (row, source, target) in &prepared {
        let alias = alias_of(source);
        let alias = alias.as_str();
        let mut expected: Vec<String> = row
            .topics
            .iter()
            .filter(|(_, replicated)| *replicated)
            .map(|(topic, _)| row.replicated_name(alias, topic))
            .collect();
        expected.sort();
        let reported = topic_names(&emulator, target, row.case).await;
        assert_eq!(
            reported, expected,
            "case {}: expected the target cluster to report {expected:?}, got {reported:?}",
            row.case
        );

        // Every topic the listing reports reads back through `DescribeTopic`, and
        // one the flow does not select is absent from both readers.
        for (topic, replicated) in row.topics {
            let name = row.replicated_name(alias, topic);
            let reply = describe_topic(&emulator, target, &name).await;
            let expected_status = if *replicated { 200 } else { 404 };
            assert_eq!(
                reply.status, expected_status,
                "case {}: expected DescribeTopic({name}) on the target to return \
                 {expected_status}, got {} {}",
                row.case, reply.status, reply.body
            );
            if *replicated {
                assert_eq!(
                    reply.at("/topicArn").as_str().unwrap_or_default(),
                    topic_arn(target, &name),
                    "case {}: expected the replicated topic's ARN to belong to the target \
                     cluster, got {}",
                    row.case,
                    reply.body
                );
            }
        }

        // A flow replicates ONE way: the source cluster reports its own topics and
        // nothing the target holds.
        let mut own: Vec<String> = row.topics.iter().map(|(topic, _)| topic.to_string()).collect();
        own.sort();
        let source_reported = topic_names(&emulator, source, row.case).await;
        assert_eq!(
            source_reported, own,
            "case {}: expected the source cluster to report {own:?}, got {source_reported:?}",
            row.case
        );
    }
}

/// One row of the detect-and-copy table: whether the flow detects new topics,
/// whether it copies topic configurations, whether the source topic was created
/// before the replicator, and what the target must then report.
#[derive(Debug, Clone)]
struct CopyRow {
    case: &'static str,
    /// `TopicReplication$DetectAndCopyNewTopics`, or `None` where the flow omits
    /// it.
    detect_new: Option<bool>,
    /// `TopicReplication$CopyTopicConfigurations`, or `None` where the flow omits
    /// it.
    copy_configurations: Option<bool>,
    /// Whether the source topic existed before the replicator was created.
    created_before_replicator: bool,
    replicated: bool,
    /// The `Configs` string the target must report for the replicated topic.
    configs: Option<&'static str>,
}

/// The source topic's own topic-level configuration, carried over by a flow that
/// copies topic configurations.
const SOURCE_CONFIGS: &str = "retention.ms=86400000";

/// The detect-and-copy table. `DetectAndCopyNewTopics` is documented as "Whether
/// to periodically check for new topics and partitions", so a flow that detects
/// them replicates a topic created after the replicator, and one that does not
/// replicates only what its source cluster already held;
/// `CopyTopicConfigurations` ("Whether to periodically configure remote topics to
/// match their corresponding upstream topics") decides whether the source topic's
/// own configuration comes with it. An omitted member tests the setting's
/// default.
fn copy_rows() -> Vec<CopyRow> {
    vec![
        CopyRow {
            case: "a flow that says nothing replicates a topic created after it",
            detect_new: None,
            copy_configurations: None,
            created_before_replicator: false,
            replicated: true,
            configs: Some(SOURCE_CONFIGS),
        },
        CopyRow {
            case: "a flow that detects new topics replicates one created after it",
            detect_new: Some(true),
            copy_configurations: Some(true),
            created_before_replicator: false,
            replicated: true,
            configs: Some(SOURCE_CONFIGS),
        },
        CopyRow {
            case: "a flow that does not detect new topics leaves a later one alone",
            detect_new: Some(false),
            copy_configurations: Some(true),
            created_before_replicator: false,
            replicated: false,
            configs: None,
        },
        CopyRow {
            case: "a flow that does not detect new topics still replicates an earlier one",
            detect_new: Some(false),
            copy_configurations: Some(true),
            created_before_replicator: true,
            replicated: true,
            configs: Some(SOURCE_CONFIGS),
        },
        CopyRow {
            case: "a flow that does not copy configurations reports none",
            detect_new: Some(true),
            copy_configurations: Some(false),
            created_before_replicator: false,
            replicated: true,
            configs: None,
        },
    ]
}

#[tokio::test]
async fn replication_follows_the_flow_s_detect_and_copy_settings() {
    let emulator = Emulator::new();
    let rows = copy_rows();
    let topic = "orders";
    let mut prepared = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let source = cluster(&emulator, &format!("copysource{index}"), 3).await;
        let target = cluster(&emulator, &format!("copytarget{index}"), 3).await;
        let mut settings = json!({ "topicsToReplicate": [".*"] });
        if let Some(detect) = row.detect_new {
            settings["detectAndCopyNewTopics"] = json!(detect);
        }
        if let Some(copy) = row.copy_configurations {
            settings["copyTopicConfigurations"] = json!(copy);
        }
        // A topic the row says existed first is created BEFORE the replicator, so
        // the flow's detect-and-copy setting decides the two cases apart.
        if row.created_before_replicator {
            create_topic_ok(&emulator, &source, topic, 2, 2, Some(SOURCE_CONFIGS), row.case).await;
        }
        create_replicator_ok(
            &emulator,
            &format!("copyflow{index}"),
            &source,
            &target,
            settings,
            row.case,
        )
        .await;
        if !row.created_before_replicator {
            create_topic_ok(&emulator, &source, topic, 2, 2, Some(SOURCE_CONFIGS), row.case).await;
        }
        prepared.push((row, source, target));
    }
    sleep_past_replication();

    for (row, source, target) in &prepared {
        let name = format!("{}.{topic}", alias_of(source));
        let reported = topic_names(&emulator, target, row.case).await;
        let expected: Vec<String> = if row.replicated {
            vec![name.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported, expected,
            "case {}: expected the target cluster to report {expected:?}, got {reported:?}",
            row.case
        );
        if row.replicated {
            let reply = describe_topic(&emulator, target, &name).await;
            assert_eq!(
                reply.status, 200,
                "case {}: expected DescribeTopic({name}) to return 200, got {} {}",
                row.case, reply.status, reply.body
            );
            let configs = reply.at("/configs").as_str().map(str::to_string);
            assert_eq!(
                configs.as_deref(),
                row.configs,
                "case {}: expected the replicated topic's Configs to be {:?}, got {configs:?}",
                row.case,
                row.configs
            );
        }
    }
}

#[tokio::test]
async fn a_replicated_topic_carries_the_source_topic_and_the_target_cluster_s_identity() {
    let emulator = Emulator::new();
    // The target holds ONE broker, which is fewer than the source topic's
    // replication factor: Apache Kafka cannot place more replicas of a partition
    // than the cluster has brokers, which is the bound `CreateTopic` enforces on
    // that cluster too.
    let case = "a replicated topic on a smaller target cluster";
    let source = cluster(&emulator, "identitysource", 3).await;
    let target = cluster(&emulator, "identitytarget", 1).await;
    create_replicator_ok(
        &emulator,
        "identityflow",
        &source,
        &target,
        json!({ "topicsToReplicate": [".*"] }),
        case,
    )
    .await;
    create_topic_ok(&emulator, &source, "orders", 5, 3, Some(SOURCE_CONFIGS), case).await;
    sleep_past_replication();

    let name = format!("{}.orders", alias_of(&source));
    let reply = describe_topic(&emulator, &target, &name).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeTopic({name}) to return 200, got {} {}",
        reply.status, reply.body
    );
    for (member, expected) in [
        ("/topicName", json!(name)),
        ("/topicArn", json!(topic_arn(&target, &name))),
        ("/partitionCount", json!(5)),
        ("/replicationFactor", json!(1)),
        ("/status", json!("ACTIVE")),
        ("/configs", json!(SOURCE_CONFIGS)),
    ] {
        assert_eq!(
            reply.at(member),
            &expected,
            "case {case}: expected {member} to be {expected}, got {}",
            reply.at(member)
        );
    }
    // The ARN the listing reports is the one the describe reports: both readers
    // answer from one derivation.
    let listed = topic_names(&emulator, &target, case).await;
    assert_eq!(
        listed,
        vec![name.clone()],
        "case {case}: expected the target cluster to report [{name}], got {listed:?}"
    );
    // The replicated topic is the target's own: the SOURCE cluster never reports a
    // prefixed name.
    let source_listed = topic_names(&emulator, &source, case).await;
    assert_eq!(
        source_listed,
        vec!["orders".to_string()],
        "case {case}: expected the source cluster to report [orders], got {source_listed:?}"
    );
    // The replicated topic belongs to the target cluster, and the account and
    // region its ARN carries are the target's.
    assert!(
        reply
            .at("/topicArn")
            .as_str()
            .unwrap_or_default()
            .starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:topic/identitytarget/")),
        "case {case}: expected the replicated topic's ARN under the target cluster, got {}",
        reply.body
    );
}

#[tokio::test]
async fn a_name_past_the_kafka_limit_is_not_reported_on_the_target() {
    let emulator = Emulator::new();
    // An Apache Kafka topic name is at most `MAX_TOPIC_NAME_LENGTH` characters
    // (`org.apache.kafka.common.internals.Topic.MAX_NAME_LENGTH`), so a prefixed
    // name past that length is one no cluster could hold — while the same topic
    // under identical naming fits.
    let topic = "o".repeat(MAX_TOPIC_NAME_LENGTH);
    let mut prepared = Vec::new();
    for (index, (case, naming, replicated)) in [
        (
            "a prefixed name past the limit is not reported",
            PREFIXED,
            false,
        ),
        (
            "the same topic under identical naming is reported",
            IDENTICAL,
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let source = cluster(&emulator, &format!("longsource{index}"), 3).await;
        let target = cluster(&emulator, &format!("longtarget{index}"), 3).await;
        create_replicator_ok(
            &emulator,
            &format!("longflow{index}"),
            &source,
            &target,
            json!({
                "topicsToReplicate": [".*"],
                "topicNameConfiguration": { "type": naming },
            }),
            case,
        )
        .await;
        create_topic_ok(&emulator, &source, &topic, 1, 2, None, case).await;
        prepared.push((case, target, replicated));
    }
    sleep_past_replication();

    for (case, target, replicated) in prepared {
        let reported = topic_names(&emulator, &target, case).await;
        let expected: Vec<String> = if replicated {
            vec![topic.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case}: expected the target cluster to report {} topics, got {reported:?}",
            expected.len()
        );
        assert_eq!(
            reported, expected,
            "case {case}: expected the target cluster to report the names {expected:?}, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_target_reports_nothing_before_the_clock_owes_the_propagation() {
    let emulator = Emulator::new();
    let case = "a topic created on the source a moment ago";
    let source = cluster(&emulator, "spansource", 3).await;
    let target = cluster(&emulator, "spantarget", 3).await;
    sleep_past_activation();
    create_replicator_ok(
        &emulator,
        "spanflow",
        &source,
        &target,
        json!({ "topicsToReplicate": [".*"] }),
        case,
    )
    .await;
    create_topic_ok(&emulator, &source, "orders", 1, 2, None, case).await;

    // Neither the replicator's activation nor the propagation is due yet, so the
    // target holds no replicated topic: the span is never zero.
    let name = format!("{}.orders", alias_of(&source));
    let reported = topic_names(&emulator, &target, case).await;
    assert!(
        reported.is_empty(),
        "case {case}: expected the target cluster to report no topic before the propagation \
         is due, got {reported:?}"
    );
    let early = describe_topic(&emulator, &target, &name).await;
    assert_eq!(
        early.status, 404,
        "case {case}: expected DescribeTopic({name}) to return 404 before the propagation is \
         due, got {} {}",
        early.status, early.body
    );

    sleep_past_replication();
    let reported = topic_names(&emulator, &target, case).await;
    assert_eq!(
        reported,
        vec![name.clone()],
        "case {case}: expected the target cluster to report [{name}] once the propagation is \
         due, got {reported:?}"
    );
}

#[tokio::test]
async fn a_replicated_topic_is_withdrawn_with_its_source_topic_and_its_replicator() {
    let emulator = Emulator::new();
    let case = "a flow replicating two topics";
    let source = cluster(&emulator, "teardownsource", 3).await;
    let target = cluster(&emulator, "teardowntarget", 3).await;
    let replicator = create_replicator_ok(
        &emulator,
        "teardownflow",
        &source,
        &target,
        json!({ "topicsToReplicate": [".*"] }),
        case,
    )
    .await;
    for topic in ["orders", "payments"] {
        create_topic_ok(&emulator, &source, topic, 1, 2, None, case).await;
    }
    sleep_past_replication();

    let alias = alias_of(&source);
    let orders = format!("{alias}.orders");
    let payments = format!("{alias}.payments");
    let reported = topic_names(&emulator, &target, case).await;
    assert_eq!(
        reported,
        vec![orders.clone(), payments.clone()],
        "case {case}: expected the target cluster to report both replicated topics, got \
         {reported:?}"
    );

    // Deleting the source topic withdraws what the flow replicated from it, and
    // leaves every other replicated topic alone.
    let deleted = delete_topic(&emulator, &source, "orders").await;
    assert_eq!(
        deleted.status, 200,
        "case {case}: expected DeleteTopic(orders) to return 200, got {} {}",
        deleted.status, deleted.body
    );
    sleep_past_topic_deletion();
    let reported = topic_names(&emulator, &target, case).await;
    assert_eq!(
        reported,
        vec![payments.clone()],
        "case {case}: expected the target cluster to report [{payments}] once the source topic \
         is gone, got {reported:?}"
    );
    let gone = describe_topic(&emulator, &target, &orders).await;
    assert_eq!(
        gone.status, 404,
        "case {case}: expected DescribeTopic({orders}) to return 404 once its source topic is \
         gone, got {} {}",
        gone.status, gone.body
    );

    // Deleting the replicator stops the replication altogether.
    let deleted = delete_replicator(&emulator, &replicator).await;
    assert_eq!(
        deleted.status, 200,
        "case {case}: expected DeleteReplicator to return 200, got {} {}",
        deleted.status, deleted.body
    );
    let reported = topic_names(&emulator, &target, case).await;
    assert!(
        reported.is_empty(),
        "case {case}: expected the target cluster to report nothing once the replicator is \
         deleted, got {reported:?}"
    );
    let gone = describe_topic(&emulator, &target, &payments).await;
    assert_eq!(
        gone.status, 404,
        "case {case}: expected DescribeTopic({payments}) to return 404 once the replicator is \
         deleted, got {} {}",
        gone.status, gone.body
    );
    // The source cluster keeps the topic the flow was replicating.
    let source_reported = topic_names(&emulator, &source, case).await;
    assert_eq!(
        source_reported,
        vec!["payments".to_string()],
        "case {case}: expected the source cluster to still report [payments], got \
         {source_reported:?}"
    );
}

#[tokio::test]
async fn an_updated_filter_decides_what_the_target_reports_next() {
    let emulator = Emulator::new();
    let case = "a flow whose patterns are narrowed by an update";
    let source = cluster(&emulator, "updatesource", 3).await;
    let target = cluster(&emulator, "updatetarget", 3).await;
    let replicator = create_replicator_ok(
        &emulator,
        "updateflow",
        &source,
        &target,
        json!({ "topicsToReplicate": [".*"] }),
        case,
    )
    .await;
    for topic in ["orders", "payments"] {
        create_topic_ok(&emulator, &source, topic, 1, 2, None, case).await;
    }
    sleep_past_replication();

    let alias = alias_of(&source);
    let reported = topic_names(&emulator, &target, case).await;
    assert_eq!(
        reported,
        vec![format!("{alias}.orders"), format!("{alias}.payments")],
        "case {case}: expected the target cluster to report both replicated topics, got \
         {reported:?}"
    );

    let version = describe_replicator(&emulator, &replicator)
        .await
        .at("/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeReplicator reported no currentVersion"))
        .to_string();
    let updated = update_replication_info(
        &emulator,
        &replicator,
        json!({
            "currentVersion": version,
            "sourceKafkaClusterArn": source,
            "targetKafkaClusterArn": target,
            "topicReplication": {
                "topicsToReplicate": ["orders"],
                "topicsToExclude": [],
                "detectAndCopyNewTopics": true,
                "copyTopicConfigurations": true,
                "copyAccessControlListsForTopics": false,
            },
        }),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case}: expected UpdateReplicationInfo to return 200, got {} {}",
        updated.status, updated.body
    );

    let reported = topic_names(&emulator, &target, case).await;
    assert_eq!(
        reported,
        vec![format!("{alias}.orders")],
        "case {case}: expected the target cluster to report only the topic the updated filter \
         selects, got {reported:?}"
    );
}

#[tokio::test]
async fn a_pattern_list_that_selects_no_topics_is_refused() {
    let emulator = Emulator::new();
    let source = cluster(&emulator, "patternsource", 3).await;
    let target = cluster(&emulator, "patterntarget", 3).await;
    let too_many: Vec<String> = (0..=MAX_TOPIC_PATTERNS_PER_FLOW)
        .map(|index| format!("topic-{index}"))
        .collect();
    // One character past the length the model's `__stringMax249` member allows:
    // that bound belongs to the member, so the request never reaches the handler
    // — the boundary validator refuses it with the service's own ONE sentence
    // for a parameter it will not accept, which names no input member.
    let too_long = "o".repeat(MAX_TOPIC_PATTERN_LENGTH + 1);
    // Each row is a pattern list the service could not select topics by: an
    // unparsable regular expression in either member, a pattern longer than the
    // model's member allows, or more patterns than one flow may carry. `member` is
    // the `TopicReplication` member the refusal must name, or `None` where the
    // refusal is the modelled constraint on the member itself.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "an unclosed character class",
            json!({ "topicsToReplicate": ["orders[", ".*"] }),
            Some("TopicsToReplicate"),
        ),
        (
            "an unclosed group",
            json!({ "topicsToReplicate": ["(orders"] }),
            Some("TopicsToReplicate"),
        ),
        (
            "an unparsable exclusion",
            json!({ "topicsToReplicate": [".*"], "topicsToExclude": ["*"] }),
            Some("TopicsToExclude"),
        ),
        (
            "more patterns than a flow may carry",
            json!({ "topicsToReplicate": too_many }),
            Some("TopicsToReplicate"),
        ),
        (
            "a pattern one character past the model's length",
            json!({ "topicsToReplicate": [too_long.clone()] }),
            None,
        ),
        (
            "an exclusion one character past the model's length",
            json!({ "topicsToReplicate": [".*"], "topicsToExclude": [too_long] }),
            None,
        ),
    ];
    for (index, (case, settings, member)) in rows.into_iter().enumerate() {
        let reply = create_replicator(
            &emulator,
            replicator_body(&format!("badpattern{index}"), &source, &target, settings),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected CreateReplicator to return 400, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().contains("BadRequestException"),
            "case {case}: expected a BadRequestException, got {} {}",
            reply.error_code(),
            reply.body
        );
        match member {
            Some(member) => {
                let expected_member =
                    format!("ReplicationInfoList[0].TopicReplication.{member}");
                assert_eq!(
                    reply.at("/invalidParameter").as_str().unwrap_or_default(),
                    expected_member,
                    "case {case}: expected the refusal to name {expected_member}, got {}",
                    reply.body
                );
            }
            None => assert_eq!(
                reply.error_message(),
                super::refusal_wording::PARAMETERS_NOT_VALID,
                "case {case}: expected the service's parameter refusal, got {}",
                reply.body
            ),
        }
    }

    // A refused create registers nothing, so the same flow with a usable pattern
    // list is accepted, and the update admits patterns by the same rule.
    let replicator = create_replicator_ok(
        &emulator,
        "badpattern0",
        &source,
        &target,
        json!({ "topicsToReplicate": [".*"] }),
        "a usable pattern list after the refusals",
    )
    .await;    let version = describe_replicator(&emulator, &replicator)
        .await
        .at("/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let refused = update_replication_info(
        &emulator,
        &replicator,
        json!({
            "currentVersion": version,
            "sourceKafkaClusterArn": source,
            "targetKafkaClusterArn": target,
            "topicReplication": {
                "topicsToReplicate": ["orders["],
                "topicsToExclude": [],
                "detectAndCopyNewTopics": true,
                "copyTopicConfigurations": true,
                "copyAccessControlListsForTopics": false,
            },
        }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case an unparsable pattern in an update: expected 400, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.at("/invalidParameter").as_str().unwrap_or_default(),
        "TopicReplication.TopicsToReplicate",
        "case an unparsable pattern in an update: expected the refusal to name the member, got {}",
        refused.body
    );
}

#[tokio::test]
async fn a_flow_at_both_published_pattern_bounds_is_accepted_and_replicates() {
    let emulator = Emulator::new();
    let case = "the largest pattern list of the longest patterns a flow may carry";
    let source = cluster(&emulator, "boundsource", 3).await;
    let target = cluster(&emulator, "boundtarget", 3).await;
    // Exactly at both bounds: `MAX_TOPIC_PATTERNS_PER_FLOW` patterns, each
    // `MAX_TOPIC_PATTERN_LENGTH` characters (the model's `__stringMax249`). The
    // last of them is the one that selects the source topic, so the row proves the
    // whole list is compiled and matched rather than refused or truncated.
    let mut patterns: Vec<String> = (0..MAX_TOPIC_PATTERNS_PER_FLOW - 1)
        .map(|index| {
            let tail = format!("-{index}");
            format!(
                "{}{tail}",
                "n".repeat(MAX_TOPIC_PATTERN_LENGTH - tail.len())
            )
        })
        .collect();
    let selecting = format!("orders{}", ".".repeat(MAX_TOPIC_PATTERN_LENGTH - "orders".len()));
    patterns.push(selecting);
    for pattern in &patterns {
        assert_eq!(
            pattern.chars().count(),
            MAX_TOPIC_PATTERN_LENGTH,
            "case {case}: every row pattern is at the model's length bound"
        );
    }
    assert_eq!(
        patterns.len(),
        MAX_TOPIC_PATTERNS_PER_FLOW,
        "case {case}: the row carries the largest list a flow may hold"
    );
    create_replicator_ok(
        &emulator,
        "boundflow",
        &source,
        &target,
        json!({
            "topicsToReplicate": patterns,
            // Identical naming, so a 249-character source topic can be reported
            // on the target at all: a prefixed name would pass the Kafka limit.
            "topicNameConfiguration": { "type": IDENTICAL },
        }),
        case,
    )
    .await;
    // The selecting pattern is `orders` followed by `MAX_TOPIC_PATTERN_LENGTH - 6`
    // single-character wildcards, so the topic it selects is `orders` plus that
    // many characters.
    let topic = format!(
        "orders{}",
        "x".repeat(MAX_TOPIC_PATTERN_LENGTH - "orders".len())
    );
    create_topic_ok(&emulator, &source, &topic, 1, 2, None, case).await;
    // A topic one character shorter cannot match a pattern that fixes the length,
    // and no other pattern of the list describes it.
    let unselected = format!(
        "orders{}",
        "x".repeat(MAX_TOPIC_PATTERN_LENGTH - "orders".len() - 1)
    );
    create_topic_ok(&emulator, &source, &unselected, 1, 2, None, case).await;
    sleep_past_replication();

    // The whole list is compiled and matched: the topic the last pattern selects
    // is replicated, and the one no pattern describes is not.
    let reported = topic_names(&emulator, &target, case).await;
    assert_eq!(
        reported,
        vec![topic.clone()],
        "case {case}: expected the target cluster to report only the topic the list's last \
         pattern selects, got {reported:?}"
    );
    let described = describe_topic(&emulator, &target, &topic).await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeTopic({topic}) on the target to return 200, got {} {}",
        described.status, described.body
    );
    let absent = describe_topic(&emulator, &target, &unselected).await;
    assert_eq!(
        absent.status, 404,
        "case {case}: expected DescribeTopic({unselected}) on the target to return 404, got {} {}",
        absent.status, absent.body
    );
}
