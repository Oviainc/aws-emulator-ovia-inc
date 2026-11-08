// SPDX-License-Identifier: CC0-1.0

//! What an operation reads off a cluster whose accepted update has already fallen
//! DUE, driven end to end over the same `MultiServiceRouter` the emulator serves
//! from, through the harness in `tests/all/kafka_harness.rs`.
//!
//! RULE: a cluster update lands at the instant the clock owes it, not when somebody
//! happens to read the cluster, so the FIRST call to reach that cluster after the
//! instant — whichever operation it is — answers from the updated record, and no
//! two operations reading one cluster can disagree about it. The span is never
//! zero, so a call that lands BEFORE that instant still answers from the record as
//! it was.
//!
//! The suites below assert the rule on the values an accepted update changes that
//! ANOTHER operation answers from: the broker count and broker set
//! `UpdateBrokerCount` installs — read by `CreateTopic`'s replication-factor bound
//! and by `DescribeTopicPartitions`' replica placement — and the Apache Kafka
//! version `UpdateClusterKafkaVersion` installs, read by
//! `GetCompatibleKafkaVersions`' upgrade ladder. Every probe is the first call to
//! touch its own cluster since its update fell due, which is what makes it a test
//! of the operation under test rather than of whichever reader happened to settle
//! the record for it first.
//!
//! The rows differ along what the rule generalises over: the direction and size of
//! the change (up by one, up to the published maximum, down to the published
//! minimum, down by four), how many client subnets the broker nodes are placed
//! across, how far along the version ladder the move goes, and which operation
//! reaches the cluster first.

use std::collections::BTreeSet;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    compatible_target_versions, CLUSTER_UPDATE_DELAY, MAX_BROKER_NODES_PER_CLUSTER,
    OPERATION_STATE_COMPLETE,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply,
};

/// The Apache Kafka version the harness's `CreateCluster` row asks for, which every
/// version row below moves AWAY from. Asserted against the created cluster rather
/// than trusted, so this suite cannot drift from the harness.
const SOURCE_VERSION: &str = "3.6.0";

/// The topic whose partitions the placement probe reads back. Created on every
/// cluster of the table BEFORE its update, so what the probe observes is the
/// cluster's broker set and not the topic's own record.
const PLACED_TOPIC: &str = "placement";

/// `CreateCluster` of `brokers` nodes across `subnets`, as the harness builds it,
/// handing back the minted ARN. The cluster is only the context the probes need, so
/// the row is the harness's minimal request with those two members set.
async fn create_cluster(
    emulator: &Emulator,
    name: &str,
    brokers: i64,
    subnets: Vec<&'static str>,
) -> String {
    let request = ClusterRequest {
        brokers,
        subnets,
        ..ClusterRequest::named(name)
    };
    emulator.create_ok(&request).await
}

/// `DescribeCluster`, asserted to have answered.
async fn described(emulator: &Emulator, arn: &str, case: &str) -> Reply {
    let reply = emulator.describe(arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// The `CurrentVersion` of a cluster that has settled `ACTIVE` — the state an
/// update is accepted from, and the token it must be submitted with.
async fn settled_version(emulator: &Emulator, arn: &str, case: &str) -> String {
    let reply = described(emulator, arn, case).await;
    assert_eq!(
        reply.at("/clusterInfo/state").as_str(),
        Some("ACTIVE"),
        "case {case}: expected the new cluster to settle ACTIVE, got {}",
        reply.body
    );
    reply
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeCluster reported no currentVersion"))
        .to_string()
}

/// `UpdateBrokerCount`: `PUT /v1/clusters/{ClusterArn}/nodes/count`.
async fn update_broker_count(
    emulator: &Emulator,
    cluster_arn: &str,
    current_version: &str,
    target: i64,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/nodes/count", pct(cluster_arn)),
            Some(json!({
                "currentVersion": current_version,
                "targetNumberOfBrokerNodes": target,
            })),
        )
        .await
}

/// `UpdateClusterKafkaVersion`: `PUT /v1/clusters/{ClusterArn}/version`.
async fn update_kafka_version(
    emulator: &Emulator,
    cluster_arn: &str,
    current_version: &str,
    target: &str,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/version", pct(cluster_arn)),
            Some(json!({
                "currentVersion": current_version,
                "targetKafkaVersion": target,
            })),
        )
        .await
}

/// The `ClusterOperationArn` an accepted update answers with — what the caller
/// polls, and what this suite reads back to confirm the operation it waited for is
/// the one that landed.
fn accepted_operation_arn(reply: &Reply, case: &str) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected the update to be accepted with 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/clusterOperationArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the accepted update reported no operation ARN"))
        .to_string()
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}`.
async fn describe_operation(emulator: &Emulator, operation_arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v1/operations/{}", pct(operation_arn)),
            None,
        )
        .await
}

/// `CreateTopic`: `POST /v1/clusters/{ClusterArn}/topics`.
async fn create_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    topic: &str,
    partitions: i64,
    replication: i64,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            Some(json!({
                "topicName": topic,
                "partitionCount": partitions,
                "replicationFactor": replication,
            })),
        )
        .await
}

/// `DescribeTopicPartitions`:
/// `GET /v1/clusters/{ClusterArn}/topics/{TopicName}/partitions`.
async fn describe_partitions(emulator: &Emulator, cluster_arn: &str, topic: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/v1/clusters/{}/topics/{}/partitions",
                pct(cluster_arn),
                pct(topic)
            ),
            None,
        )
        .await
}

/// `GetCompatibleKafkaVersions` for one cluster:
/// `GET /v1/compatible-kafka-versions?clusterArn=<arn>`.
async fn compatible_versions(emulator: &Emulator, cluster_arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/v1/compatible-kafka-versions?clusterArn={}",
                pct(cluster_arn)
            ),
            None,
        )
        .await
}

/// One row of the broker-count table: the count the cluster is created with, the
/// count the update moves it to, and the client subnets its nodes are placed
/// across.
#[derive(Debug, Clone)]
struct CountRow {
    case: &'static str,
    before: i64,
    after: i64,
    subnets: Vec<&'static str>,
}

impl CountRow {
    /// The replication factor the `CreateTopic` probe asks for: the LARGER of the
    /// two counts, so one factor is placeable under exactly one of them and the row
    /// observes the change whichever direction it goes in.
    fn probe_factor(&self) -> i64 {
        self.before.max(self.after)
    }

    /// How many partitions the row's topic holds: one per broker at the cluster's
    /// widest, so every broker leads at least one partition and the set of leaders
    /// IS the broker set the reader placed them over.
    fn partitions(&self) -> i64 {
        self.before.max(self.after)
    }
}

/// The broker-count table. The bounds are the service's published ones:
/// `TargetNumberOfBrokerNodes` is modelled `__integerMin1Max15`, which
/// [`MAX_BROKER_NODES_PER_CLUSTER`] names, so the rows move up to that maximum and
/// down to the minimum of one as well as by one and by four.
fn count_rows() -> Vec<CountRow> {
    vec![
        CountRow {
            case: "up-by-one",
            before: 3,
            after: 4,
            subnets: vec!["subnet-a", "subnet-b", "subnet-c"],
        },
        CountRow {
            case: "up-to-the-published-maximum",
            before: 3,
            after: i64::from(MAX_BROKER_NODES_PER_CLUSTER),
            subnets: vec!["subnet-a", "subnet-b", "subnet-c"],
        },
        CountRow {
            case: "down-to-the-published-minimum",
            before: 4,
            after: 1,
            subnets: vec!["subnet-a"],
        },
        CountRow {
            case: "down-by-four",
            before: 6,
            after: 2,
            subnets: vec!["subnet-a", "subnet-b"],
        },
    ]
}

/// Which operation is the first to reach a cluster after its update falls due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstCall {
    /// `CreateTopic`, which bounds `ReplicationFactor` by the brokers the cluster
    /// has to place replicas on.
    CreateTopic,
    /// `DescribeTopicPartitions`, which places every partition's replicas over the
    /// brokers the cluster holds.
    DescribeTopicPartitions,
}

impl FirstCall {
    fn name(self) -> &'static str {
        match self {
            Self::CreateTopic => "create-topic",
            Self::DescribeTopicPartitions => "describe-topic-partitions",
        }
    }
}

/// Ask `first` what the cluster's brokers are, as (whether its answer is the one a
/// cluster of `brokers` nodes gives, what it actually answered).
///
/// `CreateTopic` admits a replication factor that count can place and refuses one
/// above it naming the count in force; `DescribeTopicPartitions` leads and
/// replicates every partition on the ids that count spans. The verdict is handed
/// back rather than asserted here so a caller can decide whether the probe landed
/// inside the update's span, which is only known once the call has returned.
///
/// `slug` names the phase and seeds the topic a `CreateTopic` probe mints, so two
/// probes of one cluster never collide on a name.
async fn probe_broker_count(
    emulator: &Emulator,
    first: FirstCall,
    row: &CountRow,
    slug: &str,
    arn: &str,
    brokers: i64,
) -> (bool, String) {
    let held: BTreeSet<i64> = (1..=brokers).collect();
    match first {
        FirstCall::CreateTopic => {
            let factor = row.probe_factor();
            let reply = create_topic(emulator, arn, &format!("probe-{slug}"), 1, factor).await;
            let agrees = if factor <= brokers {
                reply.status == 200
            } else {
                reply.status == 400
                    && reply.error_code() == "BadRequestException"
                    && reply
                        .error_message()
                        .contains(&format!("available brokers: {brokers}"))
            };
            (
                agrees,
                format!(
                    "CreateTopic with replicationFactor {factor} answered {} {}",
                    reply.status, reply.body
                ),
            )
        }
        FirstCall::DescribeTopicPartitions => {
            let reply = describe_partitions(emulator, arn, PLACED_TOPIC).await;
            let partitions = reply
                .at("/partitions")
                .as_array()
                .cloned()
                .unwrap_or_default();
            let leaders: BTreeSet<i64> = partitions
                .iter()
                .filter_map(|partition| partition.get("leader").and_then(Value::as_i64))
                .collect();
            let replicas: BTreeSet<i64> = partitions
                .iter()
                .filter_map(|partition| partition.get("replicas").and_then(Value::as_array))
                .flatten()
                .filter_map(Value::as_i64)
                .collect();
            let agrees = reply.status == 200
                && i64::try_from(partitions.len()).unwrap_or(i64::MAX) == row.partitions()
                && leaders == held
                && !replicas.is_empty()
                && replicas.is_subset(&held);
            (
                agrees,
                format!(
                    "DescribeTopicPartitions answered {} {}, leading on {leaders:?} and \
                     replicating on {replicas:?}",
                    reply.status, reply.body
                ),
            )
        }
    }
}

/// RULE: the broker nodes an accepted `UpdateBrokerCount` installs are the cluster's
/// own record from the instant the clock owes that operation its completion, so the
/// first call to reach the cluster afterwards — `CreateTopic` bounding a replication
/// factor, or `DescribeTopicPartitions` placing replicas — answers from the new
/// count, agrees with what `DescribeCluster` reports, and a call that lands inside
/// the span still answers from the old one.
#[tokio::test]
async fn a_due_broker_count_update_is_what_the_next_topic_call_reads() {
    let emulator = Emulator::new();
    let rows = count_rows();

    // PHASE 1 — one cluster per (row, first call): each probe has to be the FIRST
    // call to reach its own cluster after the update falls due, and settling is
    // scoped to the one cluster a call addresses, so the clusters do not interfere.
    let mut cases = Vec::new();
    for row in &rows {
        for first in [FirstCall::CreateTopic, FirstCall::DescribeTopicPartitions] {
            let case = format!("{}-{}", row.case, first.name());
            let arn = create_cluster(&emulator, &case, row.before, row.subnets.clone()).await;
            cases.push((row.clone(), first, case, arn));
        }
    }
    // One wait for the whole table: each cluster was owed its `ACTIVE` at the span
    // its own create stored, whether or not anybody read it.
    sleep_past_activation();

    // PHASE 2 — the topic the placement probe reads, created while the cluster still
    // holds `before` brokers, and then the update itself.
    let mut started = Vec::new();
    for (row, first, case, arn) in &cases {
        let created = create_topic(&emulator, arn, PLACED_TOPIC, row.partitions(), 1).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateTopic to return 200, got {} {}",
            created.status, created.body
        );
        let version = settled_version(&emulator, arn, case).await;
        let before_update = Instant::now();
        let accepted = update_broker_count(&emulator, arn, &version, row.after).await;
        let operation_arn = accepted_operation_arn(&accepted, case);

        // The span is never zero: a call that lands inside it still answers from the
        // brokers the cluster held before. Judged against the MEASURED elapsed time
        // once the probe has returned, so a machine slow enough to have passed the
        // stored span before the probe landed fails nothing for a scheduling reason.
        let (agrees, observed) =
            probe_broker_count(&emulator, *first, row, "in-span", arn, row.before).await;
        let elapsed = before_update.elapsed();
        if elapsed < CLUSTER_UPDATE_DELAY {
            assert!(
                agrees,
                "case {case}: probed {elapsed:?} after the update, inside the stored \
                 {CLUSTER_UPDATE_DELAY:?} span, the cluster still holds {} brokers, but \
                 {observed}",
                row.before
            );
        }
        started.push((row.clone(), *first, case.clone(), arn.clone(), operation_arn));
    }

    // One wait again: every update came due at the span its own call stored.
    sleep_past_update();

    // PHASE 3 — the first call to reach each cluster since its update fell due.
    for (row, first, case, arn, operation_arn) in &started {
        let (agrees, observed) =
            probe_broker_count(&emulator, *first, row, "due", arn, row.after).await;
        assert!(
            agrees,
            "case {case}: once the update was due the cluster holds {} brokers, so the first \
             call to reach it must answer from them, but {observed}",
            row.after
        );

        // ... and every other reader of that cluster agrees with it.
        let after = described(&emulator, arn, case).await;
        assert_eq!(
            after.at("/clusterInfo/numberOfBrokerNodes").as_i64(),
            Some(row.after),
            "case {case}: expected DescribeCluster to report the {} brokers the update \
             installed, got {}",
            row.after,
            after.body
        );
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: expected the cluster to settle ACTIVE again, got {}",
            after.body
        );
        let operation = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            operation
                .at("/clusterOperationInfo/operationState")
                .as_str(),
            Some(OPERATION_STATE_COMPLETE),
            "case {case}: expected the operation the probe waited for to have completed, got {}",
            operation.body
        );
    }
}

/// RULE: the Apache Kafka version an accepted `UpdateClusterKafkaVersion` installs is
/// the cluster's own record from the instant the clock owes that operation its
/// completion, so `GetCompatibleKafkaVersions` — whose answer is derived from the
/// version the cluster RUNS — publishes the ladder of the new version as soon as it
/// is due, agrees with what `DescribeCluster` reports, and publishes the old
/// version's ladder while the update is still in flight.
#[tokio::test]
async fn a_due_version_update_is_what_the_next_compatible_versions_read_reports() {
    let emulator = Emulator::new();
    // The rows are moves the service's OWN ladder offers from the version the
    // harness creates a cluster at: `compatible_target_versions` derives it from
    // `KAFKA_VERSION_CATALOG`, so a row can never ask for a move the service does
    // not serve. One step along the line, two, and three.
    let offered = compatible_target_versions(SOURCE_VERSION);
    let targets = ["3.7.x", "3.8.x", "3.9.x"];
    for target in &targets {
        assert!(
            offered.iter().any(|version| version == target),
            "case {target}: the ladder from {SOURCE_VERSION} must offer it, and offers {offered:?}"
        );
    }

    // PHASE 1 — one cluster per target, for the same reason the broker-count table
    // takes one per probe: each read has to be the first to reach its own cluster.
    let mut cases = Vec::new();
    for target in &targets {
        let case = format!("to-{target}");
        let arn = create_cluster(
            &emulator,
            &case,
            3,
            vec!["subnet-a", "subnet-b", "subnet-c"],
        )
        .await;
        cases.push((*target, case, arn));
    }
    sleep_past_activation();

    // PHASE 2 — the version each cluster starts at, and then the update.
    let mut started = Vec::new();
    for (target, case, arn) in &cases {
        let version = settled_version(&emulator, arn, case).await;
        let before = described(&emulator, arn, case).await;
        assert_eq!(
            before
                .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                .as_str(),
            Some(SOURCE_VERSION),
            "case {case}: this suite's rows move away from {SOURCE_VERSION}, and the cluster \
             reports {}",
            before.at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
        );

        let before_update = Instant::now();
        let accepted = update_kafka_version(&emulator, arn, &version, target).await;
        let operation_arn = accepted_operation_arn(&accepted, case);

        // Inside the span the cluster still runs the version it ran, so the ladder is
        // still that version's. Judged against the measured elapsed time, so a slow
        // machine fails nothing for a scheduling reason.
        let published = compatible_versions(&emulator, arn).await;
        let elapsed = before_update.elapsed();
        if elapsed < CLUSTER_UPDATE_DELAY {
            assert_eq!(
                published.at("/compatibleKafkaVersions/0/sourceVersion").as_str(),
                Some(SOURCE_VERSION),
                "case {case}: read {elapsed:?} after the update, inside the stored \
                 {CLUSTER_UPDATE_DELAY:?} span, the ladder is still {SOURCE_VERSION}'s, got {}",
                published.body
            );
        }
        started.push((*target, case.clone(), arn.clone(), operation_arn));
    }

    sleep_past_update();

    // PHASE 3 — the first call to reach each cluster since its update fell due.
    for (target, case, arn, operation_arn) in &started {
        let published = compatible_versions(&emulator, arn).await;
        assert_eq!(
            published.status, 200,
            "case {case}: expected GetCompatibleKafkaVersions to return 200, got {} {}",
            published.status, published.body
        );
        assert_eq!(
            published
                .at("/compatibleKafkaVersions/0/sourceVersion")
                .as_str(),
            Some(*target),
            "case {case}: expected the ladder of the version the update installed, got {}",
            published.body
        );
        let reported: Vec<String> = published
            .at("/compatibleKafkaVersions/0/targetVersions")
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported,
            compatible_target_versions(target),
            "case {case}: expected the targets the service's own ladder offers from {target}, \
             got {reported:?}"
        );

        // ... and the cluster reader agrees about the version it runs.
        let after = described(&emulator, arn, case).await;
        assert_eq!(
            after
                .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                .as_str(),
            Some(*target),
            "case {case}: expected DescribeCluster to report the version the update installed, \
             got {}",
            after.body
        );
        let operation = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            operation
                .at("/clusterOperationInfo/operationState")
                .as_str(),
            Some(OPERATION_STATE_COMPLETE),
            "case {case}: expected the operation the read waited for to have completed, got {}",
            operation.body
        );
    }
}
