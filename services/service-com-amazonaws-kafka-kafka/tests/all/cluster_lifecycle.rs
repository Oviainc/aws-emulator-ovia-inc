// SPDX-License-Identifier: CC0-1.0

//! The provisioned-cluster lifecycle (`CreateCluster`, `DescribeCluster`,
//! `ListClusters`, `GetBootstrapBrokers`, `DeleteCluster`), driven end to end over
//! the wire through the same `MultiServiceRouter` the emulator serves from.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request: the
//! rows differ along whatever the rule generalises over (broker count, Kafka
//! version, encryption setting, authentication mechanism, connectivity, tag count,
//! page size, cluster count). Where the rule is an invariant — pagination returning
//! every cluster exactly once, a delete being visible to every later reader — the
//! input space is small enough to walk EXHAUSTIVELY, which is strictly stronger
//! than sampling it.
//!
//! The harness every suite here drives the service through lives in
//! `tests/all/kafka_harness.rs`; this file holds only the lifecycle rules.

use serde_json::Value;
use service_com_amazonaws_kafka_kafka::state::MAX_CLUSTERS_PER_REGION;

use crate::kafka_harness::{
    assert_name_already_exists, pct, sleep_past_activation, ClusterRequest, Emulator, ACCOUNT,
    REGION,
};

/// Every bootstrap-broker member the model declares, so a suite can assert that a
/// member it did not expect is ABSENT rather than merely unchecked.
pub(crate) const BOOTSTRAP_MEMBERS: &[&str] = &[
    "bootstrapBrokerString",
    "bootstrapBrokerStringTls",
    "bootstrapBrokerStringSaslScram",
    "bootstrapBrokerStringSaslIam",
    "bootstrapBrokerStringPublicTls",
    "bootstrapBrokerStringPublicSaslScram",
    "bootstrapBrokerStringPublicSaslIam",
    "bootstrapBrokerStringVpcConnectivityTls",
    "bootstrapBrokerStringVpcConnectivitySaslScram",
    "bootstrapBrokerStringVpcConnectivitySaslIam",
    "bootstrapBrokerStringIpv6",
    "bootstrapBrokerStringTlsIpv6",
    "bootstrapBrokerStringSaslScramIpv6",
    "bootstrapBrokerStringSaslIamIpv6",
];

/// RULE: a created cluster is persisted whole, and `DescribeCluster` reports every
/// member from that record — the submitted values where the request carried them
/// and the documented default where it did not.
#[tokio::test]
async fn describe_reports_exactly_what_was_created() {
    // The rows differ along everything the rule generalises over: broker count
    // (both ends of the model's 1..15 range), Kafka version, client-broker
    // encryption (absent, and each `ClientBroker` value), SASL mechanisms,
    // `StorageMode`, `EnhancedMonitoring` and tag count.
    let mut rows = vec![
        ClusterRequest::named("defaults-everywhere"),
        ClusterRequest {
            brokers: 1,
            kafka_version: "2.8.1",
            subnets: vec!["subnet-a"],
            ..ClusterRequest::named("single-broker")
        },
        ClusterRequest {
            brokers: 15,
            instance_type: "kafka.m7g.xlarge",
            ..ClusterRequest::named("fifteen-brokers")
        },
        ClusterRequest {
            client_broker: Some("TLS"),
            sasl_iam: Some(true),
            storage_mode: Some("TIERED"),
            enhanced_monitoring: Some("PER_TOPIC_PER_PARTITION"),
            tags: vec![("env", "prod"), ("team", "data"), ("owner", "msk")],
            ..ClusterRequest::named("tls-iam-tiered")
        },
        ClusterRequest {
            client_broker: Some("PLAINTEXT"),
            sasl_scram: Some(true),
            enhanced_monitoring: Some("PER_BROKER"),
            tags: vec![("only", "one")],
            ..ClusterRequest::named("plaintext-scram")
        },
        ClusterRequest {
            client_broker: Some("TLS_PLAINTEXT"),
            sasl_scram: Some(false),
            sasl_iam: Some(false),
            storage_mode: Some("LOCAL"),
            ..ClusterRequest::named("explicitly-default")
        },
    ];
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    let emulator = Emulator::new();
    let mut minted = Vec::new();
    for row in &rows {
        let reply = emulator.create(row).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected CreateCluster to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.body["clusterName"].as_str(),
            Some(row.name.as_str()),
            "case {row:?}: CreateCluster echoed the wrong clusterName, got {}",
            reply.body["clusterName"]
        );
        // AWS answers CREATING for a cluster whose brokers are still being
        // provisioned; it is ACTIVE only once the clock owes the transition.
        assert_eq!(
            reply.body["state"].as_str(),
            Some("CREATING"),
            "case {row:?}: expected state CREATING, got {}",
            reply.body["state"]
        );
        let arn = reply.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: CreateCluster returned no clusterArn"))
            .to_string();
        assert_cluster_arn(&arn, &row.name, row);
        minted.push(arn);
    }

    let mut distinct = minted.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "every cluster must get its own ARN, got {minted:?}"
    );

    sleep_past_activation();
    let listed = emulator.list("").await;
    assert_eq!(
        listed.status, 200,
        "expected ListClusters to return 200, got {} {}",
        listed.status, listed.body
    );
    let listed = listed.body["clusterInfoList"]
        .as_array()
        .expect("ListClusters must return a clusterInfoList")
        .clone();

    for (row, arn) in rows.iter().zip(&minted) {
        let reply = emulator.describe(arn).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected DescribeCluster to return 200, got {} {}",
            reply.status, reply.body
        );
        let info = reply.at("/clusterInfo");

        for (member, expected) in [
            ("/clusterArn", arn.as_str()),
            ("/clusterName", row.name.as_str()),
            ("/state", "ACTIVE"),
            ("/storageMode", row.storage_mode.unwrap_or("LOCAL")),
            (
                "/enhancedMonitoring",
                row.enhanced_monitoring.unwrap_or("DEFAULT"),
            ),
            ("/currentBrokerSoftwareInfo/kafkaVersion", row.kafka_version),
            (
                "/encryptionInfo/encryptionInTransit/clientBroker",
                row.client_broker.unwrap_or("TLS_PLAINTEXT"),
            ),
            ("/brokerNodeGroupInfo/instanceType", row.instance_type),
        ] {
            assert_eq!(
                info.pointer(member).and_then(Value::as_str),
                Some(expected),
                "case {row:?}: expected {member} to be {expected:?}, got {:?}",
                info.pointer(member)
            );
        }

        assert_eq!(
            info.pointer("/numberOfBrokerNodes").and_then(Value::as_i64),
            Some(row.brokers),
            "case {row:?}: expected numberOfBrokerNodes {}, got {:?}",
            row.brokers,
            info.pointer("/numberOfBrokerNodes")
        );
        assert_eq!(
            info.pointer("/brokerNodeGroupInfo/clientSubnets")
                .and_then(Value::as_array)
                .map(|subnets| subnets.len()),
            Some(row.subnets.len()),
            "case {row:?}: expected the submitted clientSubnets back, got {:?}",
            info.pointer("/brokerNodeGroupInfo/clientSubnets")
        );
        // The documented default of `EncryptionInTransit$InCluster` is true.
        assert_eq!(
            info.pointer("/encryptionInfo/encryptionInTransit/inCluster")
                .and_then(Value::as_bool),
            Some(true),
            "case {row:?}: expected inCluster true, got {:?}",
            info.pointer("/encryptionInfo/encryptionInTransit/inCluster")
        );
        let kms_key = info
            .pointer("/encryptionInfo/encryptionAtRest/dataVolumeKMSKeyId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            kms_key.starts_with(&format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/")),
            "case {row:?}: expected a KMS key ARN in this region, got {kms_key:?}"
        );

        let tags = info
            .pointer("/tags")
            .and_then(Value::as_object)
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

        // `CurrentVersion` is an opaque MSK version token: `K` plus 13 upper-case
        // alphanumerics. It must be present, because a caller passes it back to a
        // mutating call.
        let current_version = info
            .pointer("/currentVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            current_version.len() == 14
                && current_version.starts_with('K')
                && current_version[1..]
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()),
            "case {row:?}: expected a K-prefixed 14-character currentVersion, got {current_version:?}"
        );
        assert!(
            info.pointer("/creationTime").is_some(),
            "case {row:?}: expected a creationTime, got {info}"
        );

        // One ZooKeeper connection per node of the ensemble the cluster runs.
        let zookeeper = info
            .pointer("/zookeeperConnectString")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let nodes: Vec<&str> = zookeeper.split(',').collect();
        assert_eq!(
            nodes.len(),
            3,
            "case {row:?}: expected a three-node zookeeperConnectString, got {zookeeper:?}"
        );
        for (index, node) in nodes.iter().enumerate() {
            assert!(
                node.starts_with(&format!("z-{}.{}.", index + 1, row.name))
                    && node.ends_with(":2181"),
                "case {row:?}: zookeeper node {index} is {node:?}"
            );
        }

        // The two readers must render the same stored cluster identically.
        let from_list = listed
            .iter()
            .find(|candidate| candidate["clusterArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {row:?}: ListClusters did not include {arn}"));
        assert_eq!(
            from_list, info,
            "case {row:?}: ListClusters and DescribeCluster disagree about the same cluster"
        );
    }

    assert_eq!(
        listed.len(),
        rows.len(),
        "expected ListClusters to return every created cluster, got {} entries",
        listed.len()
    );
}

/// `arn:aws:kafka:<region>:<account>:cluster/<name>/<uuid>-<n>`.
fn assert_cluster_arn(arn: &str, name: &str, case: &ClusterRequest) {
    let prefix = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{name}/");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}"
    );
    let tail = &arn[prefix.len()..];
    let (uuid, number) = tail
        .rsplit_once('-')
        .unwrap_or_else(|| panic!("case {case:?}: ARN tail {tail:?} carries no -<n> suffix"));
    assert!(
        number.bytes().all(|b| b.is_ascii_digit()) && !number.is_empty(),
        "case {case:?}: expected a numeric ARN suffix, got {number:?} in {arn:?}"
    );
    let groups: Vec<&str> = uuid.split('-').collect();
    assert_eq!(
        groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected a uuid-shaped cluster id, got {uuid:?}"
    );
    assert!(
        groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_hexdigit())),
        "case {case:?}: expected a hexadecimal cluster id, got {uuid:?}"
    );
    // RFC 4122 §4.4: the version nibble opens the third group, and the variant
    // nibble (8, 9, a or b) opens the fourth — the shape AWS's own ids carry.
    assert!(
        groups[2].starts_with('4'),
        "case {case:?}: expected version 4 in the cluster id, got {uuid:?}"
    );
    assert!(
        matches!(groups[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
        "case {case:?}: expected the RFC 4122 variant in the cluster id, got {uuid:?}"
    );
}

/// RULE: `GetBootstrapBrokers` returns one endpoint string per access path the
/// stored cluster actually enables — client-broker encryption decides the
/// plaintext and TLS paths, client authentication the SASL ones, connectivity the
/// public, multi-VPC and IPv6 ones — each listing every persisted broker on that
/// path's documented port; a path the cluster does not enable has no string.
#[tokio::test]
async fn bootstrap_brokers_follow_the_cluster_access_paths() {
    // Ports per Amazon MSK's "Port information": 9092 plaintext, 9094 TLS,
    // 9096 SASL/SCRAM, 9098 SASL/IAM; 9194/9196/9198 over public access;
    // 14094/14096/14098 over multi-VPC connectivity.
    let rows: Vec<(ClusterRequest, Vec<(&str, u16)>)> = vec![
        (
            // No encryptionInfo at all: `ClientBroker` defaults to TLS_PLAINTEXT,
            // so both the plaintext and the TLS path are open.
            ClusterRequest::named("defaulted"),
            vec![
                ("bootstrapBrokerString", 9092),
                ("bootstrapBrokerStringTls", 9094),
            ],
        ),
        (
            ClusterRequest {
                client_broker: Some("TLS"),
                brokers: 2,
                subnets: vec!["subnet-a", "subnet-b"],
                ..ClusterRequest::named("tls-only")
            },
            vec![("bootstrapBrokerStringTls", 9094)],
        ),
        (
            ClusterRequest {
                client_broker: Some("PLAINTEXT"),
                brokers: 1,
                subnets: vec!["subnet-a"],
                ..ClusterRequest::named("plaintext-only")
            },
            vec![("bootstrapBrokerString", 9092)],
        ),
        (
            ClusterRequest {
                client_broker: Some("TLS"),
                sasl_iam: Some(true),
                ..ClusterRequest::named("tls-iam")
            },
            vec![
                ("bootstrapBrokerStringTls", 9094),
                ("bootstrapBrokerStringSaslIam", 9098),
            ],
        ),
        (
            ClusterRequest {
                client_broker: Some("TLS"),
                sasl_scram: Some(true),
                sasl_iam: Some(true),
                ..ClusterRequest::named("tls-scram-and-iam")
            },
            vec![
                ("bootstrapBrokerStringTls", 9094),
                ("bootstrapBrokerStringSaslScram", 9096),
                ("bootstrapBrokerStringSaslIam", 9098),
            ],
        ),
        (
            // SASL explicitly turned OFF is not the same as SASL enabled.
            ClusterRequest {
                client_broker: Some("TLS"),
                sasl_scram: Some(false),
                sasl_iam: Some(false),
                ..ClusterRequest::named("sasl-disabled")
            },
            vec![("bootstrapBrokerStringTls", 9094)],
        ),
        (
            ClusterRequest {
                client_broker: Some("TLS"),
                sasl_iam: Some(true),
                public_access: Some("SERVICE_PROVIDED_EIPS"),
                ..ClusterRequest::named("public-tls-iam")
            },
            vec![
                ("bootstrapBrokerStringTls", 9094),
                ("bootstrapBrokerStringSaslIam", 9098),
                ("bootstrapBrokerStringPublicTls", 9194),
                ("bootstrapBrokerStringPublicSaslIam", 9198),
            ],
        ),
        (
            ClusterRequest {
                client_broker: Some("TLS"),
                sasl_scram: Some(true),
                public_access: Some("DISABLED"),
                ..ClusterRequest::named("public-disabled")
            },
            vec![
                ("bootstrapBrokerStringTls", 9094),
                ("bootstrapBrokerStringSaslScram", 9096),
            ],
        ),
        (
            ClusterRequest {
                client_broker: Some("TLS"),
                vpc_tls: Some(true),
                vpc_iam: Some(true),
                vpc_scram: Some(false),
                ..ClusterRequest::named("multi-vpc")
            },
            vec![
                ("bootstrapBrokerStringTls", 9094),
                ("bootstrapBrokerStringVpcConnectivityTls", 14094),
                ("bootstrapBrokerStringVpcConnectivitySaslIam", 14098),
            ],
        ),
        (
            // `NetworkType` DUAL is the model's dual-stack value, so the same
            // paths are also reachable over IPv6.
            ClusterRequest {
                client_broker: Some("TLS"),
                sasl_scram: Some(true),
                network_type: Some("DUAL"),
                ..ClusterRequest::named("dual-stack")
            },
            vec![
                ("bootstrapBrokerStringTls", 9094),
                ("bootstrapBrokerStringSaslScram", 9096),
                ("bootstrapBrokerStringTlsIpv6", 9094),
                ("bootstrapBrokerStringSaslScramIpv6", 9096),
            ],
        ),
        (
            // The model's default `NetworkType` is IPV4: no IPv6 path.
            ClusterRequest {
                client_broker: Some("TLS"),
                network_type: Some("IPV4"),
                ..ClusterRequest::named("ipv4-only")
            },
            vec![("bootstrapBrokerStringTls", 9094)],
        ),
    ];

    let emulator = Emulator::new();
    // Every row's cluster is created first and the suite waits ONE activation span:
    // a cluster still `CREATING` has no brokers to bootstrap against, so the reader
    // refuses it (see `cluster_delete_state_guard`), and these rows are about which
    // access paths a SETTLED cluster reports.
    let mut minted = Vec::new();
    for (row, _) in &rows {
        minted.push(emulator.create_ok(row).await);
    }
    sleep_past_activation();
    for ((row, expected), arn) in rows.iter().zip(&minted) {
        let reply = emulator.bootstrap_brokers(arn).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected GetBootstrapBrokers to return 200, got {} {}",
            reply.status, reply.body
        );

        for (member, port) in expected {
            let value = reply
                .body
                .get(*member)
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {row:?}: expected {member} on port {port}, got {}",
                        reply.body
                    )
                });
            let endpoints: Vec<&str> = value.split(',').collect();
            assert_eq!(
                i64::try_from(endpoints.len()).unwrap_or(-1),
                row.brokers,
                "case {row:?}: expected {} entries in {member}, got {value:?}",
                row.brokers
            );
            for (index, endpoint) in endpoints.iter().enumerate() {
                assert!(
                    endpoint.starts_with(&format!("b-{}.{}.", index + 1, row.name))
                        && endpoint.ends_with(&format!(":{port}")),
                    "case {row:?}: {member} entry {index} is {endpoint:?}, expected b-{}.{}…:{port}",
                    index + 1,
                    row.name
                );
            }
        }

        let expected_members: Vec<&str> = expected.iter().map(|(member, _)| *member).collect();
        for member in BOOTSTRAP_MEMBERS {
            if expected_members.contains(member) {
                continue;
            }
            assert!(
                reply.body.get(*member).is_none(),
                "case {row:?}: {member} must be absent for a cluster that does not enable it, got {:?}",
                reply.body.get(*member)
            );
        }
    }
}

/// RULE: a cluster name identifies at most one cluster, so a create under a name
/// already in use is refused with `ConflictException` (HTTP 409) and leaves the
/// existing cluster untouched.
#[tokio::test]
async fn a_repeated_cluster_name_conflicts() {
    let emulator = Emulator::new();
    // Rows differ in how the second request differs from the first: identical,
    // different broker count, different everything-but-the-name.
    let rows = [
        (
            ClusterRequest::named("twice-identical"),
            ClusterRequest::named("twice-identical"),
        ),
        (
            ClusterRequest::named("twice-resized"),
            ClusterRequest {
                brokers: 6,
                ..ClusterRequest::named("twice-resized")
            },
        ),
        (
            ClusterRequest::named("twice-reconfigured"),
            ClusterRequest {
                brokers: 1,
                kafka_version: "2.8.1",
                instance_type: "kafka.t3.small",
                subnets: vec!["subnet-z"],
                client_broker: Some("TLS"),
                sasl_iam: Some(true),
                storage_mode: Some("TIERED"),
                ..ClusterRequest::named("twice-reconfigured")
            },
        ),
    ];

    for (first, second) in &rows {
        let arn = emulator.create_ok(first).await;
        let reply = emulator.create(second).await;
        // The conflict is the service's one fixed sentence for a taken name,
        // blaming the cluster-name parameter by its wire name.
        assert_name_already_exists(
            &reply,
            &format!("{second:?}"),
            "clusterName",
            second.name.as_str(),
            &arn,
        );

        // The conflict changed nothing: the stored cluster is still the first one.
        let described = emulator.describe(&arn).await;
        assert_eq!(
            described.at("/clusterInfo/numberOfBrokerNodes").as_i64(),
            Some(first.brokers),
            "case {second:?}: the refused create overwrote the stored cluster: {}",
            described.body
        );
    }

    let listed = emulator.list("").await;
    assert_eq!(
        listed.body["clusterInfoList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        rows.len(),
        "a refused create must not add a cluster, got {}",
        listed.body
    );
}

/// RULE: `ListClusters` pages the clusters in one stable order — every cluster
/// exactly once across the pages, no page longer than `MaxResults`, and
/// `NextToken` present exactly while more pages remain.
#[tokio::test]
async fn list_clusters_pages_every_cluster_exactly_once() {
    // Exhaustive over the small space rather than sampled: every cluster count
    // 1..=5 against every page size 1..=6, which covers a page size below, equal
    // to, dividing and exceeding the result count.
    for count in 1..=5usize {
        let emulator = Emulator::new();
        let mut created = Vec::new();
        for index in 0..count {
            created.push(
                emulator
                    .create_ok(&ClusterRequest::named(&format!("page-{index:02}")))
                    .await,
            );
        }
        created.sort();

        for page_size in 1..=6usize {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                pages += 1;
                assert!(
                    pages <= count + 1,
                    "case count={count} maxResults={page_size}: pagination did not end after {pages} pages"
                );
                let query = match &token {
                    Some(value) => {
                        format!("?maxResults={page_size}&nextToken={}", pct(value))
                    }
                    None => format!("?maxResults={page_size}"),
                };
                let reply = emulator.list(&query).await;
                assert_eq!(
                    reply.status, 200,
                    "case count={count} maxResults={page_size}: expected 200, got {} {}",
                    reply.status, reply.body
                );
                let page = reply.body["clusterInfoList"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() <= page_size,
                    "case count={count} maxResults={page_size}: page carried {} entries",
                    page.len()
                );
                for info in &page {
                    seen.push(
                        info["clusterArn"]
                            .as_str()
                            .expect("every ClusterInfo carries a clusterArn")
                            .to_string(),
                    );
                }
                match reply.body.get("nextToken").and_then(Value::as_str) {
                    Some(next) => {
                        assert!(
                            !page.is_empty(),
                            "case count={count} maxResults={page_size}: an empty page must close the list"
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                seen, created,
                "case count={count} maxResults={page_size}: the pages did not cover every cluster exactly once"
            );
        }
    }
}

/// RULE: `ClusterNameFilter` keeps exactly the clusters whose name starts with it,
/// and it composes with paging.
#[tokio::test]
async fn cluster_name_filter_keeps_the_matching_prefixes() {
    let emulator = Emulator::new();
    for name in ["alpha-one", "alpha-two", "beta-one"] {
        emulator.create_ok(&ClusterRequest::named(name)).await;
    }

    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("alpha", vec!["alpha-one", "alpha-two"]),
        ("alpha-t", vec!["alpha-two"]),
        ("a", vec!["alpha-one", "alpha-two"]),
        ("beta", vec!["beta-one"]),
        ("beta-one", vec!["beta-one"]),
        ("gamma", vec![]),
        ("one", vec![]),
    ];
    for (filter, expected) in &rows {
        let reply = emulator
            .list(&format!("?clusterNameFilter={}", pct(filter)))
            .await;
        assert_eq!(
            reply.status, 200,
            "case filter={filter:?}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let names: Vec<&str> = reply.body["clusterInfoList"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|info| info["clusterName"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            &names, expected,
            "case filter={filter:?}: expected {expected:?}, got {names:?}"
        );
    }

    // The filter survives the token round trip: one match per page.
    let mut paged: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(value) => format!("?clusterNameFilter=alpha&maxResults=1&nextToken={}", pct(value)),
            None => "?clusterNameFilter=alpha&maxResults=1".to_string(),
        };
        let reply = emulator.list(&query).await;
        let page = reply.body["clusterInfoList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            page.len() <= 1,
            "case filter=alpha maxResults=1: page carried {} entries",
            page.len()
        );
        for info in &page {
            paged.push(info["clusterName"].as_str().unwrap_or_default().to_string());
        }
        match reply.body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        paged,
        vec!["alpha-one".to_string(), "alpha-two".to_string()],
        "a filtered list must page the matches exactly once each, got {paged:?}"
    );
}

/// RULE: an unparseable `NextToken` is refused as a bad request, not served as a
/// first page.
#[tokio::test]
async fn an_invalid_pagination_token_is_refused() {
    let emulator = Emulator::new();
    emulator.create_ok(&ClusterRequest::named("solo")).await;

    // Rows: not base64 at all, valid base64 of unrelated bytes, and the empty
    // token.
    for token in ["!!!not-base64!!!", "aGVsbG8gd29ybGQ=", "%20"] {
        let reply = emulator.list(&format!("?nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case token={token:?}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case token={token:?}: expected BadRequestException, got {}",
            reply.body
        );
    }
}

/// RULE: a delete removes the cluster, so every later reader answers as if it
/// never existed; and a `currentVersion` that is not the cluster's refuses the
/// delete with HTTP 400, leaving the cluster in place.
#[tokio::test]
async fn delete_is_observable_by_every_later_reader() {
    let emulator = Emulator::new();
    let request = ClusterRequest {
        sasl_iam: Some(true),
        tags: vec![("env", "test")],
        ..ClusterRequest::named("doomed")
    };
    let arn = emulator.create_ok(&request).await;
    // A cluster is deletable once it has finished provisioning: a delete asked for
    // while the cluster is still `CREATING` is refused on its state, which
    // `cluster_delete_state_guard` is the suite for.
    sleep_past_activation();
    let current_version = emulator.describe(&arn).await.at("/clusterInfo/currentVersion")
        .as_str()
        .expect("a stored cluster has a currentVersion")
        .to_string();

    // A stale version refuses the delete and changes nothing.
    for stale in ["K0000000000000", "not-a-version", &current_version.to_lowercase()] {
        let reply = emulator
            .call(
                "DELETE",
                &format!("/v1/clusters/{}?currentVersion={}", pct(&arn), pct(stale)),
                None,
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case currentVersion={stale:?}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case currentVersion={stale:?}: expected BadRequestException, got {}",
            reply.body
        );
        assert_eq!(
            emulator.describe(&arn).await.status,
            200,
            "case currentVersion={stale:?}: a refused delete must leave the cluster in place"
        );
    }

    // The cluster's own version deletes it.
    let reply = emulator
        .call(
            "DELETE",
            &format!(
                "/v1/clusters/{}?currentVersion={}",
                pct(&arn),
                pct(&current_version)
            ),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected DeleteCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.body["state"].as_str(),
        Some("DELETING"),
        "expected DeleteCluster to answer DELETING, got {}",
        reply.body
    );
    assert_eq!(
        reply.body["clusterArn"].as_str(),
        Some(arn.as_str()),
        "expected DeleteCluster to name the deleted cluster, got {}",
        reply.body
    );

    // Every later reader answers as if it never existed.
    let described = emulator.describe(&arn).await;
    assert_eq!(
        (described.status, described.error_code()),
        (404, "NotFoundException"),
        "expected a 404 NotFoundException after the delete, got {} {}",
        described.status,
        described.body
    );
    // `GetBootstrapBrokers` models no NotFoundException, yet the service answers
    // one for a cluster it cannot resolve, like every other reader.
    let brokers = emulator.bootstrap_brokers(&arn).await;
    assert_eq!(
        (brokers.status, brokers.error_code()),
        (404, "NotFoundException"),
        "expected a 404 NotFoundException after the delete, got {} {}",
        brokers.status,
        brokers.body
    );
    let listed = emulator.list("").await;
    assert_eq!(
        listed.body["clusterInfoList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "expected ListClusters to be empty after the delete, got {}",
        listed.body
    );
    let again = emulator
        .call("DELETE", &format!("/v1/clusters/{}", pct(&arn)), None)
        .await;
    assert_eq!(
        (again.status, again.error_code()),
        (404, "NotFoundException"),
        "expected a second delete to answer 404, got {} {}",
        again.status,
        again.body
    );

    // And the name it held is free again, with a NEW identity.
    let reborn = emulator.create_ok(&ClusterRequest::named("doomed")).await;
    assert_ne!(
        reborn, arn,
        "a cluster recreated under a freed name must get its own ARN"
    );
}

/// RULE: a resource AWS reports in a non-terminal state reaches its terminal one
/// because the clock passed, not because somebody read it — a caller acting
/// immediately still sees `CREATING`, and a later one sees `ACTIVE` without any
/// further call.
#[tokio::test]
async fn a_new_cluster_becomes_active_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    // Rows differ in size, so the transition is not keyed to one shape of input.
    for request in [
        ClusterRequest::named("clock-small"),
        ClusterRequest {
            brokers: 9,
            ..ClusterRequest::named("clock-large")
        },
    ] {
        let reply = emulator.create(&request).await;
        let arn = reply.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {request:?}: no clusterArn in {}", reply.body))
            .to_string();
        assert_eq!(
            reply.body["state"].as_str(),
            Some("CREATING"),
            "case {request:?}: a cluster must be reported CREATING when it is created"
        );

        let immediately = emulator.describe(&arn).await;
        assert_eq!(
            immediately.at("/clusterInfo/state").as_str(),
            Some("CREATING"),
            "case {request:?}: a describe straight after the create must still see CREATING, got {}",
            immediately.body
        );

        sleep_past_activation();
        let later = emulator.describe(&arn).await;
        assert_eq!(
            later.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {request:?}: the cluster must be ACTIVE once its activation span has passed, got {}",
            later.body
        );
        // Reading it again does not move it on: the clock owed one transition.
        assert_eq!(
            emulator.describe(&arn).await.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {request:?}: a settled cluster must stay ACTIVE across reads"
        );
    }
}

/// RULE: the registry is bounded — an account may hold only so many clusters in
/// a region, and a create past that quota is refused with
/// `TooManyRequestsException` rather than growing the store by one more call.
/// The quota is per region, so another region still accepts.
#[tokio::test]
async fn the_per_region_cluster_quota_is_enforced() {
    let emulator = Emulator::new();
    for index in 0..MAX_CLUSTERS_PER_REGION {
        let request = ClusterRequest {
            brokers: 1,
            subnets: vec!["subnet-a"],
            ..ClusterRequest::named(&format!("quota-{index:03}"))
        };
        let reply = emulator.create(&request).await;
        assert_eq!(
            reply.status, 200,
            "case index={index}: a create within the quota must succeed, got {} {}",
            reply.status, reply.body
        );
    }

    let over = ClusterRequest {
        brokers: 1,
        subnets: vec!["subnet-a"],
        ..ClusterRequest::named("quota-over")
    };
    let reply = emulator.create(&over).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (429, "TooManyRequestsException"),
        "expected a 429 TooManyRequestsException past the quota, got {} {}",
        reply.status,
        reply.body
    );

    // The quota is per region: a different signing region is unaffected.
    let reply = emulator
        .call_in("eu-west-1", "POST", "/v1/clusters", Some(over.to_json()))
        .await;
    assert_eq!(
        reply.status, 200,
        "another region must have its own quota, got {} {}",
        reply.status, reply.body
    );
}

/// RULE: a regional endpoint serves only its own region's clusters, so a cluster
/// created in one region is invisible to every reader signed for another.
#[tokio::test]
async fn a_cluster_is_only_visible_in_its_own_region() {
    let emulator = Emulator::new();
    let request = ClusterRequest::named("elsewhere");
    let reply = emulator
        .call_in("eu-west-1", "POST", "/v1/clusters", Some(request.to_json()))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected the eu-west-1 create to succeed, got {} {}",
        reply.status, reply.body
    );
    let arn = reply.body["clusterArn"]
        .as_str()
        .expect("CreateCluster returns a clusterArn")
        .to_string();

    // Rows: every reader, each signed for a region that is not the cluster's.
    for region in [REGION, "us-east-1", "ap-southeast-2"] {
        let described = emulator
            .call_in(region, "GET", &format!("/v1/clusters/{}", pct(&arn)), None)
            .await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected a 404 from DescribeCluster, got {} {}",
            described.status,
            described.body
        );
        let deleted = emulator
            .call_in(
                region,
                "DELETE",
                &format!("/v1/clusters/{}", pct(&arn)),
                None,
            )
            .await;
        assert_eq!(
            (deleted.status, deleted.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected a 404 from DeleteCluster, got {} {}",
            deleted.status,
            deleted.body
        );
        let brokers = emulator
            .call_in(
                region,
                "GET",
                &format!("/v1/clusters/{}/bootstrap-brokers", pct(&arn)),
                None,
            )
            .await;
        assert_eq!(
            (brokers.status, brokers.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected a 404 from GetBootstrapBrokers, got {} {}",
            brokers.status,
            brokers.body
        );
        let listed = emulator.call_in(region, "GET", "/v1/clusters", None).await;
        assert_eq!(
            listed.body["clusterInfoList"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            0,
            "case region={region}: ListClusters must not report another region's cluster, got {}",
            listed.body
        );
    }

    // Its own region still sees it, intact.
    let described = emulator
        .call_in(
            "eu-west-1",
            "GET",
            &format!("/v1/clusters/{}", pct(&arn)),
            None,
        )
        .await;
    assert_eq!(
        described.at("/clusterInfo/clusterName").as_str(),
        Some("elsewhere"),
        "the cluster's own region must still describe it, got {}",
        described.body
    );
    let listed = emulator
        .call_in("eu-west-1", "GET", "/v1/clusters", None)
        .await;
    assert_eq!(
        listed.body["clusterInfoList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "the cluster's own region must list it, got {}",
        listed.body
    );
}

/// RULE: a reader that models both errors separates a `ClusterArn` it cannot
/// parse (`BadRequestException`, 400) from a well-formed one no live cluster holds
/// (`NotFoundException`, 404); `GetBootstrapBrokers`, which models no
/// `NotFoundException`, refuses both as a bad request.
#[tokio::test]
async fn an_unknown_or_malformed_cluster_arn_is_refused() {
    let emulator = Emulator::new();
    emulator.create_ok(&ClusterRequest::named("present")).await;

    // `(arn, expected status, expected error code)` for DescribeCluster and
    // DeleteCluster. Rows: well-formed but absent (two regions), then one row per
    // way the ARN grammar can fail — not an ARN at all, wrong service, wrong
    // resource type, an extra resource segment, and an empty region.
    let rows: Vec<(String, u16, &str)> = vec![
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"),
            404,
            "NotFoundException",
        ),
        (
            format!("arn:aws:kafka:us-east-1:{ACCOUNT}:cluster/present/11111111-2222-4333-8444-555555555555-1"),
            404,
            "NotFoundException",
        ),
        ("not-an-arn".to_string(), 400, "BadRequestException"),
        (
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster/present/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/present/abc"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/present/abc/extra"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka::{ACCOUNT}:cluster/present/abc-1"),
            400,
            "BadRequestException",
        ),
    ];

    for (arn, status, code) in &rows {
        let described = emulator.describe(arn).await;
        assert_eq!(
            (described.status, described.error_code()),
            (*status, *code),
            "case arn={arn:?}: expected {status} {code} from DescribeCluster, got {} {}",
            described.status,
            described.body
        );
        let deleted = emulator
            .call("DELETE", &format!("/v1/clusters/{}", pct(arn)), None)
            .await;
        assert_eq!(
            (deleted.status, deleted.error_code()),
            (*status, *code),
            "case arn={arn:?}: expected {status} {code} from DeleteCluster, got {} {}",
            deleted.status,
            deleted.body
        );
        // `GetBootstrapBrokers` models no `NotFoundException`, yet the service
        // answers the same code and status for the same reference: a malformed
        // ARN is the bad request, an absent one the not-found.
        let brokers = emulator.bootstrap_brokers(arn).await;
        assert_eq!(
            (brokers.status, brokers.error_code()),
            (*status, *code),
            "case arn={arn:?}: expected {status} {code} from GetBootstrapBrokers, got {} {}",
            brokers.status,
            brokers.body
        );
    }

    // None of those refusals disturbed the cluster that does exist.
    let listed = emulator.list("").await;
    assert_eq!(
        listed.body["clusterInfoList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "a refused read or delete must leave the registry alone, got {}",
        listed.body
    );
}

/// RULE: a broker count outside the modelled 1..15 range is refused with
/// `BadRequestException`, and no cluster is created for it.
#[tokio::test]
async fn a_broker_count_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    // Both published bounds, and one past each: the model's `NumberOfBrokerNodes`
    // is `__integerMin1Max15` (range 1..15).
    let rows: [(i64, bool); 6] = [
        (1, true),
        (15, true),
        (0, false),
        (16, false),
        (-1, false),
        (i64::from(i32::MAX), false),
    ];
    for (count, accepted) in rows {
        let request = ClusterRequest {
            brokers: count,
            ..ClusterRequest::named(&format!("brokers-{count}"))
        };
        let reply = emulator.create(&request).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case brokers={count}: expected the create to succeed, got {} {}",
                reply.status, reply.body
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case brokers={count}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert!(
            matches!(
                reply.error_code(),
                "BadRequestException" | "ValidationException"
            ),
            "case brokers={count}: expected a client-input refusal, got {}",
            reply.body
        );
    }

    // Exactly the two accepted rows are in the registry.
    let listed = emulator.list("").await;
    let names: Vec<&str> = listed.body["clusterInfoList"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|info| info["clusterName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["brokers-1", "brokers-15"],
        "a refused create must not add a cluster, got {names:?}"
    );
}

/// RULE: a cluster name is unique per account AND region, so the same name used
/// under two signing regions is two clusters, each with its own regional ARN.
#[tokio::test]
async fn the_same_cluster_name_in_two_regions_is_two_clusters() {
    let emulator = Emulator::new();
    let request = ClusterRequest::named("shared-name");
    let mut arns = Vec::new();
    // Rows: several regions, including the one the rest of this suite signs with.
    for region in [REGION, "us-east-1", "eu-west-1", "ap-southeast-2"] {
        let reply = emulator
            .call_in(region, "POST", "/v1/clusters", Some(request.to_json()))
            .await;
        assert_eq!(
            reply.status, 200,
            "case region={region}: expected the create to succeed, got {} {}",
            reply.status, reply.body
        );
        let arn = reply.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case region={region}: no clusterArn in {}", reply.body))
            .to_string();
        assert!(
            arn.starts_with(&format!("arn:aws:kafka:{region}:{ACCOUNT}:cluster/shared-name/")),
            "case region={region}: expected a {region} ARN, got {arn:?}"
        );
        arns.push(arn);
    }

    // Repeating a region IS a conflict: uniqueness holds within each region.
    let reply = emulator
        .call_in(REGION, "POST", "/v1/clusters", Some(request.to_json()))
        .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (409, "ConflictException"),
        "case region={REGION} (repeat): expected a 409 ConflictException, got {} {}",
        reply.status,
        reply.body
    );

    let mut distinct = arns.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        arns.len(),
        "each region's cluster must have its own ARN, got {arns:?}"
    );
}
