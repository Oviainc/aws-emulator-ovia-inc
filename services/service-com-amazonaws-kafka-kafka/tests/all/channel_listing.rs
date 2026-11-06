// SPDX-License-Identifier: CC0-1.0

//! `ListChannels`, driven end to end over the same `MultiServiceRouter` the
//! emulator serves from.
//!
//! RULE: the listing REPORTS the channels its cluster holds — the records
//! `CreateChannel` persisted, rendered the way the channel's own reader renders
//! them, in the registry's order — and writes nothing of its own: it is scoped to
//! the cluster in the request path, `MaxResults` / `NextToken` page it without
//! dropping or repeating an entry, `TopicNameFilter` keeps the channels streaming
//! that topic, and a `ClusterArn` this endpoint's region holds no cluster for is
//! a missing resource rather than an empty page.
//!
//! The rows differ along what the rule generalises over: how many channels the
//! cluster holds, which destination each delivers to, which topic each streams,
//! the page size (including sizes that divide the list and ones that do not), the
//! filter value, which cluster, and which region.

use serde_json::{json, Value};

use crate::kafka_harness::{
    pct, sleep_past_channel_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The IAM role every channel below assumes to write to its destination.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/msk-channel";
/// The dead-letter bucket every channel below points at.
const DLQ_BUCKET_ARN: &str = "arn:aws:s3:::msk-channel-dlq";

/// One channel a row creates: its name, the topic it streams and where it
/// delivers.
#[derive(Debug, Clone, Copy)]
struct ChannelRow {
    name: &'static str,
    topic: &'static str,
    iceberg: bool,
}

impl ChannelRow {
    /// The `CreateChannel` body for this channel.
    fn to_json(self) -> Value {
        let mut body = json!({
            "channelName": self.name,
            "topicConfigurationList": [{
                "recordConverter": { "valueConverter": "JSON" },
                "topicArn": format!(
                    "arn:aws:kafka:{REGION}:{ACCOUNT}:topic/channels/{}",
                    self.topic
                ),
            }],
        });
        if self.iceberg {
            body["icebergDestinationConfiguration"] = json!({
                "appendOnly": true,
                "deadLetterQueueS3": { "bucketArn": DLQ_BUCKET_ARN },
                "destinationTableList": [{
                    "destinationDatabaseName": "records",
                    "destinationTableName": self.topic,
                    // `PartitionStrategy` names exactly one value, TIME_HOUR.
                    "partitionSpec": {
                        "partitionStrategy": "TIME_HOUR",
                        "sourceList": [{ "sourceName": "event_time" }],
                    },
                }],
                "schemaEvolution": { "enableSchemaEvolution": false },
                "serviceExecutionRoleArn": ROLE_ARN,
                "tableCreation": { "enableTableCreation": true },
            });
        } else {
            body["s3DestinationConfiguration"] = json!({
                "deadLetterQueueS3": { "bucketArn": DLQ_BUCKET_ARN },
                "serviceExecutionRoleArn": ROLE_ARN,
                "storage": {
                    "bucketArn": "arn:aws:s3:::msk-channel-records",
                    "compressionType": "NONE",
                    "storageClass": "STANDARD",
                },
            });
        }
        body
    }

    /// The `ChannelDestinationType` a channel created from this row reports.
    fn destination_type(self) -> &'static str {
        match self.iceberg {
            true => "ICEBERG",
            false => "S3",
        }
    }
}

/// The channels the suite's cluster holds: destination kinds and topics varied,
/// and the names deliberately NOT in creation order, so an ordering assertion
/// cannot pass by accident.
fn channel_rows() -> Vec<ChannelRow> {
    vec![
        ChannelRow {
            name: "orders-s3",
            topic: "orders",
            iceberg: false,
        },
        ChannelRow {
            name: "clicks-iceberg",
            topic: "clicks",
            iceberg: true,
        },
        ChannelRow {
            name: "payments-s3",
            topic: "payments",
            iceberg: false,
        },
        ChannelRow {
            name: "audit-iceberg",
            topic: "orders",
            iceberg: true,
        },
        ChannelRow {
            name: "ledger-s3",
            topic: "ledger",
            iceberg: false,
        },
    ]
}

/// `CreateChannel` under `cluster_arn`.
async fn create_channel(emulator: &Emulator, cluster_arn: &str, body: Value) -> Reply {
    emulator.create_channel(cluster_arn, body).await
}

/// `ListChannels` under `cluster_arn`, signed for `region`, with `query` appended
/// as given (`""` for a request that carries no query member).
async fn list_channels_in(
    emulator: &Emulator,
    region: &str,
    cluster_arn: &str,
    query: &str,
) -> Reply {
    emulator.list_channels_in(region, cluster_arn, query).await
}

/// `ListChannels` in the suite's region.
async fn list_channels(emulator: &Emulator, cluster_arn: &str, query: &str) -> Reply {
    emulator.list_channels(cluster_arn, query).await
}

/// `DescribeChannel` under `cluster_arn`.
async fn describe_channel(emulator: &Emulator, cluster_arn: &str, channel_arn: &str) -> Reply {
    emulator.describe_channel(cluster_arn, channel_arn).await
}

/// The `channels` entries of a successful listing.
fn entries(reply: &Reply, case: &str) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListChannels to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/channels")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: the listing carried no channels list: {}", reply.body))
        .clone()
}

/// A member of one listing entry, as a string.
fn member(entry: &Value, name: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Walk every page of `cluster_arn`'s listing at `page_size`, asserting each page
/// but the last is full, and hand back the names in the order they were reported.
async fn paged_names(
    emulator: &Emulator,
    cluster_arn: &str,
    page_size: usize,
    case: &str,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    // One page more than the list can need, so a listing that never stops paging
    // fails the case instead of hanging it.
    for page in 0..=(channel_rows().len() + 2) {
        let query = match &token {
            Some(token) => format!("?maxResults={page_size}&nextToken={}", pct(token)),
            None => format!("?maxResults={page_size}"),
        };
        let reply = list_channels(emulator, cluster_arn, &query).await;
        let page_entries = entries(&reply, case);
        assert!(
            page_entries.len() <= page_size,
            "case {case} (page {page}): a page may hold at most {page_size} entries, got {}",
            page_entries.len()
        );
        for entry in &page_entries {
            names.push(member(entry, "channelName"));
        }
        match reply.at("/nextToken").as_str() {
            Some(next) => {
                assert_eq!(
                    page_entries.len(),
                    page_size,
                    "case {case} (page {page}): a page that carries a nextToken must be full, got {}",
                    page_entries.len()
                );
                token = Some(next.to_string());
            }
            None => return names,
        }
    }
    panic!("case {case}: the listing never stopped paging, got {names:?}");
}

/// RULE: the listing reports every channel the cluster holds, rendered as the
/// channel's own reader renders it, in name order and once each; it pages at any
/// `MaxResults` without dropping or repeating an entry; and it is a pure read, so
/// listing twice reports the same thing.
#[tokio::test]
async fn the_listing_reports_every_channel_its_cluster_holds() {
    let rows = channel_rows();
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("listing-host"))
        .await;
    // A second cluster with a channel of its own: the listing is scoped to the
    // cluster in the request path, so this one must never appear below.
    let other = emulator
        .create_ok(&ClusterRequest::named("listing-other"))
        .await;

    // An empty cluster reports an empty list, not a missing resource: the cluster
    // exists and holds no channel.
    let empty = list_channels(&emulator, &cluster, "").await;
    assert_eq!(
        entries(&empty, "empty-cluster").len(),
        0,
        "case \"empty-cluster\": expected no channels, got {}",
        empty.body
    );
    assert_eq!(
        *empty.at("/nextToken"),
        Value::Null,
        "case \"empty-cluster\": a complete listing carries no nextToken, got {}",
        empty.body
    );

    let mut created: Vec<(ChannelRow, String, String)> = Vec::new();
    for row in &rows {
        let reply = create_channel(&emulator, &cluster, row.to_json()).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected CreateChannel to return 200, got {} {}",
            reply.status, reply.body
        );
        let arn = reply
            .at("/channelArn")
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: the create returned no channelArn"))
            .to_string();
        let operation = reply
            .at("/clusterOperationArn")
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: the create returned no clusterOperationArn"))
            .to_string();
        created.push((*row, arn, operation));
    }
    create_channel(
        &emulator,
        &other,
        ChannelRow {
            name: "elsewhere",
            topic: "orders",
            iceberg: false,
        }
        .to_json(),
    )
    .await;

    // The whole listing, with no page member: every channel of this cluster, in
    // the registry's order, which is by channel name.
    let listed = list_channels(&emulator, &cluster, "").await;
    let reported = entries(&listed, "whole-listing");
    let mut expected_names: Vec<&str> = rows.iter().map(|row| row.name).collect();
    expected_names.sort_unstable();
    let reported_names: Vec<String> = reported
        .iter()
        .map(|entry| member(entry, "channelName"))
        .collect();
    assert_eq!(
        reported_names, expected_names,
        "case \"whole-listing\": expected every channel once, in name order, got {reported_names:?}"
    );
    assert_eq!(
        *listed.at("/nextToken"),
        Value::Null,
        "case \"whole-listing\": a complete listing carries no nextToken, got {}",
        listed.body
    );

    // Each entry renders the stored record: the ARN and the destination type the
    // create reported, and the status its own reader reports at the same time.
    // `ClusterOperationArn` is "returned only while the channel is in CREATING,
    // UPDATING, or DELETING" (`ChannelInfo$ClusterOperationArn`), so an entry
    // carries it exactly while the status it reports is non-terminal.
    for (row, arn, operation) in &created {
        let entry = reported
            .iter()
            .find(|entry| member(entry, "channelName") == row.name)
            .unwrap_or_else(|| panic!("case {row:?}: the listing left the channel out"));
        assert_eq!(
            member(entry, "channelArn"),
            *arn,
            "case {row:?}: expected the created ARN in the listing, got {entry}"
        );
        assert_eq!(
            member(entry, "destinationType"),
            row.destination_type(),
            "case {row:?}: expected destinationType {:?}, got {entry}",
            row.destination_type()
        );
        let status = member(entry, "status");
        assert!(
            ["CREATING", "ACTIVE"].contains(&status.as_str()),
            "case {row:?}: expected a created channel's status, got {status:?}"
        );
        assert_eq!(
            entry.get("clusterOperationArn").and_then(Value::as_str),
            (status == "CREATING").then_some(operation.as_str()),
            "case {row:?}: the in-flight operation must be reported exactly while the channel is \
             CREATING (status {status:?}), got {entry}"
        );
        // `CreationTime` targets `__timestampIso8601`, so it is an RFC 3339
        // date-time string on the wire and a number is the defect.
        let created_at = member(entry, "creationTime");
        assert!(
            ::aws_smithy_types::DateTime::from_str(
                &created_at,
                ::aws_smithy_types::date_time::Format::DateTime
            )
            .is_ok(),
            "case {row:?}: expected an ISO-8601 creationTime, got {created_at:?}"
        );
    }

    // Every page size: ones that divide the list, ones that do not, one page per
    // entry, and one far larger than the list.
    for page_size in [1, 2, 3, rows.len(), rows.len() + 1, 100] {
        let paged = paged_names(&emulator, &cluster, page_size, &format!("maxResults={page_size}")).await;
        assert_eq!(
            paged, expected_names,
            "case maxResults={page_size}: paging must report every channel once, in order, got {paged:?}"
        );
    }

    // A pure read: the listing reports the same thing again, and the channels it
    // reported still read back one by one.
    let again = list_channels(&emulator, &cluster, "").await;
    assert_eq!(
        *again.at("/channels"),
        *listed.at("/channels"),
        "listing twice must report the same channels, got {}",
        again.body
    );
    sleep_past_channel_activation();
    let settled = list_channels(&emulator, &cluster, "").await;
    for entry in entries(&settled, "settled-listing") {
        let name = member(&entry, "channelName");
        assert_eq!(
            member(&entry, "status"),
            "ACTIVE",
            "case {name}: expected ACTIVE once the clock owes it, got {entry}"
        );
        assert_eq!(
            entry.get("clusterOperationArn"),
            None,
            "case {name}: a settled channel has no operation in flight, got {entry}"
        );
        let described = describe_channel(&emulator, &cluster, &member(&entry, "channelArn")).await;
        assert_eq!(
            (
                described.at("/channelName").as_str(),
                described.at("/status").as_str()
            ),
            (Some(name.as_str()), Some("ACTIVE")),
            "case {name}: the listing and the reader must agree, got {} {}",
            described.status,
            described.body
        );
    }

    // The other cluster's channel was never in this cluster's listing, and its own
    // listing holds only it.
    let others = entries(&list_channels(&emulator, &other, "").await, "other-cluster");
    let other_names: Vec<String> = others
        .iter()
        .map(|entry| member(entry, "channelName"))
        .collect();
    assert_eq!(
        other_names,
        vec!["elsewhere".to_string()],
        "case \"other-cluster\": a listing is scoped to its own cluster, got {other_names:?}"
    );
}

/// RULE: `TopicNameFilter` keeps the channels that stream the named topic, and a
/// request whose cluster or page token does not resolve is refused rather than
/// answered with a page.
#[tokio::test]
async fn the_listing_filters_by_topic_and_refuses_what_it_cannot_resolve() {
    let rows = channel_rows();
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("filter-host"))
        .await;
    for row in &rows {
        let reply = create_channel(&emulator, &cluster, row.to_json()).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected CreateChannel to return 200, got {} {}",
            reply.status, reply.body
        );
    }

    // One row per topic the channels stream — including `orders`, which two of
    // them stream — plus a topic no channel streams and a prefix of a topic name,
    // which is not the topic's name.
    for topic in ["orders", "clicks", "payments", "ledger", "absent", "order"] {
        let reply = list_channels(&emulator, &cluster, &format!("?topicNameFilter={topic}")).await;
        let reported: Vec<String> = entries(&reply, topic)
            .iter()
            .map(|entry| member(entry, "channelName"))
            .collect();
        let mut expected: Vec<&str> = rows
            .iter()
            .filter(|row| row.topic == topic)
            .map(|row| row.name)
            .collect();
        expected.sort_unstable();
        assert_eq!(
            reported, expected,
            "case topicNameFilter={topic}: expected {expected:?}, got {reported:?}"
        );
    }

    // The filter and the page member compose: one channel per page over the two
    // channels that stream `orders`.
    let first = list_channels(&emulator, &cluster, "?topicNameFilter=orders&maxResults=1").await;
    let first_entries = entries(&first, "filtered-page-one");
    assert_eq!(
        first_entries.len(),
        1,
        "case \"filtered-page-one\": expected one entry, got {}",
        first.body
    );
    let token = first
        .at("/nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("case \"filtered-page-one\": expected a nextToken, got {}", first.body))
        .to_string();
    let second = list_channels(
        &emulator,
        &cluster,
        &format!("?topicNameFilter=orders&maxResults=1&nextToken={}", pct(&token)),
    )
    .await;
    let second_entries = entries(&second, "filtered-page-two");
    assert_eq!(
        second_entries.len(),
        1,
        "case \"filtered-page-two\": expected one entry, got {}",
        second.body
    );
    assert_ne!(
        member(&first_entries[0], "channelArn"),
        member(&second_entries[0], "channelArn"),
        "case \"filtered-page-two\": a second page must not repeat the first"
    );

    // Rows: each way the request can fail to resolve, with the status and code it
    // is refused with. A cluster ARN another endpoint's region holds, one no
    // cluster holds, and a value that is not a cluster ARN at all.
    let absent = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
    );
    let rows: Vec<(String, String, u16, &str)> = vec![
        (absent, String::new(), 404, "NotFoundException"),
        (
            "not-an-arn".to_string(),
            String::new(),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster/filter-host/abc-1"),
            String::new(),
            400,
            "BadRequestException",
        ),
        // A token this listing never minted, and one minted for another list.
        (
            cluster.clone(),
            "?nextToken=not-a-token".to_string(),
            400,
            "BadRequestException",
        ),
        (
            cluster.clone(),
            format!(
                "?nextToken={}",
                pct(&::aws_smithy_types::base64::encode("kafka:topics:whatever#x"))
            ),
            400,
            "BadRequestException",
        ),
    ];
    for (cluster_arn, query, status, code) in &rows {
        let reply = list_channels(&emulator, cluster_arn, query).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case cluster={cluster_arn:?} query={query:?}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
    }

    // Every region that is not the cluster's holds no such cluster, so it has no
    // list to serve.
    for region in ["us-east-1", "ap-southeast-2"] {
        let reply = list_channels_in(&emulator, region, &cluster, "").await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected a 404 NotFoundException, got {} {}",
            reply.status,
            reply.body
        );
    }
}
