// SPDX-License-Identifier: CC0-1.0

//! `UpdateChannel`, driven end to end over the same `MultiServiceRouter` the
//! emulator serves from.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request: the
//! rows differ along whatever the rule generalises over (destination kind, the
//! submitted `DataFreshnessInSeconds` including each published bound and a value
//! past it, how many successive updates one channel takes, the channel status the
//! call lands on, and the non-ASCII content of the members the update must NOT
//! touch). The harness — `Emulator`, `Reply`, `pct` — is the shared one in
//! `all/kafka_harness.rs`, reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    MAX_DATA_FRESHNESS_SECONDS, MIN_DATA_FRESHNESS_SECONDS,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_channel_activation, sleep_past_channel_update,
    ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The role a channel assumes to write to its destination; any role ARN does.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/msk-channel";
/// The dead-letter bucket every row below points at.
const DLQ_BUCKET_ARN: &str = "arn:aws:s3:::msk-channel-dlq";
/// The destination bucket the S3 rows deliver into.
const RECORDS_BUCKET_ARN: &str = "arn:aws:s3:::msk-channel-records";

/// The service's one fixed not-found sentence
/// (`service_com_amazonaws_kafka_kafka::state::RESOURCE_NOT_FOUND_MESSAGE`), with
/// U+2019 as its apostrophe.
const NOT_FOUND_MESSAGE: &str = "The requested resource doesn\u{2019}t exist.";

/// What MSK answers a channel-scoped call whose `ChannelArn` the cluster in the
/// path holds no channel for.
const INVALID_CHANNEL_ARN_MESSAGE: &str =
    "The specified link ARN isn\u{2019}t valid. Specify a valid link ARN, and try again.";

/// Where a channel delivers. The two values `ChannelDestinationType` names, which
/// is what an update payload has to agree with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    S3,
    Iceberg,
}

impl Destination {
    /// The `ChannelDestinationType` a channel with this destination reports.
    fn destination_type(self) -> &'static str {
        match self {
            Self::S3 => "S3",
            Self::Iceberg => "ICEBERG",
        }
    }

    /// The member a `CreateChannel` carries this destination's configuration under.
    fn configuration_member(self) -> &'static str {
        match self {
            Self::S3 => "s3DestinationConfiguration",
            Self::Iceberg => "icebergDestinationConfiguration",
        }
    }

    /// The member an `UpdateChannel` carries this destination's change under.
    fn update_member(self) -> &'static str {
        match self {
            Self::S3 => "s3DestinationUpdate",
            Self::Iceberg => "icebergDestinationUpdate",
        }
    }

    /// The other destination kind — the one an update naming it must be refused for.
    fn other(self) -> Self {
        match self {
            Self::S3 => Self::Iceberg,
            Self::Iceberg => Self::S3,
        }
    }
}

/// One row of the create table: the channel the update is then applied to. The
/// `prefix`/`table` and `tags` values are what the update must leave untouched, so
/// the rows carry non-ASCII text there.
#[derive(Debug, Clone)]
struct ChannelRow {
    name: &'static str,
    destination: Destination,
    /// `S3Storage$OutputPrefix` for an S3 row, the Iceberg destination table name
    /// for an Iceberg one.
    label: &'static str,
    /// The `dataFreshnessInSeconds` the create submits, or `None` to omit it.
    created_freshness: Option<i32>,
    tags: Vec<(&'static str, &'static str)>,
}

impl ChannelRow {
    /// A minimal S3 row under `name`, with ASCII text everywhere.
    fn s3(name: &'static str) -> Self {
        Self {
            name,
            destination: Destination::S3,
            label: "logger/records/",
            created_freshness: Some(300),
            tags: Vec::new(),
        }
    }

    fn to_json(&self) -> Value {
        let mut destination = match self.destination {
            Destination::S3 => json!({
                "deadLetterQueueS3": { "bucketArn": DLQ_BUCKET_ARN },
                "serviceExecutionRoleArn": ROLE_ARN,
                "storage": {
                    "bucketArn": RECORDS_BUCKET_ARN,
                    "compressionType": "ZSTD",
                    "storageClass": "STANDARD",
                    "outputPrefix": self.label,
                },
            }),
            Destination::Iceberg => json!({
                "appendOnly": true,
                "deadLetterQueueS3": { "bucketArn": DLQ_BUCKET_ARN },
                "destinationTableList": [{
                    "destinationDatabaseName": "records",
                    "destinationTableName": self.label,
                    "partitionSpec": {
                        "partitionStrategy": "TIME_HOUR",
                        "sourceList": [{ "sourceName": "event_time" }],
                    },
                }],
                "schemaEvolution": { "enableSchemaEvolution": true },
                "serviceExecutionRoleArn": ROLE_ARN,
                "tableCreation": { "enableTableCreation": true },
            }),
        };
        if let Some(seconds) = self.created_freshness {
            destination["dataFreshnessInSeconds"] = json!(seconds);
        }
        let mut body = json!({
            "channelName": self.name,
            "topicConfigurationList": [{
                "recordConverter": { "valueConverter": "JSON" },
                "topicArn": format!("arn:aws:kafka:{REGION}:{ACCOUNT}:topic/channels/orders"),
            }],
        });
        body[self.destination.configuration_member()] = destination;
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

/// `UpdateChannel` under `cluster_arn` for `channel_arn`, with `body` as given.
async fn update_channel(
    emulator: &Emulator,
    cluster_arn: &str,
    channel_arn: &str,
    body: Value,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!(
                "/v1/clusters/{}/channels/{}",
                pct(cluster_arn),
                pct(channel_arn)
            ),
            Some(body),
        )
        .await
}

/// The update payload that asks `destination` for `seconds`.
fn update_body(destination: Destination, seconds: i32) -> Value {
    json!({ destination.update_member(): { "dataFreshnessInSeconds": seconds } })
}

/// Create the channel `row` describes under `cluster_arn`, wait it out of
/// `CREATING`, and hand back its ARN.
async fn settled_channel(emulator: &Emulator, cluster_arn: &str, row: &ChannelRow) -> String {
    let created = emulator.create_channel(cluster_arn, row.to_json()).await;
    assert_eq!(
        created.status, 200,
        "case {row:?}: expected CreateChannel to return 200, got {} {}",
        created.status, created.body
    );
    let arn = created.body["channelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {row:?}: CreateChannel returned no channelArn"))
        .to_string();
    sleep_past_channel_activation();
    arn
}

/// The `dataFreshnessInSeconds` `DescribeChannel` reports for this channel's own
/// destination, or `None` when the member is absent.
fn reported_freshness(reply: &Reply, destination: Destination) -> Option<i64> {
    reply
        .at(&format!(
            "/{}/dataFreshnessInSeconds",
            destination.configuration_member()
        ))
        .as_i64()
}

/// The described channel with the three members an accepted update is allowed to
/// move removed, so what is left must be byte-identical before and after.
fn invariant_members(reply: &Reply, destination: Destination) -> Value {
    let mut body = reply.body.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("status");
        object.remove("clusterOperationArn");
        if let Some(configuration) = object
            .get_mut(destination.configuration_member())
            .and_then(Value::as_object_mut)
        {
            configuration.remove("dataFreshnessInSeconds");
        }
    }
    body
}

/// RULE: an accepted `UpdateChannel` writes the submitted
/// `DataFreshnessInSeconds` onto the destination the channel was created with and
/// changes NOTHING else about the channel — every other stored member comes back
/// byte-identical, non-ASCII text included — while the channel reports the
/// non-terminal `UPDATING` until the clock owes it `ACTIVE` again.
#[tokio::test]
async fn an_update_writes_the_submitted_freshness_and_leaves_every_other_member_untouched() {
    let emulator = Emulator::new();
    let cluster = emulator.create_ok(&ClusterRequest::named("updating")).await;

    // The rows differ along the destination kind, whether the create named the
    // member at all, the non-ASCII content of the members the update must not
    // touch, the tag count, and the sequence of values each channel is updated
    // through — each published bound of the documented range and a value between
    // them.
    let rows = vec![
        (
            ChannelRow {
                name: "canal-zhongwen",
                destination: Destination::S3,
                label: "logger/donnees-zhongwen/",
                created_freshness: Some(300),
                tags: vec![("sim2real-unicode", "donnees-\u{4e2d}\u{6587}-\u{1f6f8}")],
            },
            vec![600],
        ),
        (
            ChannelRow {
                name: "s3-walks-the-range",
                destination: Destination::S3,
                label: "logger/\u{44b}\u{43b}\u{430}\u{43d}/",
                created_freshness: None,
                tags: vec![("owner", "\u{4ea4}\u{6613}"), ("env", "prod")],
            },
            // Both published bounds and one value strictly between them, applied in
            // turn: the rule is about the value the request carries, not about one
            // step away from the create.
            vec![
                MIN_DATA_FRESHNESS_SECONDS,
                MAX_DATA_FRESHNESS_SECONDS,
                601,
                MIN_DATA_FRESHNESS_SECONDS,
            ],
        ),
        (
            ChannelRow {
                name: "iceberg-\u{e9}v\u{e9}nements",
                destination: Destination::Iceberg,
                label: "table_\u{e9}v\u{e9}nements",
                created_freshness: Some(900),
                tags: vec![("\u{30bf}\u{30b0}", "\u{5024}")],
            },
            vec![450, MAX_DATA_FRESHNESS_SECONDS],
        ),
    ];

    for (row, targets) in &rows {
        let arn = settled_channel(&emulator, &cluster, row).await;
        let before = emulator.describe_channel(&cluster, &arn).await;
        assert_eq!(
            before.at("/status").as_str(),
            Some("ACTIVE"),
            "case {row:?}: expected a settled channel to be ACTIVE, got {}",
            before.body
        );
        let create_operation = before
            .at("/clusterOperationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let invariant = invariant_members(&before, row.destination);
        let mut operations = vec![create_operation.clone()];

        for target in targets {
            let case = format!("{row:?} -> {target}");
            let updated =
                update_channel(&emulator, &cluster, &arn, update_body(row.destination, *target))
                    .await;
            assert_eq!(
                updated.status, 200,
                "case {case}: expected UpdateChannel to return 200, got {} {}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.at("/channelArn").as_str(),
                Some(arn.as_str()),
                "case {case}: UpdateChannel answered the wrong channelArn, got {}",
                updated.at("/channelArn")
            );
            let operation = updated
                .at("/clusterOperationArn")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: UpdateChannel returned no clusterOperationArn")
                })
                .to_string();
            assert!(
                operation.starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/")),
                "case {case}: expected a cluster-operation ARN of this region, got {operation:?}"
            );
            assert!(
                !operations.contains(&operation),
                "case {case}: every accepted update is its own operation, got {operation:?} again"
            );
            operations.push(operation.clone());

            // Read back INSIDE the clock-owed span: the change is applied and the
            // channel is reported as still being updated.
            let inflight = emulator.describe_channel(&cluster, &arn).await;
            assert_eq!(
                inflight.at("/status").as_str(),
                Some("UPDATING"),
                "case {case}: expected UPDATING straight after the update, got {}",
                inflight.body
            );
            assert_eq!(
                inflight.at("/clusterOperationArn").as_str(),
                Some(operation.as_str()),
                "case {case}: the reader disagrees with the update about the in-flight \
                 operation, got {}",
                inflight.at("/clusterOperationArn")
            );

            sleep_past_channel_update();
            let after = emulator.describe_channel(&cluster, &arn).await;
            assert_eq!(
                after.at("/status").as_str(),
                Some("ACTIVE"),
                "case {case}: expected ACTIVE once the clock owed it, got {}",
                after.body
            );
            assert_eq!(
                reported_freshness(&after, row.destination),
                Some(i64::from(*target)),
                "case {case}: expected dataFreshnessInSeconds {target}, got {}",
                after.body
            );
            assert_eq!(
                reported_freshness(&after, row.destination.other()),
                None,
                "case {case}: the channel must still hold exactly one destination, got {}",
                after.body
            );
            assert_eq!(
                invariant_members(&after, row.destination),
                invariant,
                "case {case}: expected every other member to survive the update as \
                 {invariant}, got {}",
                after.body
            );
        }
    }
}

/// RULE: the `ClusterOperationArn` a channel lifecycle call answers names a
/// cluster operation the cluster-operation readers resolve — non-terminal until the
/// clock owes the channel transition it carries out, and complete after it.
#[tokio::test]
async fn a_channel_operation_resolves_through_the_cluster_operation_readers() {
    let emulator = Emulator::new();
    let cluster = emulator.create_ok(&ClusterRequest::named("audited")).await;

    // One row per channel lifecycle call that answers an operation ARN, so the rule
    // is asserted for each rather than for the update alone.
    let created = emulator
        .create_channel(&cluster, ChannelRow::s3("audited-channel").to_json())
        .await;
    let arn = created.body["channelArn"].as_str().unwrap().to_string();
    let create_operation = created.body["clusterOperationArn"]
        .as_str()
        .unwrap()
        .to_string();
    sleep_past_channel_activation();

    let updated = update_channel(
        &emulator,
        &cluster,
        &arn,
        update_body(Destination::S3, MAX_DATA_FRESHNESS_SECONDS),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "expected UpdateChannel to return 200, got {} {}",
        updated.status, updated.body
    );
    let update_operation = updated.body["clusterOperationArn"]
        .as_str()
        .unwrap()
        .to_string();
    // Read before the span is out, so the operation the caller polls is observable
    // while it is still running.
    for (case, uri) in [
        ("v1", format!("/v1/operations/{}", pct(&update_operation))),
        ("v2", format!("/api/v2/operations/{}", pct(&update_operation))),
    ] {
        let reply = emulator.call("GET", &uri, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected the update's operation to resolve, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/clusterOperationInfo/operationState").as_str(),
            Some("UPDATE_IN_PROGRESS"),
            "case {case}: expected the update's operation to be in progress, got {}",
            reply.body
        );
        assert_eq!(
            reply.at("/clusterOperationInfo/operationType").as_str(),
            Some("UPDATE_CHANNEL"),
            "case {case}: expected operationType UPDATE_CHANNEL, got {}",
            reply.body
        );
    }
    sleep_past_channel_update();

    let deleted = emulator
        .call(
            "DELETE",
            &format!("/v1/clusters/{}/channels/{}", pct(&cluster), pct(&arn)),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteChannel to return 200, got {} {}",
        deleted.status, deleted.body
    );
    let delete_operation = deleted.body["clusterOperationArn"]
        .as_str()
        .unwrap()
        .to_string();

    for (case, operation, operation_type) in [
        ("create", &create_operation, "CREATE_CHANNEL"),
        ("update", &update_operation, "UPDATE_CHANNEL"),
        ("delete", &delete_operation, "DELETE_CHANNEL"),
    ] {
        let reply = emulator
            .call("GET", &format!("/v1/operations/{}", pct(operation)), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected the operation ARN to resolve, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/clusterOperationInfo/operationType").as_str(),
            Some(operation_type),
            "case {case}: expected operationType {operation_type}, got {}",
            reply.body
        );
        assert_eq!(
            reply.at("/clusterOperationInfo/clusterArn").as_str(),
            Some(cluster.as_str()),
            "case {case}: expected the operation to name its own cluster, got {}",
            reply.body
        );
    }
    // The create's and the update's spans are both out by now, so both report the
    // completion the channel transition they carried already reached.
    for (case, operation) in [
        ("create", &create_operation),
        ("update", &update_operation),
    ] {
        let reply = emulator
            .call("GET", &format!("/v1/operations/{}", pct(operation)), None)
            .await;
        assert_eq!(
            reply.at("/clusterOperationInfo/operationState").as_str(),
            Some("UPDATE_COMPLETE"),
            "case {case}: expected a finished operation, got {}",
            reply.body
        );
    }
}

/// RULE: only an `ACTIVE` channel is updatable — a channel in any other status is
/// refused with `BadRequestException` (HTTP 400) naming the status it is in, and
/// the refused update changes nothing.
#[tokio::test]
async fn only_an_active_channel_is_updatable() {
    let emulator = Emulator::new();
    let cluster = emulator.create_ok(&ClusterRequest::named("guarded")).await;

    // The rows are the three non-`ACTIVE` statuses a channel of this service
    // reaches, each produced by the call that puts it there.
    for status in ["CREATING", "UPDATING", "DELETING"] {
        let row = ChannelRow::s3(match status {
            "CREATING" => "guard-creating",
            "UPDATING" => "guard-updating",
            _ => "guard-deleting",
        });
        let created = emulator.create_channel(&cluster, row.to_json()).await;
        let arn = created.body["channelArn"].as_str().unwrap().to_string();
        match status {
            // Read straight after the create, inside the activation span.
            "CREATING" => {}
            "UPDATING" => {
                sleep_past_channel_activation();
                let accepted =
                    update_channel(&emulator, &cluster, &arn, update_body(Destination::S3, 600))
                        .await;
                assert_eq!(
                    accepted.status, 200,
                    "case {status}: the first update must be accepted, got {} {}",
                    accepted.status, accepted.body
                );
            }
            _ => {
                sleep_past_channel_activation();
                let accepted = emulator
                    .call(
                        "DELETE",
                        &format!("/v1/clusters/{}/channels/{}", pct(&cluster), pct(&arn)),
                        None,
                    )
                    .await;
                assert_eq!(
                    accepted.status, 200,
                    "case {status}: the delete must be accepted, got {} {}",
                    accepted.status, accepted.body
                );
            }
        }

        let before = emulator.describe_channel(&cluster, &arn).await;
        assert_eq!(
            before.at("/status").as_str(),
            Some(status),
            "case {status}: the row did not reach the status it is about, got {}",
            before.body
        );
        let stored = reported_freshness(&before, Destination::S3);

        let refused =
            update_channel(&emulator, &cluster, &arn, update_body(Destination::S3, 750)).await;
        assert_eq!(
            refused.status, 400,
            "case {status}: expected HTTP 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {status}: expected BadRequestException, got {}",
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            format!(
                "Channel cannot be updated in state {status}. \
                 Channel must be in ACTIVE state to be updated."
            ),
            "case {status}: unexpected refusal message, got {}",
            refused.body
        );
        let after = emulator.describe_channel(&cluster, &arn).await;
        assert_eq!(
            reported_freshness(&after, Destination::S3),
            stored,
            "case {status}: the refused update must change nothing, got {}",
            after.body
        );
    }
}

/// RULE: the `DataFreshnessInSeconds` an update submits must lie inside the range
/// the member documents (300 to 900, both bounds inclusive); a value outside it is
/// refused with `BadRequestException` (HTTP 400) and the stored value stands.
#[tokio::test]
async fn the_submitted_freshness_must_lie_inside_the_documented_range() {
    let emulator = Emulator::new();
    let cluster = emulator.create_ok(&ClusterRequest::named("bounded")).await;

    // Each published bound, a value one step past each, and the degenerate values a
    // client sends by mistake. The bounds come from
    // `smithy_query(query="shape",shape="S3DestinationUpdate")`:
    // "Allowed range: 300 to 900."
    let rows = [
        (MIN_DATA_FRESHNESS_SECONDS, true),
        (MIN_DATA_FRESHNESS_SECONDS + 1, true),
        (601, true),
        (MAX_DATA_FRESHNESS_SECONDS - 1, true),
        (MAX_DATA_FRESHNESS_SECONDS, true),
        (MIN_DATA_FRESHNESS_SECONDS - 1, false),
        (MAX_DATA_FRESHNESS_SECONDS + 1, false),
        (0, false),
        (-1, false),
        (i32::MAX, false),
    ];
    for (destination, name) in [
        (Destination::S3, "range-s3"),
        (Destination::Iceberg, "range-iceberg"),
    ] {
        let row = ChannelRow {
            destination,
            label: "range",
            ..ChannelRow::s3(name)
        };
        let arn = settled_channel(&emulator, &cluster, &row).await;
        for (seconds, accepted) in rows {
            let case = format!("{} {seconds}", destination.destination_type());
            let before = emulator.describe_channel(&cluster, &arn).await;
            let stored = reported_freshness(&before, destination);
            let reply =
                update_channel(&emulator, &cluster, &arn, update_body(destination, seconds)).await;
            if accepted {
                assert_eq!(
                    reply.status, 200,
                    "case {case}: a value inside the range must be accepted, got {} {}",
                    reply.status, reply.body
                );
                sleep_past_channel_update();
                let after = emulator.describe_channel(&cluster, &arn).await;
                assert_eq!(
                    reported_freshness(&after, destination),
                    Some(i64::from(seconds)),
                    "case {case}: expected the submitted value to be stored, got {}",
                    after.body
                );
                continue;
            }
            assert_eq!(
                reply.status, 400,
                "case {case}: a value outside the range must be refused, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {case}: expected BadRequestException, got {}",
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                format!(
                    "Data freshness must be between {MIN_DATA_FRESHNESS_SECONDS} and \
                     {MAX_DATA_FRESHNESS_SECONDS} seconds."
                ),
                "case {case}: unexpected refusal message, got {}",
                reply.body
            );
            let after = emulator.describe_channel(&cluster, &arn).await;
            assert_eq!(
                reported_freshness(&after, destination),
                stored,
                "case {case}: the refused update must change nothing, got {}",
                after.body
            );
        }
    }
}

/// RULE: an update must carry exactly one destination payload, and it must name the
/// destination kind the channel was created with; anything else is a
/// `BadRequestException` (HTTP 400) that changes nothing.
#[tokio::test]
async fn an_update_must_name_exactly_one_destination_of_the_channel_s_own_kind() {
    let emulator = Emulator::new();
    let cluster = emulator.create_ok(&ClusterRequest::named("matched")).await;

    for destination in [Destination::S3, Destination::Iceberg] {
        let row = ChannelRow {
            destination,
            label: "matched",
            ..ChannelRow::s3(match destination {
                Destination::S3 => "match-s3",
                Destination::Iceberg => "match-iceberg",
            })
        };
        let arn = settled_channel(&emulator, &cluster, &row).await;
        let stored = reported_freshness(
            &emulator.describe_channel(&cluster, &arn).await,
            destination,
        );

        // The rows differ along the cardinality of the payload (none, both) and
        // along naming the OTHER destination kind.
        let bodies = [
            ("neither", json!({})),
            (
                "both",
                json!({
                    "s3DestinationUpdate": { "dataFreshnessInSeconds": 600 },
                    "icebergDestinationUpdate": { "dataFreshnessInSeconds": 600 },
                }),
            ),
            ("the other kind", update_body(destination.other(), 600)),
        ];
        for (label, body) in bodies {
            let case = format!("{} {label}", destination.destination_type());
            let reply = update_channel(&emulator, &cluster, &arn, body).await;
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {case}: expected BadRequestException, got {}",
                reply.body
            );
            let after = emulator.describe_channel(&cluster, &arn).await;
            assert_eq!(
                reported_freshness(&after, destination),
                stored,
                "case {case}: the refused update must change nothing, got {}",
                after.body
            );
            assert_eq!(
                after.at("/status").as_str(),
                Some("ACTIVE"),
                "case {case}: a refused update must not move the channel, got {}",
                after.body
            );
        }
    }
}

/// RULE: the cluster in the request path is the first gate — a well-formed
/// `ClusterArn` this endpoint's region holds no cluster for is the service's one
/// fixed `NotFoundException` (HTTP 404) — and once it resolves, a `ChannelArn` that
/// cluster holds no channel for is a `BadRequestException` (HTTP 400).
#[tokio::test]
async fn an_update_resolves_its_cluster_before_its_channel() {
    let emulator = Emulator::new();
    let owner = emulator.create_ok(&ClusterRequest::named("owner")).await;
    let other = emulator.create_ok(&ClusterRequest::named("other")).await;
    let arn = settled_channel(&emulator, &owner, &ChannelRow::s3("owned")).await;
    // A cluster that existed and was deleted is as absent as one that never did.
    let deleted = emulator.create_ok(&ClusterRequest::named("gone")).await;
    sleep_past_activation();
    emulator
        .call("DELETE", &format!("/v1/clusters/{}", pct(&deleted)), None)
        .await;

    let ghost_cluster = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/never/11111111-2222-4333-8444-555555555555-9"
    );
    let ghost_channel = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:channel/owner/11111111-2222-4333-8444-555555555555-1/\
         ghost/66666666-7777-4888-8999-aaaaaaaaaaaa"
    );

    // The rows differ along which half of the path is wrong, and how.
    let rows: Vec<(&str, &str, &str, u16, &str, &str)> = vec![
        (
            "a cluster that never existed",
            &ghost_cluster,
            &arn,
            404,
            "NotFoundException",
            NOT_FOUND_MESSAGE,
        ),
        (
            "a cluster that was deleted",
            &deleted,
            &arn,
            404,
            "NotFoundException",
            NOT_FOUND_MESSAGE,
        ),
        (
            "a channel that never existed",
            &owner,
            &ghost_channel,
            400,
            "BadRequestException",
            INVALID_CHANNEL_ARN_MESSAGE,
        ),
        (
            "a channel of another cluster",
            &other,
            &arn,
            400,
            "BadRequestException",
            INVALID_CHANNEL_ARN_MESSAGE,
        ),
        (
            "a value that is no channel ARN at all",
            &owner,
            "not-an-arn",
            400,
            "BadRequestException",
            INVALID_CHANNEL_ARN_MESSAGE,
        ),
    ];
    for (case, cluster_arn, channel_arn, status, code, message) in rows {
        let reply =
            update_channel(&emulator, cluster_arn, channel_arn, update_body(Destination::S3, 600))
                .await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            message,
            "case {case}: unexpected refusal message, got {}",
            reply.body
        );
    }

    // None of those refusals touched the channel that does exist.
    let after = emulator.describe_channel(&owner, &arn).await;
    assert_eq!(
        reported_freshness(&after, Destination::S3),
        Some(i64::from(MIN_DATA_FRESHNESS_SECONDS)),
        "a refused update must leave the real channel's destination alone, got {}",
        after.body
    );
}
