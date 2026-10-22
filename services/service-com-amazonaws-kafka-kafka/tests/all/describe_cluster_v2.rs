// SPDX-License-Identifier: CC0-1.0

//! `DescribeClusterV2` (`GET /api/v2/clusters/{ClusterArn}`), driven end to end over
//! the same router the emulator serves from.
//!
//! The rule under test is that the v2 reader reports the SAME persisted cluster the
//! v1 reader does, regrouped into the v2 `Cluster` shape: the members every cluster
//! type has (`ClusterArn`, `ClusterName`, `ClusterType`, `State`, `CurrentVersion`,
//! `CreationTime`, `Tags`) at the top level and the broker-node ones under
//! `Provisioned`. Each suite walks a TABLE whose rows differ along what the rule
//! generalises over (broker count, Kafka version, encryption, authentication,
//! storage mode, monitoring level, tag count, region), never one recorded request.
//!
//! The harness is the shared `crate::kafka_harness`, so these suites and every
//! other one in this crate drive the service identically.

use serde_json::Value;

use crate::kafka_harness::{
    pct, sleep_past_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// `DescribeClusterV2`'s URI, per the model's `http` binding on the operation.
fn describe_v2_uri(arn: &str) -> String {
    format!("/api/v2/clusters/{}", pct(arn))
}

/// Call `DescribeClusterV2` for `arn` in the suite's region.
async fn describe_v2(emulator: &Emulator, arn: &str) -> Reply {
    emulator.call("GET", &describe_v2_uri(arn), None).await
}

/// The members the v2 `Cluster` shape keeps at the top level, as opposed to the
/// ones the model puts under `Provisioned` — asserted member by member below.
const TOP_LEVEL_STRING_MEMBERS: &[&str] = &["clusterArn", "clusterName", "clusterType", "state"];

/// RULE: `DescribeClusterV2` reports the stored cluster as the v2 `Cluster` shape —
/// `ClusterType` `PROVISIONED` with every broker-node member under `Provisioned` —
/// computed from the persisted record, so it agrees with `DescribeCluster` member
/// for member on the same cluster.
#[tokio::test]
async fn describe_v2_reports_the_stored_cluster_in_the_v2_shape() {
    // The rows differ along everything the rule generalises over: broker count (both
    // ends of the model's 1..15 range), Kafka version, client-broker encryption
    // (absent and each `ClientBroker` value), SASL mechanism, `StorageMode`,
    // `EnhancedMonitoring` and tag count.
    let rows = vec![
        ClusterRequest::named("v2-defaults"),
        ClusterRequest {
            brokers: 1,
            kafka_version: "2.8.1",
            subnets: vec!["subnet-a"],
            ..ClusterRequest::named("v2-single-broker")
        },
        ClusterRequest {
            brokers: 15,
            instance_type: "kafka.m7g.xlarge",
            client_broker: Some("TLS"),
            sasl_iam: Some(true),
            storage_mode: Some("TIERED"),
            enhanced_monitoring: Some("PER_TOPIC_PER_PARTITION"),
            tags: vec![("env", "prod"), ("team", "data"), ("owner", "msk")],
            ..ClusterRequest::named("v2-fifteen-brokers")
        },
        ClusterRequest {
            client_broker: Some("PLAINTEXT"),
            sasl_scram: Some(true),
            enhanced_monitoring: Some("PER_BROKER"),
            tags: vec![("only", "one")],
            ..ClusterRequest::named("v2-plaintext-scram")
        },
        ClusterRequest {
            client_broker: Some("TLS_PLAINTEXT"),
            storage_mode: Some("LOCAL"),
            public_access: Some("SERVICE_PROVIDED_EIPS"),
            network_type: Some("DUAL"),
            ..ClusterRequest::named("v2-explicit-defaults")
        },
    ];

    let emulator = Emulator::new();
    let mut minted = Vec::new();
    for row in &rows {
        minted.push(emulator.create_ok(row).await);
    }
    sleep_past_activation();

    for (row, arn) in rows.iter().zip(&minted) {
        let reply = describe_v2(&emulator, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected DescribeClusterV2 to return 200, got {} {}",
            reply.status, reply.body
        );
        let cluster = reply.at("/clusterInfo");

        for (member, expected) in [
            ("/clusterArn", arn.as_str()),
            ("/clusterName", row.name.as_str()),
            // A cluster minted by `CreateCluster` is a provisioned one, which is
            // the `ClusterType` the v2 shape must report for it.
            ("/clusterType", "PROVISIONED"),
            ("/state", "ACTIVE"),
            (
                "/provisioned/currentBrokerSoftwareInfo/kafkaVersion",
                row.kafka_version,
            ),
            ("/provisioned/storageMode", row.storage_mode.unwrap_or("LOCAL")),
            (
                "/provisioned/enhancedMonitoring",
                row.enhanced_monitoring.unwrap_or("DEFAULT"),
            ),
            (
                "/provisioned/encryptionInfo/encryptionInTransit/clientBroker",
                row.client_broker.unwrap_or("TLS_PLAINTEXT"),
            ),
            (
                "/provisioned/brokerNodeGroupInfo/instanceType",
                row.instance_type,
            ),
            ("/provisioned/customerActionStatus", "NONE"),
        ] {
            assert_eq!(
                cluster.pointer(member).and_then(Value::as_str),
                Some(expected),
                "case {row:?}: expected {member} to be {expected:?}, got {:?}",
                cluster.pointer(member)
            );
        }

        assert_eq!(
            cluster
                .pointer("/provisioned/numberOfBrokerNodes")
                .and_then(Value::as_i64),
            Some(row.brokers),
            "case {row:?}: expected provisioned.numberOfBrokerNodes {}, got {:?}",
            row.brokers,
            cluster.pointer("/provisioned/numberOfBrokerNodes")
        );
        assert_eq!(
            cluster
                .pointer("/provisioned/brokerNodeGroupInfo/clientSubnets")
                .and_then(Value::as_array)
                .map(|subnets| subnets
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()),
            Some(row.subnets.clone()),
            "case {row:?}: expected the submitted clientSubnets, got {:?}",
            cluster.pointer("/provisioned/brokerNodeGroupInfo/clientSubnets")
        );

        // ZooKeeper strings list one endpoint per ensemble node, so the v2 reader
        // must carry the same persisted strings the v1 one does.
        for member in [
            "/provisioned/zookeeperConnectString",
            "/provisioned/zookeeperConnectStringTls",
        ] {
            let value = cluster
                .pointer(member)
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                value.contains(&format!(".kafka.{REGION}.amazonaws.com:")),
                "case {row:?}: expected {member} to name this region's ZooKeeper endpoints, got {value:?}"
            );
        }

        // Tags are a top-level member of the v2 shape, and every submitted pair
        // must come back.
        let tags = cluster.pointer("/tags").cloned().unwrap_or(Value::Null);
        for (key, expected) in &row.tags {
            assert_eq!(
                tags.pointer(&format!("/{key}")).and_then(Value::as_str),
                Some(*expected),
                "case {row:?}: expected tag {key} = {expected:?}, got {:?}",
                tags.pointer(&format!("/{key}"))
            );
        }
        assert_eq!(
            tags.as_object().map(serde_json::Map::len),
            Some(row.tags.len()),
            "case {row:?}: expected exactly {} tag(s), got {tags}",
            row.tags.len()
        );

        // A provisioned cluster carries no `Serverless` member: that arm belongs to
        // a cluster created as serverless, which this record is not.
        assert!(
            cluster.pointer("/serverless").is_none(),
            "case {row:?}: a provisioned cluster must report no serverless member, got {:?}",
            cluster.pointer("/serverless")
        );

        // The same record through the v1 reader: the two views must agree on every
        // member they share.
        let v1 = emulator.describe(arn).await;
        let v1 = v1.at("/clusterInfo");
        for (v2_member, v1_member) in [
            ("/clusterArn", "/clusterArn"),
            ("/clusterName", "/clusterName"),
            ("/state", "/state"),
            ("/currentVersion", "/currentVersion"),
            ("/provisioned/storageMode", "/storageMode"),
            ("/provisioned/enhancedMonitoring", "/enhancedMonitoring"),
            (
                "/provisioned/zookeeperConnectString",
                "/zookeeperConnectString",
            ),
            (
                "/provisioned/zookeeperConnectStringTls",
                "/zookeeperConnectStringTls",
            ),
            (
                "/provisioned/currentBrokerSoftwareInfo/kafkaVersion",
                "/currentBrokerSoftwareInfo/kafkaVersion",
            ),
        ] {
            assert_eq!(
                cluster.pointer(v2_member),
                v1.pointer(v1_member),
                "case {row:?}: DescribeClusterV2 {v2_member} and DescribeCluster {v1_member} disagree: {:?} vs {:?}",
                cluster.pointer(v2_member),
                v1.pointer(v1_member)
            );
        }
        assert_eq!(
            cluster.pointer("/provisioned/brokerNodeGroupInfo"),
            v1.pointer("/brokerNodeGroupInfo"),
            "case {row:?}: the two readers disagree about brokerNodeGroupInfo: {:?} vs {:?}",
            cluster.pointer("/provisioned/brokerNodeGroupInfo"),
            v1.pointer("/brokerNodeGroupInfo")
        );
        assert_eq!(
            cluster.pointer("/creationTime"),
            v1.pointer("/creationTime"),
            "case {row:?}: the two readers disagree about creationTime: {:?} vs {:?}",
            cluster.pointer("/creationTime"),
            v1.pointer("/creationTime")
        );

        // Every member the shape keeps at the top level is populated, so none of
        // them is silently reported under `Provisioned` instead.
        for member in TOP_LEVEL_STRING_MEMBERS {
            assert!(
                cluster
                    .pointer(&format!("/{member}"))
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty()),
                "case {row:?}: expected a non-empty {member}, got {:?}",
                cluster.pointer(&format!("/{member}"))
            );
        }
    }
}

/// RULE: a read reports the status the clock owes the cluster — `CREATING` until the
/// activation span has passed, `ACTIVE` after it — and reading it never moves it.
#[tokio::test]
async fn describe_v2_reports_the_state_the_clock_owes() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_ok(&ClusterRequest::named("v2-activation"))
        .await;

    // Straight after the create, the transition is not due yet.
    for attempt in 0..3 {
        let reply = describe_v2(&emulator, &arn).await;
        assert_eq!(
            reply.at("/clusterInfo/state").as_str(),
            Some("CREATING"),
            "case read #{attempt} before activation: expected CREATING, got {}",
            reply.at("/clusterInfo/state")
        );
    }

    sleep_past_activation();
    // And repeated reads after it are stable: a read materialises the owed
    // transition, nothing else.
    for attempt in 0..3 {
        let reply = describe_v2(&emulator, &arn).await;
        assert_eq!(
            reply.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case read #{attempt} after activation: expected ACTIVE, got {}",
            reply.at("/clusterInfo/state")
        );
    }
}

/// RULE: a `ClusterArn` that is not shaped like an MSK cluster ARN is refused with
/// `BadRequestException` (400); a well-formed ARN this endpoint's region holds no
/// cluster for is `NotFoundException` (404) — the two gates the model declares,
/// in that order.
#[tokio::test]
async fn describe_v2_refuses_an_unknown_or_malformed_cluster_arn() {
    let emulator = Emulator::new();
    let live = emulator
        .create_ok(&ClusterRequest::named("v2-arn-gates"))
        .await;
    sleep_past_activation();

    // Rows: (ClusterArn, expected status, expected error code). The malformed ones
    // differ along which part of the ARN grammar they break — service, resource
    // type, missing segments, an extra one, a non-ARN string.
    let rows: Vec<(String, u16, &str)> = vec![
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/00000000-0000-4000-8000-000000000000-1"),
            404,
            "NotFoundException",
        ),
        // Well formed, but minted for another region: a regional endpoint serves
        // only its own region's clusters.
        (
            format!("arn:aws:kafka:eu-west-1:{ACCOUNT}:cluster/elsewhere/00000000-0000-4000-8000-000000000000-1"),
            404,
            "NotFoundException",
        ),
        (
            format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:cluster/wrong-service/00000000-0000-4000-8000-000000000000-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/not-a-cluster/1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/missing-id"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/too/many/segments"),
            400,
            "BadRequestException",
        ),
        ("not-an-arn-at-all".to_string(), 400, "BadRequestException"),
    ];

    for (arn, status, code) in &rows {
        let reply = describe_v2(&emulator, arn).await;
        assert_eq!(
            reply.status, *status,
            "case {arn:?}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with(code),
            "case {arn:?}: expected error code {code}, got {:?}",
            reply.error_code()
        );
    }

    // The refusals changed nothing: the live cluster still reads back.
    let reply = describe_v2(&emulator, &live).await;
    assert_eq!(
        reply.status, 200,
        "expected the live cluster to still be readable, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.at("/clusterInfo/clusterArn").as_str(),
        Some(live.as_str()),
        "expected {live:?} back, got {}",
        reply.at("/clusterInfo/clusterArn")
    );
}

/// RULE: a cluster belongs to the region it was created in, so the v2 reader of one
/// region reports it and the v2 reader of every other region answers
/// `NotFoundException` for the same ARN.
#[tokio::test]
async fn describe_v2_is_scoped_to_the_cluster_region() {
    // Rows: the region the cluster is created in, and a region that must not see
    // it. Both directions are walked, so neither is a special case.
    let rows = [
        ("us-west-2", "us-east-1"),
        ("eu-central-1", "ap-southeast-2"),
    ];

    for (home, other) in rows {
        let emulator = Emulator::new();
        let request = ClusterRequest::named("v2-regional");
        let created = emulator
            .call_in(home, "POST", "/v1/clusters", Some(request.to_json()))
            .await;
        assert_eq!(
            created.status, 200,
            "case {home}: expected CreateCluster to return 200, got {} {}",
            created.status, created.body
        );
        let arn = created.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {home}: CreateCluster returned no clusterArn"))
            .to_string();
        assert!(
            arn.contains(&format!(":kafka:{home}:")),
            "case {home}: expected an ARN minted in {home}, got {arn:?}"
        );
        sleep_past_activation();

        let at_home = emulator
            .call_in(home, "GET", &describe_v2_uri(&arn), None)
            .await;
        assert_eq!(
            at_home.status, 200,
            "case {home}: expected DescribeClusterV2 in {home} to return 200, got {} {}",
            at_home.status, at_home.body
        );
        assert_eq!(
            at_home.at("/clusterInfo/clusterArn").as_str(),
            Some(arn.as_str()),
            "case {home}: expected {arn:?} back, got {}",
            at_home.at("/clusterInfo/clusterArn")
        );

        let elsewhere = emulator
            .call_in(other, "GET", &describe_v2_uri(&arn), None)
            .await;
        assert_eq!(
            elsewhere.status, 404,
            "case {home} read from {other}: expected HTTP 404, got {} {}",
            elsewhere.status, elsewhere.body
        );
        assert!(
            elsewhere.error_code().ends_with("NotFoundException"),
            "case {home} read from {other}: expected NotFoundException, got {:?}",
            elsewhere.error_code()
        );
    }
}
