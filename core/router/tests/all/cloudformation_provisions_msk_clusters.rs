// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::MSK::Cluster` or an
//! `AWS::MSK::ServerlessCluster` really creates that cluster in Amazon MSK, and the
//! identifier the stack hands back is the cluster ARN Amazon MSK answers to.
//!
//! The rule under test: **a stack's physical resource id is the identifier the service that
//! owns the resource assigned — for a type whose identifier that service MINTS, the one its
//! create answered with — so `Ref` and `Fn::GetAtt` resolve to a value the owning service's
//! own read resolves to the resource the template configured.** Two crates have to agree
//! for that to hold (CloudFormation makes the call; Amazon MSK serves it and mints the
//! ARN), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: which of the two published cluster
//! types the template declares, how many brokers a provisioned cluster asks for and under
//! which instance type and Kafka version, how many VPC configurations and subnets a
//! serverless cluster names, whether the declaration carries nested property structures
//! (`StorageInfo.EBSStorageInfo`, whose API member is not its property name with the initial
//! letter lower-cased, `EncryptionInfo`, `ClientAuthentication`), whether it carries `Tags`
//! and the update-only `CurrentVersion` property, whether it is written in JSON or YAML, and
//! which intrinsic the outputs read the resource with (`Ref`, `Fn::GetAtt … Arn`, `Fn::Sub`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the cluster ARN itself, which is Amazon MSK's to mint:
//! what is checked about it is that ONE value appears everywhere the stack reports the
//! resource's identity, that it has the form Amazon MSK publishes, and that that service's
//! own read resolves it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
// The same encoder escapes an ARN for a non-greedy URI label, which is what an SDK sends.
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) and the
/// cluster's own `CREATING` -> `ACTIVE` span (`CLUSTER_ACTIVATION_DELAY`, 500 ms) to come
/// due, so the stack reports the template it deployed and Amazon MSK reports the cluster
/// ready.
const PAST_BOTH_SPANS: Duration = Duration::from_millis(1_200);

/// The single account id this emulator mints every ARN under.
const ACCOUNT: &str = "000000000000";

/// Which kind of cluster a row declares, and so which of Amazon MSK's two read surfaces
/// resolves the ARN the stack reported: a provisioned cluster is described by
/// `GET /v1/clusters/{ClusterArn}`, while a serverless one has no brokers for that answer's
/// `ClusterInfo` to describe and is described by `GET /api/v2/clusters/{ClusterArn}`.
#[derive(PartialEq, Eq)]
enum Kind {
    Provisioned,
    Serverless,
}

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_kafka_kafka::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Amazon MSK operation over its restJson1 wire, signed for `region`. The service
/// signs under `kafka`, which is also its endpoint prefix.
async fn kafka(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("kafka.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("kafka", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// One output whose value is COMPUTED from the cluster's ARN: the key, and the value the
/// rule says it must carry once Amazon MSK has minted that ARN.
type DerivedOutput = (&'static str, fn(&str) -> String);

/// One row of the table: a template, and what the rule says the cluster it declares must
/// then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in — which is also the Region the cluster must be created
    /// in, since the ARN carries it and Amazon MSK keys a cluster by Region.
    region: &'static str,
    /// Which published MSK cluster type the row declares.
    kind: Kind,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The `ClusterName` the template declares, which the create must carry out.
    expected_name: &'static str,
    /// The number of brokers the cluster must report, for a provisioned row.
    expected_brokers: Option<i64>,
    /// The broker instance type the cluster must report, for a provisioned row.
    expected_instance_type: Option<&'static str>,
    /// The Apache Kafka version the cluster must report, for a provisioned row.
    expected_kafka_version: Option<&'static str>,
    /// The EBS volume size the cluster's brokers must report, for a row that declares the
    /// nested `StorageInfo.EBSStorageInfo` structure.
    expected_volume_size: Option<i64>,
    /// The subnets each declared VPC configuration must report, for a serverless row.
    expected_subnets: &'static [&'static [&'static str]],
    /// The security groups each declared VPC configuration must report, for a serverless
    /// row. Empty where the declaration names none for that configuration.
    expected_security_groups: &'static [&'static [&'static str]],
    /// Whether the cluster must report SASL/IAM client authentication as turned on.
    expected_iam_authentication: bool,
    /// The tags the template declares, which the cluster must carry beside the three
    /// CloudFormation records on every resource it provisions.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the cluster ARN, each written with a different
    /// intrinsic over the same resource.
    cluster_arn_outputs: &'static [&'static str],
    /// Other outputs the stack must report, as `(key, value computed from the ARN)`.
    derived_outputs: &'static [DerivedOutput],
}

const ROWS: &[Row] = &[
    Row {
        case: "a provisioned cluster, 2 brokers, kafka.t3.small, SASL/IAM, tagged, JSON, \
               Ref and Fn::GetAtt Arn",
        region: "us-east-1",
        kind: Kind::Provisioned,
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an MSK provisioned cluster",
                "Resources": {
                    "MskCluster": {
                        "Type": "AWS::MSK::Cluster",
                        "Properties": {
                            "ClusterName": "orders-stream",
                            "KafkaVersion": "3.6.0",
                            "NumberOfBrokerNodes": 2,
                            "BrokerNodeGroupInfo": {
                                "InstanceType": "kafka.t3.small",
                                "ClientSubnets": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"],
                                "SecurityGroups": ["sg-0123456789abcdef0"]
                            },
                            "ClientAuthentication": {
                                "Sasl": { "Iam": { "Enabled": true } }
                            },
                            "Tags": { "Owner": "payments", "Tier": "prod" }
                        }
                    }
                },
                "Outputs": {
                    "ClusterArn": { "Value": { "Ref": "MskCluster" } },
                    "ClusterArnAttribute": {
                        "Value": { "Fn::GetAtt": ["MskCluster", "Arn"] }
                    }
                }
            })
            .to_string()
        },
        expected_name: "orders-stream",
        expected_brokers: Some(2),
        expected_instance_type: Some("kafka.t3.small"),
        expected_kafka_version: Some("3.6.0"),
        expected_volume_size: None,
        expected_subnets: &[],
        expected_security_groups: &[],
        expected_iam_authentication: true,
        // Amazon MSK's tag rules admit letters, digits, white space and `+ - = . _ : / @`.
        expected_tags: &[("Owner", "payments"), ("Tier", "prod")],
        cluster_arn_outputs: &["ClusterArn", "ClusterArnAttribute"],
        derived_outputs: &[],
    },
    Row {
        case: "a provisioned cluster, 3 brokers, kafka.m5.large, EBSStorageInfo and \
               EncryptionInfo, YAML, Fn::GetAtt Arn",
        region: "us-west-2",
        kind: Kind::Provisioned,
        template: || {
            [
                "Resources:",
                "  MskCluster:",
                "    Type: AWS::MSK::Cluster",
                "    Properties:",
                "      ClusterName: events-stream",
                "      KafkaVersion: 3.5.1",
                "      NumberOfBrokerNodes: 3",
                "      EnhancedMonitoring: PER_BROKER",
                "      StorageMode: LOCAL",
                "      BrokerNodeGroupInfo:",
                "        InstanceType: kafka.m5.large",
                "        ClientSubnets:",
                "          - subnet-11111111",
                "          - subnet-22222222",
                "          - subnet-33333333",
                "        StorageInfo:",
                "          EBSStorageInfo:",
                "            VolumeSize: 200",
                "      EncryptionInfo:",
                "        EncryptionInTransit:",
                "          ClientBroker: TLS",
                "          InCluster: true",
                "Outputs:",
                "  ClusterArn:",
                "    Value: !GetAtt MskCluster.Arn",
                "",
            ]
            .join("\n")
        },
        expected_name: "events-stream",
        expected_brokers: Some(3),
        expected_instance_type: Some("kafka.m5.large"),
        expected_kafka_version: Some("3.5.1"),
        expected_volume_size: Some(200),
        expected_subnets: &[],
        expected_security_groups: &[],
        expected_iam_authentication: false,
        expected_tags: &[],
        cluster_arn_outputs: &["ClusterArn"],
        derived_outputs: &[],
    },
    Row {
        case: "a provisioned cluster declaring the update-only CurrentVersion, 1 broker, \
               Fn::Sub over Ref",
        region: "eu-west-1",
        kind: Kind::Provisioned,
        template: || {
            json!({
                "Resources": {
                    "MskCluster": {
                        "Type": "AWS::MSK::Cluster",
                        "Properties": {
                            "ClusterName": "single-broker",
                            "KafkaVersion": "2.8.1",
                            "NumberOfBrokerNodes": 1,
                            "CurrentVersion": "K3AEGXETSR30VB",
                            "BrokerNodeGroupInfo": {
                                "InstanceType": "kafka.t3.small",
                                "ClientSubnets": ["subnet-aaaaaaaa"]
                            }
                        }
                    }
                },
                "Outputs": {
                    "ClusterArn": { "Value": { "Ref": "MskCluster" } },
                    "ClusterHandle": {
                        "Value": {
                            "Fn::Sub": [
                                "cluster=${Arn}",
                                { "Arn": { "Ref": "MskCluster" } }
                            ]
                        }
                    }
                }
            })
            .to_string()
        },
        expected_name: "single-broker",
        expected_brokers: Some(1),
        expected_instance_type: Some("kafka.t3.small"),
        expected_kafka_version: Some("2.8.1"),
        expected_volume_size: None,
        expected_subnets: &[],
        expected_security_groups: &[],
        expected_iam_authentication: false,
        expected_tags: &[],
        cluster_arn_outputs: &["ClusterArn"],
        derived_outputs: &[("ClusterHandle", |arn| format!("cluster={arn}"))],
    },
    Row {
        case: "a serverless cluster, one VpcConfig of two subnets and a security group, \
               SASL/IAM, tagged, JSON, Ref",
        region: "us-east-2",
        kind: Kind::Serverless,
        template: || {
            json!({
                "Resources": {
                    "MskServerless": {
                        "Type": "AWS::MSK::ServerlessCluster",
                        "Properties": {
                            "ClusterName": "serverless-orders",
                            "ClientAuthentication": {
                                "Sasl": { "Iam": { "Enabled": true } }
                            },
                            "VpcConfigs": [
                                {
                                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"],
                                    "SecurityGroups": ["sg-0123456789abcdef0"]
                                }
                            ],
                            "Tags": { "Owner": "orders" }
                        }
                    }
                },
                "Outputs": {
                    "ClusterArn": { "Value": { "Ref": "MskServerless" } }
                }
            })
            .to_string()
        },
        expected_name: "serverless-orders",
        expected_brokers: None,
        expected_instance_type: None,
        expected_kafka_version: None,
        expected_volume_size: None,
        expected_subnets: &[&["subnet-0a1b2c3d", "subnet-4e5f6a7b"]],
        expected_security_groups: &[&["sg-0123456789abcdef0"]],
        expected_iam_authentication: true,
        expected_tags: &[("Owner", "orders")],
        cluster_arn_outputs: &["ClusterArn"],
        derived_outputs: &[],
    },
    Row {
        case: "a serverless cluster, two VpcConfigs, no security groups on the second, \
               YAML, Ref and Fn::GetAtt Arn",
        region: "ap-northeast-1",
        kind: Kind::Serverless,
        template: || {
            [
                "Resources:",
                "  MskServerless:",
                "    Type: AWS::MSK::ServerlessCluster",
                "    Properties:",
                "      ClusterName: serverless-events",
                "      ClientAuthentication:",
                "        Sasl:",
                "          Iam:",
                "            Enabled: true",
                "      VpcConfigs:",
                "        - SubnetIds:",
                "            - subnet-11111111",
                "          SecurityGroups:",
                "            - sg-11111111",
                "            - sg-22222222",
                "        - SubnetIds:",
                "            - subnet-22222222",
                "            - subnet-33333333",
                "Outputs:",
                "  ClusterArn:",
                "    Value: !Ref MskServerless",
                "  ClusterArnAttribute:",
                "    Value: !GetAtt MskServerless.Arn",
                "",
            ]
            .join("\n")
        },
        expected_name: "serverless-events",
        expected_brokers: None,
        expected_instance_type: None,
        expected_kafka_version: None,
        expected_volume_size: None,
        expected_subnets: &[&["subnet-11111111"], &["subnet-22222222", "subnet-33333333"]],
        expected_security_groups: &[&["sg-11111111", "sg-22222222"], &[]],
        expected_iam_authentication: true,
        expected_tags: &[],
        cluster_arn_outputs: &["ClusterArn", "ClusterArnAttribute"],
        derived_outputs: &[],
    },
];

/// Every row: the identifier the stack reports for its MSK cluster is the ARN Amazon MSK
/// minted, that service's own read resolves it to the cluster the template configured, and
/// deleting the stack deletes it there.
#[tokio::test]
async fn a_stack_provisions_the_msk_cluster_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("msk-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The stack reports the template it deployed once the create COMPLETES.
        tokio::time::sleep(PAST_BOTH_SPANS).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        // The physical resource id of the declared resource: what a client reads out of
        // DescribeStackResources, and what every identity output must carry.
        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let cluster_arn = opt_element(&resources, "PhysicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <PhysicalResourceId> in {resources}"));
        // The form Amazon MSK publishes for a cluster ARN:
        // `arn:aws:kafka:<region>:<account>:cluster/<name>/<id>`.
        let expected_prefix =
            format!("arn:aws:kafka:{region}:{ACCOUNT}:cluster/{}/", row.expected_name);
        assert!(
            cluster_arn.starts_with(&expected_prefix),
            "case {case:?}: expected a cluster ARN starting {expected_prefix:?}, got \
             {cluster_arn:?}"
        );
        let minted_id = &cluster_arn[expected_prefix.len()..];
        assert!(
            !minted_id.is_empty() && !minted_id.contains('/'),
            "case {case:?}: expected one service-minted identifier after the cluster name, \
             got {minted_id:?}"
        );

        let outputs = outputs_of(&described);
        for key in row.cluster_arn_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&cluster_arn),
                "case {case:?}: expected output {key:?} to be the cluster ARN \
                 {cluster_arn:?}, got {:?}",
                outputs.get(*key)
            );
        }
        for (key, expected) in row.derived_outputs {
            let expected = expected(&cluster_arn);
            assert_eq!(
                outputs.get(*key),
                Some(&expected),
                "case {case:?}: expected output {key:?} to be {expected:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // And Amazon MSK itself resolves that ARN to the cluster the template configured.
        let read_uri = match row.kind {
            Kind::Provisioned => format!("/v1/clusters/{}", form_encode(&cluster_arn)),
            Kind::Serverless => format!("/api/v2/clusters/{}", form_encode(&cluster_arn)),
        };
        let (status, answer) = kafka(&router, region, "GET", &read_uri).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the Amazon MSK read of the ARN the stack \
             reported, got {status} with body {answer}"
        );
        let info = answer
            .get("clusterInfo")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no clusterInfo in {answer}"));
        assert_eq!(
            info.get("clusterArn").and_then(Value::as_str),
            Some(cluster_arn.as_str()),
            "case {case:?}: expected the cluster to report its own ARN, got {info}"
        );
        assert_eq!(
            info.get("clusterName").and_then(Value::as_str),
            Some(row.expected_name),
            "case {case:?}: expected clusterName {:?}, got {info}",
            row.expected_name
        );
        assert_eq!(
            info.get("state").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: expected the cluster to be ACTIVE once its activation span came \
             due, got {info}"
        );

        // The configuration each kind of cluster reports, as the template declared it.
        let body = match row.kind {
            // The v1 read reports a provisioned cluster's configuration at the top of
            // `ClusterInfo`.
            Kind::Provisioned => info.clone(),
            // The v2 read reports it under the member for the kind of cluster it is.
            Kind::Serverless => info
                .get("serverless")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no serverless body in {info}")),
        };
        if let Some(expected) = row.expected_brokers {
            assert_eq!(
                body.get("numberOfBrokerNodes").and_then(Value::as_i64),
                Some(expected),
                "case {case:?}: expected numberOfBrokerNodes {expected}, got {body}"
            );
        }
        let broker_group = body.get("brokerNodeGroupInfo").cloned();
        if let Some(expected) = row.expected_instance_type {
            assert_eq!(
                broker_group
                    .as_ref()
                    .and_then(|group| group.get("instanceType"))
                    .and_then(Value::as_str),
                Some(expected),
                "case {case:?}: expected instanceType {expected:?}, got {body}"
            );
        }
        if let Some(expected) = row.expected_kafka_version {
            assert_eq!(
                body.get("currentBrokerSoftwareInfo")
                    .and_then(|software| software.get("kafkaVersion"))
                    .and_then(Value::as_str),
                Some(expected),
                "case {case:?}: expected kafkaVersion {expected:?}, got {body}"
            );
        }
        // `StorageInfo.EBSStorageInfo` is the one property whose API member is not its own
        // name with the initial letter lower-cased, so a row declaring it is what proves the
        // rename reached the create.
        if let Some(expected) = row.expected_volume_size {
            assert_eq!(
                broker_group
                    .as_ref()
                    .and_then(|group| group.get("storageInfo"))
                    .and_then(|storage| storage.get("ebsStorageInfo"))
                    .and_then(|ebs| ebs.get("volumeSize"))
                    .and_then(Value::as_i64),
                Some(expected),
                "case {case:?}: expected the declared EBS volumeSize {expected}, got {body}"
            );
        }
        for (position, expected) in row.expected_subnets.iter().enumerate() {
            let reported: Vec<String> = body
                .get("vpcConfigs")
                .and_then(Value::as_array)
                .and_then(|configs| configs.get(position))
                .and_then(|config| config.get("subnetIds"))
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .map(|id| id.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected VPC configuration {position}'s subnetIds \
                 {expected:?}, got {reported:?} in {body}"
            );
        }
        // The type publishes a VPC configuration's security groups as `SecurityGroups`
        // where the API member is `securityGroupIds`, so this is what proves that
        // per-member mapping reached the create.
        for (position, expected) in row.expected_security_groups.iter().enumerate() {
            let reported: Vec<String> = body
                .get("vpcConfigs")
                .and_then(Value::as_array)
                .and_then(|configs| configs.get(position))
                .and_then(|config| config.get("securityGroupIds"))
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .map(|id| id.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected VPC configuration {position}'s securityGroupIds \
                 {expected:?}, got {reported:?} in {body}"
            );
        }
        let iam_enabled = body
            .get("clientAuthentication")
            .and_then(|authentication| authentication.get("sasl"))
            .and_then(|sasl| sasl.get("iam"))
            .and_then(|iam| iam.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert_eq!(
            iam_enabled, row.expected_iam_authentication,
            "case {case:?}: expected SASL/IAM authentication enabled={}, got {body}",
            row.expected_iam_authentication
        );

        // A provisioned cluster the stack created is reachable the way a client reaches
        // one: its bootstrap brokers.
        if row.kind == Kind::Provisioned {
            let (status, brokers) = kafka(
                &router,
                region,
                "GET",
                &format!(
                    "/v1/clusters/{}/bootstrap-brokers",
                    form_encode(&cluster_arn)
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetBootstrapBrokers on the ARN the \
                 stack reported, got {status} with body {brokers}"
            );
            assert!(
                brokers
                    .as_object()
                    .is_some_and(|strings| strings.values().any(|value| value
                        .as_str()
                        .is_some_and(|hosts| hosts.contains(":")))),
                "case {case:?}: expected at least one host:port bootstrap string, got \
                 {brokers}"
            );
        }

        // Beside what the template declared, CloudFormation records three tags of its own on
        // every resource it provisions — `aws:cloudformation:stack-name`,
        // `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id`. Amazon MSK
        // refuses the reserved `aws:` namespace to a caller, so those three are left off the
        // create and the cluster carries the declaration's own tags.
        let (status, tagged) = kafka(
            &router,
            region,
            "GET",
            &format!("/v1/tags/{}", form_encode(&cluster_arn)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with \
             body {tagged}"
        );
        let reported_tags = tagged.get("tags").cloned().unwrap_or(Value::Null);
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported_tags.get(*key).and_then(Value::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported_tags}"
            );
        }
        assert_eq!(
            reported_tags.as_object().map(serde_json::Map::len),
            Some(row.expected_tags.len()),
            "case {case:?}: expected exactly the {} tag(s) the template declared, got \
             {reported_tags}",
            row.expected_tags.len()
        );

        // Deleting the stack deletes the cluster Amazon MSK holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = kafka(&router, region, "GET", &read_uri).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from the Amazon MSK read once the stack \
             deleted the cluster, got {status} with body {gone}"
        );
    }
}

/// A second stack declaring a cluster under a name the first stack's cluster already has is
/// NOT reported as created: a cluster name identifies a cluster within an account and
/// Region, and Amazon MSK's own refusal is what the stack carries.
///
/// The rule this pins is the other half of the one above — nothing is passed off as created
/// when the owning service refused to create it — and it is where the ARN the FIRST stack
/// reported must still resolve afterwards.
#[tokio::test]
async fn a_second_cluster_of_one_name_fails_the_stack() {
    let router = build_router();
    let case = "two stacks declaring one cluster name in one Region";
    let region = "eu-central-1";
    let template = json!({
        "Resources": {
            "MskCluster": {
                "Type": "AWS::MSK::Cluster",
                "Properties": {
                    "ClusterName": "contended-name",
                    "KafkaVersion": "3.6.0",
                    "NumberOfBrokerNodes": 2,
                    "BrokerNodeGroupInfo": {
                        "InstanceType": "kafka.t3.small",
                        "ClientSubnets": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"]
                    }
                }
            }
        },
        "Outputs": { "ClusterArn": { "Value": { "Ref": "MskCluster" } } }
    })
    .to_string();

    for stack in ["msk-first", "msk-second"] {
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", stack), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack for {stack}, got {status} with \
             body {body}"
        );
        tokio::time::sleep(PAST_BOTH_SPANS).await;
    }

    let (_, first) = cloudformation(&router, region, "DescribeStacks", &[("StackName", "msk-first")])
        .await;
    assert_eq!(
        opt_element(&first, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the first stack to reach CREATE_COMPLETE, got {first}"
    );
    let cluster_arn = outputs_of(&first)
        .remove("ClusterArn")
        .unwrap_or_else(|| panic!("case {case:?}: no ClusterArn output in {first}"));

    let (_, second) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", "msk-second")]).await;
    let status = opt_element(&second, "StackStatus").unwrap_or_default();
    assert!(
        status.starts_with("ROLLBACK") || status.starts_with("CREATE_FAILED"),
        "case {case:?}: expected the second stack to fail rather than report a cluster it \
         did not create, got {status:?} in {second}"
    );

    // The first stack's cluster is untouched: its ARN still resolves.
    let (status, described) = kafka(
        &router,
        region,
        "GET",
        &format!("/v1/clusters/{}", form_encode(&cluster_arn)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first stack's cluster to still resolve, got {status} \
         with body {described}"
    );
}
