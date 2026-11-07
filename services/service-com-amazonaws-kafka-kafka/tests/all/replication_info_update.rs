// SPDX-License-Identifier: CC0-1.0

//! `UpdateReplicationInfo`: changing the replication a replicator runs between
//! one of its configured source -> target pairs, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from.
//!
//! Each suite asserts a RULE over a TABLE of inputs, not one recorded request. The
//! rows differ along what the rule generalises over: which of a replicator's
//! replications the call addresses, how each end is named (an Amazon MSK cluster by
//! ARN, a self-managed Apache Kafka cluster by id), the direction of the addressed
//! pair, which of the two update shapes the request carries, the boolean settings
//! and the pattern lists they set, whether the request also changes log delivery,
//! and the signing region.
//!
//! The HTTP harness comes from [`crate::kafka_harness`] — the same router, the same
//! SigV4-scoped request builder — rather than a second one built here.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::REPLICATOR_ACTIVATION_DELAY;

use crate::kafka_harness::{pct, ClusterRequest, Emulator, Reply, ACCOUNT, REGION};

/// The IAM role every replicator below is created with. `ServiceExecutionRoleArn`
/// is a plain `__string` in the model, so the service stores and reports it
/// verbatim.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// The cluster-id segment of the MSK cluster ARNs the rows below point at where no
/// cluster is meant to be there — a stranger to the replicator under test.
const GHOST_CLUSTER_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// The message MSK refuses an update whose pair of Kafka cluster aliases addresses
/// no configured replication with. Written out here rather than read from the
/// handler's own constant, so the wording is pinned by the suite instead of by the
/// code under test.
const ABSENT_MESSAGE: &str =
    "The replication info specified with the kafka cluster aliases does not exist.";

/// The message a replicator operation refuses an ARN it cannot resolve with: the
/// input member at fault, then the service's fixed not-found sentence (whose
/// apostrophe is U+2019).
const NOT_FOUND_MESSAGE: &str =
    "Invalid parameter replicatorArn: The requested resource doesn\u{2019}t exist.";

/// Sleep past the emulator's own span into `RUNNING`, so a later read sees a
/// settled replicator. `std::thread::sleep` because the workspace's tokio carries
/// no timer feature; nothing else has to make progress here.
fn sleep_past_running() {
    std::thread::sleep(REPLICATOR_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// One end of a replication: the Kafka cluster it runs from or to, in whichever of
/// the two ways a `KafkaCluster` can be named.
#[derive(Debug, Clone)]
enum End {
    /// An Amazon MSK cluster, named by ARN.
    Msk(String),
    /// A self-managed Apache Kafka cluster, named by the id the caller gave it.
    Apache(String),
}

impl End {
    /// An Amazon MSK cluster this suite CREATES, named by the ARN the service
    /// minted for it.
    ///
    /// A real cluster, not a fabricated ARN: Amazon MSK provisions a replicator
    /// only once the Amazon MSK clusters its setup names resolve, and a replicator
    /// that never provisions converges to the terminal `FAILED` — which is not a
    /// state a configuration change is accepted from. Every suite here is about
    /// the UPDATE, so its replicators are ones that reach `RUNNING`; the
    /// unresolvable-cluster path is `crate::replicator_provisioning`.
    async fn msk(emulator: &Emulator, name: &str) -> Self {
        End::Msk(emulator.create_ok(&ClusterRequest::named(name)).await)
    }

    fn apache(id: &str) -> Self {
        End::Apache(id.to_string())
    }

    /// The alias this cluster is reported under, from
    /// `KafkaClusterDescription$KafkaClusterAlias`: MSK DERIVES an Amazon MSK
    /// cluster's alias from its ARN — the cluster name truncated to 28 characters,
    /// a hyphen, and the final group of the uuid in the ARN's id segment — while an
    /// Apache Kafka cluster is aliased by the id the caller gave it. Spelled out
    /// here rather than built from the service's own helper, so the form is pinned
    /// by the suite instead of by the code under test.
    fn alias(&self) -> String {
        match self {
            End::Msk(arn) => {
                let mut segments = arn
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
                format!("{name}-{}", id.split('-').nth(4).unwrap_or_default())
            }
            End::Apache(id) => id.clone(),
        }
    }

    /// This cluster as one `KafkaClusters` entry of a `CreateReplicator` request.
    fn cluster_json(&self) -> Value {
        match self {
            End::Msk(arn) => json!({
                "amazonMskCluster": { "mskClusterArn": arn },
                "vpcConfig": {
                    "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
                    "securityGroupIds": ["sg-aaaaaaaaaaaaaaaaa"],
                },
            }),
            End::Apache(id) => json!({
                "apacheKafkaCluster": {
                    "apacheKafkaClusterId": id,
                    "bootstrapBrokerString": format!("b-1.{id}.example.internal:9092"),
                },
                "vpcConfig": {
                    "subnetIds": ["subnet-ccccccccccccccccc", "subnet-ddddddddddddddddd"],
                },
                "encryptionInTransit": { "encryptionType": "TLS" },
            }),
        }
    }

    /// The member that names this cluster at `end` (`"source"` or `"target"`) of a
    /// `ReplicationInfo` or an `UpdateReplicationInfo` request, and its value: an
    /// MSK cluster is named by ARN, an Apache Kafka cluster by its cluster id.
    fn flow_member(&self, end: &str) -> (String, Value) {
        match self {
            End::Msk(arn) => (format!("{end}KafkaClusterArn"), json!(arn)),
            End::Apache(id) => (format!("{end}KafkaClusterId"), json!(id)),
        }
    }
}

/// A replicator to stand up: its name, the clusters it spans, and the replications
/// it is created with, each an ordered pair of indices into those clusters plus a
/// `TargetCompressionType`.
#[derive(Debug, Clone)]
struct Setup {
    name: String,
    clusters: Vec<End>,
    flows: Vec<(usize, usize, &'static str)>,
}

impl Setup {
    /// Two MSK clusters and one replication between them — the minimal setup.
    async fn named(emulator: &Emulator, name: &str) -> Self {
        Self {
            name: name.to_string(),
            clusters: vec![
                End::msk(emulator, &format!("{name}-source")).await,
                End::msk(emulator, &format!("{name}-target")).await,
            ],
            flows: vec![(0, 1, "NONE")],
        }
    }

    /// A setup with the clusters and replications given, under `name`.
    fn spanning(name: &str, clusters: Vec<End>, flows: Vec<(usize, usize, &'static str)>) -> Self {
        Self {
            name: name.to_string(),
            clusters,
            flows,
        }
    }

    /// The `ReplicationInfo` the create submits for `flow`. Every member the two
    /// update shapes can later set is submitted here with a value an update can be
    /// seen to change, and the two members they cannot set (`StartingPosition`,
    /// `TopicNameConfiguration`) are submitted so the suite can see them survive.
    fn flow_json(&self, flow: usize) -> Value {
        let (source, target, compression) = self.flows[flow];
        let mut info = json!({
            "targetCompressionType": compression,
            "consumerGroupReplication": {
                "consumerGroupsToReplicate": ["created-.*"],
                "consumerGroupsToExclude": ["console-consumer-.*"],
                "detectAndCopyNewConsumerGroups": false,
                "synchroniseConsumerGroupOffsets": false,
            },
            "topicReplication": {
                "topicsToReplicate": ["created-.*"],
                "topicsToExclude": ["__.*"],
                "detectAndCopyNewTopics": false,
                "copyTopicConfigurations": false,
                "copyAccessControlListsForTopics": false,
                "startingPosition": { "type": "LATEST" },
                "topicNameConfiguration": { "type": "PREFIXED_WITH_SOURCE_CLUSTER_ALIAS" },
            },
        });
        for (end, index) in [("source", source), ("target", target)] {
            let (member, value) = self.clusters[index].flow_member(end);
            info[member] = value;
        }
        info
    }

    fn to_json(&self) -> Value {
        json!({
            "replicatorName": self.name,
            "serviceExecutionRoleArn": ROLE_ARN,
            "kafkaClusters": self
                .clusters
                .iter()
                .map(End::cluster_json)
                .collect::<Vec<_>>(),
            "replicationInfoList": (0..self.flows.len())
                .map(|flow| self.flow_json(flow))
                .collect::<Vec<_>>(),
        })
    }

    /// The `ReplicatorState` the clock owes a replicator of this setup once the
    /// activation span has passed: `RUNNING`, because every Amazon MSK cluster a
    /// setup here names is one this suite created, so the replicator provisions.
    /// The terminal `FAILED` a setup naming a cluster that does not exist converges
    /// to is `crate::replicator_provisioning`'s rule.
    fn settled_state(&self) -> &'static str {
        "RUNNING"
    }
}

/// The `TopicReplicationUpdate` a row submits: every member of that shape, each of
/// which the model marks required.
#[derive(Debug, Clone)]
struct TopicUpdate {
    to_replicate: Vec<&'static str>,
    to_exclude: Vec<&'static str>,
    detect_new: bool,
    copy_configurations: bool,
    copy_acls: bool,
}

impl TopicUpdate {
    fn to_json(&self) -> Value {
        json!({
            "topicsToReplicate": self.to_replicate,
            "topicsToExclude": self.to_exclude,
            "detectAndCopyNewTopics": self.detect_new,
            "copyTopicConfigurations": self.copy_configurations,
            "copyAccessControlListsForTopics": self.copy_acls,
        })
    }
}

/// The `ConsumerGroupReplicationUpdate` a row submits: every member of that shape.
#[derive(Debug, Clone)]
struct ConsumerUpdate {
    to_replicate: Vec<&'static str>,
    to_exclude: Vec<&'static str>,
    detect_new: bool,
    synchronise_offsets: bool,
}

impl ConsumerUpdate {
    fn to_json(&self) -> Value {
        json!({
            "consumerGroupsToReplicate": self.to_replicate,
            "consumerGroupsToExclude": self.to_exclude,
            "detectAndCopyNewConsumerGroups": self.detect_new,
            "synchroniseConsumerGroupOffsets": self.synchronise_offsets,
        })
    }
}

/// One row of the accepted-update table: the replicator to stand up, which of its
/// replications the update addresses, and what the update carries.
#[derive(Debug, Clone)]
struct UpdateCase {
    setup: Setup,
    /// The index into `setup.flows` the update addresses.
    flow: usize,
    topic: Option<TopicUpdate>,
    consumers: Option<ConsumerUpdate>,
    /// A `LogDelivery` document, or `None` when the update carries none.
    log_delivery: Option<Value>,
}

impl UpdateCase {
    /// The request body: the version the replicator is at, the pair of clusters the
    /// addressed replication runs between, and whichever update shapes the row
    /// carries.
    fn body(&self, current_version: &str) -> Value {
        let (source, target, _) = self.setup.flows[self.flow];
        let mut body = json!({ "currentVersion": current_version });
        for (end, index) in [("source", source), ("target", target)] {
            let (member, value) = self.setup.clusters[index].flow_member(end);
            body[member] = value;
        }
        if let Some(topic) = &self.topic {
            body["topicReplication"] = topic.to_json();
        }
        if let Some(consumers) = &self.consumers {
            body["consumerGroupReplication"] = consumers.to_json();
        }
        if let Some(log_delivery) = &self.log_delivery {
            body["logDelivery"] = log_delivery.clone();
        }
        body
    }

    /// The `ReplicationInfoDescription` the reader must report for the addressed
    /// replication after this update: the created configuration with exactly the
    /// members the submitted update shapes carry replaced, each end named by alias.
    fn expected_flow(&self) -> Value {
        let (source, target, compression) = self.setup.flows[self.flow];
        let created = self.setup.flow_json(self.flow);
        let mut topic = created["topicReplication"].clone();
        if let Some(update) = &self.topic {
            let submitted = update.to_json();
            for member in [
                "topicsToReplicate",
                "topicsToExclude",
                "detectAndCopyNewTopics",
                "copyTopicConfigurations",
                "copyAccessControlListsForTopics",
            ] {
                topic[member] = submitted[member].clone();
            }
        }
        let mut consumers = created["consumerGroupReplication"].clone();
        if let Some(update) = &self.consumers {
            let submitted = update.to_json();
            for member in [
                "consumerGroupsToReplicate",
                "consumerGroupsToExclude",
                "detectAndCopyNewConsumerGroups",
                "synchroniseConsumerGroupOffsets",
            ] {
                consumers[member] = submitted[member].clone();
            }
        }
        // The one member neither shape carries, which the service filled in when
        // the configuration was created and no update can change.
        consumers["consumerGroupOffsetSyncMode"] = json!("LEGACY");
        json!({
            "consumerGroupReplication": consumers,
            "sourceKafkaClusterAlias": self.setup.clusters[source].alias(),
            "targetCompressionType": compression,
            "targetKafkaClusterAlias": self.setup.clusters[target].alias(),
            "topicReplication": topic,
        })
    }

    /// The `ReplicationInfoDescription` every replication this update does NOT
    /// address must still be reported as: what the create submitted, with the
    /// service's own defaults applied to it.
    fn expected_untouched(&self, flow: usize) -> Value {
        let (source, target, compression) = self.setup.flows[flow];
        let created = self.setup.flow_json(flow);
        let mut consumers = created["consumerGroupReplication"].clone();
        consumers["consumerGroupOffsetSyncMode"] = json!("LEGACY");
        json!({
            "consumerGroupReplication": consumers,
            "sourceKafkaClusterAlias": self.setup.clusters[source].alias(),
            "targetCompressionType": compression,
            "targetKafkaClusterAlias": self.setup.clusters[target].alias(),
            "topicReplication": created["topicReplication"],
        })
    }
}

/// `CreateReplicator` for `setup`, signed for `REGION`, asserting it succeeded and
/// handing back the minted ARN.
async fn create_ok(emulator: &Emulator, setup: &Setup) -> String {
    let reply = emulator
        .call("POST", "/replication/v1/replicators", Some(setup.to_json()))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {setup:?}: expected CreateReplicator to return 200, got {} {}",
        reply.status, reply.body
    );
    reply.body["replicatorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {setup:?}: CreateReplicator returned no replicatorArn"))
        .to_string()
}

/// `DescribeReplicator` for `arn`, signed for `region`.
async fn describe_in(emulator: &Emulator, region: &str, arn: &str) -> Reply {
    emulator
        .call_in(
            region,
            "GET",
            &format!("/replication/v1/replicators/{}", pct(arn)),
            None,
        )
        .await
}

async fn describe(emulator: &Emulator, arn: &str) -> Reply {
    describe_in(emulator, REGION, arn).await
}

/// `UpdateReplicationInfo` for `arn` with `body`, signed for `region` — the model
/// binds it to `PUT /replication/v1/replicators/{ReplicatorArn}/replication-info`.
async fn update_in(emulator: &Emulator, region: &str, arn: &str, body: Value) -> Reply {
    emulator
        .call_in(
            region,
            "PUT",
            &format!(
                "/replication/v1/replicators/{}/replication-info",
                pct(arn)
            ),
            Some(body),
        )
        .await
}

async fn update(emulator: &Emulator, arn: &str, body: Value) -> Reply {
    update_in(emulator, REGION, arn, body).await
}

/// The `CurrentVersion` the reader reports for `arn`.
async fn current_version(emulator: &Emulator, arn: &str, case: &str) -> String {
    let reply = describe(emulator, arn).await;
    reply
        .at("/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeReplicator reported no currentVersion"))
        .to_string()
}

/// An MSK `CurrentVersion` token: `K` followed by 13 upper-case alphanumerics.
fn assert_version_token(token: &str, case: &str) {
    assert!(
        token.len() == 14
            && token.starts_with('K')
            && token[1..]
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()),
        "case {case}: expected a K-prefixed 14-character version token, got {token:?}"
    );
}

/// RULE: `UpdateReplicationInfo` changes the replication the pair of Kafka cluster
/// aliases in the request addresses — setting exactly the members the update shapes
/// carry, leaving every other member of that replication and every other
/// replication as they were — and moves the replicator into the non-terminal
/// `UPDATING` state with a new `CurrentVersion`, all of it visible to an
/// independent `DescribeReplicator`.
#[tokio::test]
async fn an_addressed_replication_is_updated_and_read_back() {
    // The rows differ along everything the rule generalises over: which
    // replication of the replicator is addressed (its only one, the second of two,
    // the reverse direction of a bidirectional pair), how each end is named (MSK
    // ARN, Apache Kafka id, and a mix), which update shapes the request carries
    // (both, topics only, consumer groups only), the booleans they set (all on,
    // all off), the pattern lists (one pattern, several, and empty), and whether
    // the request also replaces log delivery.
    let emulator = Emulator::new();
    let rows = vec![
        UpdateCase {
            setup: Setup::named(&emulator, "single-flow").await,
            flow: 0,
            topic: Some(TopicUpdate {
                to_replicate: vec!["t-.*"],
                to_exclude: vec![],
                detect_new: true,
                copy_configurations: true,
                copy_acls: false,
            }),
            consumers: None,
            log_delivery: None,
        },
        UpdateCase {
            setup: Setup::spanning(
                "two-flows",
                vec![
                    End::msk(&emulator, "two-flows-msk").await,
                    End::apache("two-flows-apache"),
                ],
                vec![(1, 0, "GZIP"), (0, 1, "LZ4")],
            ),
            // The SECOND replication, so the lookup is by pair and not "the first
            // one" — and its target is named by an Apache Kafka cluster id.
            flow: 1,
            topic: Some(TopicUpdate {
                to_replicate: vec!["orders\\..*", "payments\\..*"],
                to_exclude: vec!["__.*", "internal\\..*"],
                detect_new: true,
                copy_configurations: true,
                copy_acls: true,
            }),
            consumers: Some(ConsumerUpdate {
                to_replicate: vec![".*"],
                to_exclude: vec![],
                detect_new: true,
                synchronise_offsets: true,
            }),
            log_delivery: Some(json!({
                "replicatorLogDelivery": {
                    "cloudWatchLogs": { "enabled": true, "logGroup": "/aws/msk/replicator" },
                },
            })),
        },
        UpdateCase {
            setup: Setup::spanning(
                "bidirectional",
                vec![
                    End::apache("left-cluster"),
                    End::msk(&emulator, "right-cluster").await,
                ],
                vec![(0, 1, "NONE"), (1, 0, "ZSTD")],
            ),
            // The pair is ORDERED: this addresses the reverse replication of a
            // setup that configures both directions between the same clusters.
            flow: 1,
            topic: None,
            consumers: Some(ConsumerUpdate {
                to_replicate: vec!["svc-.*"],
                to_exclude: vec!["svc-tmp-.*"],
                detect_new: false,
                synchronise_offsets: true,
            }),
            log_delivery: None,
        },
        UpdateCase {
            setup: Setup::spanning(
                "apache-only",
                vec![
                    End::apache("self-managed-a"),
                    End::apache("self-managed-b"),
                ],
                vec![(0, 1, "SNAPPY")],
            ),
            flow: 0,
            topic: Some(TopicUpdate {
                to_replicate: vec![".*"],
                to_exclude: vec!["__.*"],
                detect_new: false,
                copy_configurations: false,
                copy_acls: false,
            }),
            consumers: Some(ConsumerUpdate {
                to_replicate: vec!["g-.*"],
                to_exclude: vec![],
                detect_new: false,
                synchronise_offsets: false,
            }),
            log_delivery: Some(json!({
                "replicatorLogDelivery": {
                    "s3": { "enabled": true, "bucket": "msk-replicator-logs", "prefix": "repl/" },
                },
            })),
        },
    ];

    for row in &rows {
        let case = row.setup.name.clone();
        let arn = create_ok(&emulator, &row.setup).await;
        sleep_past_running();
        let before = current_version(&emulator, &arn, &case).await;

        let reply = update(&emulator, &arn, row.body(&before)).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (200, ""),
            "case {case}: expected UpdateReplicationInfo to answer 200, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/replicatorArn").as_str(),
            Some(arn.as_str()),
            "case {case}: the update must report the replicator it changed, got {}",
            reply.at("/replicatorArn")
        );
        // A replicator rolling a configuration change out is non-terminal: AWS
        // reports UPDATING until the change is live.
        assert_eq!(
            reply.at("/replicatorState").as_str(),
            Some("UPDATING"),
            "case {case}: expected replicatorState UPDATING, got {}",
            reply.at("/replicatorState")
        );

        // The change is visible to an independent reader, and only where it
        // belongs: the addressed replication carries the submitted members, every
        // other replication is untouched, and the version has moved on.
        let described = describe(&emulator, &arn).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected DescribeReplicator to return 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.at(&format!("/replicationInfoList/{}", row.flow)),
            &row.expected_flow(),
            "case {case}: the addressed replication must carry the update, got {}",
            described.at(&format!("/replicationInfoList/{}", row.flow))
        );
        for other in (0..row.setup.flows.len()).filter(|flow| *flow != row.flow) {
            assert_eq!(
                described.at(&format!("/replicationInfoList/{other}")),
                &row.expected_untouched(other),
                "case {case}: replication {other} must be untouched, got {}",
                described.at(&format!("/replicationInfoList/{other}"))
            );
        }
        let after = described
            .at("/currentVersion")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the reader reported no currentVersion"))
            .to_string();
        assert_version_token(&after, &case);
        assert_ne!(
            after, before,
            "case {case}: an accepted update must bump currentVersion, got {after}"
        );
        if let Some(log_delivery) = &row.log_delivery {
            assert_eq!(
                described.at("/logDelivery"),
                log_delivery,
                "case {case}: the update must replace the replicator's log delivery, got {}",
                described.at("/logDelivery")
            );
        }
        // The clusters a replicator spans are not what this operation changes.
        assert_eq!(
            described
                .at("/kafkaClusters")
                .as_array()
                .map(Vec::len),
            Some(row.setup.clusters.len()),
            "case {case}: the update must leave the cluster list alone, got {}",
            described.at("/kafkaClusters")
        );

        // The state is non-terminal, so the clock owes the replicator its settled
        // state again — and the version it settles at is the one the update minted.
        sleep_past_running();
        let settled = describe(&emulator, &arn).await;
        assert_eq!(
            settled.at("/replicatorState").as_str(),
            Some(row.setup.settled_state()),
            "case {case}: expected replicatorState {}, got {}",
            row.setup.settled_state(),
            settled.at("/replicatorState")
        );
        assert_eq!(
            settled.at("/currentVersion").as_str(),
            Some(after.as_str()),
            "case {case}: settling must not change the version, got {}",
            settled.at("/currentVersion")
        );
        assert_eq!(
            settled.at(&format!("/replicationInfoList/{}", row.flow)),
            &row.expected_flow(),
            "case {case}: settling must not change the configuration, got {}",
            settled.at(&format!("/replicationInfoList/{}", row.flow))
        );
    }
}

/// RULE: a replication is addressed by the ORDERED pair of its Kafka cluster
/// aliases, so an update whose pair matches no configured replication is refused
/// with HTTP 400 `BadRequestException` and changes nothing — not the stored
/// configuration, not its orientation, and not `CurrentVersion`.
#[tokio::test]
async fn a_pair_that_addresses_no_replication_is_refused() {
    let emulator = Emulator::new();
    let setup = Setup::spanning(
        "mismatch",
        vec![
            End::msk(&emulator, "mismatch-source").await,
            End::msk(&emulator, "mismatch-target").await,
        ],
        vec![(0, 1, "NONE")],
    );
    let arn = create_ok(&emulator, &setup).await;
    sleep_past_running();
    let version = current_version(&emulator, &arn, "mismatch").await;
    let intact = describe(&emulator, &arn).await.body;

    let (source_arn, target_arn) = match (&setup.clusters[0], &setup.clusters[1]) {
        (End::Msk(source), End::Msk(target)) => (source.clone(), target.clone()),
        other => panic!("the setup must name both ends by ARN, got {other:?}"),
    };
    let topic = TopicUpdate {
        to_replicate: vec!["t-.*"],
        to_exclude: vec![],
        detect_new: true,
        copy_configurations: true,
        copy_acls: false,
    };

    // Rows span every way a pair of NAMED ends can address no configured
    // replication: the configured pair REVERSED, a source the replicator does not
    // span, a target it does not span, a target named as an Apache Kafka cluster id
    // the replicator does not span, and both ends named as Apache Kafka cluster ids
    // where the configured ones are MSK ARNs. A request that leaves an end unnamed
    // never reaches this rule — see `crate::replication_end_naming`.
    let rows: Vec<(&str, Value)> = vec![
        (
            "reversed",
            json!({ "sourceKafkaClusterArn": target_arn, "targetKafkaClusterArn": source_arn }),
        ),
        (
            "unknown-source",
            json!({
                "sourceKafkaClusterArn":
                    format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/stranger/{GHOST_CLUSTER_ID}"),
                "targetKafkaClusterArn": target_arn,
            }),
        ),
        (
            "unknown-target",
            json!({
                "sourceKafkaClusterArn": source_arn,
                "targetKafkaClusterArn":
                    format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/stranger/{GHOST_CLUSTER_ID}"),
            }),
        ),
        (
            "apache-id-target",
            json!({
                "sourceKafkaClusterArn": source_arn,
                "targetKafkaClusterId": "mismatch-bystander",
            }),
        ),
        (
            "ids-that-are-no-alias",
            json!({
                "sourceKafkaClusterId": "mismatch-source-cluster",
                "targetKafkaClusterId": "mismatch-target",
            }),
        ),
    ];

    for (case, pair) in &rows {
        let mut body = pair.clone();
        body["currentVersion"] = json!(version);
        body["topicReplication"] = topic.to_json();
        let reply = update(&emulator, &arn, body).await;
        assert_eq!(
            (reply.status, reply.error_code(), reply.error_message()),
            (400, "BadRequestException", ABSENT_MESSAGE),
            "case {case}: expected a 400 BadRequestException for a pair that addresses no \
             replication, got {} {}",
            reply.status,
            reply.body
        );
        // The refusal changed NOTHING: the whole reader answer, orientation,
        // configuration and version included, is the one from before the call.
        assert_eq!(
            describe(&emulator, &arn).await.body,
            intact,
            "case {case}: a refused update must leave the replicator exactly as it was"
        );
    }

    // The pair is matched by ALIAS, not by the form each end is named in: the two
    // ends of the configured replication given as Apache Kafka cluster ids that
    // are those clusters' aliases address that same replication.
    let by_alias = update(
        &emulator,
        &arn,
        json!({
            "currentVersion": version,
            "sourceKafkaClusterId": setup.clusters[0].alias(),
            "targetKafkaClusterId": setup.clusters[1].alias(),
            "topicReplication": topic.to_json(),
        }),
    )
    .await;
    assert_eq!(
        (by_alias.status, by_alias.error_code()),
        (200, ""),
        "a pair given as the configured clusters' aliases must address their replication, \
         got {} {}",
        by_alias.status,
        by_alias.body
    );
    let updated = describe(&emulator, &arn).await;
    assert_eq!(
        updated.at("/replicationInfoList/0/topicReplication/topicsToReplicate"),
        &json!(topic.to_replicate),
        "the addressed replication must carry the update, got {}",
        updated.at("/replicationInfoList/0/topicReplication")
    );
}

/// RULE: `CurrentVersion` is optimistic concurrency on the replicator, so an update
/// quoting a version the replicator is not at is refused with HTTP 400
/// `BadRequestException` and changes nothing; only the version the reader currently
/// reports is accepted, and an accepted update retires it.
#[tokio::test]
async fn a_stale_current_version_is_refused() {
    let emulator = Emulator::new();
    let setup = Setup::named(&emulator, "concurrency").await;
    let arn = create_ok(&emulator, &setup).await;
    sleep_past_running();
    let version = current_version(&emulator, &arn, "concurrency").await;
    let topic = TopicUpdate {
        to_replicate: vec!["t-.*"],
        to_exclude: vec!["__.*"],
        detect_new: true,
        copy_configurations: false,
        copy_acls: false,
    };
    let row = UpdateCase {
        setup: setup.clone(),
        flow: 0,
        topic: Some(topic),
        consumers: None,
        log_delivery: None,
    };
    let intact = describe(&emulator, &arn).await.body;

    // Rows span the ways a quoted version can be wrong: a well-formed token the
    // replicator never had, another replicator's current token, and an empty one.
    let other = create_ok(
        &emulator,
        &Setup::named(&emulator, "concurrency-other").await,
    )
    .await;
    sleep_past_running();
    let foreign = current_version(&emulator, &other, "concurrency-other").await;
    assert_ne!(
        foreign, version,
        "two replicators must not share a version token, got {foreign}"
    );
    for (case, quoted) in [
        ("never-held", "K0000000000000".to_string()),
        ("another-replicator", foreign),
        ("empty", String::new()),
    ] {
        let reply = update(&emulator, &arn, row.body(&quoted)).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case}: expected a stale currentVersion to be refused, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            describe(&emulator, &arn).await.body,
            intact,
            "case {case}: a refused update must leave the replicator exactly as it was"
        );
    }

    // The version the reader reports is accepted, and quoting it again afterwards
    // is not: an accepted update retires the version it was quoted with.
    let accepted = update(&emulator, &arn, row.body(&version)).await;
    assert_eq!(
        accepted.status, 200,
        "the replicator's own version must be accepted, got {} {}",
        accepted.status, accepted.body
    );
    let replayed = update(&emulator, &arn, row.body(&version)).await;
    assert_eq!(
        (replayed.status, replayed.error_code()),
        (400, "BadRequestException"),
        "a retired version must be refused, got {} {}",
        replayed.status,
        replayed.body
    );
    let next = current_version(&emulator, &arn, "concurrency").await;
    // The accepted update above moved the replicator to `UPDATING`, which is not a
    // state a further change is accepted from, so the clock is allowed to settle it
    // back to `RUNNING` before the new version is quoted.
    sleep_past_running();
    let again = update(&emulator, &arn, row.body(&next)).await;
    assert_eq!(
        again.status, 200,
        "the version the accepted update minted must be accepted, got {} {}",
        again.status, again.body
    );
}

/// RULE: `UpdateReplicationInfo` resolves its replicator exactly as every other
/// replicator operation does — HTTP 400 `BadRequestException` for a value that is
/// not shaped like a replicator ARN, and HTTP 404 `NotFoundException`, naming the
/// input member that resolved to nothing, for a well-formed ARN this region holds
/// no replicator for.
#[tokio::test]
async fn an_unknown_or_malformed_replicator_arn_is_refused() {
    let emulator = Emulator::new();
    let setup = Setup::named(&emulator, "resolving").await;
    let present = create_ok(&emulator, &setup).await;
    sleep_past_running();
    let version = current_version(&emulator, &present, "resolving").await;
    let row = UpdateCase {
        setup: setup.clone(),
        flow: 0,
        topic: Some(TopicUpdate {
            to_replicate: vec!["t-.*"],
            to_exclude: vec![],
            detect_new: true,
            copy_configurations: true,
            copy_acls: false,
        }),
        consumers: None,
        log_delivery: None,
    };

    // `(arn, expected status, expected error code)`. Rows: well-formed but absent,
    // then one per way the ARN grammar can fail — not an ARN at all, the wrong
    // service, the wrong resource type, and a missing resource segment.
    let rows: Vec<(String, u16, &str)> = vec![
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/absent/{GHOST_CLUSTER_ID}"),
            404,
            "NotFoundException",
        ),
        ("not-an-arn".to_string(), 400, "BadRequestException"),
        (
            format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:replicator/wrong/{GHOST_CLUSTER_ID}"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/wrong/{GHOST_CLUSTER_ID}"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/short"),
            400,
            "BadRequestException",
        ),
    ];

    for (arn, status, code) in &rows {
        let reply = update(&emulator, arn, row.body(&version)).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case arn={arn:?}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
        if *status == 404 {
            assert_eq!(
                reply.error_message(),
                NOT_FOUND_MESSAGE,
                "case arn={arn:?}: the not-found refusal must name the input member"
            );
        }
    }

    // A replicator is regional, so an endpoint elsewhere cannot update it and
    // answers for it exactly as for an ARN nobody ever issued.
    let elsewhere = update_in(&emulator, "us-east-1", &present, row.body(&version)).await;
    assert_eq!(
        (
            elsewhere.status,
            elsewhere.error_code(),
            elsewhere.error_message()
        ),
        (404, "NotFoundException", NOT_FOUND_MESSAGE),
        "a replicator must be invisible to an update outside its region, got {} {}",
        elsewhere.status,
        elsewhere.body
    );
    // None of those refusals changed the replicator: its version is still the one
    // the reader reported before them.
    assert_eq!(
        current_version(&emulator, &present, "resolving").await,
        version,
        "a refused update must leave currentVersion alone"
    );
}
