// SPDX-License-Identifier: CC0-1.0

//! `CreateClusterV2` (`POST /api/v2/clusters`), driven end to end over the same
//! router the emulator serves from.
//!
//! The RULE under test is that this create mints a cluster of the kind its body
//! names — a `Provisioned` body registers the SAME record a v1 `CreateCluster`
//! registers, a `Serverless` body registers a `SERVERLESS` one — into the one
//! cluster registry, so every reader resolves a v2-created cluster exactly as it
//! resolves a v1-created one, and the two bodies are alternatives: a request
//! naming both, or neither, describes no cluster.
//!
//! Each suite asserts that rule over a TABLE whose rows differ along what the rule
//! generalises over (cluster kind, broker count, Kafka version, encryption,
//! authentication, storage mode, monitoring level, VPC-configuration count, subnet
//! and security-group count, tag count, region) rather than over one recorded
//! request.

use serde_json::{json, Map, Value};

use crate::kafka_harness::{
    pct, sleep_past_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// `CreateClusterV2`'s URI, per the model's `http` binding on the operation.
const CREATE_V2_URI: &str = "/api/v2/clusters";

/// The SASL/IAM client-broker port, from "Port information for Amazon MSK" — the
/// one port a serverless cluster's clients bootstrap on, IAM access control being
/// the only authentication MSK Serverless offers.
const PORT_SASL_IAM: u16 = 9098;

/// Call `CreateClusterV2` with `body` in the suite's region.
async fn create_v2(emulator: &Emulator, body: Value) -> Reply {
    emulator.call("POST", CREATE_V2_URI, Some(body)).await
}

/// `DescribeClusterV2`'s URI, per the model's `http` binding on that operation.
fn describe_v2_uri(arn: &str) -> String {
    format!("/api/v2/clusters/{}", pct(arn))
}

/// Call `DescribeClusterV2` for `arn` in the suite's region.
async fn describe_v2(emulator: &Emulator, arn: &str) -> Reply {
    emulator.call("GET", &describe_v2_uri(arn), None).await
}

/// The v1 `CreateCluster` body of `row`, regrouped into the v2 request: the model's
/// `ProvisionedRequest` carries exactly the members the v1 request does apart from
/// the cluster name and the tags, which stay at the top level. Built from the ONE
/// v1 body, so a row's v1 and v2 creates cannot submit different configurations.
fn v2_provisioned_body(row: &ClusterRequest) -> Value {
    let mut provisioned = row.to_json();
    let members = provisioned
        .as_object_mut()
        .expect("the CreateCluster body is a JSON object");
    let name = members
        .remove("clusterName")
        .expect("the CreateCluster body names the cluster");
    let tags = members.remove("tags");
    let mut body = json!({ "clusterName": name, "provisioned": provisioned });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    body
}

/// One row of the serverless table: the cluster name, the `VpcConfigs` the create
/// submits as (subnets, security groups) pairs, whether it turns IAM client
/// authentication on, and the tags it carries.
#[derive(Debug, Clone)]
struct ServerlessRow {
    name: &'static str,
    vpc_configs: Vec<(Vec<&'static str>, Vec<&'static str>)>,
    iam: Option<bool>,
    tags: Vec<(&'static str, &'static str)>,
}

impl ServerlessRow {
    /// The simplest serverless create: one VPC configuration with one subnet.
    fn named(name: &'static str) -> Self {
        Self {
            name,
            vpc_configs: vec![(vec!["subnet-0a1b2c3d"], Vec::new())],
            iam: None,
            tags: Vec::new(),
        }
    }

    fn to_json(&self) -> Value {
        let configs: Vec<Value> = self
            .vpc_configs
            .iter()
            .map(|(subnets, groups)| {
                let mut config = json!({ "subnetIds": subnets });
                if !groups.is_empty() {
                    config["securityGroupIds"] = json!(groups);
                }
                config
            })
            .collect();
        let mut serverless = json!({ "vpcConfigs": configs });
        if let Some(enabled) = self.iam {
            serverless["clientAuthentication"] =
                json!({ "sasl": { "iam": { "enabled": enabled } } });
        }
        let mut body = json!({ "clusterName": self.name, "serverless": serverless });
        if !self.tags.is_empty() {
            body["tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect::<Map<String, Value>>(),
            );
        }
        body
    }

    /// The `vpcConfigs` a reader must report back for this row.
    fn expected_vpc_configs(&self) -> Value {
        Value::Array(
            self.vpc_configs
                .iter()
                .map(|(subnets, groups)| {
                    let mut config = json!({ "subnetIds": subnets });
                    if !groups.is_empty() {
                        config["securityGroupIds"] = json!(groups);
                    }
                    config
                })
                .collect(),
        )
    }
}

/// The provisioned table: the rows differ along broker count (both ends of the
/// model's 1..15 range), Kafka version, client-broker encryption, SASL mechanism,
/// storage mode, monitoring level, subnet count and tag count.
fn provisioned_rows() -> Vec<ClusterRequest> {
    vec![
        ClusterRequest::named("v2-defaults"),
        ClusterRequest {
            brokers: 1,
            kafka_version: "2.8.1",
            subnets: vec!["subnet-a"],
            ..ClusterRequest::named("v2-one-broker")
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
            kafka_version: "3.9.x",
            client_broker: Some("TLS_PLAINTEXT"),
            public_access: Some("SERVICE_PROVIDED_EIPS"),
            network_type: Some("DUAL"),
            vpc_tls: Some(true),
            ..ClusterRequest::named("v2-public-dual")
        },
    ]
}

/// RULE: a `Provisioned` body creates a `PROVISIONED` cluster and registers the
/// same record a v1 `CreateCluster` with the same configuration registers — so the
/// answer reports the minted ARN, the submitted name, `PROVISIONED` and the
/// `CREATING` state, and every reader reports the configuration that was submitted.
#[tokio::test]
async fn a_provisioned_body_registers_the_record_a_v1_create_registers() {
    let rows = provisioned_rows();
    let emulator = Emulator::new();

    // Each row is created twice: once through the v2 API and once, under another
    // name, through the v1 one. The two records must agree member for member on
    // everything that is not the cluster's identity.
    let mut created = Vec::new();
    for row in &rows {
        let reply = create_v2(&emulator, v2_provisioned_body(row)).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected CreateClusterV2 to return 200, got {} {}",
            reply.status, reply.body
        );
        let arn = reply
            .at("/clusterArn")
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: no clusterArn in {}", reply.body))
            .to_string();
        assert_eq!(
            reply.at("/clusterName").as_str(),
            Some(row.name.as_str()),
            "case {row:?}: wrong clusterName, got {}",
            reply.at("/clusterName")
        );
        assert_eq!(
            reply.at("/clusterType").as_str(),
            Some("PROVISIONED"),
            "case {row:?}: a cluster created with brokers is PROVISIONED, got {}",
            reply.at("/clusterType")
        );
        assert_eq!(
            reply.at("/state").as_str(),
            Some("CREATING"),
            "case {row:?}: a new cluster is CREATING, got {}",
            reply.at("/state")
        );
        let expected_prefix = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{}/", row.name);
        assert!(
            arn.starts_with(&expected_prefix),
            "case {row:?}: expected an ARN under {expected_prefix:?}, got {arn:?}"
        );

        let twin = ClusterRequest {
            name: format!("{}-v1", row.name),
            ..row.clone()
        };
        let twin_arn = emulator.create_ok(&twin).await;
        created.push((row.clone(), arn, twin_arn));
    }
    sleep_past_activation();

    for (row, arn, twin_arn) in &created {
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
            ("/clusterType", "PROVISIONED"),
            ("/state", "ACTIVE"),
            (
                "/provisioned/currentBrokerSoftwareInfo/kafkaVersion",
                row.kafka_version,
            ),
            (
                "/provisioned/brokerNodeGroupInfo/instanceType",
                row.instance_type,
            ),
            (
                "/provisioned/storageMode",
                row.storage_mode.unwrap_or("LOCAL"),
            ),
            (
                "/provisioned/enhancedMonitoring",
                row.enhanced_monitoring.unwrap_or("DEFAULT"),
            ),
            (
                "/provisioned/encryptionInfo/encryptionInTransit/clientBroker",
                row.client_broker.unwrap_or("TLS_PLAINTEXT"),
            ),
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
            "case {row:?}: expected {} broker node(s), got {:?}",
            row.brokers,
            cluster.pointer("/provisioned/numberOfBrokerNodes")
        );
        // A provisioned record carries no serverless arm: the per-kind detail
        // reported is the one the cluster's `ClusterType` names.
        assert!(
            cluster.pointer("/serverless").is_none(),
            "case {row:?}: a provisioned cluster reports no serverless member, got {:?}",
            cluster.pointer("/serverless")
        );
        let tags = cluster.pointer("/tags").cloned().unwrap_or(Value::Null);
        for (key, value) in &row.tags {
            assert_eq!(
                tags.pointer(&format!("/{key}")).and_then(Value::as_str),
                Some(*value),
                "case {row:?}: expected tag {key} = {value:?}, got {:?}",
                tags.pointer(&format!("/{key}"))
            );
        }

        // The v1 reader resolves it too, because both creates write one registry.
        let v1 = emulator.describe(arn).await;
        assert_eq!(
            v1.status, 200,
            "case {row:?}: expected DescribeCluster to report the v2-created cluster, got {} {}",
            v1.status, v1.body
        );
        assert_eq!(
            v1.at("/clusterInfo/clusterArn").as_str(),
            Some(arn.as_str()),
            "case {row:?}: expected {arn:?} back from the v1 reader, got {}",
            v1.at("/clusterInfo/clusterArn")
        );

        // And the record is the v1 create's record: every member that is not the
        // cluster's identity matches the twin created through `CreateCluster`.
        let twin = describe_v2(&emulator, twin_arn).await;
        let twin = twin.at("/clusterInfo");
        for member in [
            "/provisioned/brokerNodeGroupInfo",
            "/provisioned/numberOfBrokerNodes",
            "/provisioned/currentBrokerSoftwareInfo",
            "/provisioned/encryptionInfo",
            "/provisioned/enhancedMonitoring",
            "/provisioned/openMonitoring",
            "/provisioned/storageMode",
            "/provisioned/customerActionStatus",
            "/clusterType",
            "/state",
        ] {
            assert_eq!(
                cluster.pointer(member),
                twin.pointer(member),
                "case {row:?}: the v2 and v1 creates disagree about {member}: {:?} vs {:?}",
                cluster.pointer(member),
                twin.pointer(member)
            );
        }
    }
}

/// RULE: a `Serverless` body creates a `SERVERLESS` cluster, and the V2 readers
/// report it as the `Serverless` arm of the `Cluster` shape — the submitted VPC
/// configuration and client authentication, and no provisioned detail.
#[tokio::test]
async fn a_serverless_body_registers_a_serverless_cluster() {
    // The rows differ along VPC-configuration count, subnets per configuration,
    // security groups, whether IAM authentication is named, and tag count.
    let rows = vec![
        ServerlessRow::named("sl-minimal"),
        ServerlessRow {
            vpc_configs: vec![(
                vec!["subnet-0a1b2c3d", "subnet-1a2b3c4d", "subnet-2a3b4c5d"],
                vec!["sg-0a1b2c3d"],
            )],
            iam: Some(true),
            tags: vec![("env", "prod")],
            ..ServerlessRow::named("sl-three-subnets")
        },
        ServerlessRow {
            vpc_configs: vec![
                (vec!["subnet-aaa"], vec!["sg-aaa", "sg-bbb"]),
                (vec!["subnet-bbb", "subnet-ccc"], Vec::new()),
            ],
            iam: Some(false),
            tags: vec![("a", "1"), ("b", "2")],
            ..ServerlessRow::named("sl-two-vpcs")
        },
    ];

    let emulator = Emulator::new();
    let mut minted = Vec::new();
    for row in &rows {
        let reply = create_v2(&emulator, row.to_json()).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected CreateClusterV2 to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/clusterType").as_str(),
            Some("SERVERLESS"),
            "case {row:?}: a cluster created from a Serverless body is SERVERLESS, got {}",
            reply.at("/clusterType")
        );
        assert_eq!(
            reply.at("/state").as_str(),
            Some("CREATING"),
            "case {row:?}: a new cluster is CREATING, got {}",
            reply.at("/state")
        );
        assert_eq!(
            reply.at("/clusterName").as_str(),
            Some(row.name),
            "case {row:?}: wrong clusterName, got {}",
            reply.at("/clusterName")
        );
        let arn = reply
            .at("/clusterArn")
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: no clusterArn in {}", reply.body))
            .to_string();
        let expected_prefix = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{}/", row.name);
        assert!(
            arn.starts_with(&expected_prefix),
            "case {row:?}: expected an ARN under {expected_prefix:?}, got {arn:?}"
        );
        minted.push(arn);
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

        assert_eq!(
            cluster.pointer("/clusterType").and_then(Value::as_str),
            Some("SERVERLESS"),
            "case {row:?}: expected clusterType SERVERLESS, got {:?}",
            cluster.pointer("/clusterType")
        );
        assert_eq!(
            cluster.pointer("/state").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {row:?}: expected ACTIVE once the clock owed the transition, got {:?}",
            cluster.pointer("/state")
        );
        // The per-kind detail is the serverless arm, and ONLY it.
        assert!(
            cluster.pointer("/provisioned").is_none(),
            "case {row:?}: a serverless cluster reports no provisioned member, got {:?}",
            cluster.pointer("/provisioned")
        );
        assert_eq!(
            cluster
                .pointer("/serverless/vpcConfigs")
                .cloned()
                .unwrap_or(Value::Null),
            row.expected_vpc_configs(),
            "case {row:?}: expected the submitted vpcConfigs, got {:?}",
            cluster.pointer("/serverless/vpcConfigs")
        );
        match row.iam {
            Some(enabled) => assert_eq!(
                cluster
                    .pointer("/serverless/clientAuthentication/sasl/iam/enabled")
                    .and_then(Value::as_bool),
                Some(enabled),
                "case {row:?}: expected the submitted IAM setting, got {:?}",
                cluster.pointer("/serverless/clientAuthentication")
            ),
            None => assert!(
                cluster
                    .pointer("/serverless/clientAuthentication")
                    .is_none(),
                "case {row:?}: a create that named no authentication reports none, got {:?}",
                cluster.pointer("/serverless/clientAuthentication")
            ),
        }
        // `ServerlessRequest` models no connectivity member, so the cluster runs
        // the documented default network type (`NetworkType`: "By default, a
        // cluster uses the IPv4 network type").
        assert_eq!(
            cluster
                .pointer("/serverless/connectivityInfo/networkType")
                .and_then(Value::as_str),
            Some("IPV4"),
            "case {row:?}: expected the default network type, got {:?}",
            cluster.pointer("/serverless/connectivityInfo/networkType")
        );
        let tags = cluster.pointer("/tags").cloned().unwrap_or(Value::Null);
        for (key, value) in &row.tags {
            assert_eq!(
                tags.pointer(&format!("/{key}")).and_then(Value::as_str),
                Some(*value),
                "case {row:?}: expected tag {key} = {value:?}, got {:?}",
                tags.pointer(&format!("/{key}"))
            );
        }
        assert_eq!(
            tags.as_object().map(Map::len),
            Some(row.tags.len()),
            "case {row:?}: expected exactly {} tag(s), got {tags}",
            row.tags.len()
        );

        // One access path: a serverless cluster's clients bootstrap through its
        // own endpoint on the SASL/IAM port, so that is the one string reported
        // and every broker-list member is absent.
        let brokers = emulator.bootstrap_brokers(arn).await;
        assert_eq!(
            brokers.status, 200,
            "case {row:?}: expected GetBootstrapBrokers to return 200, got {} {}",
            brokers.status, brokers.body
        );
        let iam_string = brokers
            .at("/bootstrapBrokerStringSaslIam")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            iam_string.ends_with(&format!(
                ".kafka-serverless.{REGION}.amazonaws.com:{PORT_SASL_IAM}"
            )) && iam_string.starts_with("boot-"),
            "case {row:?}: expected a serverless bootstrap endpoint in {REGION}, got {iam_string:?}"
        );
        assert_eq!(
            brokers
                .body
                .as_object()
                .map(Map::len)
                .unwrap_or_default(),
            1,
            "case {row:?}: a serverless cluster reports exactly one bootstrap string, got {}",
            brokers.body
        );
    }

    // The listings partition the registry by kind: a serverless cluster is
    // reported by the V2 listing under its own `ClusterTypeFilter`, and the V1
    // listing — whose `ClusterInfo` describes broker nodes — reports none of them.
    let provisioned_arn = emulator
        .create_ok(&ClusterRequest::named("sl-provisioned-neighbour"))
        .await;
    sleep_past_activation();
    for (filter, expected) in [
        ("SERVERLESS", minted.clone()),
        ("PROVISIONED", vec![provisioned_arn.clone()]),
    ] {
        let reply = emulator
            .list_v2(&format!("?clusterTypeFilter={}", pct(filter)))
            .await;
        let mut listed: Vec<String> = reply
            .at("/clusterInfoList")
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|entry| entry["clusterArn"].as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        listed.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            listed, expected,
            "case clusterTypeFilter={filter}: expected {expected:?}, got {listed:?}"
        );
    }
    let v1_list = emulator.list("").await;
    let v1_arns: Vec<&str> = v1_list
        .at("/clusterInfoList")
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry["clusterArn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        v1_arns,
        vec![provisioned_arn.as_str()],
        "the v1 listing reports the provisioned clusters only, got {v1_arns:?}"
    );
}

/// RULE: a create names exactly ONE cluster type — a body naming both
/// `Provisioned` and `Serverless`, or neither, is refused with
/// `BadRequestException` (400) and registers nothing.
#[tokio::test]
async fn a_create_names_exactly_one_cluster_type() {
    let provisioned = v2_provisioned_body(&ClusterRequest::named("both-kinds"));
    let serverless = ServerlessRow::named("both-kinds").to_json();
    let mut both = provisioned.clone();
    both["serverless"] = serverless["serverless"].clone();

    // Rows: (case, body). Each one fails the same gate from a different side.
    let rows = vec![
        ("neither kind", json!({ "clusterName": "no-kind" })),
        ("both kinds", both),
        (
            "an empty provisioned body",
            json!({ "clusterName": "empty-provisioned", "provisioned": {} }),
        ),
        (
            "a serverless body with no VPC configuration",
            json!({ "clusterName": "no-vpcs", "serverless": { "vpcConfigs": [] } }),
        ),
        (
            "a serverless VPC configuration with no subnet",
            json!({
                "clusterName": "no-subnets",
                "serverless": { "vpcConfigs": [{ "subnetIds": [] }] }
            }),
        ),
    ];

    let emulator = Emulator::new();
    for (case, body) in &rows {
        let reply = create_v2(&emulator, body.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with("BadRequestException"),
            "case {case}: expected BadRequestException, got {:?}",
            reply.error_code()
        );
    }

    // Nothing was registered by any of them.
    let listed = emulator.list_v2("").await;
    assert_eq!(
        listed
            .at("/clusterInfoList")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "a refused create registers nothing, got {}",
        listed.body
    );
}

/// RULE: the broker count of a provisioned body stays inside the range the model
/// declares for it (1..15), and a tag the service would not accept from
/// `TagResource` is not accepted here either — each refusal is a 400 that
/// registers nothing.
#[tokio::test]
async fn a_create_outside_the_model_registers_nothing() {
    let emulator = Emulator::new();

    let too_many = {
        let mut body = v2_provisioned_body(&ClusterRequest::named("too-many-brokers"));
        body["provisioned"]["numberOfBrokerNodes"] = json!(16);
        body
    };
    let too_few = {
        let mut body = v2_provisioned_body(&ClusterRequest::named("too-few-brokers"));
        body["provisioned"]["numberOfBrokerNodes"] = json!(0);
        body
    };
    let bad_tag_key = {
        let mut body = v2_provisioned_body(&ClusterRequest::named("bad-tag-key"));
        body["tags"] = json!({ "": "empty-key" });
        body
    };
    let bad_tag_prefix = {
        let mut body = ServerlessRow::named("reserved-tag").to_json();
        body["tags"] = json!({ "aws:reserved": "no" });
        body
    };

    // Rows: (case, body). Each row breaks a different published bound of the
    // request, on both cluster kinds.
    let rows = vec![
        ("one broker node past the modelled maximum", too_many),
        ("one broker node below the modelled minimum", too_few),
        ("an empty tag key", bad_tag_key),
        ("a reserved tag prefix on a serverless create", bad_tag_prefix),
    ];

    for (case, body) in &rows {
        let reply = create_v2(&emulator, body.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with("BadRequestException"),
            "case {case}: expected BadRequestException, got {:?}",
            reply.error_code()
        );
    }

    let listed = emulator.list_v2("").await;
    assert_eq!(
        listed
            .at("/clusterInfoList")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "a refused create registers nothing, got {}",
        listed.body
    );

    // The broker count at each end of the range IS accepted, so the refusals
    // above are the bound and not the gate itself.
    for brokers in [1_i64, 15] {
        let row = ClusterRequest {
            brokers,
            subnets: vec!["subnet-a"],
            ..ClusterRequest::named(if brokers == 1 { "edge-one" } else { "edge-fifteen" })
        };
        let reply = create_v2(&emulator, v2_provisioned_body(&row)).await;
        assert_eq!(
            reply.status, 200,
            "case {brokers} broker node(s): expected 200, got {} {}",
            reply.status, reply.body
        );
    }
}

/// RULE: a cluster name identifies a cluster within an account and region
/// whichever kind it is, so a second create under a name already taken is a
/// `ConflictException` (409) — across the two kinds and across the two create
/// APIs — while the same name in another region is a different cluster.
#[tokio::test]
async fn a_cluster_name_is_unique_per_region_across_kinds_and_apis() {
    // Rows: (case, the create that takes the name, the create that must conflict).
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "provisioned then serverless",
            v2_provisioned_body(&ClusterRequest::named("dup-a")),
            ServerlessRow::named("dup-a").to_json(),
        ),
        (
            "serverless then provisioned",
            ServerlessRow::named("dup-b").to_json(),
            v2_provisioned_body(&ClusterRequest::named("dup-b")),
        ),
        (
            "serverless then serverless",
            ServerlessRow::named("dup-c").to_json(),
            ServerlessRow {
                vpc_configs: vec![(vec!["subnet-other"], Vec::new())],
                ..ServerlessRow::named("dup-c")
            }
            .to_json(),
        ),
    ];

    for (case, first, second) in &rows {
        let emulator = Emulator::new();
        let created = create_v2(&emulator, first.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected the first create to return 200, got {} {}",
            created.status, created.body
        );
        let arn = created.at("/clusterArn").as_str().unwrap_or_default().to_string();

        let repeated = create_v2(&emulator, second.clone()).await;
        assert_eq!(
            repeated.status, 409,
            "case {case}: expected HTTP 409 for a name already taken, got {} {}",
            repeated.status, repeated.body
        );
        assert!(
            repeated.error_code().ends_with("ConflictException"),
            "case {case}: expected ConflictException, got {:?}",
            repeated.error_code()
        );

        // The same name in another region is another cluster.
        let elsewhere = emulator
            .call_in("eu-west-1", "POST", CREATE_V2_URI, Some(second.clone()))
            .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {case}: expected the same name in another region to be created, got {} {}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.at("/clusterArn").as_str(),
            Some(arn.as_str()),
            "case {case}: the two regions must hold two clusters, got one ARN {arn:?}"
        );

        // And the conflict changed nothing: the first cluster still reads back.
        let reply = describe_v2(&emulator, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected the first cluster to still be readable, got {} {}",
            reply.status, reply.body
        );
    }

    // Across the two create APIs: a v1 `CreateCluster` takes the name first.
    let emulator = Emulator::new();
    let row = ClusterRequest::named("dup-across-apis");
    let _ = emulator.create_ok(&row).await;
    let repeated = create_v2(&emulator, v2_provisioned_body(&row)).await;
    assert_eq!(
        repeated.status, 409,
        "expected a v2 create to conflict with a v1-created name, got {} {}",
        repeated.status, repeated.body
    );
}

/// RULE: a cluster this create mints is reported `CREATING` until the activation
/// span the clock owes it has passed and `ACTIVE` after it — for either kind, and
/// reading it never moves it.
#[tokio::test]
async fn a_created_cluster_reports_the_state_the_clock_owes() {
    // Rows: (case, body) — one per cluster kind, since both kinds are minted in a
    // non-terminal state.
    let rows: Vec<(&str, Value)> = vec![
        (
            "provisioned",
            v2_provisioned_body(&ClusterRequest::named("clock-provisioned")),
        ),
        ("serverless", ServerlessRow::named("clock-serverless").to_json()),
    ];

    let emulator = Emulator::new();
    let mut minted = Vec::new();
    for (case, body) in &rows {
        let reply = create_v2(&emulator, body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200, got {} {}",
            reply.status, reply.body
        );
        minted.push((
            *case,
            reply.at("/clusterArn").as_str().unwrap_or_default().to_string(),
        ));
    }

    for (case, arn) in &minted {
        for attempt in 0..3 {
            let reply = describe_v2(&emulator, arn).await;
            assert_eq!(
                reply.at("/clusterInfo/state").as_str(),
                Some("CREATING"),
                "case {case} read #{attempt} before activation: expected CREATING, got {}",
                reply.at("/clusterInfo/state")
            );
        }
    }

    sleep_past_activation();
    for (case, arn) in &minted {
        for attempt in 0..3 {
            let reply = describe_v2(&emulator, arn).await;
            assert_eq!(
                reply.at("/clusterInfo/state").as_str(),
                Some("ACTIVE"),
                "case {case} read #{attempt} after activation: expected ACTIVE, got {}",
                reply.at("/clusterInfo/state")
            );
        }
    }
}

/// RULE: an operation whose subject is the broker nodes — the `ClusterInfo` views
/// and the broker-reconfiguring updates — refuses a cluster that has none, with
/// `BadRequestException` (400), and the refusal leaves the cluster as it was.
#[tokio::test]
async fn the_broker_node_operations_refuse_a_serverless_cluster() {
    let emulator = Emulator::new();
    let serverless = create_v2(&emulator, ServerlessRow::named("sl-guarded").to_json()).await;
    let arn = serverless
        .at("/clusterArn")
        .as_str()
        .expect("the serverless create returns a clusterArn")
        .to_string();
    sleep_past_activation();
    let version = describe_v2(&emulator, &arn)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .expect("a cluster reports its current version")
        .to_string();

    // Rows: (case, method, URI, body) — every implemented operation whose subject
    // is the broker nodes.
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "DescribeCluster",
            "GET",
            format!("/v1/clusters/{}", pct(&arn)),
            None,
        ),
        (
            "GetCompatibleKafkaVersions",
            "GET",
            format!("/v1/compatible-kafka-versions?clusterArn={}", pct(&arn)),
            None,
        ),
        (
            "ListNodes",
            "GET",
            format!("/v1/clusters/{}/nodes", pct(&arn)),
            None,
        ),
        (
            "UpdateBrokerType",
            "PUT",
            format!("/v1/clusters/{}/nodes/type", pct(&arn)),
            Some(json!({ "currentVersion": version, "targetInstanceType": "kafka.m5.large" })),
        ),
        (
            "UpdateBrokerStorage",
            "PUT",
            format!("/v1/clusters/{}/nodes/storage", pct(&arn)),
            Some(json!({
                "currentVersion": version,
                "targetBrokerEBSVolumeInfo": [{ "kafkaBrokerNodeId": "ALL", "volumeSizeGB": 200 }]
            })),
        ),
        (
            "UpdateStorage",
            "PUT",
            format!("/v1/clusters/{}/storage", pct(&arn)),
            Some(json!({ "currentVersion": version, "volumeSizeGB": 200 })),
        ),
        (
            "UpdateConnectivity",
            "PUT",
            format!("/v1/clusters/{}/connectivity", pct(&arn)),
            Some(json!({
                "currentVersion": version,
                "connectivityInfo": { "publicAccess": { "type": "DISABLED" } }
            })),
        ),
    ];

    for (case, method, uri, body) in &rows {
        let reply = emulator.call(method, uri, body.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400 for a serverless cluster, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with("BadRequestException"),
            "case {case}: expected BadRequestException, got {:?}",
            reply.error_code()
        );
    }

    // None of the refusals touched the cluster: it is still ACTIVE, at the same
    // version, with no operation in flight.
    let after = describe_v2(&emulator, &arn).await;
    assert_eq!(
        after.at("/clusterInfo/state").as_str(),
        Some("ACTIVE"),
        "the refusals must leave the cluster ACTIVE, got {}",
        after.at("/clusterInfo/state")
    );
    assert_eq!(
        after.at("/clusterInfo/currentVersion").as_str(),
        Some(version.as_str()),
        "the refusals must leave the version alone, got {}",
        after.at("/clusterInfo/currentVersion")
    );
    assert!(
        after.body.pointer("/clusterInfo/activeOperationArn").is_none(),
        "the refusals must start no operation, got {}",
        after.body
    );

    // The same operations still serve a provisioned cluster, so the refusals are
    // about the cluster's kind and not about the operations.
    let provisioned = emulator
        .create_ok(&ClusterRequest::named("sl-guarded-neighbour"))
        .await;
    sleep_past_activation();
    let reply = emulator.describe(&provisioned).await;
    assert_eq!(
        reply.status, 200,
        "expected DescribeCluster to serve a provisioned cluster, got {} {}",
        reply.status, reply.body
    );
    let versions = emulator
        .call(
            "GET",
            &format!(
                "/v1/compatible-kafka-versions?clusterArn={}",
                pct(&provisioned)
            ),
            None,
        )
        .await;
    assert_eq!(
        versions.status, 200,
        "expected GetCompatibleKafkaVersions to serve a provisioned cluster, got {} {}",
        versions.status, versions.body
    );
}
