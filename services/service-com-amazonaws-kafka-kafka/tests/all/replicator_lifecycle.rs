// SPDX-License-Identifier: CC0-1.0

//! The MSK Replicator lifecycle (`CreateReplicator`, `DescribeReplicator`), driven
//! end to end over the same `MultiServiceRouter` the emulator serves from.
//!
//! Each suite asserts a RULE over a TABLE of inputs, not one recorded request. The
//! rows differ along whatever the rule generalises over: how many Kafka clusters
//! the replicator spans, how each one is named (an Amazon MSK cluster by ARN, a
//! self-managed Apache Kafka cluster by id), how many replication flows it carries
//! and in which direction, the `TargetCompressionType` (every value the model's
//! enum lists), the client authentication, the log-delivery destination, the tag
//! count, the description, the replicator-name form and the signing region.
//!
//! The HTTP harness comes from [`crate::kafka_harness`] — the same router, the
//! same SigV4-scoped request builder — rather than a second one built here.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_kafka_kafka::state::{
    MAX_CLUSTERS_PER_REPLICATOR, MAX_FLOWS_PER_REPLICATOR, MAX_TAGS_PER_REPLICATOR,
    REPLICATOR_ACTIVATION_DELAY,
};

use crate::kafka_harness::{pct, Emulator, Reply, ACCOUNT, REGION};

/// The IAM role every row below hands the replicator. `ServiceExecutionRoleArn` is
/// a plain `__string` in the model, so the service stores and reports it verbatim.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// The cluster-id segment of the MSK cluster ARNs these rows point at. The
/// clusters themselves are never created: `CreateReplicator` is asynchronous and
/// does not resolve them.
const GHOST_CLUSTER_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// Sleep past the emulator's own `CREATING` -> `RUNNING` span, so a later read sees
/// a settled replicator. `std::thread::sleep` because the workspace's tokio carries
/// no timer feature; nothing else has to make progress here.
fn sleep_past_running() {
    std::thread::sleep(REPLICATOR_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// One Kafka cluster of a replicator, as the request names it.
#[derive(Debug, Clone)]
enum ClusterKind {
    /// An Amazon MSK cluster, named by ARN.
    Msk(String),
    /// A self-managed Apache Kafka cluster, named by the id the caller gave it.
    Apache(String),
}

/// One entry of `KafkaClusters`: which cluster, and how clients authenticate to it.
#[derive(Debug, Clone)]
struct ClusterRef {
    kind: ClusterKind,
    /// `KafkaClusterClientAuthentication` member to set: `"saslScram"`, `"mTLS"`,
    /// or `None` for a request that carries no client authentication.
    auth: Option<&'static str>,
}

impl ClusterRef {
    /// An MSK cluster ARN in `REGION`/`ACCOUNT` for a cluster named `name`.
    fn msk(name: &str) -> Self {
        Self::msk_arn(&format!(
            "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{name}/{GHOST_CLUSTER_ID}"
        ))
    }

    fn msk_arn(arn: &str) -> Self {
        Self {
            kind: ClusterKind::Msk(arn.to_string()),
            auth: None,
        }
    }

    fn apache(id: &str) -> Self {
        Self {
            kind: ClusterKind::Apache(id.to_string()),
            auth: None,
        }
    }

    fn with_auth(mut self, auth: &'static str) -> Self {
        self.auth = Some(auth);
        self
    }

    /// The alias this cluster must be reported under.
    ///
    /// From `KafkaClusterDescription$KafkaClusterAlias` ("The alias of the Kafka
    /// cluster. Used to prefix names of replicated topics."): an MSK cluster is
    /// aliased by the cluster name its ARN carries (`...:cluster/<name>/<id>`), and
    /// an Apache Kafka cluster by the id the caller gave it.
    fn alias(&self) -> String {
        match &self.kind {
            ClusterKind::Msk(arn) => arn
                .splitn(6, ':')
                .nth(5)
                .and_then(|resource| resource.split('/').nth(1))
                .unwrap_or_default()
                .to_string(),
            ClusterKind::Apache(id) => id.clone(),
        }
    }

    fn to_json(&self) -> Value {
        let mut entry = match &self.kind {
            ClusterKind::Msk(arn) => json!({
                "amazonMskCluster": { "mskClusterArn": arn },
                "vpcConfig": {
                    "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
                    "securityGroupIds": ["sg-aaaaaaaaaaaaaaaaa"],
                },
            }),
            ClusterKind::Apache(id) => json!({
                "apacheKafkaCluster": {
                    "apacheKafkaClusterId": id,
                    "bootstrapBrokerString": format!("b-1.{id}.example.internal:9092"),
                },
                "vpcConfig": { "subnetIds": ["subnet-ccccccccccccccccc"] },
                "encryptionInTransit": { "encryptionType": "TLS" },
            }),
        };
        if let Some(auth) = self.auth {
            let detail = match auth {
                "saslScram" => json!({
                    "mechanism": "SHA512",
                    "secretArn": "arn:aws:secretsmanager:us-west-2:000000000000:secret:AmazonMSK_repl",
                }),
                "mTLS" => json!({
                    "secretArn": "arn:aws:secretsmanager:us-west-2:000000000000:secret:AmazonMSK_mtls",
                }),
                other => panic!("unsupported client authentication {other:?}"),
            };
            entry["clientAuthentication"] = json!({ auth: detail });
        }
        entry
    }

    /// The `ReplicationInfo` member that names this cluster at `end` (`"source"` or
    /// `"target"`), and its value: an MSK cluster is named by ARN, an Apache Kafka
    /// cluster by its cluster id.
    fn flow_member(&self, end: &str) -> (String, Value) {
        match &self.kind {
            ClusterKind::Msk(arn) => (format!("{end}KafkaClusterArn"), json!(arn)),
            ClusterKind::Apache(id) => (format!("{end}KafkaClusterId"), json!(id)),
        }
    }
}

/// Owned tag pairs from a borrowed table, so a row can write them inline.
fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One replication flow: which entry of `KafkaClusters` is the source, which is the
/// target, and how records are compressed when produced to the target.
#[derive(Debug, Clone)]
struct Flow {
    source: usize,
    target: usize,
    /// A `TargetCompressionType` value.
    compression: &'static str,
}

impl Flow {
    fn new(source: usize, target: usize, compression: &'static str) -> Self {
        Self {
            source,
            target,
            compression,
        }
    }
}

/// One row of a `CreateReplicator` table: the required members plus whichever
/// optional ones the row exercises. `None`/empty means "the request omits it".
#[derive(Debug, Clone)]
struct ReplicatorRequest {
    name: String,
    description: Option<String>,
    clusters: Vec<ClusterRef>,
    flows: Vec<Flow>,
    tags: Vec<(String, String)>,
    /// A `LogDelivery` document, or `None` when the request carries none.
    log_delivery: Option<Value>,
}

impl ReplicatorRequest {
    /// A minimal request: two ghost MSK clusters, one flow between them, and none
    /// of the optional members.
    fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            clusters: vec![
                ClusterRef::msk(&format!("{name}-source")),
                ClusterRef::msk(&format!("{name}-target")),
            ],
            flows: vec![Flow::new(0, 1, "NONE")],
            tags: Vec::new(),
            log_delivery: None,
        }
    }

    fn flow_json(&self, flow: &Flow) -> Value {
        let mut info = json!({
            "targetCompressionType": flow.compression,
            "consumerGroupReplication": {
                "consumerGroupsToReplicate": [".*"],
                "consumerGroupsToExclude": ["console-consumer-.*"],
                "detectAndCopyNewConsumerGroups": true,
                "synchroniseConsumerGroupOffsets": true,
            },
            "topicReplication": {
                "topicsToReplicate": [".*"],
                "topicsToExclude": ["__.*"],
                "detectAndCopyNewTopics": true,
                "copyTopicConfigurations": true,
                "copyAccessControlListsForTopics": false,
                "startingPosition": { "type": "LATEST" },
                "topicNameConfiguration": { "type": "PREFIXED_WITH_SOURCE_CLUSTER_ALIAS" },
            },
        });
        for (end, index) in [("source", flow.source), ("target", flow.target)] {
            let cluster = self
                .clusters
                .get(index)
                .unwrap_or_else(|| panic!("case {self:?}: flow names cluster {index}"));
            let (member, value) = cluster.flow_member(end);
            info[member] = value;
        }
        info
    }

    fn to_json(&self) -> Value {
        let mut body = json!({
            "replicatorName": self.name,
            "serviceExecutionRoleArn": ROLE_ARN,
            "kafkaClusters": self
                .clusters
                .iter()
                .map(ClusterRef::to_json)
                .collect::<Vec<_>>(),
            "replicationInfoList": self
                .flows
                .iter()
                .map(|flow| self.flow_json(flow))
                .collect::<Vec<_>>(),
        });
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if !self.tags.is_empty() {
            body["tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), json!(value)))
                    .collect::<Map<String, Value>>(),
            );
        }
        if let Some(log_delivery) = &self.log_delivery {
            body["logDelivery"] = log_delivery.clone();
        }
        body
    }

    /// The `KafkaClusterDescription` the reader must report for cluster `index`:
    /// exactly what was submitted, plus that cluster's alias.
    fn expected_cluster(&self, index: usize) -> Value {
        let cluster = &self.clusters[index];
        let mut expected = cluster.to_json();
        expected["kafkaClusterAlias"] = json!(cluster.alias());
        expected
    }

    /// The `ReplicationInfoDescription` the reader must report for `flow`: the
    /// submitted configuration, with each end named by ALIAS rather than by the ARN
    /// or id the request used.
    fn expected_flow(&self, flow: &Flow) -> Value {
        let submitted = self.flow_json(flow);
        json!({
            "consumerGroupReplication": submitted["consumerGroupReplication"],
            "sourceKafkaClusterAlias": self.clusters[flow.source].alias(),
            "targetCompressionType": flow.compression,
            "targetKafkaClusterAlias": self.clusters[flow.target].alias(),
            "topicReplication": submitted["topicReplication"],
        })
    }
}

/// `CreateReplicator` for `request`, signed for `region`.
async fn create_in(emulator: &Emulator, region: &str, request: &ReplicatorRequest) -> Reply {
    emulator
        .call_in(
            region,
            "POST",
            "/replication/v1/replicators",
            Some(request.to_json()),
        )
        .await
}

async fn create(emulator: &Emulator, request: &ReplicatorRequest) -> Reply {
    create_in(emulator, REGION, request).await
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

/// Create `request` and hand back the minted ARN, asserting it succeeded.
async fn create_ok(emulator: &Emulator, request: &ReplicatorRequest) -> String {
    let reply = create(emulator, request).await;
    assert_eq!(
        reply.status, 200,
        "case {request:?}: expected CreateReplicator to return 200, got {} {}",
        reply.status, reply.body
    );
    reply.body["replicatorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {request:?}: CreateReplicator returned no replicatorArn"))
        .to_string()
}

/// `arn:aws:kafka:<region>:<account>:replicator/<name>/<uuid>-<ordinal>` — the ARN
/// grammar an MSK replicator's identity follows.
fn assert_replicator_arn(arn: &str, region: &str, name: &str, case: &str) {
    let prefix = format!("arn:aws:kafka:{region}:{ACCOUNT}:replicator/{name}/");
    let tail = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case}: expected an ARN starting {prefix:?}, got {arn:?}")
    });
    let (uuid, ordinal) = tail
        .rsplit_once('-')
        .unwrap_or_else(|| panic!("case {case}: ARN tail {tail:?} carries no <uuid>-<ordinal>"));
    assert_eq!(
        uuid.len(),
        36,
        "case {case}: expected a 36-character uuid, got {uuid:?}"
    );
    for (index, character) in uuid.chars().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            assert_eq!(
                character, '-',
                "case {case}: expected a hyphen at index {index} of {uuid:?}, got {character:?}"
            );
        } else {
            assert!(
                character.is_ascii_hexdigit() && !character.is_ascii_uppercase(),
                "case {case}: expected a lower-case hex digit at index {index} of {uuid:?}, \
                 got {character:?}"
            );
        }
    }
    // RFC 4122: version 4 in the high nibble of byte 6, variant 10xx in byte 8.
    assert_eq!(
        uuid.as_bytes()[14],
        b'4',
        "case {case}: expected a version-4 uuid, got {uuid:?}"
    );
    assert!(
        matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
        "case {case}: expected the RFC 4122 variant bits, got {uuid:?}"
    );
    let ordinal: u64 = ordinal
        .parse()
        .unwrap_or_else(|_| panic!("case {case}: expected a numeric ordinal, got {ordinal:?}"));
    assert!(
        ordinal >= 1,
        "case {case}: expected a 1-based ordinal, got {ordinal}"
    );
}

/// An MSK `CurrentVersion` token: `K` followed by 13 upper-case alphanumerics.
fn assert_current_version(value: &Value, case: &str) {
    let token = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a currentVersion string, got {value}"));
    assert!(
        token.len() == 14
            && token.starts_with('K')
            && token[1..]
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()),
        "case {case}: expected a K-prefixed 14-character version token, got {token:?}"
    );
}

/// RULE: a created replicator is persisted whole, and `DescribeReplicator` reports
/// every member from that record — the submitted values where the request carried
/// them, each Kafka cluster's alias, and nothing where the request carried nothing.
#[tokio::test]
async fn describe_reports_exactly_what_was_created() {
    // The rows differ along everything the rule generalises over: cluster count
    // (2 and 3), how each cluster is named (MSK ARN, Apache Kafka id, and a mix),
    // client authentication (absent, SASL/SCRAM, mTLS), flow count and direction
    // (0 -> 1, 1 -> 0, and two flows out of one source), every
    // `TargetCompressionType` the model's enum lists (NONE | GZIP | SNAPPY | LZ4 |
    // ZSTD), each `ReplicatorLogDelivery` destination (absent, CloudWatch Logs,
    // Firehose, S3), tag count (0, 1, 3) and description (absent, short, 1024
    // characters — the `__stringMax1024` ceiling on `Description`).
    let rows = vec![
        ReplicatorRequest::named("minimal"),
        ReplicatorRequest {
            description: Some("apache source into msk target".to_string()),
            clusters: vec![
                ClusterRef::apache("self-managed-a").with_auth("saslScram"),
                ClusterRef::msk("msk-target"),
            ],
            flows: vec![Flow::new(0, 1, "GZIP")],
            tags: tags(&[("env", "prod")]),
            log_delivery: Some(json!({
                "replicatorLogDelivery": {
                    "cloudWatchLogs": { "enabled": true, "logGroup": "/aws/msk/replicator" },
                },
            })),
            ..ReplicatorRequest::named("apache-to-msk")
        },
        ReplicatorRequest {
            clusters: vec![
                ClusterRef::msk("fan-out-source"),
                ClusterRef::msk("fan-out-first"),
                ClusterRef::msk("fan-out-second"),
            ],
            flows: vec![Flow::new(0, 1, "SNAPPY"), Flow::new(0, 2, "LZ4")],
            tags: tags(&[("env", "test"), ("owner", "msk"), ("team", "data")]),
            log_delivery: Some(json!({
                "replicatorLogDelivery": {
                    "s3": { "enabled": true, "bucket": "msk-replicator-logs", "prefix": "repl/" },
                },
            })),
            ..ReplicatorRequest::named("fan-out")
        },
        ReplicatorRequest {
            description: Some("z".repeat(1024)),
            clusters: vec![
                ClusterRef::msk("mtls-source").with_auth("mTLS"),
                ClusterRef::msk("mtls-target").with_auth("mTLS"),
            ],
            flows: vec![Flow::new(1, 0, "ZSTD")],
            log_delivery: Some(json!({
                "replicatorLogDelivery": {
                    "firehose": { "enabled": false, "deliveryStream": "msk-replicator-logs" },
                },
            })),
            ..ReplicatorRequest::named("reversed-mtls")
        },
        ReplicatorRequest {
            clusters: vec![
                ClusterRef::apache("self-managed-left"),
                ClusterRef::apache("self-managed-right"),
            ],
            flows: vec![Flow::new(0, 1, "NONE"), Flow::new(1, 0, "GZIP")],
            ..ReplicatorRequest::named("bidirectional")
        },
    ];

    let emulator = Emulator::new();
    let mut minted = Vec::new();
    for row in &rows {
        let reply = create(&emulator, row).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected CreateReplicator to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.body["replicatorName"].as_str(),
            Some(row.name.as_str()),
            "case {row:?}: CreateReplicator echoed the wrong replicatorName, got {}",
            reply.body["replicatorName"]
        );
        // AWS answers CREATING for a replicator whose fleet is still being stood
        // up; it is RUNNING only once the clock owes the transition.
        assert_eq!(
            reply.body["replicatorState"].as_str(),
            Some("CREATING"),
            "case {row:?}: expected replicatorState CREATING, got {}",
            reply.body["replicatorState"]
        );
        let arn = reply.body["replicatorArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: CreateReplicator returned no replicatorArn"))
            .to_string();
        assert_replicator_arn(&arn, REGION, &row.name, &row.name);
        minted.push(arn);
    }

    let mut distinct = minted.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "every replicator must get its own ARN, got {minted:?}"
    );

    sleep_past_running();
    for (row, arn) in rows.iter().zip(&minted) {
        let reply = describe(&emulator, arn).await;
        let case = row.name.clone();
        assert_eq!(
            reply.status, 200,
            "case {case}: expected DescribeReplicator to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/replicatorArn").as_str(),
            Some(arn.as_str()),
            "case {case}: expected replicatorArn {arn}, got {}",
            reply.at("/replicatorArn")
        );
        // "The ARN of the replicator resource in the region where the replicator
        // was created" — this one was created here, so it is its own ARN.
        assert_eq!(
            reply.at("/replicatorResourceArn").as_str(),
            Some(arn.as_str()),
            "case {case}: expected replicatorResourceArn {arn}, got {}",
            reply.at("/replicatorResourceArn")
        );
        assert_eq!(
            reply.at("/replicatorName").as_str(),
            Some(row.name.as_str()),
            "case {case}: expected replicatorName {}, got {}",
            row.name,
            reply.at("/replicatorName")
        );
        assert_eq!(
            reply.at("/serviceExecutionRoleArn").as_str(),
            Some(ROLE_ARN),
            "case {case}: expected serviceExecutionRoleArn {ROLE_ARN}, got {}",
            reply.at("/serviceExecutionRoleArn")
        );
        assert_eq!(
            reply.at("/replicatorState").as_str(),
            Some("RUNNING"),
            "case {case}: expected replicatorState RUNNING, got {}",
            reply.at("/replicatorState")
        );
        assert_eq!(
            reply.at("/isReplicatorReference"),
            &json!(false),
            "case {case}: a replicator created here is not a reference, got {}",
            reply.at("/isReplicatorReference")
        );
        assert_current_version(reply.at("/currentVersion"), &case);
        // Present, not format-checked: how a `@timestampFormat` member is rendered
        // on the wire is the framework serializer's business, not this handler's.
        assert!(
            !reply.at("/creationTime").is_null(),
            "case {case}: expected a creationTime, got {}",
            reply.at("/creationTime")
        );
        match &row.description {
            Some(description) => assert_eq!(
                reply.at("/replicatorDescription").as_str(),
                Some(description.as_str()),
                "case {case}: expected the submitted replicatorDescription, got {}",
                reply.at("/replicatorDescription")
            ),
            None => assert!(
                reply.at("/replicatorDescription").is_null(),
                "case {case}: expected no replicatorDescription, got {}",
                reply.at("/replicatorDescription")
            ),
        }
        // A replicator that carries no tags reports no `tags` member at all, the
        // way every other absent request member is reported.
        let expected_tags = if row.tags.is_empty() {
            Value::Null
        } else {
            Value::Object(
                row.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), json!(value)))
                    .collect::<Map<String, Value>>(),
            )
        };
        assert_eq!(
            reply.at("/tags"),
            &expected_tags,
            "case {case}: expected tags {expected_tags}, got {}",
            reply.at("/tags")
        );
        match &row.log_delivery {
            Some(expected) => assert_eq!(
                reply.at("/logDelivery"),
                expected,
                "case {case}: expected logDelivery {expected}, got {}",
                reply.at("/logDelivery")
            ),
            None => assert!(
                reply.at("/logDelivery").is_null(),
                "case {case}: expected no logDelivery, got {}",
                reply.at("/logDelivery")
            ),
        }
        // `StateInfo` explains a state that needs explaining; RUNNING does not.
        assert!(
            reply.at("/stateInfo").is_null(),
            "case {case}: expected no stateInfo for a RUNNING replicator, got {}",
            reply.at("/stateInfo")
        );
        let expected_clusters = Value::Array(
            (0..row.clusters.len())
                .map(|index| row.expected_cluster(index))
                .collect(),
        );
        assert_eq!(
            reply.at("/kafkaClusters"),
            &expected_clusters,
            "case {case}: expected kafkaClusters {expected_clusters}, got {}",
            reply.at("/kafkaClusters")
        );
        let expected_flows = Value::Array(
            row.flows
                .iter()
                .map(|flow| row.expected_flow(flow))
                .collect(),
        );
        assert_eq!(
            reply.at("/replicationInfoList"),
            &expected_flows,
            "case {case}: expected replicationInfoList {expected_flows}, got {}",
            reply.at("/replicationInfoList")
        );
    }
}

/// RULE: a replicator name identifies a replicator within an account and region, so
/// a create under a name already taken in that region is refused with HTTP 409
/// `ConflictException` and registers nothing, while the same name in another region
/// is a different replicator.
#[tokio::test]
async fn a_repeated_replicator_name_conflicts() {
    // Rows span the name forms the model's `^[0-9A-Za-z][0-9A-Za-z-]{0,}$` pattern
    // and 1..128 length admit: one character, hyphenated, mixed case with digits,
    // and the 128-character ceiling.
    let names = vec![
        "r".to_string(),
        "repl-with-hyphens".to_string(),
        "Replicator9".to_string(),
        format!("x{}", "y".repeat(127)),
    ];

    let emulator = Emulator::new();
    for name in &names {
        let first = ReplicatorRequest::named(name);
        let arn = create_ok(&emulator, &first).await;
        assert_replicator_arn(&arn, REGION, name, name);

        // A second create under the same name, differing in everything else, is
        // still a conflict: the name alone is the uniqueness namespace.
        let second = ReplicatorRequest {
            description: Some("a second attempt".to_string()),
            clusters: vec![
                ClusterRef::apache("other-source"),
                ClusterRef::msk("other-target"),
            ],
            flows: vec![Flow::new(0, 1, "ZSTD")],
            tags: tags(&[("attempt", "two")]),
            ..ReplicatorRequest::named(name)
        };
        let conflict = create(&emulator, &second).await;
        assert_eq!(
            (conflict.status, conflict.error_code()),
            (409, "ConflictException"),
            "case {name}: expected a 409 ConflictException for a repeated name, got {} {}",
            conflict.status,
            conflict.body
        );

        // The refused create registered nothing: the first replicator is still the
        // one the name resolves to, with its own configuration intact.
        let described = describe(&emulator, &arn).await;
        assert_eq!(
            described.status, 200,
            "case {name}: the first replicator must survive a refused create, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.at("/replicationInfoList"),
            &Value::Array(vec![first.expected_flow(&first.flows[0])]),
            "case {name}: the refused create must not alter the stored replicator, got {}",
            described.at("/replicationInfoList")
        );

        // The same name in another region is another replicator.
        let elsewhere = create_in(&emulator, "us-east-1", &first).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {name}: the same name in another region must succeed, got {} {}",
            elsewhere.status, elsewhere.body
        );
        let other_arn = elsewhere.body["replicatorArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: no replicatorArn in another region"));
        assert_replicator_arn(other_arn, "us-east-1", name, name);
        assert_ne!(
            other_arn, arn,
            "case {name}: two regions must mint two ARNs, got {other_arn} twice"
        );
    }
}

/// RULE: the two ends of a replication flow are told apart by their Kafka cluster
/// aliases, so a flow whose source and target resolve to one alias is refused with
/// HTTP 400 `BadRequestException` and registers nothing.
#[tokio::test]
async fn a_flow_between_one_cluster_is_refused() {
    let emulator = Emulator::new();

    // Rows span how the two ends can collide: the same MSK cluster named by ARN,
    // the same Apache Kafka cluster named by id, an MSK cluster and an Apache
    // cluster that resolve to one alias, and a collision in the SECOND flow of a
    // multi-flow request (so the check is per flow, not only on the first).
    let refused = vec![
        ReplicatorRequest {
            clusters: vec![ClusterRef::msk("loop-msk")],
            flows: vec![Flow::new(0, 0, "NONE")],
            ..ReplicatorRequest::named("same-msk-arn")
        },
        ReplicatorRequest {
            clusters: vec![ClusterRef::apache("loop-apache")],
            flows: vec![Flow::new(0, 0, "GZIP")],
            ..ReplicatorRequest::named("same-apache-id")
        },
        ReplicatorRequest {
            clusters: vec![ClusterRef::msk("twin"), ClusterRef::apache("twin")],
            flows: vec![Flow::new(0, 1, "SNAPPY")],
            ..ReplicatorRequest::named("same-alias-two-kinds")
        },
        ReplicatorRequest {
            clusters: vec![ClusterRef::msk("second-flow-a"), ClusterRef::msk("second-flow-b")],
            flows: vec![Flow::new(0, 1, "LZ4"), Flow::new(1, 1, "ZSTD")],
            ..ReplicatorRequest::named("second-flow-loops")
        },
    ];

    for row in &refused {
        let reply = create(&emulator, row).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {row:?}: expected a 400 BadRequestException for a self-replicating flow, \
             got {} {}",
            reply.status,
            reply.body
        );
        // Nothing was registered, so the name is still free: a well-formed create
        // under it succeeds rather than conflicting with a half-written record.
        let repaired = ReplicatorRequest::named(&row.name);
        let arn = create_ok(&emulator, &repaired).await;
        assert_replicator_arn(&arn, REGION, &row.name, &row.name);
    }

    // The control: the same requests with distinct ends are accepted, so the
    // refusal is about the collision and not about the shape of the request.
    let accepted = vec![
        ReplicatorRequest {
            clusters: vec![ClusterRef::msk("distinct-a"), ClusterRef::msk("distinct-b")],
            flows: vec![Flow::new(0, 1, "NONE")],
            ..ReplicatorRequest::named("distinct-msk")
        },
        ReplicatorRequest {
            clusters: vec![
                ClusterRef::apache("distinct-left"),
                ClusterRef::apache("distinct-right"),
            ],
            flows: vec![Flow::new(0, 1, "GZIP"), Flow::new(1, 0, "ZSTD")],
            ..ReplicatorRequest::named("distinct-apache")
        },
    ];
    for row in &accepted {
        let arn = create_ok(&emulator, row).await;
        assert_replicator_arn(&arn, REGION, &row.name, &row.name);
    }
}

/// RULE: `CreateReplicator` is asynchronous and does not resolve the Kafka clusters
/// a replicator names, so it registers the replicator whether or not those clusters
/// exist in this region, this account, or at all.
#[tokio::test]
async fn clusters_the_emulator_does_not_hold_are_still_accepted() {
    let emulator = Emulator::new();

    // One really-created MSK cluster, so a row can mix a cluster the emulator holds
    // with one it does not and both must be accepted the same way.
    let created = emulator
        .call(
            "POST",
            "/v1/clusters",
            Some(json!({
                "clusterName": "really-here",
                "kafkaVersion": "3.6.0",
                "numberOfBrokerNodes": 1,
                "brokerNodeGroupInfo": {
                    "instanceType": "kafka.m5.large",
                    "clientSubnets": ["subnet-aaaaaaaaaaaaaaaaa"],
                },
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected CreateCluster to return 200, got {} {}",
        created.status, created.body
    );
    let real_arn = created.body["clusterArn"]
        .as_str()
        .expect("CreateCluster must return a clusterArn")
        .to_string();

    // Rows differ along how far from this emulator's state the named clusters are:
    // both absent, one present and one absent, another region, another account, and
    // self-managed Apache clusters the emulator never holds at all.
    let rows = vec![
        ReplicatorRequest::named("both-absent"),
        ReplicatorRequest {
            clusters: vec![
                ClusterRef::msk_arn(&real_arn),
                ClusterRef::msk("absent-target"),
            ],
            flows: vec![Flow::new(0, 1, "NONE")],
            ..ReplicatorRequest::named("half-present")
        },
        ReplicatorRequest {
            clusters: vec![
                ClusterRef::msk_arn(&format!(
                    "arn:aws:kafka:eu-west-1:{ACCOUNT}:cluster/far-source/{GHOST_CLUSTER_ID}"
                )),
                ClusterRef::msk("near-target"),
            ],
            flows: vec![Flow::new(0, 1, "GZIP")],
            ..ReplicatorRequest::named("cross-region")
        },
        ReplicatorRequest {
            clusters: vec![
                ClusterRef::msk_arn(&format!(
                    "arn:aws:kafka:{REGION}:111122223333:cluster/other-account/{GHOST_CLUSTER_ID}"
                )),
                ClusterRef::msk("same-account-target"),
            ],
            flows: vec![Flow::new(0, 1, "SNAPPY")],
            ..ReplicatorRequest::named("cross-account")
        },
        ReplicatorRequest {
            clusters: vec![
                ClusterRef::apache("never-registered-a"),
                ClusterRef::apache("never-registered-b"),
            ],
            flows: vec![Flow::new(0, 1, "LZ4")],
            ..ReplicatorRequest::named("self-managed-only")
        },
    ];

    let mut minted = Vec::new();
    for row in &rows {
        let reply = create(&emulator, row).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: CreateReplicator must accept clusters it cannot resolve, got {} {}",
            reply.status, reply.body
        );
        minted.push((row, create_arn(&reply, row)));
    }

    sleep_past_running();
    for (row, arn) in &minted {
        let described = describe(&emulator, arn).await;
        assert_eq!(
            described.status, 200,
            "case {}: the accepted replicator must be readable back, got {} {}",
            row.name, described.status, described.body
        );
        let expected_clusters = Value::Array(
            (0..row.clusters.len())
                .map(|index| row.expected_cluster(index))
                .collect(),
        );
        assert_eq!(
            described.at("/kafkaClusters"),
            &expected_clusters,
            "case {}: expected kafkaClusters {expected_clusters}, got {}",
            row.name,
            described.at("/kafkaClusters")
        );
    }
}

/// The minted ARN of a successful `CreateReplicator` reply.
fn create_arn(reply: &Reply, row: &ReplicatorRequest) -> String {
    reply.body["replicatorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {row:?}: CreateReplicator returned no replicatorArn"))
        .to_string()
}

/// A row holding `clusters` MSK clusters, `flows` replications between adjacent
/// ones, and `tag_count` tags — the shape the ceiling suite varies.
fn sized(name: &str, clusters: usize, flows: usize, tag_count: usize) -> ReplicatorRequest {
    let refs: Vec<ClusterRef> = (0..clusters)
        .map(|index| ClusterRef::msk(&format!("{name}-c{index}")))
        .collect();
    // Each flow runs between two DIFFERENT clusters, so the only thing a row
    // tests is the count. With fewer than two clusters there is no flow to make.
    let pairs = refs.len().saturating_sub(1).max(1);
    let built: Vec<Flow> = (0..flows)
        .map(|index| Flow::new(index % pairs, (index % pairs) + 1, "NONE"))
        .collect();
    ReplicatorRequest {
        clusters: refs,
        flows: built,
        tags: (0..tag_count)
            .map(|index| (format!("key-{index}"), format!("value-{index}")))
            .collect(),
        ..ReplicatorRequest::named(name)
    }
}

/// RULE: the lists a `CreateReplicator` request sizes are bounded, so a request
/// exactly AT a ceiling is accepted while one past it is refused with HTTP 400
/// `BadRequestException` naming that member, registering nothing.
#[tokio::test]
async fn a_request_past_a_per_replicator_ceiling_is_refused() {
    let emulator = Emulator::new();

    // One row per published ceiling, each tested AT the bound and one past it —
    // `(member, accepted row, refused row)`.
    let rows = vec![
        (
            "KafkaClusters",
            sized("clusters-at", MAX_CLUSTERS_PER_REPLICATOR, 1, 0),
            sized("clusters-past", MAX_CLUSTERS_PER_REPLICATOR + 1, 1, 0),
        ),
        (
            "ReplicationInfoList",
            sized("flows-at", 3, MAX_FLOWS_PER_REPLICATOR, 0),
            sized("flows-past", 3, MAX_FLOWS_PER_REPLICATOR + 1, 0),
        ),
        (
            "Tags",
            sized("tags-at", 2, 1, MAX_TAGS_PER_REPLICATOR),
            sized("tags-past", 2, 1, MAX_TAGS_PER_REPLICATOR + 1),
        ),
    ];

    for (member, accepted, refused) in &rows {
        let arn = create_ok(&emulator, accepted).await;
        assert_replicator_arn(&arn, REGION, &accepted.name, member);
        let described = describe(&emulator, &arn).await;
        assert_eq!(
            described.at("/kafkaClusters").as_array().map(Vec::len),
            Some(accepted.clusters.len()),
            "case {member}: a replicator at the ceiling must keep every entry, got {}",
            described.at("/kafkaClusters")
        );
        assert_eq!(
            described.at("/replicationInfoList").as_array().map(Vec::len),
            Some(accepted.flows.len()),
            "case {member}: a replicator at the ceiling must keep every replication, got {}",
            described.at("/replicationInfoList")
        );

        let reply = create(&emulator, refused).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {member}: expected a 400 BadRequestException past the ceiling, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some(*member),
            "case {member}: the refusal must name the offending member, got {}",
            reply.at("/invalidParameter")
        );
        // Nothing was registered, so the name is still free.
        let repaired = create_ok(&emulator, &ReplicatorRequest::named(&refused.name)).await;
        assert_replicator_arn(&repaired, REGION, &refused.name, member);
    }
}

/// RULE: every `KafkaClusters` entry must identify a cluster — by an Amazon MSK
/// cluster ARN or by an Apache Kafka cluster id — so an entry that identifies
/// none is refused with HTTP 400 `BadRequestException` naming that entry.
#[tokio::test]
async fn a_kafka_cluster_that_identifies_nothing_is_refused() {
    let emulator = Emulator::new();
    // A well-formed replication flow, so the refusal below is about the cluster
    // entry and not about the flow.
    let flow = json!({
        "sourceKafkaClusterArn":
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/identified/{GHOST_CLUSTER_ID}"),
        "targetKafkaClusterId": "identified-target",
        "targetCompressionType": "NONE",
        "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
        "topicReplication": { "topicsToReplicate": [".*"] },
    });

    // Rows differ in where the unidentified entry sits and what else it carries:
    // the only entry carrying nothing, the SECOND of two (so the check is per
    // entry, not only on the first), and one carrying every member a
    // `KafkaCluster` models EXCEPT a cluster identity.
    let rows: Vec<(&str, Vec<Value>, &str)> = vec![
        ("only-entry", vec![json!({})], "KafkaClusters[0]"),
        (
            "second-entry",
            vec![
                ClusterRef::msk("identified").to_json(),
                json!({ "vpcConfig": { "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa"] } }),
            ],
            "KafkaClusters[1]",
        ),
        (
            "everything-but-identity",
            vec![json!({
                "vpcConfig": { "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa"] },
                "encryptionInTransit": { "encryptionType": "TLS" },
                "clientAuthentication": {
                    "mTLS": {
                        "secretArn":
                            "arn:aws:secretsmanager:us-west-2:000000000000:secret:AmazonMSK_mtls",
                    },
                },
            })],
            "KafkaClusters[0]",
        ),
    ];

    for (case, clusters, member) in &rows {
        let reply = emulator
            .call(
                "POST",
                "/replication/v1/replicators",
                Some(json!({
                    "replicatorName": format!("unidentified-{case}"),
                    "serviceExecutionRoleArn": ROLE_ARN,
                    "kafkaClusters": clusters,
                    "replicationInfoList": [flow.clone()],
                })),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case}: expected a 400 BadRequestException for a cluster that identifies \
             nothing, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some(*member),
            "case {case}: the refusal must name the offending entry, got {}",
            reply.at("/invalidParameter")
        );
    }
}

/// RULE: a reader refuses a value that is not shaped like a replicator ARN with
/// HTTP 400 `BadRequestException`, and reserves HTTP 404 `NotFoundException` for a
/// well-formed ARN this region holds no replicator for.
#[tokio::test]
async fn an_unknown_or_malformed_replicator_arn_is_refused() {
    let emulator = Emulator::new();
    let present = create_ok(&emulator, &ReplicatorRequest::named("present")).await;

    // `(arn, expected status, expected error code)`. Rows: well-formed but absent,
    // the PRESENT replicator read from another region, then one row per way the ARN
    // grammar can fail — not an ARN at all, wrong service, wrong resource type, a
    // missing resource segment, an extra one, and an empty region.
    let rows: Vec<(String, u16, &str)> = vec![
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/absent/{GHOST_CLUSTER_ID}"),
            404,
            "NotFoundException",
        ),
        ("not-an-arn".to_string(), 400, "BadRequestException"),
        (
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:replicator/present/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/present/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/present"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/present/abc-1/extra"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka::{ACCOUNT}:replicator/present/abc-1"),
            400,
            "BadRequestException",
        ),
    ];

    for (arn, status, code) in &rows {
        let described = describe(&emulator, arn).await;
        assert_eq!(
            (described.status, described.error_code()),
            (*status, *code),
            "case arn={arn:?}: expected {status} {code} from DescribeReplicator, got {} {}",
            described.status,
            described.body
        );
    }

    // A replicator belongs to the region it was created in: a regional endpoint
    // elsewhere answers for it exactly as it does for an ARN nobody ever issued.
    let elsewhere = describe_in(&emulator, "us-east-1", &present).await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code()),
        (404, "NotFoundException"),
        "case arn={present:?}: a replicator must be invisible outside its region, got {} {}",
        elsewhere.status,
        elsewhere.body
    );
}

/// RULE: a replicator AWS returns in the non-terminal `CREATING` state becomes
/// `RUNNING` once the clock owes it that transition — and the span is never zero, so
/// a caller reading straight after the create still observes `CREATING`.
#[tokio::test]
async fn a_new_replicator_runs_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let row = ReplicatorRequest::named("settling");
    let arn = create_ok(&emulator, &row).await;

    let immediate = describe(&emulator, &arn).await;
    assert_eq!(
        immediate.at("/replicatorState").as_str(),
        Some("CREATING"),
        "case settling: a replicator read straight after the create is still CREATING, got {}",
        immediate.at("/replicatorState")
    );
    let creation_time = immediate.at("/creationTime").clone();
    let current_version = immediate.at("/currentVersion").clone();

    sleep_past_running();
    // Read twice: the transition is idempotent, and the identity members a reader
    // reports must not change because the resource was read.
    for attempt in 1..=2 {
        let settled = describe(&emulator, &arn).await;
        assert_eq!(
            settled.at("/replicatorState").as_str(),
            Some("RUNNING"),
            "case settling (read {attempt}): expected RUNNING once the clock owes it, got {}",
            settled.at("/replicatorState")
        );
        assert_eq!(
            settled.at("/creationTime"),
            &creation_time,
            "case settling (read {attempt}): creationTime must be stable across reads, got {}",
            settled.at("/creationTime")
        );
        assert_eq!(
            settled.at("/currentVersion"),
            &current_version,
            "case settling (read {attempt}): currentVersion must be stable across reads, got {}",
            settled.at("/currentVersion")
        );
    }
}
