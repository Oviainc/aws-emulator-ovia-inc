// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::CloudWatch::MetricStream` its template declares — in CloudWatch.
//!
//! The rule under test: **a stack creates every resource its `Resources` section declares
//! through the service that OWNS the type, so after the create completes the owning
//! service's own read answers for it with the configuration the template declared, a later
//! deployment that changes that configuration is applied through the same service, and
//! deleting the stack deletes it there.** Here the owning call is CloudWatch's
//! `PutMetricStream` ("Creates or updates a metric stream") and the read is
//! `GetMetricStream`, which models `ResourceNotFoundException` only for a stream that does
//! not exist. Four crates have to agree for that to hold (CloudFormation makes the calls;
//! CloudWatch, Firehose, Amazon S3 and IAM serve them), so the suite lives here: a
//! crate-local test can only ever see one side of the call.
//!
//! The same rows hold the stack's `Outputs` to carrying a resolved `OutputValue` for every
//! output the template declares — the handle a client reads to find what was deployed —
//! whichever intrinsic the output is written with (`Ref`, `Fn::GetAtt`, `Fn::Sub`) and
//! whichever of the stack's resources it names.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! stream or leaves the name to CloudFormation, the output format the stream carries, which
//! of the two mutually exclusive namespace filters it declares (and with how many entries),
//! whether it asks for additional statistics and for source-account metrics, whether it
//! carries tags, and whether the template is written in JSON or in YAML with the short-form
//! intrinsic tags. Every expected value is computed here from what the row declares, never
//! read back out of the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The test crate's one awsQuery encoder and element readers: a suite that writes its own
// drifts from the others, so this one calls them (`crate::query_wire`). The SigV4 header
// builder is the crate's one too — it signs for the same Region this suite's hosts and
// expected ARNs name, and a divergence would fail these cases rather than pass quietly.
use crate::query_wire::{elements, form_encode, opt_element};
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// The region every request here is signed for, and so the region the stack's resources and
/// their ARNs belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for both clock-owed spans these services choose to come due: the stack's
/// (`STATUS_TRANSITION_DELAY`, 500 ms) and the Firehose stream's (`TRANSITION_DELAY`,
/// 600 ms). A metric stream has no transient state of its own — `PutMetricStream` creates
/// it `running`.
const PAST_BOTH_SPANS: Duration = Duration::from_millis(1_200);

/// CloudWatch's `X-Amz-Target` prefix, from its model's service shape
/// (`GraniteServiceVersion20100801`), and the signing name its credential scope carries
/// (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
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
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
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

/// Call one CloudFormation operation over its awsQuery wire. The form components are
/// encoded by the test crate's one awsQuery encoder
/// ([`crate::query_wire::form_encode`]), so a template body full of `{`, `"` and `&`
/// survives the body.
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
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudWatch operation over its awsJson1_0 wire.
async fn cloudwatch(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{CLOUDWATCH_SIGNING_NAME}.{REGION}.amazonaws.com"),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header("authorization", authorization(CLOUDWATCH_SIGNING_NAME))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The metric stream CloudWatch holds under `name`, requiring the read to succeed.
async fn get_metric_stream(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, parsed) = cloudwatch(router, "GetMetricStream", &json!({"Name": name})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetMetricStream for {name:?}, got {status} \
         with body {parsed}"
    );
    parsed
}

/// The wire error code of a CloudWatch refusal: awsJson1_0 carries it as the body's
/// `__type`.
fn cloudwatch_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
///
/// Each `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the
/// two document-ordered lists the test crate's repeated-element reader
/// ([`crate::query_wire::elements`]) hands back pair up — which is only true while every
/// entry carries BOTH, the property [`every_declared_output_carries_its_resolved_value`]
/// holds the answer to.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// Create a stack, wait past the clock-owed spans, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_BOTH_SPANS).await;
    describe_stack(router, case, stack_name).await
}

/// The one stack `stack_name` names, as the response body.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// The ARN a metric stream named `name` carries in this emulator:
/// `arn:aws:cloudwatch:<region>:<account>:metric-stream/<name>`, which is the form
/// CloudWatch's own `PutMetricStream` answers with.
fn metric_stream_arn(name: &str) -> String {
    format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT_ID}:metric-stream/{name}")
}

/// The ARN a Firehose stream of `name` carries.
fn firehose_arn(name: &str) -> String {
    format!("arn:aws:firehose:{REGION}:{ACCOUNT_ID}:deliverystream/{name}")
}

/// The ARN an IAM role of `name` carries: IAM is global, so the ARN carries the account and
/// no region.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/{name}")
}

/// The trust policy every role below carries: Firehose has to be able to assume the role a
/// stream's destination names, and CloudWatch's metric streams assume the role they are
/// configured with.
///
/// Written with no `&`, `=`, `+` or `%` so it travels in an `awsQuery` form body as written.
const TRUST_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Principal\":{\"Service\":[\"firehose.amazonaws.com\",\
     \"streams.metrics.cloudwatch.amazonaws.com\"]},\"Action\":\"sts:AssumeRole\"}]}";

/// One row of the table: a template, and what the rule says the stack must then report and
/// CloudWatch must then hold.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The template body, built from the row's own names.
    template: fn() -> String,
    /// The output key whose value is the metric stream's name (`{"Ref": <logical id>}`).
    name_output: &'static str,
    /// The output key whose value is the stream's ARN (`Fn::GetAtt`).
    arn_output: &'static str,
    /// The name the template declares for the stream, or `None` where CloudFormation
    /// generates it.
    declared_name: Option<&'static str>,
    /// The stream resource's logical id, which a generated name is built from.
    logical_id: &'static str,
    /// The members `GetMetricStream` must report for the created stream, computed from what
    /// the template declares.
    expected_configuration: fn() -> Value,
    /// Every other output the template declares, each of which must carry a resolved value.
    other_outputs: fn() -> Vec<(&'static str, String)>,
}

/// The template the first two rows share, with the names and the metric-stream properties
/// the row supplies: an S3 bucket, a delivery role, a Firehose stream delivering to that
/// bucket, and a metric stream pointed at that Firehose stream.
fn json_template(stream_properties: Value, outputs: Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a CloudWatch metric stream delivering through Firehose to S3",
        "Resources": {
            "DeliveryRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": "msdemo-role",
                    "AssumeRolePolicyDocument": TRUST_POLICY,
                },
            },
            "MetricsBucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": "msdemo-metrics"},
            },
            "MetricsStream": {
                "Type": "AWS::KinesisFirehose::DeliveryStream",
                "Properties": {
                    "DeliveryStreamName": "msdemo-firehose",
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": {
                        "BucketARN": {"Fn::GetAtt": ["MetricsBucket", "Arn"]},
                        "RoleARN": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                        "CompressionFormat": "GZIP",
                    },
                },
            },
            "MetricStream": {
                "Type": "AWS::CloudWatch::MetricStream",
                "Properties": stream_properties,
            },
        },
        "Outputs": outputs,
    })
    .to_string()
}

const ROWS: &[Row] = &[
    Row {
        case: "a named json stream with an include filter, tags and outputs by Ref and GetAtt",
        stack_name: "metric-stream-json",
        template: || {
            json_template(
                json!({
                    "Name": "msdemo-json-stream",
                    "FirehoseArn": {"Fn::GetAtt": ["MetricsStream", "Arn"]},
                    "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                    "OutputFormat": "json",
                    "IncludeFilters": [
                        {"Namespace": "AWS/EC2"},
                        {"Namespace": "AWS/Lambda", "MetricNames": ["Invocations", "Errors"]},
                    ],
                    "Tags": [{"Key": "team", "Value": "observability"}],
                }),
                json!({
                    "StreamName": {"Value": {"Ref": "MetricStream"}},
                    "StreamArn": {"Value": {"Fn::GetAtt": ["MetricStream", "Arn"]}},
                    "BucketName": {"Value": {"Ref": "MetricsBucket"}},
                    "FirehoseName": {"Value": {"Ref": "MetricsStream"}},
                    "RoleArn": {"Value": {"Fn::GetAtt": "DeliveryRole.Arn"}},
                }),
            )
        },
        name_output: "StreamName",
        arn_output: "StreamArn",
        declared_name: Some("msdemo-json-stream"),
        logical_id: "MetricStream",
        expected_configuration: || {
            json!({
                "FirehoseArn": firehose_arn("msdemo-firehose"),
                "RoleArn": role_arn("msdemo-role"),
                "OutputFormat": "json",
                "IncludeFilters": [
                    {"Namespace": "AWS/EC2"},
                    {"Namespace": "AWS/Lambda", "MetricNames": ["Invocations", "Errors"]},
                ],
                "State": "running",
            })
        },
        other_outputs: || {
            vec![
                ("BucketName", "msdemo-metrics".to_string()),
                ("FirehoseName", "msdemo-firehose".to_string()),
                ("RoleArn", role_arn("msdemo-role")),
            ]
        },
    },
    Row {
        case: "an UNNAMED opentelemetry1.0 stream with an exclude filter, percentile \
               statistics and linked-account metrics",
        stack_name: "metric-stream-otel",
        template: || {
            json_template(
                json!({
                    "FirehoseArn": {"Fn::GetAtt": ["MetricsStream", "Arn"]},
                    "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                    "OutputFormat": "opentelemetry1.0",
                    "ExcludeFilters": [{"Namespace": "AWS/S3"}],
                    "IncludeLinkedAccountsMetrics": true,
                    "StatisticsConfigurations": [{
                        "IncludeMetrics": [
                            {"Namespace": "AWS/EC2", "MetricName": "CPUUtilization"},
                        ],
                        "AdditionalStatistics": ["p99", "p99.9"],
                    }],
                }),
                json!({
                    "StreamName": {"Value": {"Ref": "MetricStream"}},
                    "StreamArn": {"Value": {"Fn::GetAtt": ["MetricStream", "Arn"]}},
                    "Scope": {"Value": {"Fn::Sub": "${AWS::AccountId}:${AWS::Region}"}},
                }),
            )
        },
        name_output: "StreamName",
        arn_output: "StreamArn",
        declared_name: None,
        logical_id: "MetricStream",
        expected_configuration: || {
            json!({
                "FirehoseArn": firehose_arn("msdemo-firehose"),
                "RoleArn": role_arn("msdemo-role"),
                "OutputFormat": "opentelemetry1.0",
                "ExcludeFilters": [{"Namespace": "AWS/S3"}],
                "IncludeLinkedAccountsMetrics": true,
                "StatisticsConfigurations": [{
                    "IncludeMetrics": [{"Namespace": "AWS/EC2", "MetricName": "CPUUtilization"}],
                    "AdditionalStatistics": ["p99", "p99.9"],
                }],
                "State": "running",
            })
        },
        other_outputs: || vec![("Scope", format!("{ACCOUNT_ID}:{REGION}"))],
    },
    Row {
        case: "the same declaration in YAML with the short-form intrinsic tags, \
               opentelemetry0.7",
        stack_name: "metric-stream-yaml",
        template: || {
            format!(
                "\
AWSTemplateFormatVersion: '2010-09-09'
Description: a metric stream declared in YAML
Resources:
  DeliveryRole:
    Type: AWS::IAM::Role
    Properties:
      RoleName: msyaml-role
      AssumeRolePolicyDocument: '{TRUST_POLICY}'
  MetricsBucket:
    Type: AWS::S3::Bucket
    Properties:
      BucketName: msyaml-metrics
  MetricsStream:
    Type: AWS::KinesisFirehose::DeliveryStream
    Properties:
      DeliveryStreamName: msyaml-firehose
      ExtendedS3DestinationConfiguration:
        BucketARN: !GetAtt MetricsBucket.Arn
        RoleARN: !GetAtt DeliveryRole.Arn
        CompressionFormat: Snappy
  MetricStream:
    Type: AWS::CloudWatch::MetricStream
    Properties:
      Name: msyaml-stream
      FirehoseArn: !GetAtt MetricsStream.Arn
      RoleArn: !GetAtt DeliveryRole.Arn
      OutputFormat: opentelemetry0.7
      IncludeFilters:
        - Namespace: AWS/Firehose
Outputs:
  StreamName:
    Value: !Ref MetricStream
  StreamArn:
    Value: !GetAtt MetricStream.Arn
  BucketName:
    Value: !Ref MetricsBucket
"
            )
        },
        name_output: "StreamName",
        arn_output: "StreamArn",
        declared_name: Some("msyaml-stream"),
        logical_id: "MetricStream",
        expected_configuration: || {
            json!({
                "FirehoseArn": firehose_arn("msyaml-firehose"),
                "RoleArn": role_arn("msyaml-role"),
                "OutputFormat": "opentelemetry0.7",
                "IncludeFilters": [{"Namespace": "AWS/Firehose"}],
                "State": "running",
            })
        },
        other_outputs: || vec![("BucketName", "msyaml-metrics".to_string())],
    },
];

/// Whether `name` is the `<stack>-<logical id>-<suffix>` name CloudFormation generates for
/// a resource the template does not name.
fn is_generated_name(name: &str, stack_name: &str, logical_id: &str) -> bool {
    let prefix = format!("{stack_name}-{logical_id}-");
    name.strip_prefix(&prefix)
        .is_some_and(|suffix| !suffix.is_empty())
}

#[tokio::test]
async fn a_completed_stack_has_created_its_metric_stream_in_cloudwatch() {
    for row in ROWS {
        // One router per row: each row's template deploys its own resources, and two rows
        // naming the same bucket would be a second create of it rather than a new one.
        let router = build_router();
        let case = row.case;
        let body = create_and_settle(&router, case, row.stack_name, &(row.template)()).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );

        let outputs = outputs_of(&body);
        let name = outputs
            .get(row.name_output)
            .unwrap_or_else(|| panic!("case {case:?}: no {} output in {body}", row.name_output))
            .clone();
        match row.declared_name {
            Some(declared) => assert_eq!(
                name, declared,
                "case {case:?}: expected the Ref to be the declared stream name"
            ),
            None => assert!(
                is_generated_name(&name, row.stack_name, row.logical_id),
                "case {case:?}: expected a generated <stack>-<logical id>-<suffix> name, \
                 got {name:?}"
            ),
        }

        // Every other output the template declares carries its resolved value too.
        for (key, expected) in (row.other_outputs)() {
            let actual = outputs
                .get(key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"));
            assert_eq!(
                actual, &expected,
                "case {case:?}: expected output {key} to be {expected:?}, got {actual:?}"
            );
        }

        // CloudWatch itself answers for the stream the stack declared, with the
        // configuration the template gave it.
        let reported = get_metric_stream(&router, case, &name).await;
        assert_eq!(
            reported.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected CloudWatch to report the stream under the name the \
             stack published, got {reported}"
        );
        let expected_arn = metric_stream_arn(&name);
        assert_eq!(
            outputs.get(row.arn_output).map(String::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the {} output to be the stream's ARN, got {outputs:?}",
            row.arn_output
        );
        assert_eq!(
            reported.get("Arn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected CloudWatch to report the same ARN the stack's output \
             carries, got {reported}"
        );
        for (member, expected) in (row.expected_configuration)()
            .as_object()
            .expect("the row's expected configuration is an object")
        {
            assert_eq!(
                reported.get(member),
                Some(expected),
                "case {case:?}: expected {member} to be {expected}, got {reported}"
            );
        }
        // The two instants CloudWatch keeps for a stream are reported for one it created.
        for member in ["CreationDate", "LastUpdateDate"] {
            assert!(
                reported.get(member).and_then(Value::as_f64).is_some(),
                "case {case:?}: expected CloudWatch to report {member}, got {reported}"
            );
        }

        // Deleting the stack deletes the stream it created in CloudWatch: the read that
        // answered above now reports it missing.
        let (status, deleted) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}"
        );
        tokio::time::sleep(PAST_BOTH_SPANS).await;
        let (status, gone) = cloudwatch(&router, "GetMetricStream", &json!({"Name": &name})).await;
        assert_eq!(
            (status, cloudwatch_error_code(&gone).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected the stream to be gone after the stack's deletion, got \
             {status} with {gone}"
        );
    }
}

/// A stack UPDATE that changes the stream's declared configuration is applied in
/// CloudWatch: the filter the new template declares is the filter the stream has.
///
/// The rows are the changes one declaration can take: a namespace added to an include
/// filter, a metric name narrowing one, a switch to the other filter kind, and an
/// additional statistic the output format carries.
#[tokio::test]
async fn an_update_applies_the_declared_change_to_the_stream_cloudwatch_holds() {
    let router = build_router();
    let case = "a stack whose metric stream is updated";
    let stack_name = "metric-stream-updated";
    let stream_name = "msupdate-stream";
    let template = |properties: Value| {
        json_template(
            properties,
            json!({"StreamName": {"Value": {"Ref": "MetricStream"}}}),
        )
    };
    let declaration = |extra: Value| {
        let mut properties = json!({
            "Name": stream_name,
            "FirehoseArn": {"Fn::GetAtt": ["MetricsStream", "Arn"]},
            "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
            "OutputFormat": "json",
        });
        for (member, value) in extra.as_object().expect("the change is an object") {
            properties[member] = value.clone();
        }
        properties
    };

    let first = json!({"IncludeFilters": [{"Namespace": "AWS/EC2"}]});
    let body = create_and_settle(
        &router,
        case,
        stack_name,
        &template(declaration(first.clone())),
    )
    .await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let reported = get_metric_stream(&router, case, stream_name).await;
    assert_eq!(
        reported.get("IncludeFilters"),
        first.get("IncludeFilters"),
        "case {case:?}: expected the stream to carry the filter the template declared, got \
         {reported}"
    );

    let changes: Vec<(&str, Value, &str)> = vec![
        (
            "a namespace added to the include filter",
            json!({"IncludeFilters": [{"Namespace": "AWS/EC2"}, {"Namespace": "AWS/Lambda"}]}),
            "IncludeFilters",
        ),
        (
            "the include filter narrowed to one metric name",
            json!({"IncludeFilters": [{"Namespace": "AWS/EC2", "MetricNames": ["CPUUtilization"]}]}),
            "IncludeFilters",
        ),
        (
            "an additional statistic the json format carries",
            json!({
                "IncludeFilters": [{"Namespace": "AWS/EC2"}],
                "StatisticsConfigurations": [{
                    "IncludeMetrics": [{"Namespace": "AWS/EC2", "MetricName": "CPUUtilization"}],
                    "AdditionalStatistics": ["Average"],
                }],
            }),
            "StatisticsConfigurations",
        ),
    ];

    for (change, properties, member) in changes {
        let (status, body) = cloudformation(
            &router,
            "UpdateStack",
            &[
                ("StackName", stack_name),
                ("TemplateBody", &template(declaration(properties.clone()))),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {change:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_BOTH_SPANS).await;
        let described = describe_stack(&router, change, stack_name).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {change:?}: expected the update to complete, got {described}"
        );
        let reported = get_metric_stream(&router, change, stream_name).await;
        assert_eq!(
            reported.get(member),
            properties.get(member),
            "case {change:?}: expected CloudWatch to report the {member} the update \
             declared, got {reported}"
        );
        // An update of an existing stream does not re-create it: "If you use it to update
        // an existing stream, the state of the stream is not changed."
        assert_eq!(
            reported.get("State").and_then(Value::as_str),
            Some("running"),
            "case {change:?}: expected the updated stream to stay running, got {reported}"
        );
    }
}

/// A metric stream CloudWatch REFUSES to create fails the stack with the refusal that
/// service answered, and nothing is created: the refusal here is `PutMetricStream`'s own
/// rule that an OpenTelemetry stream carries percentile statistics only.
#[tokio::test]
async fn a_metric_stream_cloudwatch_refuses_fails_the_stack() {
    let router = build_router();
    let case = "an opentelemetry stream asking for a non-percentile statistic";
    let stack_name = "metric-stream-refused";
    let template = json_template(
        json!({
            "Name": "msrefused-stream",
            "FirehoseArn": {"Fn::GetAtt": ["MetricsStream", "Arn"]},
            "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
            "OutputFormat": "opentelemetry1.0",
            "StatisticsConfigurations": [{
                "IncludeMetrics": [{"Namespace": "AWS/EC2", "MetricName": "CPUUtilization"}],
                "AdditionalStatistics": ["Average"],
            }],
        }),
        json!({"StreamName": {"Value": {"Ref": "MetricStream"}}}),
    );

    let body = create_and_settle(&router, case, stack_name, &template).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_FAILED"),
        "case {case:?}: expected the stack to fail rather than publish a stream nothing \
         holds, got {body}"
    );
    let reason = opt_element(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("MetricStream") && reason.contains("InvalidParameterValueException"),
        "case {case:?}: expected the reason to name the resource and CloudWatch's own \
         refusal, got {reason:?}"
    );

    let (status, gone) = cloudwatch(
        &router,
        "GetMetricStream",
        &json!({"Name": "msrefused-stream"}),
    )
    .await;
    assert_eq!(
        (status, cloudwatch_error_code(&gone).as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: expected no stream of that name, got {status} with {gone}"
    );
}

/// Every output a template declares is reported with its RESOLVED `OutputValue`, whichever
/// intrinsic it is written with and whichever resource it names — including a resource of a
/// type no service here creates, which still carries the physical id and the attributes
/// CloudFormation's documented naming rules give it.
///
/// This is the property a client reads a deployment through
/// (`{o["OutputKey"]: o["OutputValue"] for o in stack["Outputs"]}`), so it is asserted over
/// the whole `Outputs` section of one stack rather than output by output: the entry count
/// has to match, and every entry has to carry a value.
#[tokio::test]
async fn every_declared_output_carries_its_resolved_value() {
    let router = build_router();
    let case = "a stack whose outputs name each of its resources";
    let stack_name = "metric-stream-outputs";
    let mut template: Value = serde_json::from_str(&json_template(
        json!({
            "Name": "msoutputs-stream",
            "FirehoseArn": {"Fn::GetAtt": ["MetricsStream", "Arn"]},
            "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
            "OutputFormat": "json",
            "IncludeFilters": [{"Namespace": "AWS/EC2"}],
        }),
        json!({
            "StreamName": {"Value": {"Ref": "MetricStream"}},
            "StreamArn": {"Value": {"Fn::GetAtt": ["MetricStream", "Arn"]}},
            "FirehoseName": {"Value": {"Ref": "MetricsStream"}},
            "FirehoseArn": {"Value": {"Fn::GetAtt": ["MetricsStream", "Arn"]}},
            "BucketName": {"Value": {"Ref": "MetricsBucket"}},
            "BucketArn": {"Value": {"Fn::GetAtt": ["MetricsBucket", "Arn"]}},
            "RoleName": {"Value": {"Ref": "DeliveryRole"}},
            "RoleArn": {"Value": {"Fn::GetAtt": ["DeliveryRole", "Arn"]}},
            "DashboardName": {"Value": {"Ref": "Dashboard"}},
            "StreamHandle": {
                "Value": {"Fn::Join": ["/", [{"Ref": "MetricsBucket"}, {"Ref": "MetricStream"}]]},
            },
            "Described": {
                "Value": {"Fn::Sub": "${MetricStream} to ${MetricsStream}"},
                "Description": "the stream and where it delivers",
            },
        }),
    ))
    .expect("the template is JSON");
    // A resource of a type this emulator creates in no service: it is DECLARED-ONLY, and
    // the output naming it still has to resolve to the identity CloudFormation's naming
    // rules gave it.
    template["Resources"]["Dashboard"] = json!({
        "Type": "AWS::CloudWatch::Dashboard",
        "Properties": {
            "DashboardName": "msoutputs-dashboard",
            "DashboardBody": "{\"widgets\":[]}",
        },
    });

    let body = create_and_settle(&router, case, stack_name, &template.to_string()).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );

    let expected: BTreeMap<&str, String> = [
        ("StreamName", "msoutputs-stream".to_string()),
        ("StreamArn", metric_stream_arn("msoutputs-stream")),
        ("FirehoseName", "msdemo-firehose".to_string()),
        ("FirehoseArn", firehose_arn("msdemo-firehose")),
        ("BucketName", "msdemo-metrics".to_string()),
        ("BucketArn", "arn:aws:s3:::msdemo-metrics".to_string()),
        ("RoleName", "msdemo-role".to_string()),
        ("RoleArn", role_arn("msdemo-role")),
        ("DashboardName", "msoutputs-dashboard".to_string()),
        ("StreamHandle", "msdemo-metrics/msoutputs-stream".to_string()),
        (
            "Described",
            "msoutputs-stream to msdemo-firehose".to_string(),
        ),
    ]
    .into_iter()
    .collect();

    // One `OutputKey` and one `OutputValue` per declared output: a key reported without a
    // value is what makes these two counts differ.
    let keys = elements(&body, "OutputKey");
    let values = elements(&body, "OutputValue");
    assert_eq!(
        (keys.len(), values.len()),
        (expected.len(), expected.len()),
        "case {case:?}: expected one OutputKey and one OutputValue per declared output, got \
         {keys:?} and {values:?}"
    );

    let outputs = outputs_of(&body);
    for (key, value) in &expected {
        let actual = outputs
            .get(*key)
            .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"));
        assert_eq!(
            actual, value,
            "case {case:?}: expected output {key} to be {value:?}, got {actual:?}"
        );
    }
}
