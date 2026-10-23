// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack provisions the resources its template declares, in the services
//! that own them — and deleting the stack deletes them again.
//!
//! The rule under test: **creating a stack calls the owning service's own create API once
//! per resource the template's `Resources` section declares, and the stack's `Outputs`
//! are resolved from those resources' physical ids and attributes.** Two crates have to
//! agree for that to hold (CloudFormation makes the call; Firehose serves it), so the
//! suite lives here rather than in either crate: a crate-local test can only ever see one
//! side of the call.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! stream or leaves the name to CloudFormation, which destination configuration it
//! declares, whether it carries tags and encryption, and which intrinsic functions its
//! outputs are written with (`Ref`, `Fn::GetAtt`, `Fn::Sub`, `Fn::Join`, a `Ref` to a
//! parameter and to a pseudo parameter). Every expected value is computed here from what
//! the row declares, never read back out of the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and so the region the stack's resources
/// and their ARNs belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for both clock-owed spans these services choose to come due: the stack's
/// (`STATUS_TRANSITION_DELAY`, 500 ms) and the stream's (`TRANSITION_DELAY`, 600 ms).
const PAST_BOTH_SPANS: Duration = Duration::from_millis(1_200);

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
///
/// Firehose is the service whose streams the suite deploys, and Amazon S3 and IAM are
/// here because the templates below declare an `AWS::S3::Bucket` and an `AWS::IAM::Role`
/// beside each stream — resource types CloudFormation provisions too, so the services
/// that own them have to be reachable for the stack to complete.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` value the way an AWS SDK does,
/// so a template body full of `{`, `"` and `&` survives the body.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Firehose operation over its awsJson1_1 wire.
async fn firehose(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("firehose.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Firehose_20150804.{operation}"))
        .header("authorization", authorization("firehose"))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Every `<tag>` text of an XML body, in document order, with the serializer's escaping
/// undone.
fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(
            after[..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        );
        rest = &after[end + close.len()..];
    }
    found
}

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// Create a stack from `template` and answer its id, failing the case with the response
/// body when the create is refused.
async fn create_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut members: Vec<(&str, &str)> =
        vec![("StackName", stack_name), ("TemplateBody", template)];
    // `Parameters.member.N.ParameterKey` / `.ParameterValue`, as an SDK flattens the list.
    let flattened: Vec<(String, String)> = parameters
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                (
                    format!("Parameters.member.{position}.ParameterKey"),
                    (*key).to_string(),
                ),
                (
                    format!("Parameters.member.{position}.ParameterValue"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect();
    for (key, value) in &flattened {
        members.push((key.as_str(), value.as_str()));
    }
    let (status, body) = cloudformation(router, "CreateStack", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    xml_text(&body, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {body}"))
}

/// The one stack `stack_name` names, as the response body.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// One row of the table: a template, and what the rule says the stack must then report
/// and must have created.
struct Row {
    case: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML), built from the
    /// stack name the row runs under.
    template: fn(&str) -> String,
    /// The parameters the create supplies.
    parameters: &'static [(&'static str, &'static str)],
    /// The outputs the stack must report, computed from the template.
    expected_outputs: fn(&str) -> BTreeMap<String, String>,
    /// The output key whose value is the stream's name.
    stream_name_output: &'static str,
    /// What the stream's destination description must report:
    /// `(the description member, the member inside it, the value)`.
    expected_destination: fn(&str) -> Vec<(&'static str, &'static str, String)>,
    /// Output keys the stack must NOT report, because nothing resolves their value.
    absent_outputs: &'static [&'static str],
    /// The `KeyType` the stream's encryption configuration must report, for a template
    /// that asked for server-side encryption.
    ///
    /// The stream's `Tags` are not read back here: Firehose's own
    /// `ListTagsForDeliveryStream` is still a 501 stub in this emulator, so a template's
    /// tags are passed to the create and have no reader yet.
    expected_encryption_key_type: Option<&'static str>,
}

/// The ARN a Firehose stream of `name` carries in this emulator.
fn stream_arn(name: &str) -> String {
    format!("arn:aws:firehose:{REGION}:{ACCOUNT_ID}:deliverystream/{name}")
}

/// The ARN the `Fn::GetAtt` of an `AWS::S3::Bucket` reports: an S3 bucket ARN carries
/// neither region nor account.
fn bucket_arn(name: &str) -> String {
    format!("arn:aws:s3:::{name}")
}

/// The ARN the `Fn::GetAtt` of an `AWS::IAM::Role` reports: IAM is global, so the ARN
/// carries the account and no region.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/{name}")
}

/// A trust policy for the delivery role the templates declare.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "firehose.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

const ROWS: &[Row] = &[
    Row {
        case: "a named stream with an ExtendedS3 destination, outputs by Ref and Fn::GetAtt",
        template: |stack| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a named Firehose stream delivering to a bucket",
                "Parameters": { "Prefix": { "Type": "String", "Default": "unused/" } },
                "Resources": {
                    "DeliveryRole": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "RoleName": format!("{stack}-role"),
                            "AssumeRolePolicyDocument": trust_policy()
                        }
                    },
                    "DestinationBucket": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": { "BucketName": format!("{stack}-data") }
                    },
                    "Stream": {
                        "Type": "AWS::KinesisFirehose::DeliveryStream",
                        "Properties": {
                            "DeliveryStreamName": format!("{stack}-stream"),
                            "DeliveryStreamType": "DirectPut",
                            "ExtendedS3DestinationConfiguration": {
                                "BucketARN": { "Fn::GetAtt": ["DestinationBucket", "Arn"] },
                                "RoleARN": { "Fn::GetAtt": ["DeliveryRole", "Arn"] },
                                "Prefix": { "Ref": "Prefix" },
                                "CompressionFormat": "GZIP"
                            }
                        }
                    }
                },
                "Outputs": {
                    "StreamName": { "Value": { "Ref": "Stream" } },
                    "StreamArn": { "Value": { "Fn::GetAtt": ["Stream", "Arn"] } },
                    "BucketName": { "Value": { "Ref": "DestinationBucket" } },
                    "RoleArn": { "Value": { "Fn::GetAtt": "DeliveryRole.Arn" } }
                }
            })
            .to_string()
        },
        parameters: &[("Prefix", "raw/events/")],
        expected_outputs: |stack| {
            [
                ("StreamName".to_string(), format!("{stack}-stream")),
                ("StreamArn".to_string(), stream_arn(&format!("{stack}-stream"))),
                ("BucketName".to_string(), format!("{stack}-data")),
                ("RoleArn".to_string(), role_arn(&format!("{stack}-role"))),
            ]
            .into_iter()
            .collect()
        },
        stream_name_output: "StreamName",
        expected_destination: |stack| {
            vec![
                (
                    "ExtendedS3DestinationDescription",
                    "BucketARN",
                    bucket_arn(&format!("{stack}-data")),
                ),
                (
                    "ExtendedS3DestinationDescription",
                    "RoleARN",
                    role_arn(&format!("{stack}-role")),
                ),
                (
                    "ExtendedS3DestinationDescription",
                    "Prefix",
                    "raw/events/".to_string(),
                ),
                (
                    "ExtendedS3DestinationDescription",
                    "CompressionFormat",
                    "GZIP".to_string(),
                ),
            ]
        },
        absent_outputs: &[],
        expected_encryption_key_type: None,
    },
    Row {
        case: "an unnamed stream with an S3 destination, outputs by Fn::Sub and Fn::Join",
        template: |_stack| {
            json!({
                "Resources": {
                    "LogRole": {
                        "Type": "AWS::IAM::Role",
                        "Properties": { "AssumeRolePolicyDocument": trust_policy() }
                    },
                    "Landing": { "Type": "AWS::S3::Bucket" },
                    "Stream": {
                        "Type": "AWS::KinesisFirehose::DeliveryStream",
                        "Properties": {
                            "S3DestinationConfiguration": {
                                "BucketARN": { "Fn::GetAtt": ["Landing", "Arn"] },
                                "RoleARN": { "Fn::GetAtt": ["LogRole", "Arn"] },
                                "Prefix": { "Fn::Sub": "${AWS::Region}/${AWS::StackName}/" }
                            }
                        }
                    }
                },
                "Outputs": {
                    "StreamName": { "Value": { "Ref": "Stream" } },
                    "BucketName": { "Value": { "Ref": "Landing" } },
                    "Endpoint": {
                        "Value": {
                            "Fn::Join": [
                                "",
                                ["https://firehose.", { "Ref": "AWS::Region" }, ".amazonaws.com"]
                            ]
                        }
                    },
                    "Scope": { "Value": { "Fn::Sub": "${AWS::AccountId}:${AWS::Region}:${AWS::StackName}" } },
                    // An output whose value no intrinsic here can resolve is ABSENT, not
                    // invented: `Fn::ImportValue` reads another stack's exports, which
                    // this emulator does not keep.
                    "Imported": { "Value": { "Fn::ImportValue": "some-other-stacks-export" } }
                }
            })
            .to_string()
        },
        parameters: &[],
        expected_outputs: |stack| {
            [
                ("BucketName".to_string(), String::new()),
                ("StreamName".to_string(), String::new()),
                (
                    "Endpoint".to_string(),
                    format!("https://firehose.{REGION}.amazonaws.com"),
                ),
                (
                    "Scope".to_string(),
                    format!("{ACCOUNT_ID}:{REGION}:{stack}"),
                ),
            ]
            .into_iter()
            .collect()
        },
        stream_name_output: "StreamName",
        expected_destination: |stack| {
            vec![(
                "S3DestinationDescription",
                "Prefix",
                format!("{REGION}/{stack}/"),
            )]
        },
        absent_outputs: &["Imported"],
        expected_encryption_key_type: None,
    },
    Row {
        case: "a stream with tags and server-side encryption, an output exporting its name",
        template: |stack| {
            json!({
                "Resources": {
                    "Stream": {
                        "Type": "AWS::KinesisFirehose::DeliveryStream",
                        "Properties": {
                            "DeliveryStreamName": format!("{stack}-encrypted"),
                            "DeliveryStreamEncryptionConfigurationInput": { "KeyType": "AWS_OWNED_CMK" },
                            "Tags": [
                                { "Key": "Environment", "Value": "test" },
                                { "Key": "Owner", "Value": { "Ref": "AWS::StackName" } }
                            ],
                            "ExtendedS3DestinationConfiguration": {
                                "BucketARN": { "Fn::Sub": "arn:${AWS::Partition}:s3:::${AWS::StackName}-sink" },
                                "RoleARN": { "Fn::Sub": "arn:${AWS::Partition}:iam::${AWS::AccountId}:role/${AWS::StackName}-role" },
                                "ErrorOutputPrefix": "errors/"
                            }
                        }
                    }
                },
                "Outputs": {
                    "StreamName": {
                        "Value": { "Ref": "Stream" },
                        "Description": "the stream this stack created",
                        "Export": { "Name": { "Fn::Sub": "${AWS::StackName}-stream" } }
                    },
                    "BucketName": { "Value": { "Fn::Sub": "${AWS::StackName}-sink" } }
                }
            })
            .to_string()
        },
        parameters: &[],
        expected_outputs: |stack| {
            [
                ("StreamName".to_string(), format!("{stack}-encrypted")),
                ("BucketName".to_string(), format!("{stack}-sink")),
            ]
            .into_iter()
            .collect()
        },
        stream_name_output: "StreamName",
        expected_destination: |stack| {
            vec![
                (
                    "ExtendedS3DestinationDescription",
                    "BucketARN",
                    bucket_arn(&format!("{stack}-sink")),
                ),
                (
                    "ExtendedS3DestinationDescription",
                    "ErrorOutputPrefix",
                    "errors/".to_string(),
                ),
            ]
        },
        absent_outputs: &[],
        expected_encryption_key_type: Some("AWS_OWNED_CMK"),
    },
    Row {
        case: "the same thing declared in YAML, with the short-form intrinsic tags",
        // CloudFormation accepts a template in JSON or in YAML; a YAML template writes
        // its intrinsics in the short form (`!Ref`, `!GetAtt`, `!Sub`), so this row is
        // the same declaration as the first one in the other language.
        template: |stack| {
            format!(
                "\
AWSTemplateFormatVersion: '2010-09-09'
Description: a Firehose stream declared in YAML
Resources:
  DeliveryRole:
    Type: AWS::IAM::Role
    Properties:
      RoleName: {stack}-role
      AssumeRolePolicyDocument:
        Version: '2012-10-17'
        Statement:
          - Effect: Allow
            Principal:
              Service: firehose.amazonaws.com
            Action: sts:AssumeRole
  DestinationBucket:
    Type: AWS::S3::Bucket
    Properties:
      BucketName: {stack}-data
  Stream:
    Type: AWS::KinesisFirehose::DeliveryStream
    Properties:
      DeliveryStreamName: {stack}-stream
      ExtendedS3DestinationConfiguration:
        BucketARN: !GetAtt DestinationBucket.Arn
        RoleARN: !GetAtt [DeliveryRole, Arn]
        Prefix: !Sub '${{AWS::StackName}}/raw/'
        CompressionFormat: Snappy
Outputs:
  StreamName:
    Value: !Ref Stream
  StreamArn:
    Value: !GetAtt Stream.Arn
  BucketName:
    Value: !Ref DestinationBucket
"
            )
        },
        parameters: &[],
        expected_outputs: |stack| {
            [
                ("StreamName".to_string(), format!("{stack}-stream")),
                (
                    "StreamArn".to_string(),
                    stream_arn(&format!("{stack}-stream")),
                ),
                ("BucketName".to_string(), format!("{stack}-data")),
            ]
            .into_iter()
            .collect()
        },
        stream_name_output: "StreamName",
        expected_destination: |stack| {
            vec![
                (
                    "ExtendedS3DestinationDescription",
                    "BucketARN",
                    bucket_arn(&format!("{stack}-data")),
                ),
                (
                    "ExtendedS3DestinationDescription",
                    "RoleARN",
                    role_arn(&format!("{stack}-role")),
                ),
                (
                    "ExtendedS3DestinationDescription",
                    "Prefix",
                    format!("{stack}/raw/"),
                ),
                (
                    "ExtendedS3DestinationDescription",
                    "CompressionFormat",
                    "Snappy".to_string(),
                ),
            ]
        },
        absent_outputs: &[],
        expected_encryption_key_type: None,
    },
];

/// Every row: the stack reports the outputs its template declares, the stream the template
/// declares exists in Firehose with the configuration the template gave it, it takes a
/// record once it is active, and deleting the stack deletes it.
#[tokio::test]
async fn a_stack_provisions_the_firehose_stream_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("provisioning-{index}");
        let template = (row.template)(&stack_name);
        let stack_id = create_stack(&router, case, &stack_name, &template, row.parameters).await;
        assert!(
            stack_id.contains(&format!("stack/{stack_name}/")),
            "case {case:?}: expected the stack id to name the stack, got {stack_id:?}"
        );

        // The stack's resources are created by the call itself; the stack reports the
        // outputs of the template it deployed once that create COMPLETES, so the case
        // waits past both clock-owed spans before reading them.
        tokio::time::sleep(PAST_BOTH_SPANS).await;
        let body = describe_stack(&router, case, &stack_name).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );
        let outputs = outputs_of(&body);
        let stream_name = outputs
            .get(row.stream_name_output)
            .unwrap_or_else(|| {
                panic!("case {case:?}: no {} output in {body}", row.stream_name_output)
            })
            .clone();

        // Every declared output is reported, with the value its intrinsics resolve to. An
        // output whose value is a name CloudFormation generated is checked for the SHAPE
        // of a generated name instead (the row leaves its expected value empty).
        for (key, expected) in (row.expected_outputs)(&stack_name) {
            let actual = outputs
                .get(&key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {outputs:?}"));
            if expected.is_empty() {
                let prefix = format!("{stack_name}-");
                assert!(
                    actual.starts_with(&prefix.to_lowercase()) || actual.starts_with(&prefix),
                    "case {case:?}: expected the generated {key} to start with \
                     {prefix:?}, got {actual:?}"
                );
                continue;
            }
            assert_eq!(
                actual, &expected,
                "case {case:?}: expected output {key} to be {expected:?}, got {actual:?}"
            );
        }
        for absent in row.absent_outputs {
            assert!(
                outputs.get(*absent).is_none_or(String::is_empty),
                "case {case:?}: expected no value for the {absent} output (nothing \
                 resolves it), got {outputs:?}"
            );
        }

        // The stream the template declared exists in Firehose, under the ARN its name
        // gives it.
        let (status, described) = firehose(
            &router,
            "DescribeDeliveryStream",
            &json!({ "DeliveryStreamName": stream_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeDeliveryStream for \
             {stream_name:?}, got {status} with body {described}"
        );
        let description = &described["DeliveryStreamDescription"];
        assert_eq!(
            description["DeliveryStreamARN"].as_str(),
            Some(stream_arn(&stream_name).as_str()),
            "case {case:?}: expected the stream ARN to name {stream_name:?}, got {description}"
        );
        // Past its own span the stream is active.
        assert_eq!(
            description["DeliveryStreamStatus"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: expected the stream to reach ACTIVE, got {described}"
        );

        // The encryption a template asked for is in force on the stream.
        if let Some(key_type) = row.expected_encryption_key_type {
            let encryption = &description["DeliveryStreamEncryptionConfiguration"];
            assert_eq!(
                encryption["KeyType"].as_str(),
                Some(key_type),
                "case {case:?}: expected the stream's encryption KeyType to be \
                 {key_type:?}, got {encryption}"
            );
            assert_eq!(
                encryption["Status"].as_str(),
                Some("ENABLED"),
                "case {case:?}: expected the stream's encryption to reach ENABLED, got \
                 {encryption}"
            );
        }

        let destination = &description["Destinations"][0];
        // The destination carries what the template configured, under the description
        // matching the configuration the template declared.
        for (member, inner, expected) in (row.expected_destination)(&stack_name) {
            let actual = destination[member][inner].as_str();
            assert_eq!(
                actual,
                Some(expected.as_str()),
                "case {case:?}: expected {member}.{inner} to be {expected:?}, got \
                 {destination}"
            );
        }

        // An active stream takes records.
        let (status, put) = firehose(
            &router,
            "PutRecord",
            &json!({
                "DeliveryStreamName": stream_name,
                "Record": { "Data": "aGVsbG8gZmlyZWhvc2U=" }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from PutRecord against {stream_name:?}, got \
             {status} with body {put}"
        );
        assert!(
            put["RecordId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected PutRecord to answer a RecordId, got {put}"
        );

        // Deleting the stack deletes the stream it created: Firehose's own delete is
        // asynchronous ("the deletion request is sent... the Firehose stream is in the
        // DELETING state"), so the stream is DELETING at once and gone once that span is
        // due.
        let (status, body) =
            cloudformation(&router, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, deleting) = firehose(
            &router,
            "DescribeDeliveryStream",
            &json!({ "DeliveryStreamName": stream_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stream to be answered while deleting, got {status} \
             with body {deleting}"
        );
        assert_eq!(
            deleting["DeliveryStreamDescription"]["DeliveryStreamStatus"].as_str(),
            Some("DELETING"),
            "case {case:?}: expected the stack's deletion to start the stream's, got \
             {deleting}"
        );

        // And the stack itself completes its deletion rather than failing it, with the
        // stream gone by then.
        tokio::time::sleep(PAST_BOTH_SPANS).await;
        let (status, gone) = firehose(
            &router,
            "DescribeDeliveryStream",
            &json!({ "DeliveryStreamName": stream_name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the stream to be gone after the stack's deletion, got \
             {status} with body {gone}"
        );
        assert!(
            gone["__type"]
                .as_str()
                .is_some_and(|code| code.contains("ResourceNotFoundException")),
            "case {case:?}: expected ResourceNotFoundException for the deleted stream, got \
             {gone}"
        );
        let body = describe_stack(&router, case, &stack_id).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("DELETE_COMPLETE"),
            "case {case:?}: expected the stack's deletion to complete, got {body}"
        );
    }
}

/// A stack whose template declares a stream Firehose refuses does not report a resource
/// nothing created: the stack fails, with the refusal the owning service gave.
///
/// The refusal here is Firehose's own `ResourceInUseException` ("This name must be unique
/// per Amazon Web Services account in the same Amazon Web Services Region"), reached by
/// pointing a template at a stream a client already created.
#[tokio::test]
async fn a_resource_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a template naming a stream that already exists";
    let stream_name = "already-taken-stream";

    let (status, created) = firehose(
        &router,
        "CreateDeliveryStream",
        &json!({
            "DeliveryStreamName": stream_name,
            "ExtendedS3DestinationConfiguration": {
                "BucketARN": bucket_arn("some-bucket"),
                "RoleARN": role_arn("some-role")
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's own CreateDeliveryStream to succeed, got \
         {status} with body {created}"
    );

    let template = json!({
        "Resources": {
            "Stream": {
                "Type": "AWS::KinesisFirehose::DeliveryStream",
                "Properties": {
                    "DeliveryStreamName": stream_name,
                    "ExtendedS3DestinationConfiguration": {
                        "BucketARN": bucket_arn("some-bucket"),
                        "RoleARN": role_arn("some-role")
                    }
                }
            }
        },
        "Outputs": { "StreamName": { "Value": { "Ref": "Stream" } } }
    });
    let stack_name = "doomed-stack";
    create_stack(&router, case, stack_name, &template.to_string(), &[]).await;

    let body = describe_stack(&router, case, stack_name).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_FAILED"),
        "case {case:?}: expected the stack to fail, got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Stream") && reason.contains("ResourceInUseException"),
        "case {case:?}: expected the reason to name the resource and the service's \
         refusal, got {reason:?}"
    );

    // The stream the client created is untouched: the stack created nothing, so its
    // deletion deletes nothing.
    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    let (status, described) = firehose(
        &router,
        "DescribeDeliveryStream",
        &json!({ "DeliveryStreamName": stream_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's own stream to survive the stack's deletion, \
         got {status} with body {described}"
    );
}

/// One stack's resources are its own: a second stack's lifecycle neither created nor
/// deletes them.
///
/// This is the isolation the resource records have to carry — the stack's deletion deletes
/// exactly what THAT stack created, judged on its own stored records and not on the name
/// space the owning service keeps. The rows are two stacks from the same template, each
/// naming its stream after itself, and one of them is deleted.
#[tokio::test]
async fn one_stacks_deletion_leaves_another_stacks_resources_untouched() {
    let router = build_router();
    let case = "two stacks from one template, one deleted";
    let template = |stack: &str| {
        json!({
            "Resources": {
                "Stream": {
                    "Type": "AWS::KinesisFirehose::DeliveryStream",
                    "Properties": {
                        "DeliveryStreamName": format!("{stack}-stream"),
                        "ExtendedS3DestinationConfiguration": {
                            "BucketARN": bucket_arn("shared-bucket"),
                            "RoleARN": role_arn("shared-role")
                        }
                    }
                }
            },
            "Outputs": { "StreamName": { "Value": { "Ref": "Stream" } } }
        })
        .to_string()
    };

    for stack_name in ["neighbour-a", "neighbour-b"] {
        create_stack(&router, case, stack_name, &template(stack_name), &[]).await;
        let (status, described) = firehose(
            &router,
            "DescribeDeliveryStream",
            &json!({ "DeliveryStreamName": format!("{stack_name}-stream") }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {stack_name}'s own stream to exist, got {status} with \
             body {described}"
        );
    }

    // Both streams are active, as a client deleting a deployed stack would find them.
    tokio::time::sleep(PAST_BOTH_SPANS).await;

    // Deleting the first stack deletes the first stack's stream, and only that one.
    let (status, body) =
        cloudformation(&router, "DeleteStack", &[("StackName", "neighbour-a")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_BOTH_SPANS).await;

    let (status, gone) = firehose(
        &router,
        "DescribeDeliveryStream",
        &json!({ "DeliveryStreamName": "neighbour-a-stream" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the deleted stack's own stream to be gone, got {status} \
         with body {gone}"
    );
    let (status, survivor) = firehose(
        &router,
        "DescribeDeliveryStream",
        &json!({ "DeliveryStreamName": "neighbour-b-stream" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other stack's stream to survive, got {status} with \
         body {survivor}"
    );
    assert_eq!(
        survivor["DeliveryStreamDescription"]["DeliveryStreamStatus"].as_str(),
        Some("ACTIVE"),
        "case {case:?}: expected the surviving stream to be untouched, got {survivor}"
    );
}

/// An UPDATE deploys its template the same way a create does: the stream the new template
/// declares is created in Firehose, the one it stops declaring is deleted there, and the
/// stream it declares unchanged is left exactly as it was — so the stack's later deletion
/// still deletes what the stack really has.
#[tokio::test]
async fn an_update_creates_the_streams_it_declares_and_deletes_the_ones_it_drops() {
    let router = build_router();
    let case = "an update that replaces one stream and keeps another";
    let stack_name = "updated-stack";
    // Both templates declare `Kept` identically; only the second declares `Replaced`'s
    // stream under a new name, which is what the update has to create and clean up after.
    let template = |replaced: &str| {
        json!({
            "Resources": {
                "Kept": {
                    "Type": "AWS::KinesisFirehose::DeliveryStream",
                    "Properties": {
                        "DeliveryStreamName": "updated-stack-kept",
                        "ExtendedS3DestinationConfiguration": {
                            "BucketARN": bucket_arn("update-bucket"),
                            "RoleARN": role_arn("update-role")
                        }
                    }
                },
                "Replaced": {
                    "Type": "AWS::KinesisFirehose::DeliveryStream",
                    "Properties": {
                        "DeliveryStreamName": replaced,
                        "ExtendedS3DestinationConfiguration": {
                            "BucketARN": bucket_arn("update-bucket"),
                            "RoleARN": role_arn("update-role")
                        }
                    }
                }
            }
        })
        .to_string()
    };

    create_stack(&router, case, stack_name, &template("updated-stack-before"), &[]).await;
    tokio::time::sleep(PAST_BOTH_SPANS).await;

    let (status, body) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &template("updated-stack-after")),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_BOTH_SPANS).await;

    let body = describe_stack(&router, case, stack_name).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {body}"
    );

    // Each row is what the update owed the stream: created, deleted, or left alone.
    let rows = [
        ("the stream the update declares anew", "updated-stack-after", 200),
        ("the stream the update stopped declaring", "updated-stack-before", 400),
        ("the stream the update declares unchanged", "updated-stack-kept", 200),
    ];
    for (row, stream_name, expected) in rows {
        let (status, described) = firehose(
            &router,
            "DescribeDeliveryStream",
            &json!({ "DeliveryStreamName": stream_name }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}/{row}: expected HTTP {expected} for {stream_name:?}, got {status} \
             with body {described}"
        );
    }

    // And the stack's deletion deletes what the stack really has, including the stream the
    // UPDATE created — the record of it was carried through the re-deploy.
    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_BOTH_SPANS).await;
    for stream_name in ["updated-stack-after", "updated-stack-kept"] {
        let (status, gone) = firehose(
            &router,
            "DescribeDeliveryStream",
            &json!({ "DeliveryStreamName": stream_name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {stream_name:?} to be gone with the stack, got {status} \
             with body {gone}"
        );
    }
}

/// An update that asks for a DIFFERENT configuration of a resource that already exists is
/// not reported as applied: this emulator does not change a resource in place, so the
/// stack fails, names the resource, and the resource keeps the configuration it has.
#[tokio::test]
async fn an_update_that_would_change_an_existing_resource_in_place_fails_the_stack() {
    let router = build_router();
    let case = "an update that changes a kept stream's compression";
    let stack_name = "in-place-stack";
    let stream_name = "in-place-stack-stream";
    let template = |compression: &str| {
        json!({
            "Resources": {
                "Stream": {
                    "Type": "AWS::KinesisFirehose::DeliveryStream",
                    "Properties": {
                        "DeliveryStreamName": stream_name,
                        "ExtendedS3DestinationConfiguration": {
                            "BucketARN": bucket_arn("in-place-bucket"),
                            "RoleARN": role_arn("in-place-role"),
                            "CompressionFormat": compression
                        }
                    }
                }
            }
        })
        .to_string()
    };

    create_stack(&router, case, stack_name, &template("GZIP"), &[]).await;
    tokio::time::sleep(PAST_BOTH_SPANS).await;

    let (status, body) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &template("Snappy")),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_BOTH_SPANS).await;

    let body = describe_stack(&router, case, stack_name).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_FAILED"),
        "case {case:?}: expected the update to fail rather than drop the caller's change, \
         got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Stream") && reason.contains("NotImplemented"),
        "case {case:?}: expected the reason to name the resource and the change it will not \
         make, got {reason:?}"
    );

    // The stream still has the configuration it was created with, and the stack's own
    // deletion still deletes it.
    let (status, described) = firehose(
        &router,
        "DescribeDeliveryStream",
        &json!({ "DeliveryStreamName": stream_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the stream to survive the failed update, got {status} with \
         body {described}"
    );
    let destination = &described["DeliveryStreamDescription"]["Destinations"][0];
    assert_eq!(
        destination["ExtendedS3DestinationDescription"]["CompressionFormat"].as_str(),
        Some("GZIP"),
        "case {case:?}: expected the stream to keep the compression it was created with, \
         got {destination}"
    );

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_BOTH_SPANS).await;
    let (status, gone) = firehose(
        &router,
        "DescribeDeliveryStream",
        &json!({ "DeliveryStreamName": stream_name }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the stack's deletion to delete the stream it created, got \
         {status} with body {gone}"
    );
}

/// A resource the owning service cannot delete YET is reported as it is: the stack's
/// deletion fails with that service's own refusal, the resource survives, and a retried
/// deletion — once the service will take it — completes.
///
/// Firehose refuses `DeleteDeliveryStream` while a stream is still `CREATING` ("The
/// resource is already in use and not available for this operation"), so deleting the
/// stack inside that span is the case: nothing reports a deletion that did not happen.
#[tokio::test]
async fn a_deletion_the_owning_service_refuses_fails_the_stack_and_can_be_retried() {
    let router = build_router();
    let case = "a stack deleted while its stream is still creating";
    let stack_name = "retried-deletion";
    let stream_name = format!("{stack_name}-stream");
    let template = json!({
        "Resources": {
            "Stream": {
                "Type": "AWS::KinesisFirehose::DeliveryStream",
                "Properties": {
                    "DeliveryStreamName": stream_name,
                    "ExtendedS3DestinationConfiguration": {
                        "BucketARN": bucket_arn("retry-bucket"),
                        "RoleARN": role_arn("retry-role")
                    }
                }
            }
        }
    })
    .to_string();
    create_stack(&router, case, stack_name, &template, &[]).await;

    // Inside the stream's creation span, Firehose will not delete it.
    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    let body = describe_stack(&router, case, stack_name).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("DELETE_FAILED"),
        "case {case:?}: expected the stack's deletion to fail rather than report a \
         deletion that did not happen, got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Stream") && reason.contains("ResourceInUseException"),
        "case {case:?}: expected the reason to name the resource and the service's \
         refusal, got {reason:?}"
    );
    let (status, surviving) = firehose(
        &router,
        "DescribeDeliveryStream",
        &json!({ "DeliveryStreamName": stream_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the stream to survive the refused deletion, got {status} \
         with body {surviving}"
    );

    // Past the stream's creation span the same deletion goes through.
    tokio::time::sleep(PAST_BOTH_SPANS).await;
    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the retried DeleteStack, got {status} with \
         body {body}"
    );
    tokio::time::sleep(PAST_BOTH_SPANS).await;
    let (status, gone) = firehose(
        &router,
        "DescribeDeliveryStream",
        &json!({ "DeliveryStreamName": stream_name }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the retried deletion to have deleted the stream, got \
         {status} with body {gone}"
    );
}
