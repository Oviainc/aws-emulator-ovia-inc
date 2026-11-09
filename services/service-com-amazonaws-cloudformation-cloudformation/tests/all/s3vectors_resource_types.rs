// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 Vectors resource types a template may declare, and the identifier each
//! one reports.
//!
//! The rule under test: **a resource type AWS publishes for a service this emulator ships
//! is one a template may declare — CloudFormation accepts the body and answers a
//! `StackId` — and the identifier such a resource reports is the one that type's published
//! return values define, which for both S3 Vectors types is the resource's own ARN** (a
//! vector bucket's `bucket/<name>`, an index's `bucket/<bucket>/index/<name>`). The
//! catalogue is `state::resource_types`; the identifiers are `state::template`'s
//! return-value catalogue, exercised here through `template::provision`, which is the one
//! function a deployment computes them with.
//!
//! The rows span what the rule generalises over rather than one reported body: both S3
//! Vectors types, both template spellings, a name the template states and one it leaves
//! CloudFormation to generate, a name built with an intrinsic, and all the ways an index's
//! declaration can name its parent bucket (a literal name, a literal bucket ARN, a `Ref` to
//! a bucket resource of the same template, and not at all).
//!
//! The resources themselves are CREATED in Amazon S3 Vectors, which a crate-local suite
//! cannot mount, so nothing here reads them back through that service — that round trip is
//! `core/router/tests/all/cloudformation_provisions_s3vectors_resources.rs`. What this
//! suite owns is the half that is CloudFormation's: which types a body may declare, and
//! what identifier each resource then reports.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::stack_lifecycle::{
    assert_stack_id_shape, create_stack, describe_stack, router, xml_text, ACCOUNT_ID, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::{
    is_published, is_recognized,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    self, published_attribute_names, Budget, Scope, StackScope, IMPLEMENTED_RESOURCE_TYPES,
    S3VECTORS_INDEX, S3VECTORS_VECTOR_BUCKET,
};

/// The stack the deployment rows are computed for.
const STACK_NAME: &str = "s3vectors-stack";
const STACK_ID: &str = "arn:aws:cloudformation:us-east-1:000000000000:stack/s3vectors-stack/0001";

/// How many characters CloudFormation's own generated name segment carries
/// (`<stack name>-<logical id>-<unique id>`), so a generated bucket name is recognisable
/// as one without pinning the digest.
const GENERATED_NAME_CHARACTERS: usize = 12;

/// The ARN of an Amazon S3 Vectors resource in the Region these rows deploy in.
fn s3vectors_arn(resource: &str) -> String {
    format!("arn:aws:s3vectors:{REGION}:{ACCOUNT_ID}:{resource}")
}

/// A vector bucket's ARN, as `VectorBucketArn`'s pattern spells it.
fn bucket_arn(name: &str) -> String {
    s3vectors_arn(&format!("bucket/{name}"))
}

/// A vector index's ARN, which nests inside its bucket's.
fn index_arn(bucket: &str, index: &str) -> String {
    s3vectors_arn(&format!("bucket/{bucket}/index/{index}"))
}

/// Whether `name` is inside the name form the S3 Vectors model's `VectorBucketName` and
/// `IndexName` members publish: 3 to 63 characters of lowercase letters, digits, hyphens
/// and dots, beginning and ending with a letter or a digit
/// (`[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]`, the pattern on `VectorBucketArn`/`IndexArn`).
fn is_s3vectors_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (3..=63).contains(&bytes.len())
        && alnum(bytes[0])
        && alnum(bytes[bytes.len() - 1])
        && bytes.iter().all(|b| alnum(*b) || *b == b'-' || *b == b'.')
}

/// The resources a template declares, as a deployment of it computes them: keyed by
/// logical id, carrying the physical resource id `Ref` resolves to and the attributes
/// `Fn::GetAtt` reads.
fn provisioned(template_body: &str, parameters: &[(&str, &str)]) -> BTreeMap<String, Resource> {
    let parsed = template_body
        .parse::<Body>()
        .expect("every row's body is a template this emulator parses");
    let parameter_values: BTreeMap<String, String> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    let parameter_types: BTreeMap<String, String> = parsed.0.parameter_types();
    let resources = BTreeMap::new();
    let mappings = Value::Null;
    let conditions = Value::Null;
    let imports = BTreeMap::new();
    let scope = Scope {
        stack: StackScope {
            region: REGION,
            stack_name: STACK_NAME,
            stack_id: STACK_ID,
            notification_arns: &[],
        },
        parameters: &parameter_values,
        parameter_types: &parameter_types,
        resources: &resources,
        mappings: &mappings,
        conditions: &conditions,
        imports: &imports,
    };
    let mut budget = Budget::new();
    template::provision(&parsed.0, &scope, &mut budget)
        .into_iter()
        .map(|resource| {
            (
                resource.logical_id.clone(),
                Resource {
                    physical_id: resource.physical_resource_id,
                    attributes: resource.attributes,
                },
            )
        })
        .collect()
}

/// One deployed resource, reduced to what this suite asserts about it.
struct Resource {
    physical_id: String,
    attributes: BTreeMap<String, String>,
}

/// A parsed template body, so a row can carry its body as text whatever it is spelled in.
struct Body(template::Template);

impl std::str::FromStr for Body {
    type Err = ();

    fn from_str(body: &str) -> Result<Self, Self::Err> {
        template::parse(body).map(Body).ok_or(())
    }
}

/// What one row expects of one of its resources.
struct Expected {
    logical_id: &'static str,
    /// The physical resource id, where the row can state it exactly.
    physical_id: Option<String>,
    /// The attribute the type publishes, and its value.
    published: Option<(&'static str, String)>,
}

/// One row: a template, and what the rule says each resource it declares must report.
struct Row {
    case: &'static str,
    template: String,
    parameters: &'static [(&'static str, &'static str)],
    expected: Vec<Expected>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // The body the reported refusal cited, in the Region these rows deploy in.
            case: "a vector bucket the template names",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Vectors": {
                        "Type": "AWS::S3Vectors::VectorBucket",
                        "Properties": { "VectorBucketName": "sim2real-probe-vec" }
                    }
                }
            })
            .to_string(),
            parameters: &[],
            expected: vec![Expected {
                logical_id: "Vectors",
                physical_id: Some(bucket_arn("sim2real-probe-vec")),
                published: Some(("VectorBucketArn", bucket_arn("sim2real-probe-vec"))),
            }],
        },
        Row {
            case: "a vector bucket declared in YAML, with an encryption configuration",
            template: [
                "Resources:",
                "  Vectors:",
                "    Type: AWS::S3Vectors::VectorBucket",
                "    Properties:",
                "      VectorBucketName: yaml.vector.bucket",
                "      EncryptionConfiguration:",
                "        SseType: AES256",
                "",
            ]
            .join("\n"),
            parameters: &[],
            expected: vec![Expected {
                logical_id: "Vectors",
                physical_id: Some(bucket_arn("yaml.vector.bucket")),
                published: Some(("VectorBucketArn", bucket_arn("yaml.vector.bucket"))),
            }],
        },
        Row {
            case: "a vector bucket whose name a template parameter supplies",
            template: json!({
                "Parameters": { "BucketName": { "Type": "String" } },
                "Resources": {
                    "Vectors": {
                        "Type": "AWS::S3Vectors::VectorBucket",
                        "Properties": { "VectorBucketName": { "Ref": "BucketName" } }
                    }
                }
            })
            .to_string(),
            parameters: &[("BucketName", "parameterised-vectors")],
            expected: vec![Expected {
                logical_id: "Vectors",
                physical_id: Some(bucket_arn("parameterised-vectors")),
                published: Some(("VectorBucketArn", bucket_arn("parameterised-vectors"))),
            }],
        },
        Row {
            // CloudFormation decides the name, so the row states only its FORM; the
            // assertion below holds the ARN to that name.
            case: "a vector bucket the template does not name",
            template: json!({
                "Resources": { "Vectors": { "Type": "AWS::S3Vectors::VectorBucket" } }
            })
            .to_string(),
            parameters: &[],
            expected: vec![Expected {
                logical_id: "Vectors",
                physical_id: None,
                published: None,
            }],
        },
        Row {
            case: "an index whose declaration names its bucket by name",
            template: json!({
                "Resources": {
                    "Embeddings": {
                        "Type": "AWS::S3Vectors::Index",
                        "Properties": {
                            "VectorBucketName": "named-bucket",
                            "IndexName": "embeddings",
                            "DataType": "float32",
                            "Dimension": 1024,
                            "DistanceMetric": "cosine"
                        }
                    }
                }
            })
            .to_string(),
            parameters: &[],
            expected: vec![Expected {
                logical_id: "Embeddings",
                physical_id: Some(index_arn("named-bucket", "embeddings")),
                published: Some(("IndexArn", index_arn("named-bucket", "embeddings"))),
            }],
        },
        Row {
            case: "an index whose declaration names its bucket by ARN",
            template: json!({
                "Resources": {
                    "Embeddings": {
                        "Type": "AWS::S3Vectors::Index",
                        "Properties": {
                            "VectorBucketArn": bucket_arn("arn-named-bucket"),
                            "IndexName": "by-arn",
                            "DataType": "float32",
                            "Dimension": 8,
                            "DistanceMetric": "euclidean"
                        }
                    }
                }
            })
            .to_string(),
            parameters: &[],
            expected: vec![Expected {
                logical_id: "Embeddings",
                physical_id: Some(index_arn("arn-named-bucket", "by-arn")),
                published: Some(("IndexArn", index_arn("arn-named-bucket", "by-arn"))),
            }],
        },
        Row {
            // `Ref` of a vector bucket is its ARN, so this is how one template declares
            // an index in a bucket it also declares — and the index's identity can only be
            // computed once that bucket's is known.
            case: "an index in a vector bucket of the same template",
            template: json!({
                "Resources": {
                    "Vectors": {
                        "Type": "AWS::S3Vectors::VectorBucket",
                        "Properties": { "VectorBucketName": "same-template-bucket" }
                    },
                    "Embeddings": {
                        "Type": "AWS::S3Vectors::Index",
                        "Properties": {
                            "VectorBucketArn": { "Ref": "Vectors" },
                            "IndexName": "nested",
                            "DataType": "float32",
                            "Dimension": 1536,
                            "DistanceMetric": "cosine"
                        }
                    }
                }
            })
            .to_string(),
            parameters: &[],
            expected: vec![
                Expected {
                    logical_id: "Vectors",
                    physical_id: Some(bucket_arn("same-template-bucket")),
                    published: Some(("VectorBucketArn", bucket_arn("same-template-bucket"))),
                },
                Expected {
                    logical_id: "Embeddings",
                    physical_id: Some(index_arn("same-template-bucket", "nested")),
                    published: Some(("IndexArn", index_arn("same-template-bucket", "nested"))),
                },
            ],
        },
        Row {
            // No bucket is named, so there is no ARN to nest the index inside: the
            // resource keeps the name CloudFormation gave it rather than an ARN built on a
            // bucket nothing named.
            case: "an index whose declaration names no bucket at all",
            template: json!({
                "Resources": {
                    "Embeddings": {
                        "Type": "AWS::S3Vectors::Index",
                        "Properties": { "IndexName": "orphan-index" }
                    }
                }
            })
            .to_string(),
            parameters: &[],
            expected: vec![Expected {
                logical_id: "Embeddings",
                physical_id: Some("orphan-index".to_string()),
                published: None,
            }],
        },
    ]
}

/// Every row: the identifier the deployment gives each declared S3 Vectors resource is the
/// one its type's published return values define.
#[test]
fn an_s3vectors_resource_reports_the_identifier_its_type_publishes() {
    for row in rows() {
        let case = row.case;
        let deployed = provisioned(&row.template, row.parameters);
        for expected in &row.expected {
            let logical_id = expected.logical_id;
            let resource = deployed.get(logical_id).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the deployment to carry {logical_id:?}, got {:?}",
                    deployed.keys().collect::<Vec<_>>()
                )
            });
            if let Some(physical_id) = &expected.physical_id {
                assert_eq!(
                    &resource.physical_id, physical_id,
                    "case {case:?}: expected {logical_id:?} to report the physical id \
                     {physical_id:?}, got {:?}",
                    resource.physical_id
                );
            }
            if let Some((attribute, value)) = &expected.published {
                let actual = resource.attributes.get(*attribute);
                assert_eq!(
                    actual,
                    Some(value),
                    "case {case:?}: expected {logical_id:?} to publish {attribute:?} = \
                     {value:?}, got {actual:?}"
                );
            }
        }
    }
}

/// The one row whose name CloudFormation decides: the physical id is the ARN of a
/// GENERATED bucket name, and that name is one Amazon S3 Vectors would accept — a name
/// outside its published form could not be created there at all.
#[test]
fn a_generated_vector_bucket_name_is_one_s3_vectors_accepts() {
    let case = "a vector bucket the template does not name";
    let template =
        json!({ "Resources": { "Vectors": { "Type": "AWS::S3Vectors::VectorBucket" } } })
            .to_string();
    let deployed = provisioned(&template, &[]);
    let resource = deployed
        .get("Vectors")
        .unwrap_or_else(|| panic!("case {case:?}: no Vectors resource in {:?}", deployed.keys()));
    let name = resource
        .physical_id
        .rsplit_once("bucket/")
        .map(|(_, name)| name.to_string())
        .unwrap_or_default();
    assert_eq!(
        resource.physical_id,
        bucket_arn(&name),
        "case {case:?}: expected the physical id to be a vector bucket ARN, got {:?}",
        resource.physical_id
    );
    assert!(
        is_s3vectors_name(&name),
        "case {case:?}: expected a generated name inside the published name form, got {name:?}"
    );
    assert!(
        name.starts_with(STACK_NAME) && name.len() > STACK_NAME.len() + GENERATED_NAME_CHARACTERS,
        "case {case:?}: expected a <stack>-<logical id>-<unique id> name, got {name:?}"
    );
}

/// `CreateStack` ACCEPTS every one of those bodies — it answers a `StackId`, and the stack
/// it created is one a later caller reads back — rather than refusing the template with
/// `ValidationError` before anything is provisioned.
#[tokio::test]
async fn create_stack_accepts_a_template_declaring_the_s3vectors_types() {
    let router = router();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let stack_name = format!("s3vectors-accepted-{index}");
        let parameters: Vec<(String, String)> = row
            .parameters
            .iter()
            .enumerate()
            .flat_map(|(position, (key, value))| {
                let entry = position + 1;
                [
                    (
                        format!("Parameters.member.{entry}.ParameterKey"),
                        (*key).to_string(),
                    ),
                    (
                        format!("Parameters.member.{entry}.ParameterValue"),
                        (*value).to_string(),
                    ),
                ]
            })
            .collect();
        let (status, body) = create_stack(&router, &stack_name, &row.template, &parameters).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack for a published resource \
             type, got {status} with body {body}"
        );
        let stack_id = xml_text(&body, "StackId").unwrap_or_default();
        assert_stack_id_shape(case, &stack_id, &stack_name);
        let (described, described_body) = describe_stack(&router, &stack_name).await;
        assert_eq!(
            described, 200,
            "case {case:?}: expected DescribeStacks to report the created stack, got \
             {described} with body {described_body}"
        );
    }
}

/// The catalogue side of the rule: both S3 Vectors types are types a template may declare
/// and types this emulator carries the published return values of, and each publishes
/// exactly the one attribute its "Return values" section names.
#[test]
fn both_s3vectors_types_are_catalogued_with_their_published_attribute() {
    for (resource_type, attribute) in [
        (S3VECTORS_VECTOR_BUCKET, "VectorBucketArn"),
        (S3VECTORS_INDEX, "IndexArn"),
    ] {
        assert!(
            is_published(resource_type) && is_recognized(resource_type),
            "case {resource_type:?}: expected a published type to be recognized, it is not"
        );
        assert!(
            IMPLEMENTED_RESOURCE_TYPES.contains(&resource_type),
            "case {resource_type:?}: expected the implemented set to carry the type, it does \
             not"
        );
        let published = published_attribute_names(resource_type);
        assert_eq!(
            published,
            [attribute],
            "case {resource_type:?}: expected the published attributes to be [{attribute:?}], \
             got {published:?}"
        );
    }
    // ...and a name neither the catalogue nor the reference carries still is not
    // recognized, however close it is to one that is.
    for misspelled in [
        "AWS::S3Vectors::VectorBuckets",
        "AWS::S3Vectors::Indexes",
        "AWS::S3Vector::VectorBucket",
    ] {
        assert!(
            !is_recognized(misspelled),
            "case {misspelled:?}: expected an uncatalogued name to be unrecognized, it is \
             recognized"
        );
    }
}
