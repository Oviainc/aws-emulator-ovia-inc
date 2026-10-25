// SPDX-License-Identifier: CC0-1.0

//! `ListNodes`: the broker nodes one cluster holds, as the stored record
//! describes them.
//!
//! RULE: `ListNodes` reports one `NodeInfo` per broker node the named cluster's
//! record holds, in broker-id order, every member derived from that record — the
//! node's own placement and endpoint, and the broker group's instance type and
//! deployed software — paged by `MaxResults`/`NextToken`; a `ClusterArn` that is
//! not a cluster ARN of this service is a bad request, a well-formed one this
//! region holds no cluster for is a 404, and a cluster with no broker nodes is
//! refused as provisioned-only.
//!
//! The rows differ along what the rule generalises over: the number of broker
//! nodes (both ends of the model's 1..15 range), the number of client subnets
//! the nodes are placed across (fewer, equal and more brokers than subnets), the
//! instance type and Kafka version the group runs, the page size, and the ways a
//! `ClusterArn` or a `NextToken` can fail to resolve.

use std::collections::BTreeSet;

use aws_smithy_types::date_time::Format;
use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    is_ec2_resource_id, MAX_RESULTS_LIMIT, NODE_TYPE_BROKER, PROVISIONED_ONLY_MESSAGE,
    RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// `GET /v1/clusters/{ClusterArn}/nodes` — the route this suite drives. `query`
/// is the whole query string including its `?`, or empty.
async fn list_nodes(emulator: &Emulator, cluster_arn: &str, query: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/nodes{query}", pct(cluster_arn)),
            None,
        )
        .await
}

/// The `nodeInfoList` of a listing that must have succeeded.
fn nodes(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListNodes to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/nodeInfoList")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListNodes reported no nodeInfoList: {}", reply.body))
        .clone()
}

/// The broker ids a listing reported, in the order it reported them.
fn broker_ids(case: &str, listed: &[Value]) -> Vec<i64> {
    listed
        .iter()
        .map(|node| {
            node.pointer("/brokerNodeInfo/brokerId")
                .and_then(Value::as_f64)
                .map(|id| id as i64)
                .unwrap_or_else(|| panic!("case {case}: a node reported no brokerId: {node}"))
        })
        .collect()
}

/// An ISO-8601 instant as its whole epoch seconds.
fn instant_secs(case: &str, text: &str) -> i64 {
    ::aws_smithy_types::DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case}: {text:?} is not an ISO-8601 instant: {error}")
        })
        .secs()
}

/// Every broker host a `GetBootstrapBrokers` answer names, with the per-path port
/// stripped: the same endpoint set the node listing must report, since both
/// readers render the same stored brokers.
fn bootstrap_hosts(case: &str, reply: &Reply) -> BTreeSet<String> {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected GetBootstrapBrokers to return 200, got {} {}",
        reply.status, reply.body
    );
    let mut hosts = BTreeSet::new();
    let members = reply
        .body
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: GetBootstrapBrokers returned {}", reply.body));
    for value in members.values() {
        let Some(text) = value.as_str() else { continue };
        for endpoint in text.split(',') {
            let (host, _port) = endpoint
                .rsplit_once(':')
                .unwrap_or_else(|| panic!("case {case}: {endpoint:?} carries no port"));
            hosts.insert(host.to_string());
        }
    }
    hosts
}

/// The table: rows differing along broker count and the number of client subnets
/// the brokers are placed across, plus the instance type and Kafka version the
/// group runs.
fn cluster_rows() -> Vec<ClusterRequest> {
    vec![
        // One broker in one subnet: the smallest cluster the model's
        // `NumberOfBrokerNodes` range (1..15) allows.
        ClusterRequest {
            brokers: 1,
            subnets: vec!["subnet-0a1b2c3d"],
            instance_type: "kafka.t3.small",
            ..ClusterRequest::named("nodes-one-broker")
        },
        // As many brokers as subnets: one node per Availability Zone.
        ClusterRequest::named("nodes-three-brokers"),
        // More brokers than subnets: the placement wraps round the subnet list.
        ClusterRequest {
            brokers: 7,
            subnets: vec!["subnet-aaaa1111", "subnet-bbbb2222", "subnet-cccc3333"],
            instance_type: "kafka.m7g.xlarge",
            kafka_version: "2.8.1",
            ..ClusterRequest::named("nodes-seven-brokers")
        },
        // The other end of the range, over two subnets so the wrap is uneven.
        ClusterRequest {
            brokers: 15,
            subnets: vec!["subnet-dddd4444", "subnet-eeee5555"],
            instance_type: "express.m7g.large",
            ..ClusterRequest::named("nodes-fifteen-brokers")
        },
        // Fewer brokers than subnets: the surplus subnets hold no node.
        ClusterRequest {
            brokers: 2,
            subnets: vec!["subnet-ffff6666", "subnet-00007777", "subnet-11118888"],
            kafka_version: "3.9.x",
            ..ClusterRequest::named("nodes-two-brokers")
        },
    ]
}

/// RULE: the listing reports one node per stored broker, in broker-id order,
/// with every member derived from the cluster record — and reporting them changes
/// nothing, so a second listing answers exactly the same.
#[tokio::test]
async fn the_node_listing_reports_every_stored_broker() {
    let emulator = Emulator::new();

    for row in cluster_rows() {
        let case = row.name.clone();
        let arn = emulator.create_ok(&row).await;
        sleep_past_activation();

        let reply = list_nodes(&emulator, &arn, "").await;
        let listed = nodes(&case, &reply);
        assert_eq!(
            listed.len() as i64,
            row.brokers,
            "case {case}: expected one node per broker ({}), got {}",
            row.brokers,
            listed.len()
        );
        // Every broker the record holds, once, in broker-id order.
        let ids = broker_ids(&case, &listed);
        let expected_ids: Vec<i64> = (1..=row.brokers).collect();
        assert_eq!(
            ids, expected_ids,
            "case {case}: expected broker ids {expected_ids:?} in order, got {ids:?}"
        );
        // A page that reported every node closes the list.
        assert!(
            reply.at("/nextToken").is_null(),
            "case {case}: a complete page carries no nextToken, got {:?}",
            reply.at("/nextToken")
        );

        let creation_time = emulator
            .describe(&arn)
            .await
            .at("/clusterInfo/creationTime")
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: DescribeCluster reported no creationTime"))
            as i64;

        let mut arns = BTreeSet::new();
        let mut endpoints = BTreeSet::new();
        for node in &listed {
            let id = node
                .pointer("/brokerNodeInfo/brokerId")
                .and_then(Value::as_f64)
                .map(|id| id as i64)
                .unwrap_or_else(|| panic!("case {case}: a node reported no brokerId"));
            let case = format!("{case} broker {id}");

            // The model's `NodeType` names `BROKER` alone, and this listing's
            // subject is the broker nodes.
            assert_eq!(
                node.get("nodeType").and_then(Value::as_str),
                Some(NODE_TYPE_BROKER),
                "case {case}: expected nodeType {NODE_TYPE_BROKER}, got {node}"
            );
            // A broker node is neither a ZooKeeper nor a controller node.
            assert!(
                node.get("zookeeperNodeInfo").is_none() && node.get("controllerNodeInfo").is_none(),
                "case {case}: a broker node carries no zookeeper or controller detail, got {node}"
            );
            // The node is named under the cluster that holds it, uniquely.
            let node_arn = node
                .get("nodeARN")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a node reported no nodeARN: {node}"));
            assert!(
                node_arn.starts_with(&format!("{arn}/")),
                "case {case}: expected a nodeARN under {arn}, got {node_arn:?}"
            );
            assert!(
                arns.insert(node_arn.to_string()),
                "case {case}: nodeARN {node_arn:?} was reported twice"
            );
            // The group's attributes come off the cluster, so every node reports
            // the type and software the cluster runs.
            assert_eq!(
                node.get("instanceType").and_then(Value::as_str),
                Some(row.instance_type),
                "case {case}: expected instanceType {:?}, got {node}",
                row.instance_type
            );
            assert_eq!(
                node.pointer("/brokerNodeInfo/currentBrokerSoftwareInfo/kafkaVersion")
                    .and_then(Value::as_str),
                Some(row.kafka_version),
                "case {case}: expected kafkaVersion {:?}, got {node}",
                row.kafka_version
            );
            // The brokers are placed round-robin over the submitted client
            // subnets, so broker 1 lands in the first one.
            let expected_subnet = row.subnets[((id - 1) as usize) % row.subnets.len()];
            assert_eq!(
                node.pointer("/brokerNodeInfo/clientSubnet")
                    .and_then(Value::as_str),
                Some(expected_subnet),
                "case {case}: expected clientSubnet {expected_subnet:?}, got {node}"
            );
            // One network interface per node, named as EC2 names one.
            let eni = node
                .pointer("/brokerNodeInfo/attachedENIId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a node reported no attachedENIId: {node}"));
            assert!(
                is_ec2_resource_id("eni", eni),
                "case {case}: expected an EC2 network-interface id, got {eni:?}"
            );
            // That interface answers on a private address of the client VPC.
            let ip = node
                .pointer("/brokerNodeInfo/clientVpcIpAddress")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case}: a node reported no clientVpcIpAddress: {node}")
                });
            let octets: Vec<u16> = ip
                .split('.')
                .map(|octet| {
                    octet.parse().unwrap_or_else(|_| {
                        panic!("case {case}: {ip:?} is not an IPv4 address")
                    })
                })
                .collect();
            assert_eq!(
                octets.len(),
                4,
                "case {case}: expected four octets, got {ip:?}"
            );
            assert_eq!(
                octets[0], 10,
                "case {case}: expected a 10.0.0.0/8 address, got {ip:?}"
            );
            assert!(
                (1..=254).contains(&octets[3]),
                "case {case}: expected a host octet in 1..=254, got {ip:?}"
            );
            // One endpoint per node: the host the bootstrap strings carry for it.
            let node_endpoints = node
                .pointer("/brokerNodeInfo/endpoints")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: a node reported no endpoints: {node}"));
            assert_eq!(
                node_endpoints.len(),
                1,
                "case {case}: expected one endpoint per node, got {node_endpoints:?}"
            );
            let endpoint = node_endpoints[0]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: an endpoint is not a string: {node}"));
            assert!(
                endpoint.starts_with(&format!("b-{id}.")),
                "case {case}: expected the b-{id} endpoint, got {endpoint:?}"
            );
            assert!(
                endpoints.insert(endpoint.to_string()),
                "case {case}: endpoint {endpoint:?} was reported twice"
            );
            // A broker the create laid out joined the cluster when the cluster
            // was created.
            let added = node
                .get("addedToClusterTime")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case}: a node reported no addedToClusterTime: {node}")
                });
            assert_eq!(
                instant_secs(&case, added),
                creation_time,
                "case {case}: expected addedToClusterTime at the cluster's creation instant \
                 ({creation_time}), got {added:?}"
            );
        }

        // The two readers of the same stored brokers agree about the endpoints.
        let hosts = bootstrap_hosts(&case, &emulator.bootstrap_brokers(&arn).await);
        assert_eq!(
            endpoints, hosts,
            "case {case}: expected the node endpoints to be the bootstrap hosts {hosts:?}, \
             got {endpoints:?}"
        );

        // Reporting the nodes changes nothing a later caller sees.
        let again = list_nodes(&emulator, &arn, "").await;
        assert_eq!(
            again.body, reply.body,
            "case {case}: a second listing must answer the same, got {} then {}",
            reply.body, again.body
        );
    }
}

/// RULE: `MaxResults` bounds one page and `NextToken` resumes after the broker the
/// previous page stopped at, so walking the pages reports every node exactly once
/// in broker-id order; only a page that left nodes behind carries a token.
#[tokio::test]
async fn the_node_listing_pages_in_broker_id_order() {
    let emulator = Emulator::new();
    let request = ClusterRequest {
        brokers: 15,
        ..ClusterRequest::named("nodes-paged")
    };
    let total = request.brokers;
    let arn = emulator.create_ok(&request).await;
    sleep_past_activation();

    // Page sizes spanning the class: one at a time, a size that divides the node
    // count, two that do not, the whole list in one page, a size larger than the
    // list, and the absent `MaxResults` that pages at the model's own maximum.
    let sizes: Vec<Option<i64>> = vec![
        Some(1),
        Some(3),
        Some(4),
        Some(7),
        Some(total),
        Some(MAX_RESULTS_LIMIT as i64),
        None,
    ];

    for size in sizes {
        let case = match size {
            Some(size) => format!("maxResults={size}"),
            None => "maxResults absent".to_string(),
        };
        let page_size = size.unwrap_or(MAX_RESULTS_LIMIT as i64);
        let mut seen: Vec<i64> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match (&token, size) {
                (None, None) => String::new(),
                (None, Some(size)) => format!("?maxResults={size}"),
                (Some(token), None) => format!("?nextToken={}", pct(token)),
                (Some(token), Some(size)) => {
                    format!("?maxResults={size}&nextToken={}", pct(token))
                }
            };
            let reply = list_nodes(&emulator, &arn, &query).await;
            let listed = nodes(&case, &reply);
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case}: the pages do not terminate, {pages} pages so far"
            );
            let remaining = total - seen.len() as i64;
            assert_eq!(
                listed.len() as i64,
                page_size.min(remaining),
                "case {case}: expected a page of {}, got {}",
                page_size.min(remaining),
                listed.len()
            );
            seen.extend(broker_ids(&case, &listed));
            match reply.at("/nextToken").as_str() {
                Some(next) => {
                    assert!(
                        (seen.len() as i64) < total,
                        "case {case}: a token was offered after every node was reported ({seen:?})"
                    );
                    token = Some(next.to_string());
                }
                None => {
                    assert_eq!(
                        seen.len() as i64,
                        total,
                        "case {case}: the listing closed after {seen:?}, expected {total} nodes"
                    );
                    break;
                }
            }
        }
        let expected: Vec<i64> = (1..=total).collect();
        assert_eq!(
            seen, expected,
            "case {case}: expected {expected:?} across the pages, got {seen:?}"
        );
    }
}

/// RULE: a `ClusterArn` that is not a cluster ARN of this service, and a
/// `NextToken` that was not minted for this cluster's node listing, are bad
/// requests (400); a well-formed cluster ARN this region holds no cluster for is
/// a 404; and a cluster with no broker nodes is refused as provisioned-only.
#[tokio::test]
async fn the_node_listing_refuses_an_unresolvable_request() {
    let emulator = Emulator::new();
    let live = emulator
        .create_ok(&ClusterRequest::named("nodes-refusals"))
        .await;
    let other = emulator
        .create_ok(&ClusterRequest::named("nodes-refusals-other"))
        .await;
    let serverless = emulator
        .call(
            "POST",
            "/api/v2/clusters",
            Some(json!({
                "clusterName": "nodes-serverless",
                "serverless": { "vpcConfigs": [{ "subnetIds": ["subnet-0a1b2c3d"] }] }
            })),
        )
        .await;
    assert_eq!(
        serverless.status, 200,
        "expected the serverless create to return 200, got {} {}",
        serverless.status, serverless.body
    );
    let serverless_arn = serverless
        .at("/clusterArn")
        .as_str()
        .expect("the serverless create returns a clusterArn")
        .to_string();
    sleep_past_activation();

    // A token minted for ANOTHER cluster's node listing: well formed, and not a
    // resume point for this list.
    let foreign_token = list_nodes(&emulator, &other, "?maxResults=1")
        .await
        .at("/nextToken")
        .as_str()
        .expect("a bounded page of a three-broker cluster carries a token")
        .to_string();

    // Rows: every way the request can fail to resolve — (case, ARN, query,
    // status, the message the refusal must carry when the service fixes one).
    let rows: Vec<(&str, String, String, u16, Option<&str>)> = vec![
        (
            "not an ARN at all",
            "nodes-refusals".to_string(),
            String::new(),
            400,
            None,
        ),
        (
            "an ARN of another service",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0a1b2c3d"),
            String::new(),
            400,
            None,
        ),
        (
            "a topic ARN of this service",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:topic/nodes-refusals/\
                 11111111-2222-3333-4444-555555555555-1/orders"
            ),
            String::new(),
            400,
            None,
        ),
        (
            "a well-formed cluster ARN no cluster holds",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/\
                 11111111-2222-3333-4444-555555555555-1"
            ),
            String::new(),
            404,
            Some(RESOURCE_NOT_FOUND_MESSAGE),
        ),
        (
            "a cluster with no broker nodes",
            serverless_arn.clone(),
            String::new(),
            400,
            Some(PROVISIONED_ONLY_MESSAGE),
        ),
        (
            "a token minted for another cluster",
            live.clone(),
            format!("?nextToken={}", pct(&foreign_token)),
            400,
            None,
        ),
        (
            "a token this listing never minted",
            live.clone(),
            "?nextToken=not-a-token".to_string(),
            400,
            None,
        ),
        (
            "a page size below the modelled minimum",
            live.clone(),
            "?maxResults=0".to_string(),
            400,
            None,
        ),
        (
            "a page size above the modelled maximum",
            live.clone(),
            format!("?maxResults={}", MAX_RESULTS_LIMIT + 1),
            400,
            None,
        ),
    ];

    for (case, cluster_arn, query, status, message) in &rows {
        let reply = list_nodes(&emulator, cluster_arn, query).await;
        assert_eq!(
            reply.status, *status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        let expected_code = if *status == 404 {
            "NotFoundException"
        } else {
            "BadRequestException"
        };
        assert!(
            reply.error_code().ends_with(expected_code),
            "case {case}: expected {expected_code}, got {:?}",
            reply.error_code()
        );
        if let Some(message) = message {
            assert_eq!(
                reply.error_message(),
                *message,
                "case {case}: expected the service's own wording, got {:?}",
                reply.error_message()
            );
        }
    }

    // A cluster lives in one region, so another region's endpoint holds none for
    // its ARN — the same answer an absent cluster gets.
    let elsewhere = emulator
        .call_in(
            "eu-west-1",
            "GET",
            &format!("/v1/clusters/{}/nodes", pct(&live)),
            None,
        )
        .await;
    assert_eq!(
        elsewhere.status, 404,
        "case region=eu-west-1: expected a 404 for another region's cluster, got {} {}",
        elsewhere.status, elsewhere.body
    );
}

/// RULE: the group-level attributes a node reports come off the cluster record,
/// so an applied `UpdateBrokerType` moves the instance type every node reports
/// while each node keeps the identity and placement it was given.
#[tokio::test]
async fn the_reported_instance_type_follows_an_applied_broker_type_update() {
    let emulator = Emulator::new();
    let request = ClusterRequest {
        brokers: 3,
        instance_type: "kafka.m5.large",
        ..ClusterRequest::named("nodes-retyped")
    };
    let arn = emulator.create_ok(&request).await;
    sleep_past_activation();
    let before = nodes("before the update", &list_nodes(&emulator, &arn, "").await);

    let version = emulator
        .describe(&arn)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .expect("DescribeCluster reports a currentVersion")
        .to_string();
    let target = "kafka.m7g.2xlarge";
    let update = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/nodes/type", pct(&arn)),
            Some(json!({ "currentVersion": version, "targetInstanceType": target })),
        )
        .await;
    assert_eq!(
        update.status, 200,
        "expected UpdateBrokerType to return 200, got {} {}",
        update.status, update.body
    );

    // Before the update is due, the nodes still report the type the cluster runs.
    let during = nodes("during the update", &list_nodes(&emulator, &arn, "").await);
    for node in &during {
        assert_eq!(
            node.get("instanceType").and_then(Value::as_str),
            Some(request.instance_type),
            "case during the update: expected the pre-update type {:?}, got {node}",
            request.instance_type
        );
    }

    sleep_past_update();
    let after = nodes("after the update", &list_nodes(&emulator, &arn, "").await);
    assert_eq!(
        after.len(),
        before.len(),
        "case after the update: expected the same {} nodes, got {}",
        before.len(),
        after.len()
    );
    for (old, new) in before.iter().zip(after.iter()) {
        assert_eq!(
            new.get("instanceType").and_then(Value::as_str),
            Some(target),
            "case after the update: expected instanceType {target:?}, got {new}"
        );
        // The node's own identity and placement are not what the update changed.
        for member in [
            "/nodeARN",
            "/addedToClusterTime",
            "/brokerNodeInfo/brokerId",
            "/brokerNodeInfo/attachedENIId",
            "/brokerNodeInfo/clientSubnet",
            "/brokerNodeInfo/clientVpcIpAddress",
            "/brokerNodeInfo/endpoints",
        ] {
            assert_eq!(
                new.pointer(member),
                old.pointer(member),
                "case after the update: {member} must not move, got {:?} then {:?}",
                old.pointer(member),
                new.pointer(member)
            );
        }
    }
}
