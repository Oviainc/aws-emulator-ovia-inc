// SPDX-License-Identifier: CC0-1.0

//! `ListClustersV2`, driven end to end over the wire through the same
//! `MultiServiceRouter` the emulator serves from.
//!
//! Each suite asserts a RULE over a table of inputs rather than one request: the
//! rows differ along whatever the rule generalises over (cluster count, page size,
//! name prefix, cluster type, region, broker count, encryption and monitoring
//! settings, tag count). Where the rule is an invariant — every cluster reported
//! exactly once across the pages, the V2 view agreeing with the V1 one — the input
//! space is small enough to walk EXHAUSTIVELY, which is strictly stronger than
//! sampling it.

use serde_json::Value;

use crate::kafka_harness::{pct, sleep_past_activation, ClusterRequest, Emulator, ACCOUNT, REGION};

/// The values `ClusterTypeFilter` may name, from the model's `ClusterType` enum
/// (`smithy_query(query="shape",shape="ClusterType")` -> PROVISIONED, SERVERLESS).
const CLUSTER_TYPES: &[&str] = &["PROVISIONED", "SERVERLESS"];

/// RULE: a cluster listing reports every cluster of the endpoint's region that
/// passes the request's filters, each as the `Cluster` shape whose members are
/// computed from the stored record.
#[tokio::test]
async fn list_v2_reports_the_stored_cluster_as_the_v2_shape() {
    // The rows differ along everything the rendering is computed from: broker
    // count (both ends of the model's 1..15 range), Kafka version, client-broker
    // encryption, SASL mechanism, storage mode, monitoring level and tag count.
    let rows = vec![
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
            storage_mode: Some("TIERED"),
            ..ClusterRequest::named("fifteen-brokers")
        },
        ClusterRequest {
            client_broker: Some("TLS"),
            sasl_iam: Some(true),
            enhanced_monitoring: Some("PER_TOPIC_PER_PARTITION"),
            tags: vec![("env", "prod"), ("team", "data")],
            ..ClusterRequest::named("tls-iam-monitored")
        },
        ClusterRequest {
            client_broker: Some("PLAINTEXT"),
            sasl_scram: Some(true),
            enhanced_monitoring: Some("PER_BROKER"),
            tags: vec![("only", "one")],
            ..ClusterRequest::named("plaintext-scram")
        },
    ];

    let emulator = Emulator::new();
    let mut arns = Vec::new();
    for row in &rows {
        arns.push(emulator.create_ok(row).await);
    }
    sleep_past_activation();

    let reply = emulator.list_v2("").await;
    assert_eq!(
        reply.status, 200,
        "expected ListClustersV2 to return 200, got {} {}",
        reply.status, reply.body
    );
    let listed = reply
        .at("/clusterInfoList")
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListClustersV2 must return a clusterInfoList, got {}", reply.body));
    assert_eq!(
        listed.len(),
        rows.len(),
        "expected {} clusters, got {listed:?}",
        rows.len()
    );
    assert!(
        reply.body.get("nextToken").is_none(),
        "a single page that reports every cluster must carry no nextToken, got {}",
        reply.body
    );

    for (row, arn) in rows.iter().zip(&arns) {
        let entry = listed
            .iter()
            .find(|entry| entry["clusterArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {row:?}: {arn} is missing from {listed:?}"));

        // The identity members every cluster kind carries.
        assert_eq!(
            entry["clusterName"].as_str(),
            Some(row.name.as_str()),
            "case {row:?}: wrong clusterName, got {}",
            entry["clusterName"]
        );
        assert_eq!(
            entry["clusterType"].as_str(),
            Some("PROVISIONED"),
            "case {row:?}: a cluster created with brokers is PROVISIONED, got {}",
            entry["clusterType"]
        );
        assert_eq!(
            entry["state"].as_str(),
            Some("ACTIVE"),
            "case {row:?}: expected ACTIVE once the clock owed the transition, got {}",
            entry["state"]
        );
        // Present and in the past: the serialized form of a modelled timestamp is
        // the framework's business (the V1 listing renders it the same way), so
        // this suite pins that the value is reported, not how it is encoded.
        assert!(
            !entry["creationTime"].is_null(),
            "case {row:?}: expected a creationTime, got {entry}"
        );
        let current_version = entry["currentVersion"].as_str().unwrap_or_default();
        assert!(
            current_version.len() == 14 && current_version.starts_with('K'),
            "case {row:?}: expected a `K`-prefixed currentVersion token, got {current_version:?}"
        );
        assert!(
            entry.get("serverless").is_none(),
            "case {row:?}: a provisioned cluster carries no serverless detail, got {entry}"
        );

        // The per-kind detail: computed from the stored record, so it reports the
        // submitted value where the request carried one and the documented
        // default where it did not.
        let provisioned = entry
            .get("provisioned")
            .unwrap_or_else(|| panic!("case {row:?}: expected a provisioned detail in {entry}"));
        assert_eq!(
            provisioned["numberOfBrokerNodes"].as_i64(),
            Some(row.brokers),
            "case {row:?}: wrong numberOfBrokerNodes, got {}",
            provisioned["numberOfBrokerNodes"]
        );
        assert_eq!(
            provisioned.pointer("/currentBrokerSoftwareInfo/kafkaVersion")
                .and_then(Value::as_str),
            Some(row.kafka_version),
            "case {row:?}: wrong kafkaVersion, got {provisioned}"
        );
        assert_eq!(
            provisioned.pointer("/brokerNodeGroupInfo/instanceType")
                .and_then(Value::as_str),
            Some(row.instance_type),
            "case {row:?}: wrong instanceType, got {provisioned}"
        );
        // The documented default of `EncryptionInTransit$ClientBroker` is
        // TLS_PLAINTEXT, and of `StorageMode` local storage.
        assert_eq!(
            provisioned.pointer("/encryptionInfo/encryptionInTransit/clientBroker")
                .and_then(Value::as_str),
            Some(row.client_broker.unwrap_or("TLS_PLAINTEXT")),
            "case {row:?}: wrong clientBroker, got {provisioned}"
        );
        assert_eq!(
            provisioned["storageMode"].as_str(),
            Some(row.storage_mode.unwrap_or("LOCAL")),
            "case {row:?}: wrong storageMode, got {}",
            provisioned["storageMode"]
        );
        assert_eq!(
            provisioned["enhancedMonitoring"].as_str(),
            Some(row.enhanced_monitoring.unwrap_or("DEFAULT")),
            "case {row:?}: wrong enhancedMonitoring, got {}",
            provisioned["enhancedMonitoring"]
        );
        assert_eq!(
            provisioned.pointer("/clientAuthentication/sasl/scram/enabled")
                .and_then(Value::as_bool),
            row.sasl_scram,
            "case {row:?}: wrong SASL/SCRAM setting, got {provisioned}"
        );
        assert_eq!(
            provisioned.pointer("/clientAuthentication/sasl/iam/enabled")
                .and_then(Value::as_bool),
            row.sasl_iam,
            "case {row:?}: wrong SASL/IAM setting, got {provisioned}"
        );
        // An exporter nobody asked for is reported off rather than absent.
        for exporter in ["jmxExporter", "nodeExporter"] {
            assert_eq!(
                provisioned
                    .pointer(&format!("/openMonitoring/prometheus/{exporter}/enabledInBroker"))
                    .and_then(Value::as_bool),
                Some(false),
                "case {row:?}: expected {exporter} reported off, got {provisioned}"
            );
        }
        // One ZooKeeper connect string per node, each `host:port`.
        for (member, port) in [
            ("zookeeperConnectString", "2181"),
            ("zookeeperConnectStringTls", "2182"),
        ] {
            let value = provisioned[member].as_str().unwrap_or_default();
            let hosts: Vec<&str> = value.split(',').collect();
            assert_eq!(
                hosts.len(),
                3,
                "case {row:?}: expected a three-node ensemble in {member}, got {value:?}"
            );
            assert!(
                hosts.iter().all(|host| host.ends_with(&format!(":{port}"))),
                "case {row:?}: every {member} endpoint must carry port {port}, got {value:?}"
            );
        }
        let tags = entry["tags"].as_object().cloned().unwrap_or_default();
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
                "case {row:?}: wrong value for tag {key}, got {tags:?}"
            );
        }
        assert!(
            arn.starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{}/", row.name)),
            "case {row:?}: {arn} is not an MSK cluster ARN for this region"
        );
    }
}

/// RULE: the V1 and V2 cluster listings report the same clusters in the same
/// order, because both render one registry — the V2 shape nests the
/// provisioned-only members the V1 shape carries inline.
#[tokio::test]
async fn the_two_listings_agree_on_one_registry() {
    let emulator = Emulator::new();
    for name in ["agree-a", "agree-b", "agree-c"] {
        emulator.create_ok(&ClusterRequest::named(name)).await;
    }
    sleep_past_activation();

    let v1 = emulator.list("").await;
    let v2 = emulator.list_v2("").await;
    let v1_list = v1.at("/clusterInfoList").as_array().cloned().unwrap_or_default();
    let v2_list = v2.at("/clusterInfoList").as_array().cloned().unwrap_or_default();
    let v1_arns: Vec<&str> = v1_list
        .iter()
        .filter_map(|entry| entry["clusterArn"].as_str())
        .collect();
    let v2_arns: Vec<&str> = v2_list
        .iter()
        .filter_map(|entry| entry["clusterArn"].as_str())
        .collect();
    assert_eq!(
        v1_arns, v2_arns,
        "the two listings must page one order: v1 {v1_arns:?}, v2 {v2_arns:?}"
    );

    for (v1_entry, v2_entry) in v1_list.iter().zip(&v2_list) {
        // Members both shapes carry at the top level.
        for member in ["clusterName", "state", "currentVersion", "creationTime"] {
            assert_eq!(
                v1_entry[member], v2_entry[member],
                "case member={member}: the two views of one cluster disagree, v1 {v1_entry}, v2 {v2_entry}"
            );
        }
        // Members the V2 shape nests under the per-kind detail.
        for member in [
            "numberOfBrokerNodes",
            "enhancedMonitoring",
            "storageMode",
            "zookeeperConnectString",
            "zookeeperConnectStringTls",
            "encryptionInfo",
            "brokerNodeGroupInfo",
            "currentBrokerSoftwareInfo",
        ] {
            assert_eq!(
                v1_entry[member],
                v2_entry.pointer(&format!("/provisioned/{member}")).cloned().unwrap_or(Value::Null),
                "case member={member}: the two views of one cluster disagree, v1 {v1_entry}, v2 {v2_entry}"
            );
        }
    }
}

/// RULE: a cluster listing pages its clusters in one stable order — every cluster
/// exactly once across the pages, no page longer than `MaxResults`, and `NextToken`
/// present exactly while more pages remain.
#[tokio::test]
async fn list_v2_pages_every_cluster_exactly_once() {
    // Exhaustive over the small space rather than sampled: every cluster count
    // 1..=4 against every page size 1..=5, which covers a page size below, equal
    // to, dividing and exceeding the result count.
    for count in 1..=4usize {
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

        for page_size in 1..=5usize {
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
                    Some(value) => format!("?maxResults={page_size}&nextToken={}", pct(value)),
                    None => format!("?maxResults={page_size}"),
                };
                let reply = emulator.list_v2(&query).await;
                assert_eq!(
                    reply.status, 200,
                    "case count={count} maxResults={page_size}: expected 200, got {} {}",
                    reply.status, reply.body
                );
                let page = reply
                    .at("/clusterInfoList")
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() <= page_size,
                    "case count={count} maxResults={page_size}: page carried {} entries",
                    page.len()
                );
                for entry in &page {
                    seen.push(
                        entry["clusterArn"]
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!(
                                    "case count={count} maxResults={page_size}: an entry carried no clusterArn: {entry}"
                                )
                            })
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
async fn list_v2_name_filter_keeps_the_matching_prefixes() {
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
            .list_v2(&format!("?clusterNameFilter={}", pct(filter)))
            .await;
        assert_eq!(
            reply.status, 200,
            "case filter={filter:?}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let names: Vec<&str> = reply
            .at("/clusterInfoList")
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|entry| entry["clusterName"].as_str())
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
        let reply = emulator.list_v2(&query).await;
        let page = reply
            .at("/clusterInfoList")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            page.len() <= 1,
            "case filter=alpha maxResults=1: page carried {} entries",
            page.len()
        );
        for entry in &page {
            paged.push(entry["clusterName"].as_str().unwrap_or_default().to_string());
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

/// RULE: `ClusterTypeFilter` keeps the clusters of the `ClusterType` it names, and
/// a value the enum does not name is refused as a bad request.
#[tokio::test]
async fn list_v2_type_filter_keeps_the_named_cluster_type() {
    let emulator = Emulator::new();
    for name in ["typed-one", "typed-two"] {
        emulator.create_ok(&ClusterRequest::named(name)).await;
    }

    // Every value the model's enum names, plus the absent filter: a cluster
    // created with brokers is PROVISIONED, so the SERVERLESS row keeps nothing.
    for kind in CLUSTER_TYPES {
        let reply = emulator
            .list_v2(&format!("?clusterTypeFilter={}", pct(kind)))
            .await;
        assert_eq!(
            reply.status, 200,
            "case clusterTypeFilter={kind}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let names: Vec<&str> = reply
            .at("/clusterInfoList")
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|entry| entry["clusterName"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<&str> = if *kind == "PROVISIONED" {
            vec!["typed-one", "typed-two"]
        } else {
            vec![]
        };
        assert_eq!(
            names, expected,
            "case clusterTypeFilter={kind}: expected {expected:?}, got {names:?}"
        );
    }

    // The two filters compose.
    let reply = emulator
        .list_v2("?clusterTypeFilter=PROVISIONED&clusterNameFilter=typed-t")
        .await;
    let names: Vec<&str> = reply
        .at("/clusterInfoList")
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry["clusterName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["typed-two"],
        "the two filters must compose, got {names:?}"
    );

    // Rows outside the enum: a neighbouring case, an unrelated word, the empty
    // value and a V1-ish name.
    for kind in ["provisioned", "Serverless", "", "CLUSTER"] {
        let reply = emulator
            .list_v2(&format!("?clusterTypeFilter={}", pct(kind)))
            .await;
        assert_eq!(
            reply.status, 400,
            "case clusterTypeFilter={kind:?}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case clusterTypeFilter={kind:?}: expected BadRequestException, got {}",
            reply.body
        );
    }
}

/// RULE: a pagination token this service did not mint is refused as a bad request,
/// not served as a first page.
#[tokio::test]
async fn list_v2_refuses_an_invalid_pagination_token() {
    let emulator = Emulator::new();
    emulator.create_ok(&ClusterRequest::named("solo")).await;

    // Rows: not base64 at all, valid base64 of unrelated bytes, the empty token,
    // and base64 of a marked plaintext from some other list.
    for token in [
        "!!!not-base64!!!",
        "aGVsbG8gd29ybGQ=",
        "%20",
        "a2Fma2E6dG9waWNzOmFybjphd3M6",
    ] {
        let reply = emulator.list_v2(&format!("?nextToken={token}")).await;
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

/// RULE: a cluster listing is regional, so an endpoint reports only the clusters
/// its own region holds.
#[tokio::test]
async fn list_v2_reports_only_this_regions_clusters() {
    let emulator = Emulator::new();
    // One cluster per region, each created through that region's endpoint.
    let regions = [REGION, "us-east-1", "eu-west-1"];
    for region in regions {
        let request = ClusterRequest::named(&format!("home-{region}"));
        let reply = emulator
            .call_in(region, "POST", "/v1/clusters", Some(request.to_json()))
            .await;
        assert_eq!(
            reply.status, 200,
            "case region={region}: expected CreateCluster to return 200, got {} {}",
            reply.status, reply.body
        );
    }

    for region in regions {
        let reply = emulator.list_v2_in(region, "").await;
        assert_eq!(
            reply.status, 200,
            "case region={region}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let names: Vec<&str> = reply
            .at("/clusterInfoList")
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|entry| entry["clusterName"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec![format!("home-{region}").as_str()],
            "case region={region}: an endpoint reports only its own region's clusters, got {names:?}"
        );
    }

    // A region holding nothing reports an empty list rather than another
    // region's clusters.
    let reply = emulator.list_v2_in("ap-southeast-2", "").await;
    assert_eq!(
        reply.status, 200,
        "case region=ap-southeast-2: expected 200, got {} {}",
        reply.status, reply.body
    );
    let listed = reply
        .at("/clusterInfoList")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        listed.is_empty(),
        "case region=ap-southeast-2: expected no clusters, got {listed:?}"
    );
}

/// RULE: a listing is a report, so calling it does not change what a later caller
/// sees — beyond the `CREATING` -> `ACTIVE` transition the clock already owed.
#[tokio::test]
async fn list_v2_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    for name in ["stable-a", "stable-b"] {
        emulator.create_ok(&ClusterRequest::named(name)).await;
    }
    sleep_past_activation();

    let first = emulator.list_v2("").await;
    for round in 1..=3 {
        let again = emulator.list_v2("").await;
        assert_eq!(
            again.status, 200,
            "case round={round}: expected 200, got {} {}",
            again.status, again.body
        );
        assert_eq!(
            again.body, first.body,
            "case round={round}: a repeated listing must report the same clusters, got {} then {}",
            first.body, again.body
        );
    }

    // And a delete between two listings is the only thing that changes them.
    let arn = first
        .at("/clusterInfoList/0/clusterArn")
        .as_str()
        .expect("the first listing must report a clusterArn")
        .to_string();
    let version = first
        .at("/clusterInfoList/0/currentVersion")
        .as_str()
        .expect("the first listing must report a currentVersion")
        .to_string();
    let deleted = emulator
        .call(
            "DELETE",
            &format!(
                "/v1/clusters/{}?currentVersion={}",
                pct(&arn),
                pct(&version)
            ),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteCluster to return 200, got {} {}",
        deleted.status, deleted.body
    );
    let after = emulator.list_v2("").await;
    let remaining: Vec<&str> = after
        .at("/clusterInfoList")
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry["clusterArn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !remaining.contains(&arn.as_str()),
        "a deleted cluster must not be listed, got {remaining:?}"
    );
}
