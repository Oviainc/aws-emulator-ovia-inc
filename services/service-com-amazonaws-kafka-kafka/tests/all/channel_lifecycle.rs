// SPDX-License-Identifier: CC0-1.0

//! The channel lifecycle (`CreateChannel`, `DescribeChannel`), driven end to end
//! over the wire through the same `MultiServiceRouter` the emulator serves from.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request: the
//! rows differ along whatever the rule generalises over (destination kind,
//! compression, storage class, record converter, logging destination, tag count,
//! topic-list length, which cluster, which region). The harness — `Emulator`,
//! `Reply`, `pct`, `ClusterRequest` — is the shared one in `all/kafka_harness.rs`,
//! reused rather than copied, because a channel is created under a cluster that
//! harness already knows how to mint.

use serde_json::{json, Map, Value};
use service_com_amazonaws_kafka_kafka::state::{
    MAX_CHANNELS_PER_CLUSTER, TOPIC_CONFIGURATIONS_PER_CHANNEL,
};

use crate::kafka_harness::{
    assert_channel_name_already_exists, pct, reported_instant_secs, sleep_past_activation,
    sleep_past_channel_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// A role ARN a channel assumes to write to its destination. Any role ARN does:
/// the service stores what it was given and reports it back.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/msk-channel";
/// The dead-letter bucket every row below points at.
const DLQ_BUCKET_ARN: &str = "arn:aws:s3:::msk-channel-dlq";
/// The destination bucket the S3 rows point at.
const RECORDS_BUCKET_ARN: &str = "arn:aws:s3:::msk-channel-records";

/// Where a channel delivers. `S3` and `Iceberg` are the two destinations
/// `ChannelDestinationType` names; `Both` and `Neither` exist because the model
/// documents the two configurations as mutually exclusive, and a channel has to
/// deliver somewhere.
#[derive(Debug, Clone)]
enum Destination {
    /// `S3Storage$CompressionType` (NONE | GZIP | ZSTD) and
    /// `S3Storage$StorageClass` (STANDARD | INTELLIGENT_TIERING | GLACIER_IR),
    /// from the model's enums.
    S3 {
        compression: &'static str,
        storage_class: &'static str,
        output_prefix: Option<&'static str>,
        freshness: Option<i64>,
    },
    /// `IcebergCompressionType` (ZSTD | SNAPPY), from the model's enum, plus the
    /// switches `IcebergDestinationConfiguration` carries.
    Iceberg {
        append_only: bool,
        compression: Option<&'static str>,
        tables: usize,
        catalog: bool,
        schema_evolution: bool,
        table_creation: bool,
    },
    Both,
    Neither,
}

impl Destination {
    /// A minimal S3 destination: the enum values a bucket starts out with.
    fn s3() -> Self {
        Self::S3 {
            compression: "NONE",
            storage_class: "STANDARD",
            output_prefix: None,
            freshness: None,
        }
    }

    /// A minimal Iceberg destination: one table, every switch off.
    fn iceberg() -> Self {
        Self::Iceberg {
            append_only: true,
            compression: None,
            tables: 1,
            catalog: false,
            schema_evolution: false,
            table_creation: false,
        }
    }

    /// The `s3DestinationConfiguration` member this destination submits, if any.
    fn s3_json(&self) -> Option<Value> {
        let (compression, storage_class, output_prefix, freshness) = match self {
            Self::S3 {
                compression,
                storage_class,
                output_prefix,
                freshness,
            } => (*compression, *storage_class, *output_prefix, *freshness),
            Self::Both => ("GZIP", "STANDARD", None, None),
            Self::Iceberg { .. } | Self::Neither => return None,
        };
        let mut storage = json!({
            "bucketArn": RECORDS_BUCKET_ARN,
            "compressionType": compression,
            "storageClass": storage_class,
        });
        if let Some(prefix) = output_prefix {
            storage["outputPrefix"] = json!(prefix);
        }
        let mut configuration = json!({
            "deadLetterQueueS3": { "bucketArn": DLQ_BUCKET_ARN },
            "serviceExecutionRoleArn": ROLE_ARN,
            "storage": storage,
        });
        if let Some(seconds) = freshness {
            configuration["dataFreshnessInSeconds"] = json!(seconds);
        }
        Some(configuration)
    }

    /// The `icebergDestinationConfiguration` member this destination submits, if any.
    fn iceberg_json(&self) -> Option<Value> {
        let (append_only, compression, tables, catalog, schema_evolution, table_creation) =
            match self {
                Self::Iceberg {
                    append_only,
                    compression,
                    tables,
                    catalog,
                    schema_evolution,
                    table_creation,
                } => (
                    *append_only,
                    *compression,
                    *tables,
                    *catalog,
                    *schema_evolution,
                    *table_creation,
                ),
                Self::Both => (true, None, 1, false, false, false),
                Self::S3 { .. } | Self::Neither => return None,
            };
        let table_list: Vec<Value> = (0..tables)
            .map(|index| {
                json!({
                    "destinationDatabaseName": "records",
                    "destinationTableName": format!("topic_{index}"),
                    // `PartitionStrategy` names exactly one value, TIME_HOUR.
                    "partitionSpec": {
                        "partitionStrategy": "TIME_HOUR",
                        "sourceList": [{ "sourceName": "event_time" }],
                    },
                })
            })
            .collect();
        let mut configuration = json!({
            "appendOnly": append_only,
            "deadLetterQueueS3": { "bucketArn": DLQ_BUCKET_ARN },
            "destinationTableList": table_list,
            "schemaEvolution": { "enableSchemaEvolution": schema_evolution },
            "serviceExecutionRoleArn": ROLE_ARN,
            "tableCreation": { "enableTableCreation": table_creation },
        });
        if let Some(kind) = compression {
            configuration["compressionType"] = json!(kind);
        }
        if catalog {
            configuration["catalog"] = json!({
                "catalogArn": format!("arn:aws:glue:{REGION}:{ACCOUNT}:catalog"),
                "warehouseLocation": "s3://msk-channel-records/warehouse/",
            });
        }
        Some(configuration)
    }

    /// The `ChannelDestinationType` a created channel must report, or `None` when
    /// the row is one the service has to refuse.
    fn expected_type(&self) -> Option<&'static str> {
        match self {
            Self::S3 { .. } => Some("S3"),
            Self::Iceberg { .. } => Some("ICEBERG"),
            Self::Both | Self::Neither => None,
        }
    }
}

/// One topic configuration of a channel. `converter` is a `ValueConverter` value
/// (BYTE_ARRAY | JSON | JSON_SCHEMA_GSR | STRING) from the model's enum; the
/// GSR-schema converter is the one that carries a `recordSchema`.
#[derive(Debug, Clone)]
struct TopicRow {
    topic: &'static str,
    converter: &'static str,
}

impl TopicRow {
    fn to_json(&self) -> Value {
        let mut configuration = json!({
            "recordConverter": { "valueConverter": self.converter },
            "topicArn": format!("arn:aws:kafka:{REGION}:{ACCOUNT}:topic/channels/{}", self.topic),
        });
        if self.converter == "JSON_SCHEMA_GSR" {
            configuration["recordSchema"] = json!({
                "gsrArn": format!("arn:aws:glue:{REGION}:{ACCOUNT}:schema/records/{}", self.topic),
            });
        }
        configuration
    }
}

/// One row of a `CreateChannel` table: the required members plus whichever
/// optional ones the row exercises. An empty `Vec`/`None` means "the request omits
/// it", which is how a row tests what a reader reports for an absent member.
#[derive(Debug, Clone)]
struct ChannelRequest {
    name: String,
    topics: Vec<TopicRow>,
    destination: Destination,
    kms_key_arn: Option<String>,
    cloud_watch_log_group: Option<&'static str>,
    firehose_stream: Option<&'static str>,
    log_bucket: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

impl ChannelRequest {
    /// A minimal request: the three required members, with one topic and an S3
    /// destination, so every optional member is absent.
    fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            topics: vec![TopicRow {
                topic: "orders",
                converter: "JSON",
            }],
            destination: Destination::s3(),
            kms_key_arn: None,
            cloud_watch_log_group: None,
            firehose_stream: None,
            log_bucket: None,
            tags: Vec::new(),
        }
    }

    /// The `loggingInfo` member this row submits, if any.
    fn logging_json(&self) -> Option<Value> {
        let mut logging = Map::new();
        if let Some(group) = self.cloud_watch_log_group {
            logging.insert(
                "cloudWatchLogs".to_string(),
                json!({ "enabled": true, "logGroup": group }),
            );
        }
        if let Some(stream) = self.firehose_stream {
            logging.insert(
                "firehose".to_string(),
                json!({ "enabled": true, "deliveryStream": stream }),
            );
        }
        if let Some(bucket) = self.log_bucket {
            logging.insert(
                "s3".to_string(),
                json!({ "enabled": true, "bucket": bucket, "prefix": "channel-logs/" }),
            );
        }
        (!logging.is_empty()).then_some(Value::Object(logging))
    }

    fn to_json(&self) -> Value {
        let mut body = json!({
            "channelName": self.name,
            "topicConfigurationList": self
                .topics
                .iter()
                .map(TopicRow::to_json)
                .collect::<Vec<_>>(),
        });
        if let Some(configuration) = self.destination.s3_json() {
            body["s3DestinationConfiguration"] = configuration;
        }
        if let Some(configuration) = self.destination.iceberg_json() {
            body["icebergDestinationConfiguration"] = configuration;
        }
        if let Some(key) = &self.kms_key_arn {
            body["encryptionConfiguration"] = json!({ "kmsKeyArn": key });
        }
        if let Some(logging) = self.logging_json() {
            body["loggingInfo"] = logging;
        }
        if !self.tags.is_empty() {
            body["tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            );
        }
        body
    }
}

/// `CreateChannel` for `cluster_arn`, signed for `region`.
async fn create_channel_in(
    emulator: &Emulator,
    region: &str,
    cluster_arn: &str,
    request: &ChannelRequest,
) -> Reply {
    emulator
        .create_channel_in(region, cluster_arn, request.to_json())
        .await
}

/// `CreateChannel` for `cluster_arn` in the suite's region.
async fn create_channel(emulator: &Emulator, cluster_arn: &str, request: &ChannelRequest) -> Reply {
    create_channel_in(emulator, REGION, cluster_arn, request).await
}

/// `DescribeChannel`, signed for `region`.
async fn describe_channel_in(
    emulator: &Emulator,
    region: &str,
    cluster_arn: &str,
    channel_arn: &str,
) -> Reply {
    emulator
        .describe_channel_in(region, cluster_arn, channel_arn)
        .await
}

/// `DescribeChannel` in the suite's region.
async fn describe_channel(emulator: &Emulator, cluster_arn: &str, channel_arn: &str) -> Reply {
    describe_channel_in(emulator, REGION, cluster_arn, channel_arn).await
}

/// Create `request` under `cluster_arn`, assert it succeeded, and hand back the
/// minted channel ARN.
async fn create_channel_ok(
    emulator: &Emulator,
    cluster_arn: &str,
    request: &ChannelRequest,
) -> String {
    let reply = create_channel(emulator, cluster_arn, request).await;
    assert_eq!(
        reply.status, 200,
        "case {request:?}: expected CreateChannel to return 200, got {} {}",
        reply.status, reply.body
    );
    reply.body["channelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {request:?}: CreateChannel returned no channelArn"))
        .to_string()
}

/// Whether `value` is shaped like the UUID an AWS resource id carries: the
/// 8-4-4-4-12 hexadecimal grouping of RFC 4122.
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

/// A channel ARN names the cluster it streams from and then itself:
/// `arn:aws:kafka:<region>:<account>:channel/<cluster>/<cluster-id>/<name>/<id>`.
fn assert_channel_arn(arn: &str, cluster_arn: &str, channel_name: &str, case: &str) {
    let cluster_resource = cluster_arn
        .splitn(6, ':')
        .nth(5)
        .unwrap_or_default()
        .to_string();
    let mut cluster_parts = cluster_resource.split('/').skip(1);
    let cluster_name = cluster_parts.next().unwrap_or_default();
    let cluster_id = cluster_parts.next().unwrap_or_default();
    let expected_prefix =
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/{cluster_name}/{cluster_id}/{channel_name}/");
    assert!(
        arn.starts_with(&expected_prefix),
        "case {case}: expected a channel ARN starting {expected_prefix:?}, got {arn:?}"
    );
    let channel_id = &arn[expected_prefix.len()..];
    assert!(
        is_uuid_shaped(channel_id),
        "case {case}: expected a UUID-shaped channel id, got {channel_id:?}"
    );
}

/// RULE: a created channel is persisted whole under the cluster it streams from,
/// and `DescribeChannel` reports every member from that record — the submitted
/// value where the request carried one, nothing where it did not — with the
/// destination type DERIVED from which destination configuration was submitted.
#[tokio::test]
async fn describe_reports_exactly_what_was_created() {
    // The rows differ along everything the rule generalises over: destination
    // kind, each `S3CompressionType`/`S3StorageClass`/`IcebergCompressionType`
    // value, the Iceberg switches, destination-table count, record converter,
    // logging destination, encryption and tag count.
    let rows = vec![
        ChannelRequest::named("s3-defaults"),
        ChannelRequest {
            destination: Destination::S3 {
                compression: "GZIP",
                storage_class: "INTELLIGENT_TIERING",
                output_prefix: Some("records/"),
                freshness: Some(300),
            },
            topics: vec![TopicRow {
                topic: "payments",
                converter: "BYTE_ARRAY",
            }],
            kms_key_arn: Some(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-4333-8444-555555555555"
            )),
            cloud_watch_log_group: Some("/aws/msk/channels"),
            tags: vec![("env", "prod"), ("team", "data"), ("owner", "msk")],
            ..ChannelRequest::named("s3-gzip-intelligent")
        },
        ChannelRequest {
            destination: Destination::S3 {
                compression: "ZSTD",
                storage_class: "GLACIER_IR",
                output_prefix: None,
                freshness: Some(900),
            },
            topics: vec![TopicRow {
                topic: "clicks",
                converter: "STRING",
            }],
            firehose_stream: Some("channel-logs"),
            tags: vec![("only", "one")],
            ..ChannelRequest::named("s3-zstd-glacier")
        },
        ChannelRequest {
            destination: Destination::iceberg(),
            ..ChannelRequest::named("iceberg-defaults")
        },
        ChannelRequest {
            destination: Destination::Iceberg {
                append_only: false,
                compression: Some("SNAPPY"),
                tables: 3,
                catalog: true,
                schema_evolution: true,
                table_creation: true,
            },
            topics: vec![TopicRow {
                topic: "ledger",
                converter: "JSON_SCHEMA_GSR",
            }],
            log_bucket: Some("msk-channel-logs"),
            tags: vec![("env", "stage")],
            ..ChannelRequest::named("iceberg-everything")
        },
        ChannelRequest {
            destination: Destination::Iceberg {
                append_only: true,
                compression: Some("ZSTD"),
                tables: 2,
                catalog: false,
                schema_evolution: false,
                table_creation: true,
                },
            ..ChannelRequest::named("iceberg-zstd")
        },
    ];

    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("channel-host"))
        .await;

    let mut minted = Vec::new();
    for row in &rows {
        let reply = create_channel(&emulator, &cluster, row).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected CreateChannel to return 200, got {} {}",
            reply.status, reply.body
        );
        let arn = reply.body["channelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: CreateChannel returned no channelArn"))
            .to_string();
        assert_channel_arn(&arn, &cluster, &row.name, &row.name);
        // The create reports the cluster operation that is carrying it out; the
        // reader has to agree with it, which the describe below checks.
        let operation = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {row:?}: CreateChannel returned no clusterOperationArn")
            })
            .to_string();
        assert!(
            operation.starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/")),
            "case {row:?}: expected a cluster-operation ARN in this region, got {operation:?}"
        );
        // Read back IMMEDIATELY, inside the clock-owed span: a channel's pipeline
        // is still being stood up right after the create. Asserted here rather
        // than in the loop below, so the check does not depend on how long the
        // whole table takes.
        let fresh = describe_channel(&emulator, &cluster, &arn).await;
        assert_eq!(
            fresh.at("/status").as_str(),
            Some("CREATING"),
            "case {row:?}: expected status CREATING straight after the create, got {}",
            fresh.body
        );
        minted.push((arn, operation));
    }

    let mut distinct: Vec<&String> = minted.iter().map(|(arn, _)| arn).collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "every channel must get its own ARN, got {minted:?}"
    );

    for (row, (arn, operation)) in rows.iter().zip(&minted) {
        let reply = describe_channel(&emulator, &cluster, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected DescribeChannel to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/channelArn").as_str(),
            Some(arn.as_str()),
            "case {row:?}: DescribeChannel reported the wrong channelArn, got {}",
            reply.at("/channelArn")
        );
        assert_eq!(
            reply.at("/channelName").as_str(),
            Some(row.name.as_str()),
            "case {row:?}: DescribeChannel reported the wrong channelName, got {}",
            reply.at("/channelName")
        );
        assert_eq!(
            reply.at("/clusterOperationArn").as_str(),
            Some(operation.as_str()),
            "case {row:?}: the reader disagrees with the create about clusterOperationArn, got {}",
            reply.at("/clusterOperationArn")
        );
        assert_eq!(
            reply.at("/destinationType").as_str(),
            row.destination.expected_type(),
            "case {row:?}: expected destinationType {:?}, got {}",
            row.destination.expected_type(),
            reply.at("/destinationType")
        );
        // `CreationTime` targets `__timestampIso8601`, so the member is an RFC
        // 3339 date-time string on the wire and a number is the defect.
        assert!(
            reported_instant_secs(reply.at("/creationTime"), &format!("{row:?}")) > 0.0,
            "case {row:?}: expected a creationTime date-time, got {}",
            reply.at("/creationTime")
        );
        // The whole submitted configuration comes back, member for member.
        let submitted = row.to_json();
        for member in [
            "topicConfigurationList",
            "s3DestinationConfiguration",
            "icebergDestinationConfiguration",
            "encryptionConfiguration",
            "loggingInfo",
        ] {
            let expected = submitted.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                *reply.at(&format!("/{member}")),
                expected,
                "case {row:?}: expected {member} to round-trip as {expected}, got {}",
                reply.at(&format!("/{member}"))
            );
        }
        let tags = reply
            .at("/tags")
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            tags.len(),
            row.tags.len(),
            "case {row:?}: expected {} tags, got {tags:?}",
            row.tags.len()
        );
        for (key, value) in &row.tags {
            assert_eq!(
                tags.get(*key).and_then(Value::as_str),
                Some(*value),
                "case {row:?}: expected tag {key}={value}, got {:?}",
                tags.get(*key)
            );
        }
        // Nothing has gone wrong with the channel, so there is no state to explain.
        assert_eq!(
            *reply.at("/stateInfo"),
            Value::Null,
            "case {row:?}: expected no stateInfo, got {}",
            reply.at("/stateInfo")
        );
    }
}

/// RULE: a channel name identifies at most one channel WITHIN a cluster, so a
/// create under a name already taken in that cluster is refused with
/// `BadRequestException` (HTTP 400) naming that name, and leaves the existing
/// channel untouched — while the same name under another cluster is a different
/// channel.
#[tokio::test]
async fn a_repeated_channel_name_conflicts_within_its_cluster() {
    let emulator = Emulator::new();
    let first_cluster = emulator.create_ok(&ClusterRequest::named("host-one")).await;
    let second_cluster = emulator.create_ok(&ClusterRequest::named("host-two")).await;

    // Rows differ in how the second request differs from the first: identical,
    // a different destination kind, a different topic and logging — only the name
    // repeats.
    let rows = vec![
        (
            ChannelRequest::named("repeat-identical"),
            ChannelRequest::named("repeat-identical"),
        ),
        (
            ChannelRequest::named("repeat-other-destination"),
            ChannelRequest {
                destination: Destination::iceberg(),
                ..ChannelRequest::named("repeat-other-destination")
            },
        ),
        (
            ChannelRequest {
                destination: Destination::iceberg(),
                tags: vec![("env", "prod")],
                ..ChannelRequest::named("repeat-other-topic")
            },
            ChannelRequest {
                topics: vec![TopicRow {
                    topic: "elsewhere",
                    converter: "STRING",
                }],
                cloud_watch_log_group: Some("/aws/msk/other"),
                ..ChannelRequest::named("repeat-other-topic")
            },
        ),
    ];

    for (first, second) in &rows {
        let arn = create_channel_ok(&emulator, &first_cluster, first).await;
        let reply = create_channel(&emulator, &first_cluster, second).await;
        // The refusal is a bad request naming the channel name the second call
        // submitted, blaming the channel-name parameter by its wire name.
        assert_channel_name_already_exists(&reply, &format!("{second:?}"), &second.name, &arn);

        // The refused create changed nothing: the stored channel is still the first one.
        let described = describe_channel(&emulator, &first_cluster, &arn).await;
        assert_eq!(
            described.at("/destinationType").as_str(),
            first.destination.expected_type(),
            "case {second:?}: the refused create must not have changed the channel, got {}",
            described.body
        );

        // The same name under ANOTHER cluster is a different channel.
        let other = create_channel_ok(&emulator, &second_cluster, second).await;
        assert_ne!(
            other, arn,
            "case {second:?}: the same name under another cluster must mint a new channel"
        );
        assert_channel_arn(&other, &second_cluster, &second.name, &second.name);
    }
}

/// RULE: a channel is created under the cluster named in the request path, so a
/// `ClusterArn` this endpoint's region does not hold is refused with
/// `NotFoundException` (HTTP 404) and a value that is not a cluster ARN at all
/// with `BadRequestException` (HTTP 400) — and neither persists a channel.
#[tokio::test]
async fn a_create_for_an_absent_or_malformed_cluster_is_refused() {
    let emulator = Emulator::new();
    let live = emulator.create_ok(&ClusterRequest::named("live")).await;
    // A cluster that existed and was deleted is as absent as one that never did.
    let deleted = emulator.create_ok(&ClusterRequest::named("deleted")).await;
    // A cluster is deletable once it has finished provisioning, so it is waited
    // out of `CREATING` first.
    sleep_past_activation();
    let dropped = emulator
        .call("DELETE", &format!("/v1/clusters/{}", pct(&deleted)), None)
        .await;
    assert_eq!(
        dropped.status, 200,
        "expected the cluster delete to succeed, got {} {}",
        dropped.status, dropped.body
    );
    // A cluster in another region belongs to another endpoint.
    let elsewhere = emulator
        .call_in(
            "us-east-1",
            "POST",
            "/v1/clusters",
            Some(ClusterRequest::named("elsewhere").to_json()),
        )
        .await;
    assert_eq!(
        elsewhere.status, 200,
        "expected the other region's create to succeed, got {} {}",
        elsewhere.status, elsewhere.body
    );
    let elsewhere = elsewhere.body["clusterArn"]
        .as_str()
        .expect("the other region's create returned no clusterArn")
        .to_string();

    // `(cluster ARN, expected status, expected error code)`: a well-formed ARN no
    // live cluster holds, each way a cluster can be absent, and each way the ARN
    // grammar can fail.
    let rows: Vec<(String, u16, &str)> = vec![
        (
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
            ),
            404,
            "NotFoundException",
        ),
        (deleted, 404, "NotFoundException"),
        (elsewhere, 404, "NotFoundException"),
        ("not-an-arn".to_string(), 400, "BadRequestException"),
        (
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster/live/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/live/abc-1/c/9"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka::{ACCOUNT}:cluster/live/abc-1"),
            400,
            "BadRequestException",
        ),
    ];

    for (cluster_arn, status, code) in &rows {
        let request = ChannelRequest::named("orphan");
        let reply = create_channel(&emulator, cluster_arn, &request).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case cluster={cluster_arn:?}: expected {status} {code} from CreateChannel, got {} {}",
            reply.status,
            reply.body
        );
    }

    // The refusals persisted nothing: the name is still free under a live cluster.
    let arn = create_channel_ok(&emulator, &live, &ChannelRequest::named("orphan")).await;
    assert_channel_arn(&arn, &live, "orphan", "orphan-after-refusals");
}

/// RULE: a create must carry exactly one topic configuration
/// (`CreateChannelRequest$TopicConfigurationList`: "Currently exactly one topic
/// must be specified") and exactly one destination (the S3 and Iceberg
/// configurations are documented as mutually exclusive, and a channel has to
/// deliver somewhere); anything else is refused as a bad request.
#[tokio::test]
async fn a_create_must_carry_one_topic_and_one_destination() {
    let emulator = Emulator::new();
    let cluster = emulator.create_ok(&ClusterRequest::named("strict")).await;

    let topic = |name: &'static str| TopicRow {
        topic: name,
        converter: "JSON",
    };
    // Rows: each topic-list length around the documented one, and each way the
    // destination pair can be wrong. The last row is the control: one of each.
    let rows = vec![
        ChannelRequest {
            topics: Vec::new(),
            ..ChannelRequest::named("no-topics")
        },
        ChannelRequest {
            topics: vec![topic("orders"), topic("payments")],
            ..ChannelRequest::named("two-topics")
        },
        ChannelRequest {
            topics: vec![topic("orders"), topic("payments"), topic("clicks")],
            destination: Destination::iceberg(),
            ..ChannelRequest::named("three-topics")
        },
        ChannelRequest {
            destination: Destination::Both,
            ..ChannelRequest::named("both-destinations")
        },
        ChannelRequest {
            destination: Destination::Neither,
            ..ChannelRequest::named("no-destination")
        },
    ];

    for row in &rows {
        let reply = create_channel(&emulator, &cluster, row).await;
        assert_eq!(
            reply.status, 400,
            "case {row:?}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert!(
            matches!(
                reply.error_code(),
                "BadRequestException" | "ValidationException"
            ),
            "case {row:?}: expected a client-input refusal, got {}",
            reply.body
        );
        // A refused create persists nothing, so the name stays free.
        let described = describe_channel(
            &emulator,
            &cluster,
            &format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:channel/strict/abc-1/{}/11111111-2222-4333-8444-555555555555",
                row.name
            ),
        )
        .await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "NotFoundException"),
            "case {row:?}: a refused create must leave no channel behind, got {} {}",
            described.status,
            described.body
        );
    }

    // The control: exactly one topic and exactly one destination is accepted.
    let accepted = ChannelRequest {
        topics: vec![topic("orders"); TOPIC_CONFIGURATIONS_PER_CHANNEL],
        ..ChannelRequest::named("just-right")
    };
    let arn = create_channel_ok(&emulator, &cluster, &accepted).await;
    assert_channel_arn(&arn, &cluster, "just-right", "just-right");
}

/// RULE: a channel AWS returns in a non-terminal status reaches `ACTIVE` once the
/// clock owes it the transition, and stays there — a read never moves it back or
/// on.
#[tokio::test]
async fn a_new_channel_becomes_active_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let cluster = emulator.create_ok(&ClusterRequest::named("settling")).await;

    // Rows differ in destination kind, because the transition belongs to the
    // channel rather than to where it delivers.
    let rows = vec![
        ChannelRequest::named("settling-s3"),
        ChannelRequest {
            destination: Destination::iceberg(),
            ..ChannelRequest::named("settling-iceberg")
        },
    ];

    let mut minted = Vec::new();
    for row in &rows {
        let arn = create_channel_ok(&emulator, &cluster, row).await;
        let described = describe_channel(&emulator, &cluster, &arn).await;
        assert_eq!(
            described.at("/status").as_str(),
            Some("CREATING"),
            "case {row:?}: a caller reading straight after the create must still see CREATING, got {}",
            described.body
        );
        minted.push(arn);
    }

    sleep_past_channel_activation();
    for (row, arn) in rows.iter().zip(&minted) {
        // Read twice: the transition is materialised once and is idempotent.
        for attempt in 1..=2 {
            let described = describe_channel(&emulator, &cluster, arn).await;
            assert_eq!(
                described.at("/status").as_str(),
                Some("ACTIVE"),
                "case {row:?} (read {attempt}): expected ACTIVE once the clock owes it, got {}",
                described.body
            );
        }
    }
}

/// RULE: a channel is addressable only under the cluster that holds it and only
/// from that cluster's region, and it disappears with its cluster: every other
/// reader answers as if it never existed.
#[tokio::test]
async fn a_channel_is_only_addressable_under_its_own_cluster() {
    let emulator = Emulator::new();
    let owner = emulator.create_ok(&ClusterRequest::named("owner")).await;
    let other = emulator.create_ok(&ClusterRequest::named("other")).await;
    let arn = create_channel_ok(&emulator, &owner, &ChannelRequest::named("owned")).await;

    // Rows: the right channel under the wrong cluster, a well-formed channel ARN
    // nothing holds, and each way the channel ARN grammar can fail.
    let rows: Vec<(String, String, u16, &str)> = vec![
        (other.clone(), arn.clone(), 404, "NotFoundException"),
        (
            owner.clone(),
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:channel/owner/abc-1/absent/11111111-2222-4333-8444-555555555555"
            ),
            404,
            "NotFoundException",
        ),
        (
            owner.clone(),
            // The same absent channel, named without the channel id the service's
            // own records carry: the identity is the cluster, the cluster's id and
            // the channel name, so this resolves to a missing channel rather than
            // a malformed request.
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/owner/abc-1/absent"),
            404,
            "NotFoundException",
        ),
        (
            owner.clone(),
            "not-an-arn".to_string(),
            400,
            "BadRequestException",
        ),
        (
            owner.clone(),
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/owner/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            owner.clone(),
            // Too few segments to identify a channel at all: a channel ARN names
            // the cluster, the cluster's id and the channel.
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/owner"),
            400,
            "BadRequestException",
        ),
    ];

    for (cluster_arn, channel_arn, status, code) in &rows {
        let described = describe_channel(&emulator, cluster_arn, channel_arn).await;
        assert_eq!(
            (described.status, described.error_code()),
            (*status, *code),
            "case cluster={cluster_arn:?} channel={channel_arn:?}: expected {status} {code}, got {} {}",
            described.status,
            described.body
        );
    }

    // Every region that is not the channel's answers as if it did not exist.
    for region in ["us-east-1", "ap-southeast-2"] {
        let described = describe_channel_in(&emulator, region, &owner, &arn).await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected a 404 NotFoundException, got {} {}",
            described.status,
            described.body
        );
        let created = create_channel_in(
            &emulator,
            region,
            &owner,
            &ChannelRequest::named("cross-region"),
        )
        .await;
        assert_eq!(
            (created.status, created.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected CreateChannel to answer 404, got {} {}",
            created.status,
            created.body
        );
    }

    // Deleting the cluster takes its channels with it; a cluster is deletable once
    // it has finished provisioning, so it is waited out of `CREATING` first.
    sleep_past_activation();
    let dropped = emulator
        .call("DELETE", &format!("/v1/clusters/{}", pct(&owner)), None)
        .await;
    assert_eq!(
        dropped.status, 200,
        "expected the cluster delete to succeed, got {} {}",
        dropped.status, dropped.body
    );
    let described = describe_channel(&emulator, &owner, &arn).await;
    assert_eq!(
        (described.status, described.error_code()),
        (404, "NotFoundException"),
        "expected a deleted cluster's channel to be gone, got {} {}",
        described.status,
        described.body
    );
}

/// RULE: the channel registry is bounded per cluster, so a create past the
/// ceiling is refused with `TooManyRequestsException` (HTTP 429) instead of
/// growing the store with the number of calls — and the ceiling is per cluster,
/// so another cluster still accepts channels.
#[tokio::test]
async fn a_create_past_the_per_cluster_ceiling_is_refused() {
    let emulator = Emulator::new();
    let full = emulator.create_ok(&ClusterRequest::named("full")).await;
    let roomy = emulator.create_ok(&ClusterRequest::named("roomy")).await;

    for index in 0..MAX_CHANNELS_PER_CLUSTER {
        let request = ChannelRequest::named(&format!("channel-{index}"));
        let reply = create_channel(&emulator, &full, &request).await;
        assert_eq!(
            reply.status, 200,
            "case index={index}: expected the create to succeed below the ceiling, got {} {}",
            reply.status, reply.body
        );
    }

    let over = ChannelRequest::named("one-too-many");
    let reply = create_channel(&emulator, &full, &over).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (429, "TooManyRequestsException"),
        "expected a 429 TooManyRequestsException past the ceiling, got {} {}",
        reply.status,
        reply.body
    );

    // The ceiling is per cluster, not per emulator.
    let arn = create_channel_ok(&emulator, &roomy, &over).await;
    assert_channel_arn(&arn, &roomy, "one-too-many", "other-cluster-after-ceiling");
}
