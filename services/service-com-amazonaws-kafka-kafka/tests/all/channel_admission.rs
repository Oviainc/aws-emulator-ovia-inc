// SPDX-License-Identifier: CC0-1.0

//! `CreateChannel`'s ADMISSION path, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from.
//!
//! RULE 1 (the member's range): a channel destination's `dataFreshnessInSeconds`
//! must fall inside the range that member documents — 300 to 900 seconds
//! inclusive, on BOTH the S3 and the Iceberg destination configuration — so a
//! value outside it is refused as a bad request and no channel is minted, while
//! every value inside it (and an absent member) is accepted.
//!
//! RULE 2 (a name identifies one channel in its cluster): simultaneous
//! `CreateChannel` calls carrying one `ChannelName` against one cluster race to a
//! single winner; every loser is refused as a bad request whose sentence names
//! the requested channel name, and the cluster then holds exactly one channel
//! with that name — the winner's.
//!
//! The rows differ along what the rules generalise over: which destination
//! configuration carries the member, where the submitted value sits relative to
//! each published bound, how many callers race for one name, and the destination
//! the racing request delivers to. The crate's shared harness (`Emulator`,
//! `Reply`, `pct`, `ClusterRequest`) is reused rather than copied.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    MAX_DATA_FRESHNESS_SECONDS, MIN_DATA_FRESHNESS_SECONDS,
};

use crate::kafka_harness::{
    assert_channel_name_already_exists, pct, sleep_past_channel_activation, ClusterRequest,
    Emulator, Reply, ACCOUNT, REGION,
};

/// The IAM role a channel assumes to write to its destination; any role ARN does,
/// since the service stores what it was given.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/msk-channel";
/// The dead-letter bucket every row below points at.
const DLQ_BUCKET_ARN: &str = "arn:aws:s3:::msk-channel-dlq";
/// The destination bucket the S3 rows deliver to.
const RECORDS_BUCKET_ARN: &str = "arn:aws:s3:::msk-channel-records";

/// What MSK answers a `dataFreshnessInSeconds` outside the documented range:
/// fixed text naming the two published bounds, with the submitted value not
/// echoed into it. Written out here rather than imported from the
/// implementation, so an assertion cannot agree with a constant that is wrong.
const DATA_FRESHNESS_RANGE: &str = "Data freshness must be between 300 and 900 seconds.";

/// Which destination configuration a row submits. The member under RULE 1 is
/// documented identically on both
/// (`smithy_query(query="shape",shape="S3DestinationConfiguration,IcebergDestinationConfiguration")`:
/// "Allowed range: 300 to 900. Default: 600."), so both are driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DestinationKind {
    S3,
    Iceberg,
}

impl DestinationKind {
    /// The `ChannelDestinationType` a channel created with this destination
    /// reports.
    fn destination_type(self) -> &'static str {
        match self {
            Self::S3 => "S3",
            Self::Iceberg => "ICEBERG",
        }
    }
}

/// A `CreateChannel` body for `name` delivering through `kind`, carrying
/// `freshness` as `dataFreshnessInSeconds` when the row submits one.
fn create_body(name: &str, kind: DestinationKind, freshness: Option<i64>) -> Value {
    let mut destination = match kind {
        DestinationKind::S3 => json!({
            "deadLetterQueueS3": { "bucketArn": DLQ_BUCKET_ARN },
            "serviceExecutionRoleArn": ROLE_ARN,
            "storage": {
                "bucketArn": RECORDS_BUCKET_ARN,
                "compressionType": "NONE",
                "storageClass": "STANDARD",
            },
        }),
        DestinationKind::Iceberg => json!({
            "appendOnly": true,
            "deadLetterQueueS3": { "bucketArn": DLQ_BUCKET_ARN },
            "destinationTableList": [{
                "destinationDatabaseName": "records",
                "destinationTableName": "orders",
                // `PartitionStrategy` names exactly one value, TIME_HOUR.
                "partitionSpec": {
                    "partitionStrategy": "TIME_HOUR",
                    "sourceList": [{ "sourceName": "event_time" }],
                },
            }],
            "schemaEvolution": { "enableSchemaEvolution": false },
            "serviceExecutionRoleArn": ROLE_ARN,
            "tableCreation": { "enableTableCreation": true },
        }),
    };
    if let Some(seconds) = freshness {
        destination["dataFreshnessInSeconds"] = json!(seconds);
    }
    let member = match kind {
        DestinationKind::S3 => "s3DestinationConfiguration",
        DestinationKind::Iceberg => "icebergDestinationConfiguration",
    };
    let mut body = json!({
        "channelName": name,
        "topicConfigurationList": [{
            "recordConverter": { "valueConverter": "JSON" },
            "topicArn": format!("arn:aws:kafka:{REGION}:{ACCOUNT}:topic/channels/orders"),
        }],
    });
    body[member] = destination;
    body
}

/// `CreateChannel` under `cluster_arn`.
async fn create_channel(emulator: &Emulator, cluster_arn: &str, body: Value) -> Reply {
    emulator.create_channel(cluster_arn, body).await
}

/// `DescribeChannel` under `cluster_arn`.
async fn describe_channel(emulator: &Emulator, cluster_arn: &str, channel_arn: &str) -> Reply {
    emulator.describe_channel(cluster_arn, channel_arn).await
}

/// The ARNs the cluster's own listing reports for the channels called `name`,
/// walked to the last page — what a caller uses to answer "how many channels does
/// this name identify?".
async fn listed_channels_named(emulator: &Emulator, cluster_arn: &str, name: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(token) => format!("?nextToken={}", pct(token)),
            None => String::new(),
        };
        let listed = emulator.list_channels(cluster_arn, &query).await;
        assert_eq!(
            listed.status, 200,
            "expected ListChannels to return 200, got {} {}",
            listed.status, listed.body
        );
        for channel in listed.at("/channels").as_array().into_iter().flatten() {
            if channel.get("channelName").and_then(Value::as_str) == Some(name) {
                found.push(
                    channel
                        .get("channelArn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        match listed.at("/nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    found
}

/// One row of the freshness table: the value submitted and whether the member's
/// documented range admits it.
#[derive(Debug, Clone, Copy)]
struct FreshnessRow {
    case: &'static str,
    kind: DestinationKind,
    /// `None` is a request that omits the member, which the range cannot refuse.
    seconds: Option<i64>,
    accepted: bool,
}

/// The freshness table: for each destination configuration, each published bound,
/// one step inside and one step outside each of them, the documented default, an
/// absent member, and values far below and far above the range — including the
/// `0`, negative and tiny values a caller reaches for when it means "as fresh as
/// possible".
fn freshness_rows() -> Vec<FreshnessRow> {
    let mut rows = Vec::new();
    for kind in [DestinationKind::S3, DestinationKind::Iceberg] {
        for (case, seconds, accepted) in [
            ("absent", None, true),
            ("lower-bound", Some(300), true),
            ("one-inside-lower-bound", Some(301), true),
            ("documented-default", Some(600), true),
            ("one-inside-upper-bound", Some(899), true),
            ("upper-bound", Some(900), true),
            ("one-below-lower-bound", Some(299), false),
            ("one-above-upper-bound", Some(901), false),
            ("one-second", Some(1), false),
            ("zero", Some(0), false),
            ("negative", Some(-1), false),
            ("a-day", Some(86_400), false),
        ] {
            rows.push(FreshnessRow {
                case,
                kind,
                seconds,
                accepted,
            });
        }
    }
    rows
}

/// RULE 1: `dataFreshnessInSeconds` is admitted only inside the range its member
/// documents, whichever destination configuration carries it; outside it the
/// create is refused 400 `BadRequestException` with the range sentence and
/// persists nothing, and inside it the channel is created and reaches `ACTIVE`.
#[tokio::test]
async fn data_freshness_is_admitted_only_inside_the_documented_range() {
    // The range the rows are written against is the one the implementation
    // enforces, so a row cannot test a bound the service does not hold.
    assert_eq!(
        (MIN_DATA_FRESHNESS_SECONDS, MAX_DATA_FRESHNESS_SECONDS),
        (300, 900),
        "the documented data-freshness range is 300 to 900 seconds, got {MIN_DATA_FRESHNESS_SECONDS} to {MAX_DATA_FRESHNESS_SECONDS}"
    );

    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("freshness-host"))
        .await;

    let mut accepted: Vec<(FreshnessRow, String)> = Vec::new();
    for row in freshness_rows() {
        let name = format!("{}-{}", row.kind.destination_type().to_lowercase(), row.case);
        let reply = create_channel(&emulator, &cluster, create_body(&name, row.kind, row.seconds)).await;
        if row.accepted {
            assert_eq!(
                reply.status, 200,
                "case {row:?}: expected the create to be accepted, got {} {}",
                reply.status, reply.body
            );
            let arn = reply.at("/channelArn")
                .as_str()
                .unwrap_or_else(|| panic!("case {row:?}: the create returned no channelArn"))
                .to_string();
            accepted.push((row, arn));
            continue;
        }
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {row:?}: expected 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            DATA_FRESHNESS_RANGE,
            "case {row:?}: expected the freshness-range sentence, got {:?}",
            reply.error_message()
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("dataFreshnessInSeconds"),
            "case {row:?}: the refusal must name the member at fault, got {}",
            reply.body
        );
        // A refused create mints nothing: no ARN comes back, and the cluster
        // holds no channel under that name.
        assert_eq!(
            *reply.at("/channelArn"),
            Value::Null,
            "case {row:?}: a refused create must report no channelArn, got {}",
            reply.body
        );
        let listed = listed_channels_named(&emulator, &cluster, &name).await;
        assert!(
            listed.is_empty(),
            "case {row:?}: a refused create must leave no channel named {name:?}, got {listed:?}"
        );
        // The name is still free: the same request with a value inside the range
        // is accepted (the positive control for this row).
        let control = create_channel(
            &emulator,
            &cluster,
            create_body(&name, row.kind, Some(i64::from(MIN_DATA_FRESHNESS_SECONDS))),
        )
        .await;
        assert_eq!(
            control.status, 200,
            "case {row:?}: the same request inside the range must be accepted, got {} {}",
            control.status, control.body
        );
    }

    // Every accepted channel reads back with the value it was created with, and
    // reaches the terminal status the clock owes it.
    sleep_past_channel_activation();
    for (row, arn) in &accepted {
        let described = describe_channel(&emulator, &cluster, arn).await;
        assert_eq!(
            described.status, 200,
            "case {row:?}: expected DescribeChannel to return 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.at("/status").as_str(),
            Some("ACTIVE"),
            "case {row:?}: expected ACTIVE once the clock owes it, got {}",
            described.body
        );
        let member = match row.kind {
            DestinationKind::S3 => "/s3DestinationConfiguration/dataFreshnessInSeconds",
            DestinationKind::Iceberg => "/icebergDestinationConfiguration/dataFreshnessInSeconds",
        };
        assert_eq!(
            described.at(member).as_i64(),
            row.seconds,
            "case {row:?}: expected the submitted freshness to round-trip as {:?}, got {}",
            row.seconds,
            described.at(member)
        );
    }
}

/// One row of the race table: how many callers send the identical request, and
/// which destination it delivers to.
#[derive(Debug, Clone, Copy)]
struct RaceRow {
    case: &'static str,
    callers: usize,
    kind: DestinationKind,
    /// The freshness the racing request carries, inside the documented range.
    seconds: Option<i64>,
}

/// RULE 2: simultaneous identical creates of one channel name race to a single
/// winner, every loser is refused 400 `BadRequestException` naming that name, and
/// the cluster ends up holding exactly one channel with the name — the winner's,
/// which reads back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_creates_of_one_name_leave_one_winner() {
    // Rows differ in how many callers race, which destination the request
    // carries, and whether it carries the freshness member at all.
    let rows = [
        RaceRow {
            case: "two-callers-s3",
            callers: 2,
            kind: DestinationKind::S3,
            seconds: None,
        },
        RaceRow {
            case: "three-callers-iceberg",
            callers: 3,
            kind: DestinationKind::Iceberg,
            seconds: Some(600),
        },
        RaceRow {
            case: "five-callers-s3",
            callers: 5,
            kind: DestinationKind::S3,
            seconds: Some(900),
        },
    ];

    let emulator = Arc::new(Emulator::new());
    let cluster = emulator.create_ok(&ClusterRequest::named("race-host")).await;

    for row in rows {
        let body = create_body(row.case, row.kind, row.seconds);
        let mut racers = Vec::with_capacity(row.callers);
        for _ in 0..row.callers {
            let handle = Arc::clone(&emulator);
            let cluster = cluster.clone();
            let body = body.clone();
            racers.push(tokio::spawn(async move {
                create_channel(&handle, &cluster, body).await
            }));
        }

        let mut replies: Vec<Reply> = Vec::with_capacity(row.callers);
        for racer in racers {
            replies.push(racer.await.expect("a racing CreateChannel panicked"));
        }
        let mut winners: Vec<String> = replies
            .iter()
            .filter(|reply| reply.status == 200)
            .map(|reply| {
                reply
                    .at("/channelArn")
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {row:?}: an accepted create returned no channelArn")
                    })
                    .to_string()
            })
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "case {row:?}: exactly one racing create may win, got {winners:?}"
        );
        let winner = winners.remove(0);
        // Every loser of the race is refused the same way, naming the channel
        // name it submitted and never the winner's ARN.
        for reply in replies.iter().filter(|reply| reply.status != 200) {
            assert_channel_name_already_exists(reply, row.case, row.case, &winner);
        }

        // Exactly one channel with that name exists, and it is the winner's.
        let listed = listed_channels_named(&emulator, &cluster, row.case).await;
        assert_eq!(
            listed,
            vec![winner.clone()],
            "case {row:?}: the cluster must list exactly the winner's channel under {:?}, got {listed:?}",
            row.case
        );
        let described = describe_channel(&emulator, &cluster, &winner).await;
        assert_eq!(
            described.at("/channelArn").as_str(),
            Some(winner.as_str()),
            "case {row:?}: the winner's channel must read back, got {} {}",
            described.status,
            described.body
        );
        assert_eq!(
            described.at("/destinationType").as_str(),
            Some(row.kind.destination_type()),
            "case {row:?}: expected destinationType {:?}, got {}",
            row.kind.destination_type(),
            described.at("/destinationType")
        );

        // A later caller meets the same refusal the losers did: the rule is about
        // the name being taken, not about the calls arriving together.
        let repeat = create_channel(&emulator, &cluster, body).await;
        assert_channel_name_already_exists(&repeat, row.case, row.case, &winner);
    }
}
