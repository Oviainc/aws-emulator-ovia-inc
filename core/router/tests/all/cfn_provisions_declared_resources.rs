// SPDX-License-Identifier: CC0-1.0

//! CloudFormation creates the resources a template declares in the services that OWN
//! them.
//!
//! The rule under test: a stack's resources are not CloudFormation's own. An
//! `AWS::S3::Bucket` is a bucket in Amazon S3 and an `AWS::EMR::Cluster` is a cluster in
//! Amazon EMR, each created through that service's own API by whichever operation DEPLOYS
//! the template (`CreateStack`, `UpdateStack`, `ExecuteChangeSet`), and the stack's physical
//! resource id — which is what `Ref` resolves to — is the identifier THAT service
//! assigned: the bucket's name for a bucket (a name S3's naming rules admit, so lowercase
//! and DNS-compliant even when the stack and logical id are not), and the `j-` job-flow id
//! for a cluster. Reading the stack back therefore hands a client identifiers it can use:
//! `HeadBucket`/`PutObject` on the bucket succeed and `DescribeCluster` on the cluster id
//! answers the cluster, carrying the `Configurations` the template declared.
//!
//! This suite needs THREE services on one engine — CloudFormation reaches S3 and EMR
//! through the in-process callback, so a single-service router has neither to call — which
//! is why it lives here rather than in any one service's crate. The rows span what the rule
//! generalises over: whether the template names the resource or lets CloudFormation
//! generate the name, the case of the stack and logical id the generated name is built
//! from, the Region the stack is created in, how many stacks declare a resource of the same
//! type, a property that refers to another resource of the same stack, a resource type
//! this layer provisions nowhere (which keeps the identifier CloudFormation derives for
//! it), a create that is REFUSED (which must provision nothing at all), WHICH operation
//! deploys the template (a create, an update, and a change set's execution), and a resource
//! an update re-declares unchanged (which keeps the identifier it already has rather than
//! being made a second time).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The form encoder and the XML text reader every awsQuery suite in this crate shares:
// `query_wire` owns them once, "because a suite that writes its own drifts from the
// others", so this suite calls them rather than carrying copies.
use crate::query_wire::{elements, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Longer than CloudFormation's clock-owed stack transition (500 ms), so a stack this
/// suite created has reached `CREATE_COMPLETE` — which is when its resources are reported —
/// by the time it is read.
const PAST_STACK_TRANSITION: Duration = Duration::from_millis(900);

/// The documented bounds on an Amazon S3 general purpose bucket name.
const MIN_BUCKET_NAME_LEN: usize = 3;
/// See [`MIN_BUCKET_NAME_LEN`].
const MAX_BUCKET_NAME_LEN: usize = 63;

/// A SigV4 `Authorization` header whose credential scope names `signing_name` in `region`.
/// The signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// The emulator this suite drives: the whole router over one engine, and the runtime its
/// requests are blocked on.
struct Emulator {
    router: MultiServiceRouter,
    runtime: tokio::runtime::Runtime,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = build_router(engine);
        router.set_request_authentication(false);
        Self {
            router,
            runtime: tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime"),
        }
    }

    /// Drive one request and answer its status, headers and body.
    fn send(
        &self,
        signing_name: &str,
        region: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, http::HeaderMap, Vec<u8>) {
        let host = format!("{signing_name}.{region}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("authorization", authorization(signing_name, region));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.runtime.block_on(self.router.dispatch(request));
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        (status, headers, response.into_body().to_vec())
    }

    /// One `awsQuery` CloudFormation action: `POST /` with the members form-encoded.
    fn cloudformation(
        &self,
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
        let (status, _, payload) = self.send(
            "cloudformation",
            region,
            "POST",
            "/",
            &[("content-type", "application/x-www-form-urlencoded")],
            body.into_bytes(),
        );
        (status, String::from_utf8_lossy(&payload).into_owned())
    }

    /// One `awsJson1_1` EMR operation.
    fn emr(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let (status, _, payload) = self.send(
            "elasticmapreduce",
            region,
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", &format!("ElasticMapReduce.{operation}")),
            ],
            serde_json::to_vec(&body).expect("the request body serializes"),
        );
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One S3 request against a bucket or object path.
    fn s3(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: &[u8],
    ) -> (u16, http::HeaderMap, Vec<u8>) {
        self.send("s3", region, method, uri, &[], body.to_vec())
    }
}

/// The stack's resources as `(LogicalResourceId, PhysicalResourceId)` pairs, read member by
/// member so each logical id stays paired with its own physical id. Built on
/// [`crate::query_wire::elements`], which is this crate's one XML text reader.
fn resources_of(xml: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find("<member") {
        let after_tag = &rest[open + "<member".len()..];
        let Some(tag_end) = after_tag.find('>') else {
            break;
        };
        let after_open = &after_tag[tag_end + 1..];
        let Some(close) = after_open.find("</member>") else {
            break;
        };
        let entry = &after_open[..close];
        let logical = elements(entry, "LogicalResourceId").into_iter().next();
        let physical = elements(entry, "PhysicalResourceId").into_iter().next();
        if let (Some(logical), Some(physical)) = (logical, physical) {
            found.push((logical, physical));
        }
        rest = &after_open[close + "</member>".len()..];
    }
    found
}

/// One stack's outputs as `(OutputKey, OutputValue)` pairs.
fn outputs_of(xml: &str) -> Vec<(String, String)> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The physical resource id of one logical id, or a panic naming what was reported.
fn physical_id(resources: &[(String, String)], logical_id: &str) -> String {
    resources
        .iter()
        .find(|(logical, _)| logical == logical_id)
        .map(|(_, physical)| physical.clone())
        .unwrap_or_else(|| panic!("the stack must report {logical_id}, got {resources:?}"))
}

/// Whether `name` satisfies the documented Amazon S3 general purpose bucket naming rules
/// this suite can check: 3 to 63 characters, only lowercase letters, numbers, dots and
/// hyphens, beginning and ending with a letter or a number.
fn is_legal_bucket_name(name: &str) -> bool {
    let length = (MIN_BUCKET_NAME_LEN..=MAX_BUCKET_NAME_LEN).contains(&name.len());
    let charset = name.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '.'
            || character == '-'
    });
    let bounded = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && name
            .chars()
            .next_back()
            .is_some_and(|last| last.is_ascii_alphanumeric());
    length && charset && bounded
}

/// Whether `id` is shaped like an Amazon EMR job-flow id: `j-` and 13 upper-case
/// alphanumerics, the form EMR's own minted ids carry.
fn is_job_flow_id(id: &str) -> bool {
    let Some(body) = id.strip_prefix("j-") else {
        return false;
    };
    body.len() == 13
        && body
            .chars()
            .all(|character| character.is_ascii_digit() || character.is_ascii_uppercase())
}

/// A template declaring a bucket CloudFormation names, a bucket the template names, an EMR
/// cluster configured for the Glue Data Catalog and logging into that named bucket, and a
/// resource type this provisioning layer creates nowhere.
const FULL_TEMPLATE: &str = r#"{
 "Resources": {
  "ScriptBucket": { "Type": "AWS::S3::Bucket" },
  "NamedBucket": {
   "Type": "AWS::S3::Bucket",
   "Properties": { "BucketName": "cfn-named-logs-bucket" }
  },
  "Notifications": {
   "Type": "AWS::SNS::Topic",
   "Properties": { "TopicName": "cfn-provisioning-topic" }
  },
  "Cluster": {
   "Type": "AWS::EMR::Cluster",
   "Properties": {
    "Name": "cfn-emr-glue-catalog",
    "ReleaseLabel": "emr-6.15.0",
    "JobFlowRole": "EMR_EC2_DefaultRole",
    "ServiceRole": "EMR_DefaultRole",
    "LogUri": { "Fn::Sub": "s3://${NamedBucket}/logs/" },
    "VisibleToAllUsers": true,
    "Applications": [ { "Name": "Spark" }, { "Name": "Hive" } ],
    "Configurations": [
     {
      "Classification": "hive-site",
      "ConfigurationProperties": {
       "hive.metastore.client.factory.class": "com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory"
      }
     },
     {
      "Classification": "spark-hive-site",
      "ConfigurationProperties": {
       "hive.metastore.client.factory.class": "com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory"
      }
     }
    ],
    "Instances": {
     "MasterInstanceGroup": { "InstanceCount": 1, "InstanceType": "m5.xlarge", "Name": "Primary" },
     "CoreInstanceGroup": { "InstanceCount": 2, "InstanceType": "m5.xlarge", "Market": "ON_DEMAND" },
     "KeepJobFlowAliveWhenNoSteps": true,
     "TerminationProtected": false
    },
    "Tags": [ { "Key": "suite", "Value": "cfn-provisioning" } ]
   }
  }
 },
 "Outputs": {
  "ClusterId": { "Value": { "Ref": "Cluster" } },
  "ScriptBucketName": { "Value": { "Ref": "ScriptBucket" } },
  "ScriptBucketArn": { "Value": { "Fn::GetAtt": [ "ScriptBucket", "Arn" ] } },
  "TopicArn": { "Value": { "Ref": "Notifications" } }
 }
}"#;

/// A second template: one unnamed bucket and a cluster the template does not name either,
/// deployed in another Region.
const MINIMAL_TEMPLATE: &str = r#"{
 "Resources": {
  "DataBucket": { "Type": "AWS::S3::Bucket" },
  "Cluster": {
   "Type": "AWS::EMR::Cluster",
   "Properties": {
    "ReleaseLabel": "emr-7.1.0",
    "JobFlowRole": "EMR_EC2_DefaultRole",
    "ServiceRole": "EMR_DefaultRole",
    "Instances": {
     "MasterInstanceGroup": { "InstanceCount": 1, "InstanceType": "m5.xlarge" }
    }
   }
  }
 },
 "Outputs": { "DataBucketName": { "Value": { "Ref": "DataBucket" } } }
}"#;

/// [`MINIMAL_TEMPLATE`] as an UPDATE restates it: the same two resources, unchanged, plus
/// a bucket the template names and a second cluster. The rows it drives are what an
/// `UpdateStack` must do — create what it ADDS in the owning service, and leave what it
/// re-declares unchanged with the identifier that resource already has.
const UPDATED_SECOND_TEMPLATE: &str = r#"{
 "Resources": {
  "DataBucket": { "Type": "AWS::S3::Bucket" },
  "AddedBucket": {
   "Type": "AWS::S3::Bucket",
   "Properties": { "BucketName": "cfn-update-added-bucket" }
  },
  "Cluster": {
   "Type": "AWS::EMR::Cluster",
   "Properties": {
    "ReleaseLabel": "emr-7.1.0",
    "JobFlowRole": "EMR_EC2_DefaultRole",
    "ServiceRole": "EMR_DefaultRole",
    "Instances": {
     "MasterInstanceGroup": { "InstanceCount": 1, "InstanceType": "m5.xlarge" }
    }
   }
  },
  "AddedCluster": {
   "Type": "AWS::EMR::Cluster",
   "Properties": {
    "Name": "cfn-update-added-cluster",
    "ReleaseLabel": "emr-6.15.0",
    "JobFlowRole": "EMR_EC2_DefaultRole",
    "ServiceRole": "EMR_DefaultRole",
    "Instances": {
     "MasterInstanceGroup": { "InstanceCount": 1, "InstanceType": "m5.xlarge" }
    }
   }
  }
 }
}"#;

/// The template a CREATE change set carries, deployed by `ExecuteChangeSet` rather than by
/// `CreateStack` — the third operation that deploys a template.
const CHANGE_SET_TEMPLATE: &str = r#"{
 "Resources": {
  "StagingBucket": { "Type": "AWS::S3::Bucket" },
  "Cluster": {
   "Type": "AWS::EMR::Cluster",
   "Properties": {
    "Name": "cfn-change-set-cluster",
    "ReleaseLabel": "emr-7.1.0",
    "JobFlowRole": "EMR_EC2_DefaultRole",
    "ServiceRole": "EMR_DefaultRole",
    "Instances": {
     "MasterInstanceGroup": { "InstanceCount": 1, "InstanceType": "m5.xlarge" }
    }
   }
  }
 }
}"#;

/// A template whose create is REFUSED (its stack name is already live), declaring a bucket
/// and a cluster so the refusal can be checked to have provisioned neither.
const REFUSED_TEMPLATE: &str = r#"{
 "Resources": {
  "RefusedBucket": {
   "Type": "AWS::S3::Bucket",
   "Properties": { "BucketName": "cfn-refused-bucket" }
  },
  "RefusedCluster": {
   "Type": "AWS::EMR::Cluster",
   "Properties": {
    "Name": "cfn-refused-cluster",
    "ReleaseLabel": "emr-7.1.0",
    "JobFlowRole": "EMR_EC2_DefaultRole",
    "ServiceRole": "EMR_DefaultRole",
    "Instances": {
     "MasterInstanceGroup": { "InstanceCount": 1, "InstanceType": "m5.xlarge" }
    }
   }
  }
 }
}"#;

/// A cluster declared with instance FLEETS rather than instance GROUPS — the other form the
/// same three node types take. The fold onto `RunJobFlow`'s one `InstanceFleets` list is
/// what makes EMR report the cluster as an `INSTANCE_FLEET` collection; nothing the template
/// declared may be dropped on the way.
const FLEET_CLUSTER_TEMPLATE: &str = r#"{
 "Resources": {
  "FleetCluster": {
   "Type": "AWS::EMR::Cluster",
   "Properties": {
    "Name": "cfn-emr-fleet-cluster",
    "ReleaseLabel": "emr-6.15.0",
    "JobFlowRole": "EMR_EC2_DefaultRole",
    "ServiceRole": "EMR_DefaultRole",
    "Instances": {
     "MasterInstanceFleet": {
      "Name": "Primary",
      "TargetOnDemandCapacity": 1,
      "InstanceTypeConfigs": [ { "InstanceType": "m5.xlarge" } ]
     },
     "CoreInstanceFleet": {
      "Name": "Core",
      "TargetSpotCapacity": 2,
      "InstanceTypeConfigs": [ { "InstanceType": "m5.xlarge", "WeightedCapacity": 1 } ]
     }
    }
   }
  }
 }
}"#;

#[test]
fn a_cfn_declared_bucket_and_cluster_exist_in_s3_and_emr_under_the_ids_the_stack_reports() {
    std::thread::Builder::new()
        .name("cfn-provisioning".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = Emulator::new();

            // ---- The first stack, in us-east-1 (no S3 location constraint). ----
            let region = "us-east-1";
            let (status, body) = emulator.cloudformation(
                region,
                "CreateStack",
                &[
                    ("StackName", "cfn-provisioning-Suite"),
                    ("TemplateBody", FULL_TEMPLATE),
                ],
            );
            assert_eq!(
                status, 200,
                "CreateStack must succeed, got {status} with body {body}"
            );
            std::thread::sleep(PAST_STACK_TRANSITION);

            let (status, body) = emulator.cloudformation(
                region,
                "DescribeStackResources",
                &[("StackName", "cfn-provisioning-Suite")],
            );
            assert_eq!(
                status, 200,
                "DescribeStackResources must succeed, got {status} with body {body}"
            );
            let resources = resources_of(&body);

            // A bucket CloudFormation named: the physical id is a legal bucket name built
            // from the stack and logical id, whose own case the fold removes.
            let generated = physical_id(&resources, "ScriptBucket");
            assert!(
                is_legal_bucket_name(&generated),
                "a generated bucket name must satisfy S3's naming rules, got {generated:?}"
            );
            assert!(
                generated.starts_with("cfn-provisioning-suite-scriptbucket-"),
                "a generated bucket name is built from the stack and logical id, got \
                 {generated:?}"
            );

            // The bucket, the named bucket and an object written into each: the ids the
            // stack reports are ids a client can use.
            let named = physical_id(&resources, "NamedBucket");
            assert_eq!(
                named, "cfn-named-logs-bucket",
                "a bucket the template names keeps that name, got {named:?}"
            );
            for bucket in [generated.as_str(), named.as_str()] {
                let (status, headers, body) =
                    emulator.s3(region, "HEAD", &format!("/{bucket}"), b"");
                assert_eq!(
                    status,
                    200,
                    "case {bucket:?}: HeadBucket on the stack's bucket must answer 200, got \
                     {status} with body {:?}",
                    String::from_utf8_lossy(&body)
                );
                assert_eq!(
                    headers
                        .get("x-amz-bucket-region")
                        .and_then(|value| value.to_str().ok()),
                    Some(region),
                    "case {bucket:?}: the bucket must live in the stack's Region"
                );

                let (status, _, body) = emulator.s3(
                    region,
                    "PUT",
                    &format!("/{bucket}/scripts/etl.py"),
                    b"print('etl')",
                );
                assert_eq!(
                    status,
                    200,
                    "case {bucket:?}: PutObject into the stack's bucket must answer 200, got \
                     {status} with body {:?}",
                    String::from_utf8_lossy(&body)
                );
                let (status, _, body) = emulator.s3(
                    region,
                    "GET",
                    &format!("/{bucket}/scripts/etl.py"),
                    b"",
                );
                assert_eq!(
                    (status, body.as_slice()),
                    (200, b"print('etl')".as_slice()),
                    "case {bucket:?}: the object must read back, got {status} with body {:?}",
                    String::from_utf8_lossy(&body)
                );
            }

            // The cluster: the physical id is the job-flow id EMR assigned, and EMR holds
            // the cluster the template described under it.
            let cluster_id = physical_id(&resources, "Cluster");
            assert!(
                is_job_flow_id(&cluster_id),
                "an EMR cluster's physical id is the job-flow id EMR assigned, got \
                 {cluster_id:?}"
            );
            let (status, described) = emulator.emr(
                region,
                "DescribeCluster",
                json!({ "ClusterId": cluster_id }),
            );
            assert_eq!(
                status, 200,
                "DescribeCluster on the stack's cluster must answer 200, got {status} with \
                 {described}"
            );
            let cluster = &described["Cluster"];
            assert_eq!(
                cluster["Name"].as_str(),
                Some("cfn-emr-glue-catalog"),
                "the cluster must carry the name the template declared, got {cluster}"
            );
            assert_eq!(
                cluster["ReleaseLabel"].as_str(),
                Some("emr-6.15.0"),
                "the cluster must carry the release label the template declared, got {cluster}"
            );
            // A property that refers to another resource of the same stack was evaluated
            // before the cluster was created, so the log URI names the real bucket.
            assert_eq!(
                cluster["LogUri"].as_str(),
                Some("s3://cfn-named-logs-bucket/logs/"),
                "the cluster's log URI must name the stack's own bucket, got {cluster}"
            );
            let classifications: Vec<&str> = cluster["Configurations"]
                .as_array()
                .map(|configurations| {
                    configurations
                        .iter()
                        .filter_map(|configuration| configuration["Classification"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                classifications,
                vec!["hive-site", "spark-hive-site"],
                "the cluster must report the Configurations the template declared, got {cluster}"
            );
            assert_eq!(
                cluster["Configurations"][0]["Properties"]
                    ["hive.metastore.client.factory.class"]
                    .as_str(),
                Some("com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory"),
                "a declared configuration's properties must reach EMR, got {cluster}"
            );
            let applications: Vec<&str> = cluster["Applications"]
                .as_array()
                .map(|applications| {
                    applications
                        .iter()
                        .filter_map(|application| application["Name"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                applications,
                vec!["Spark", "Hive"],
                "the cluster must report the Applications the template declared, got {cluster}"
            );
            assert_eq!(
                cluster["Ec2InstanceAttributes"]["Ec2SubnetId"].as_str(),
                None,
                "the template declared no subnet, so none is reported, got {cluster}"
            );
            assert_eq!(
                cluster["ServiceRole"].as_str(),
                Some("EMR_DefaultRole"),
                "the cluster must carry the service role the template declared, got {cluster}"
            );
            assert_eq!(
                cluster["Tags"][0]["Key"].as_str(),
                Some("suite"),
                "the cluster must carry the tags the template declared, got {cluster}"
            );

            // A resource type this layer provisions nowhere keeps the identifier
            // CloudFormation's documented return values give it.
            let topic = physical_id(&resources, "Notifications");
            assert!(
                topic.ends_with(":cfn-provisioning-topic") && topic.starts_with("arn:aws:sns:"),
                "an unprovisioned type keeps its derived physical id, got {topic:?}"
            );

            // The stack's outputs were evaluated over the SAME identifiers, so `Ref` of the
            // cluster is the job-flow id and not the name CloudFormation generated.
            let (status, body) = emulator.cloudformation(
                region,
                "DescribeStacks",
                &[("StackName", "cfn-provisioning-Suite")],
            );
            assert_eq!(
                status, 200,
                "DescribeStacks must succeed, got {status} with body {body}"
            );
            let outputs = outputs_of(&body);
            let output = |key: &str| {
                outputs
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| panic!("the stack must report {key}, got {outputs:?}"))
            };
            assert_eq!(
                output("ClusterId"),
                cluster_id,
                "Ref of the cluster must be the id EMR assigned, got {outputs:?}"
            );
            assert_eq!(
                output("ScriptBucketName"),
                generated,
                "Ref of the bucket must be the bucket's name, got {outputs:?}"
            );
            assert_eq!(
                output("ScriptBucketArn"),
                format!("arn:aws:s3:::{generated}"),
                "the bucket's Arn attribute must name the bucket that exists, got {outputs:?}"
            );

            // A REFUSED create provisions nothing: the duplicate-name refusal is decided
            // before the template is deployed, so neither the bucket the refused template
            // names nor a cluster is created. (A client looping on a taken name would
            // otherwise spawn resources no stack references.)
            let clusters_before = emulator
                .emr(region, "ListClusters", json!({}))
                .1["Clusters"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            let (status, body) = emulator.cloudformation(
                region,
                "CreateStack",
                &[
                    ("StackName", "cfn-provisioning-Suite"),
                    ("TemplateBody", REFUSED_TEMPLATE),
                ],
            );
            assert_eq!(
                status, 400,
                "a second CreateStack for a live stack's name must be refused, got {status} \
                 with body {body}"
            );
            assert!(
                body.contains("AlreadyExistsException"),
                "the refusal must be AlreadyExistsException, got {body}"
            );
            let (status, _, _) = emulator.s3(region, "HEAD", "/cfn-refused-bucket", b"");
            // HeadBucket's miss carries the framework's client-error default rather than
            // S3's 404 (`NotFound` is the only error it declares and the model puts no
            // `@httpError` on it), so what is pinned here is that the bucket is not there.
            assert_ne!(
                status, 200,
                "a refused create must not have created the bucket its template names"
            );
            let clusters_after = emulator
                .emr(region, "ListClusters", json!({}))
                .1["Clusters"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                clusters_after, clusters_before,
                "a refused create must not have run a job flow"
            );

            // ---- A second stack, in another Region: its own resources, its own ids. ----
            let other_region = "us-west-2";
            let (status, body) = emulator.cloudformation(
                other_region,
                "CreateStack",
                &[
                    ("StackName", "cfn-provisioning-second"),
                    ("TemplateBody", MINIMAL_TEMPLATE),
                ],
            );
            assert_eq!(
                status, 200,
                "the second CreateStack must succeed, got {status} with body {body}"
            );
            std::thread::sleep(PAST_STACK_TRANSITION);
            let (status, body) = emulator.cloudformation(
                other_region,
                "DescribeStackResources",
                &[("StackName", "cfn-provisioning-second")],
            );
            assert_eq!(
                status, 200,
                "the second DescribeStackResources must succeed, got {status} with body {body}"
            );
            let second = resources_of(&body);

            let second_bucket = physical_id(&second, "DataBucket");
            assert!(
                is_legal_bucket_name(&second_bucket),
                "the second stack's generated bucket name must be legal, got {second_bucket:?}"
            );
            assert_ne!(
                second_bucket, generated,
                "two stacks must not generate the same bucket name"
            );
            let (status, headers, payload) =
                emulator.s3(other_region, "HEAD", &format!("/{second_bucket}"), b"");
            assert_eq!(
                status,
                200,
                "the second stack's bucket must exist, got {status} with body {:?}",
                String::from_utf8_lossy(&payload)
            );
            assert_eq!(
                headers
                    .get("x-amz-bucket-region")
                    .and_then(|value| value.to_str().ok()),
                Some(other_region),
                "a bucket of a stack in another Region must live in that Region"
            );
            // The first stack's bucket is untouched by the second deployment.
            let (status, _, _) = emulator.s3(region, "HEAD", &format!("/{generated}"), b"");
            assert_eq!(
                status, 200,
                "the first stack's bucket must still exist, got {status}"
            );

            let second_cluster = physical_id(&second, "Cluster");
            assert!(
                is_job_flow_id(&second_cluster),
                "the second cluster's physical id must be a job-flow id, got {second_cluster:?}"
            );
            assert_ne!(
                second_cluster, cluster_id,
                "two stacks must not report the same cluster id"
            );
            let (status, described) = emulator.emr(
                other_region,
                "DescribeCluster",
                json!({ "ClusterId": second_cluster }),
            );
            assert_eq!(
                status, 200,
                "DescribeCluster on the second cluster must answer 200, got {status} with \
                 {described}"
            );
            // The template named no cluster, so EMR holds it under the name CloudFormation
            // generated for the resource.
            assert!(
                described["Cluster"]["Name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("cfn-provisioning-second-Cluster-")),
                "an unnamed cluster takes the name CloudFormation generated, got {described}"
            );

            // ---- UpdateStack deploys through the same seam. ----
            // The rule is about DEPLOYING a template, not about one operation: an update
            // that adds a declared resource must create it in the service that owns it,
            // and must leave a resource it re-declares unchanged with the identifier that
            // resource already has.
            let clusters_before_update = emulator
                .emr(other_region, "ListClusters", json!({}))
                .1["Clusters"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            let (status, body) = emulator.cloudformation(
                other_region,
                "UpdateStack",
                &[
                    ("StackName", "cfn-provisioning-second"),
                    ("TemplateBody", UPDATED_SECOND_TEMPLATE),
                ],
            );
            assert_eq!(
                status, 200,
                "UpdateStack must succeed, got {status} with body {body}"
            );
            std::thread::sleep(PAST_STACK_TRANSITION);
            let (status, body) = emulator.cloudformation(
                other_region,
                "DescribeStackResources",
                &[("StackName", "cfn-provisioning-second")],
            );
            assert_eq!(
                status, 200,
                "DescribeStackResources after the update must succeed, got {status} with \
                 body {body}"
            );
            let updated = resources_of(&body);

            // The resources the update re-declared unchanged keep the identifiers they
            // already have: no second bucket, and no second job flow.
            assert_eq!(
                physical_id(&updated, "DataBucket"),
                second_bucket,
                "case \"a resource an update re-declares unchanged\": it keeps its physical \
                 id, got {updated:?}"
            );
            assert_eq!(
                physical_id(&updated, "Cluster"),
                second_cluster,
                "case \"a cluster an update re-declares unchanged\": it keeps the job-flow \
                 id EMR assigned, got {updated:?}"
            );

            // The bucket the update ADDED exists in S3 under the name the stack reports.
            let added_bucket = physical_id(&updated, "AddedBucket");
            assert_eq!(
                added_bucket, "cfn-update-added-bucket",
                "case \"a bucket an update adds\": it takes the name the template gives it, \
                 got {added_bucket:?}"
            );
            let (status, _, payload) =
                emulator.s3(other_region, "HEAD", &format!("/{added_bucket}"), b"");
            assert_eq!(
                status,
                200,
                "case {added_bucket:?}: HeadBucket on a bucket an update added must answer \
                 200, got {status} with body {:?}",
                String::from_utf8_lossy(&payload)
            );
            let (status, _, payload) = emulator.s3(
                other_region,
                "PUT",
                &format!("/{added_bucket}/scripts/updated.py"),
                b"print('updated')",
            );
            assert_eq!(
                status,
                200,
                "case {added_bucket:?}: PutObject into a bucket an update added must answer \
                 200, got {status} with body {:?}",
                String::from_utf8_lossy(&payload)
            );

            // The cluster the update ADDED exists in EMR under the job-flow id the stack
            // reports, and it is a cluster of its own.
            let added_cluster = physical_id(&updated, "AddedCluster");
            assert!(
                is_job_flow_id(&added_cluster),
                "case \"a cluster an update adds\": its physical id is the job-flow id EMR \
                 assigned, got {added_cluster:?}"
            );
            assert_ne!(
                added_cluster, second_cluster,
                "a cluster an update adds is not the one the create made"
            );
            let (status, described) = emulator.emr(
                other_region,
                "DescribeCluster",
                json!({ "ClusterId": added_cluster }),
            );
            assert_eq!(
                status, 200,
                "DescribeCluster on a cluster an update added must answer 200, got {status} \
                 with {described}"
            );
            assert_eq!(
                described["Cluster"]["Name"].as_str(),
                Some("cfn-update-added-cluster"),
                "it must carry the name the updated template declared, got {described}"
            );
            let clusters_after_update = emulator
                .emr(other_region, "ListClusters", json!({}))
                .1["Clusters"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                clusters_after_update,
                clusters_before_update + 1,
                "an update must run exactly one job flow for the one cluster it adds"
            );

            // ---- ExecuteChangeSet deploys through the same seam. ----
            let (status, body) = emulator.cloudformation(
                region,
                "CreateChangeSet",
                &[
                    ("StackName", "cfn-provisioning-change-set"),
                    ("ChangeSetName", "cfn-provisioning-plan"),
                    ("ChangeSetType", "CREATE"),
                    ("TemplateBody", CHANGE_SET_TEMPLATE),
                ],
            );
            assert_eq!(
                status, 200,
                "CreateChangeSet must succeed, got {status} with body {body}"
            );
            // A change set's own creation settles on the same clock as a stack operation,
            // and only a settled change set is executable.
            std::thread::sleep(PAST_STACK_TRANSITION);
            let (status, body) = emulator.cloudformation(
                region,
                "ExecuteChangeSet",
                &[
                    ("StackName", "cfn-provisioning-change-set"),
                    ("ChangeSetName", "cfn-provisioning-plan"),
                ],
            );
            assert_eq!(
                status, 200,
                "ExecuteChangeSet must succeed, got {status} with body {body}"
            );
            std::thread::sleep(PAST_STACK_TRANSITION);
            let (status, body) = emulator.cloudformation(
                region,
                "DescribeStackResources",
                &[("StackName", "cfn-provisioning-change-set")],
            );
            assert_eq!(
                status, 200,
                "DescribeStackResources on the executed stack must succeed, got {status} \
                 with body {body}"
            );
            let executed = resources_of(&body);

            let staged = physical_id(&executed, "StagingBucket");
            assert!(
                is_legal_bucket_name(&staged),
                "case \"a bucket a change set declares\": its generated name must satisfy \
                 S3's naming rules, got {staged:?}"
            );
            let (status, _, payload) = emulator.s3(region, "HEAD", &format!("/{staged}"), b"");
            assert_eq!(
                status,
                200,
                "case {staged:?}: HeadBucket on a bucket a change set deployed must answer \
                 200, got {status} with body {:?}",
                String::from_utf8_lossy(&payload)
            );
            let staged_cluster = physical_id(&executed, "Cluster");
            assert!(
                is_job_flow_id(&staged_cluster),
                "case \"a cluster a change set declares\": its physical id is the job-flow \
                 id EMR assigned, got {staged_cluster:?}"
            );
            let (status, described) = emulator.emr(
                region,
                "DescribeCluster",
                json!({ "ClusterId": staged_cluster }),
            );
            assert_eq!(
                status, 200,
                "DescribeCluster on a cluster a change set deployed must answer 200, got \
                 {status} with {described}"
            );
            assert_eq!(
                described["Cluster"]["Name"].as_str(),
                Some("cfn-change-set-cluster"),
                "it must carry the name the change set's template declared, got {described}"
            );

            // ---- A cluster declared with instance FLEETS, the other capacity form. ----
            let (status, body) = emulator.cloudformation(
                region,
                "CreateStack",
                &[
                    ("StackName", "cfn-provisioning-fleets"),
                    ("TemplateBody", FLEET_CLUSTER_TEMPLATE),
                ],
            );
            assert_eq!(
                status, 200,
                "CreateStack for a fleet cluster must succeed, got {status} with body {body}"
            );
            std::thread::sleep(PAST_STACK_TRANSITION);
            let (status, body) = emulator.cloudformation(
                region,
                "DescribeStacks",
                &[("StackName", "cfn-provisioning-fleets")],
            );
            assert_eq!(
                status, 200,
                "DescribeStacks must succeed, got {status} with body {body}"
            );
            assert_eq!(
                elements(&body, "StackStatus").first().map(String::as_str),
                Some("CREATE_COMPLETE"),
                "case \"a cluster declared with instance fleets\": the stack must complete, \
                 got {body}"
            );
            let (status, body) = emulator.cloudformation(
                region,
                "DescribeStackResources",
                &[("StackName", "cfn-provisioning-fleets")],
            );
            assert_eq!(
                status, 200,
                "DescribeStackResources must succeed, got {status} with body {body}"
            );
            let fleet_cluster = physical_id(&resources_of(&body), "FleetCluster");
            assert!(
                is_job_flow_id(&fleet_cluster),
                "case \"a cluster declared with instance fleets\": its physical id is the \
                 job-flow id EMR assigned, got {fleet_cluster:?}"
            );
            let (status, described) = emulator.emr(
                region,
                "DescribeCluster",
                json!({ "ClusterId": fleet_cluster }),
            );
            assert_eq!(
                status, 200,
                "DescribeCluster on the fleet cluster must answer 200, got {status} with \
                 {described}"
            );
            // The capacity the template declared really reached EMR: a cluster run with
            // `InstanceFleets` reports an INSTANCE_FLEET collection, where one run with no
            // capacity at all would report the group form.
            assert_eq!(
                described["Cluster"]["InstanceCollectionType"].as_str(),
                Some("INSTANCE_FLEET"),
                "case \"a cluster declared with instance fleets\": the declared fleets must \
                 reach EMR as its own InstanceFleets, got {described}"
            );
        })
        .expect("spawn the provisioning thread")
        .join()
        .expect("every declared resource must exist in the service that owns it");
}
