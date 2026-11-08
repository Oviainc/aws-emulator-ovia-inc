// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares the Amazon S3 Vectors resource types really creates
//! them in S3 Vectors, and the handles the stack hands back are the ARNs that service
//! answers to.
//!
//! The rule under test: **a resource type AWS publishes for a service this emulator ships
//! is one a template may declare, and a stack that declares it creates the resource in the
//! service that OWNS it, through that service's own public API, under the identifier that
//! type's published return values define — so `Ref` and `Fn::GetAtt` resolve to a value
//! `s3vectors GetVectorBucket`/`GetIndex` resolves to the resource the template declared,
//! and a name that service REFUSES fails the stack rather than being reported as
//! created.** Two crates have to agree for that to hold (CloudFormation makes the call, S3
//! Vectors serves it), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: which of the two types the
//! template declares, whether it declares an index in a bucket of the SAME template,
//! whether the bucket is NAMED or left for CloudFormation to generate a name, whether
//! `Tags` and an `EncryptionConfiguration` are declared, the Region deployed in, both
//! template spellings, and which intrinsic the outputs read the resource with (`Ref`,
//! `Fn::GetAtt`, `Fn::Sub`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none,
//! which is CloudFormation's to compute: what is checked about it is that the ARN the stack
//! publishes is that name's vector-bucket ARN and that S3 Vectors resolves it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the template it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request either
/// way (the credential scope is what routes it).
pub(crate) fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
pub(crate) fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
pub(crate) async fn cloudformation(
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

/// Call one Amazon S3 Vectors operation over its restJson1 wire, signed for `region`: a
/// POST to the operation's own URI, which is how that model binds every one of them.
pub(crate) async fn s3vectors(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/{operation}"))
        .header("host", format!("s3vectors.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("s3vectors", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
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

/// A `DescribeStackResources` answer's resources, as the logical-id to physical-id map. The
/// two lists are in document order, one entry per reported resource, so they pair up.
fn physical_ids_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "LogicalResourceId")
        .into_iter()
        .zip(elements(xml, "PhysicalResourceId"))
        .collect()
}

/// A vector bucket's ARN in `region`: the same total function of the Region and the name
/// both services compute it from.
fn bucket_arn(region: &str, name: &str) -> String {
    format!("arn:aws:s3vectors:{region}:{ACCOUNT}:bucket/{name}")
}

/// A vector index's ARN, which nests inside its bucket's.
pub(crate) fn index_arn(region: &str, bucket: &str, index: &str) -> String {
    format!("{}/index/{index}", bucket_arn(region, bucket))
}

/// Whether `name` is inside the name form the S3 Vectors model publishes for a vector
/// bucket and an index (`[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]`: 3 to 63 characters of lowercase
/// letters, digits, hyphens and dots, beginning and ending with a letter or a digit).
fn is_s3vectors_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (3..=63).contains(&bytes.len())
        && alnum(bytes[0])
        && alnum(bytes[bytes.len() - 1])
        && bytes.iter().all(|b| alnum(*b) || *b == b'-' || *b == b'.')
}

/// The index one row declares beside its vector bucket.
struct DeclaredIndex {
    /// The index's logical id in the template.
    logical_id: &'static str,
    /// The name the template gives it.
    name: &'static str,
    /// The members `GetIndex` must then report for it, each a property the template
    /// declared under the same name with an upper-case initial.
    reported: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the index's ARN.
    arn_outputs: &'static [&'static str],
}

/// One row of the table: a template, and what the rule says the resources it declares must
/// then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the resources are created in and
    /// the one their ARNs carry.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The vector bucket's logical id in the template.
    bucket_logical_id: &'static str,
    /// The bucket name the template declares, or `None` for a template that names nothing —
    /// whose bucket carries the name CloudFormation generated.
    declared_bucket: Option<&'static str>,
    /// The output keys whose value must be the bucket's ARN, each written with a different
    /// intrinsic over the same resource.
    arn_outputs: &'static [&'static str],
    /// The index the row declares, if any.
    index: Option<DeclaredIndex>,
}

const ROWS: &[Row] = &[
    Row {
        case: "a named, tagged vector bucket in JSON, read with Ref and the published ARN",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an S3 Vectors vector bucket",
                "Resources": {
                    "Vectors": {
                        "Type": "AWS::S3Vectors::VectorBucket",
                        "Properties": {
                            "VectorBucketName": "sim2real-probe-vec",
                            "Tags": [
                                { "Key": "Owner", "Value": "search" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "BucketRef": { "Value": { "Ref": "Vectors" } },
                    "BucketArn": {
                        "Value": { "Fn::GetAtt": ["Vectors", "VectorBucketArn"] }
                    }
                }
            })
            .to_string()
        },
        bucket_logical_id: "Vectors",
        declared_bucket: Some("sim2real-probe-vec"),
        arn_outputs: &["BucketRef", "BucketArn"],
        index: None,
    },
    Row {
        case: "a named vector bucket in YAML with an encryption configuration, no tags",
        region: "us-west-2",
        template: || {
            [
                "Resources:",
                "  Vectors:",
                "    Type: AWS::S3Vectors::VectorBucket",
                "    Properties:",
                "      VectorBucketName: yaml.vector.bucket",
                "      EncryptionConfiguration:",
                "        SseType: AES256",
                "Outputs:",
                "  BucketRef:",
                "    Value: !Ref Vectors",
                "",
            ]
            .join("\n")
        },
        bucket_logical_id: "Vectors",
        declared_bucket: Some("yaml.vector.bucket"),
        arn_outputs: &["BucketRef"],
        index: None,
    },
    Row {
        case: "an index in a vector bucket of the same template, both read with Ref",
        region: "eu-west-1",
        template: || {
            json!({
                "Resources": {
                    "Vectors": {
                        "Type": "AWS::S3Vectors::VectorBucket",
                        "Properties": { "VectorBucketName": "embeddings-store" }
                    },
                    "Embeddings": {
                        "Type": "AWS::S3Vectors::Index",
                        "Properties": {
                            "VectorBucketArn": { "Ref": "Vectors" },
                            "IndexName": "titan-v2",
                            "DataType": "float32",
                            "Dimension": 1024,
                            "DistanceMetric": "cosine",
                            "MetadataConfiguration": {
                                "NonFilterableMetadataKeys": ["chunk"]
                            }
                        }
                    }
                },
                "Outputs": {
                    "BucketRef": { "Value": { "Ref": "Vectors" } },
                    "IndexRef": { "Value": { "Ref": "Embeddings" } },
                    "IndexSub": {
                        "Value": { "Fn::Sub": ["${Arn}", { "Arn": { "Ref": "Embeddings" } }] }
                    }
                }
            })
            .to_string()
        },
        bucket_logical_id: "Vectors",
        declared_bucket: Some("embeddings-store"),
        arn_outputs: &["BucketRef"],
        index: Some(DeclaredIndex {
            logical_id: "Embeddings",
            name: "titan-v2",
            reported: &[
                ("dataType", "float32"),
                ("distanceMetric", "cosine"),
                ("indexName", "titan-v2"),
                ("vectorBucketName", "embeddings-store"),
            ],
            arn_outputs: &["IndexRef", "IndexSub"],
        }),
    },
    Row {
        case: "a vector bucket the template does not name, so CloudFormation generates one",
        region: "ap-northeast-1",
        template: || {
            [
                "Resources:",
                "  Vectors:",
                "    Type: AWS::S3Vectors::VectorBucket",
                "Outputs:",
                "  BucketRef:",
                "    Value: !Ref Vectors",
                "",
            ]
            .join("\n")
        },
        bucket_logical_id: "Vectors",
        declared_bucket: None,
        arn_outputs: &["BucketRef"],
        index: None,
    },
];

/// Every row: the ARN the stack reports for its S3 Vectors resources is each resource's
/// own, S3 Vectors itself resolves those ARNs to the resources the template declared, and
/// deleting the stack deletes them there.
#[tokio::test]
async fn a_stack_provisions_the_s3vectors_resources_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("s3vectors-stack-{index}");
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
        tokio::time::sleep(PAST_STACK_SPAN).await;
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
        let reported = physical_ids_of(&resources);
        let physical_id = reported.get(row.bucket_logical_id).unwrap_or_else(|| {
            panic!(
                "case {case:?}: no physical id for {:?} in {resources}",
                row.bucket_logical_id
            )
        });

        // "Ref returns the vector bucket ARN": the physical id is that ARN, and the name
        // inside it is the one the template asked for — or, for a template that named
        // nothing, the one CloudFormation generated.
        let bucket = match row.declared_bucket {
            Some(declared) => declared.to_string(),
            None => physical_id
                .rsplit_once("bucket/")
                .map(|(_, name)| name.to_string())
                .unwrap_or_default(),
        };
        let arn = bucket_arn(region, &bucket);
        assert_eq!(
            physical_id, &arn,
            "case {case:?}: expected the physical id to be the vector bucket ARN {arn}, got \
             {physical_id:?}"
        );
        assert!(
            is_s3vectors_name(&bucket),
            "case {case:?}: expected a bucket name inside the published name form, got \
             {bucket:?}"
        );

        let outputs = outputs_of(&described);
        for key in row.arn_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&arn),
                "case {case:?}: expected output {key:?} to be the vector bucket ARN {arn:?}, \
                 got {:?}",
                outputs.get(*key)
            );
        }

        // And S3 Vectors itself resolves that ARN to the bucket the template declared.
        let (status, answered) = s3vectors(
            &router,
            region,
            "GetVectorBucket",
            &json!({ "vectorBucketArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetVectorBucket on the ARN the stack \
             reported, got {status} with body {answered}"
        );
        let reported_bucket = answered.get("vectorBucket").cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported_bucket.get("vectorBucketArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the read to report {arn}, got {answered}"
        );
        assert_eq!(
            reported_bucket
                .get("vectorBucketName")
                .and_then(Value::as_str),
            Some(bucket.as_str()),
            "case {case:?}: expected the read to report the bucket name {bucket:?}, got \
             {answered}"
        );

        // The index the row declares, where it declares one: its own ARN nests inside the
        // bucket's, and S3 Vectors resolves it to the index the template configured.
        let declared_index_arn = match &row.index {
            None => None,
            Some(declared) => {
                let index_arn = index_arn(region, &bucket, declared.name);
                let index_physical = reported.get(declared.logical_id).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: no physical id for {:?} in {resources}",
                        declared.logical_id
                    )
                });
                assert_eq!(
                    index_physical, &index_arn,
                    "case {case:?}: expected the index's physical id to be {index_arn}, got \
                     {index_physical:?}"
                );
                for key in declared.arn_outputs {
                    assert_eq!(
                        outputs.get(*key),
                        Some(&index_arn),
                        "case {case:?}: expected output {key:?} to be the index ARN \
                         {index_arn:?}, got {:?}",
                        outputs.get(*key)
                    );
                }
                let (status, answered) = s3vectors(
                    &router,
                    region,
                    "GetIndex",
                    &json!({ "indexArn": index_arn }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 from GetIndex on the ARN the stack \
                     reported, got {status} with body {answered}"
                );
                let reported_index = answered.get("index").cloned().unwrap_or(Value::Null);
                assert_eq!(
                    reported_index.get("indexArn").and_then(Value::as_str),
                    Some(index_arn.as_str()),
                    "case {case:?}: expected the read to report {index_arn}, got {answered}"
                );
                for (member, expected) in declared.reported {
                    assert_eq!(
                        reported_index.get(*member).and_then(Value::as_str),
                        Some(*expected),
                        "case {case:?}: expected the index's {member} to be {expected:?}, got \
                         {answered}"
                    );
                }
                assert_eq!(
                    reported_index.get("dimension").and_then(Value::as_i64),
                    Some(1024),
                    "case {case:?}: expected the declared dimension, got {answered}"
                );
                Some(index_arn)
            }
        };

        // Deleting the stack deletes what S3 Vectors holds for it — the index before the
        // bucket that holds it, which is the order the resources were created in reversed.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        if let Some(index_arn) = declared_index_arn {
            let (status, gone) = s3vectors(
                &router,
                region,
                "GetIndex",
                &json!({ "indexArn": index_arn }),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 from GetIndex once the stack deleted the \
                 index, got {status} with body {gone}"
            );
        }
        let (status, gone) = s3vectors(
            &router,
            region,
            "GetVectorBucket",
            &json!({ "vectorBucketArn": arn }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from GetVectorBucket once the stack deleted the \
             bucket, got {status} with body {gone}"
        );
    }
}

/// A stack whose template declares a vector bucket S3 Vectors REFUSES does not report a
/// resource nothing created: the stack fails with the refusal the owning service gave, and
/// no bucket exists under the name the template asked for.
///
/// The rows are the two ways a declared name leaves the form S3 Vectors publishes for one
/// (`[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]`): upper case, and shorter than the three-character
/// minimum.
#[tokio::test]
async fn a_vector_bucket_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let region = "us-east-2";

    for (index, (case, declared)) in [
        ("an upper-case vector bucket name", "Sim2Real-Probe-Vec"),
        ("a vector bucket name under the published minimum", "ab"),
    ]
    .into_iter()
    .enumerate()
    {
        let stack_name = format!("s3vectors-refused-{index}");
        let template = json!({
            "Resources": {
                "Vectors": {
                    "Type": "AWS::S3Vectors::VectorBucket",
                    "Properties": { "VectorBucketName": declared }
                }
            }
        })
        .to_string();
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack — the template declares a type \
             CloudFormation publishes — got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        let reached = opt_element(&described, "StackStatus").unwrap_or_default();
        assert!(
            reached.starts_with("CREATE_FAILED") || reached.starts_with("ROLLBACK"),
            "case {case:?}: expected the stack not to report a created resource, got \
             {reached:?} in {described}"
        );
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("Vectors"),
            "case {case:?}: expected the failure to name the resource, got {reason:?}"
        );

        // Nothing was created under the name the template asked for.
        let (status, gone) = s3vectors(
            &router,
            region,
            "GetVectorBucket",
            &json!({ "vectorBucketName": declared }),
        )
        .await;
        assert_ne!(
            status, 200,
            "case {case:?}: expected no vector bucket named {declared:?}, the read answered \
             {status} with body {gone}"
        );
    }
}
