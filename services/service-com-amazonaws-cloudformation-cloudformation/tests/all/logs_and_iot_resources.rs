// SPDX-License-Identifier: CC0-1.0

//! `AWS::Logs::LogGroup`, `AWS::Logs::LogStream`, `AWS::Logs::MetricFilter` and
//! `AWS::IoT::Authorizer` end to end: the resources a stack declares exist in Amazon
//! CloudWatch Logs and in AWS IoT themselves, under the physical ids the stack publishes.
//!
//! RULE: a resource type CloudFormation deploys is created in the service that OWNS it,
//! through that service's own public API, under the physical id the stack publishes — so the
//! name a client reads out of a stack's outputs is one that service answers for, and a
//! second declaration of the same name is refused by the service that owns the name. A type
//! this emulator has no provider for is left declared-only instead, which is the half
//! `unprovisioned_resources.rs` pins.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES each
//! resource or leaves CloudFormation to name it, the template spelling (JSON or YAML), the
//! log-group class and retention, how many streams and filters one group carries, how a
//! metric transformation is declared (with a default value and a unit, or with the
//! dimensions that service documents as mutually exclusive with a default value), and the
//! authorizer's status and token configuration.
//!
//! Every case drives the real request path for all three services through one router — the
//! CloudFormation calls over the awsQuery wire (the harness `stack_lifecycle.rs` owns), the
//! CloudWatch Logs calls over its awsJson1_1 wire and the AWS IoT calls over its REST wire,
//! each as a client's SDK makes them — so what is asserted is what a client that deploys the
//! template and then reads the resources back observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text, ACCOUNT_ID, REGION,
};

/// The awsJson version Amazon CloudWatch Logs's model declares (`awsJson1_1`), and the
/// service shape its `X-Amz-Target` names an operation under (`Logs_20140328`).
const LOGS_JSON: &str = "application/x-amz-json-1.1";
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// Call one CloudWatch Logs operation through the same router, signed for `logs` the way a
/// client's SDK signs it: the status and the parsed answer document.
async fn logs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri(format!("https://logs.{REGION}.amazonaws.com/"))
        .header("content-type", LOGS_JSON)
        .header("x-amz-target", format!("{LOGS_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/logs/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(request.to_string()))
        .expect("a well-formed CloudWatch Logs request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// Call one AWS IoT operation through the same router, on the method and URI that service's
/// model binds it to (`restJson1`): the status and the parsed answer document.
async fn iot(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(ToString::to_string).unwrap_or_default();
    let http_request = http::Request::builder()
        .method(method)
        .uri(format!("https://iot.{REGION}.amazonaws.com{uri}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed AWS IoT request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let answer = response.into_body().to_vec();
    (status, serde_json::from_slice(&answer).unwrap_or(Value::Null))
}

/// The log group CloudWatch Logs holds under `name`, or `None` where it holds none.
async fn described_group(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, document) = logs(
        router,
        "DescribeLogGroups",
        &json!({"logGroupNamePrefix": name}),
    )
    .await;
    if status != 200 {
        return None;
    }
    document
        .get("logGroups")?
        .as_array()?
        .iter()
        .find(|group| group.get("logGroupName").and_then(Value::as_str) == Some(name))
        .cloned()
}

/// The names of the streams CloudWatch Logs holds in the group `group`.
async fn stream_names(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let (status, document) = logs(
        router,
        "DescribeLogStreams",
        &json!({"logGroupName": group}),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    document
        .get("logStreams")
        .and_then(Value::as_array)
        .map(|streams| {
            streams
                .iter()
                .filter_map(|stream| {
                    stream
                        .get("logStreamName")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The metric filter CloudWatch Logs holds on `group` under `name`, or `None` where it holds
/// none.
async fn described_filter(
    router: &MultiServiceRouter,
    group: &str,
    name: &str,
) -> Option<Value> {
    let (status, document) = logs(
        router,
        "DescribeMetricFilters",
        &json!({"logGroupName": group}),
    )
    .await;
    if status != 200 {
        return None;
    }
    document
        .get("metricFilters")?
        .as_array()?
        .iter()
        .find(|filter| filter.get("filterName").and_then(Value::as_str) == Some(name))
        .cloned()
}

/// The tags CloudWatch Logs holds for the resource `arn` names, as `(key, value)` pairs.
async fn group_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<(String, String)> {
    let (status, document) = logs(
        router,
        "ListTagsForResource",
        &json!({"resourceArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource for {arn:?}, got {status} \
         with {document}"
    );
    let mut tags: Vec<(String, String)> = document
        .get("tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (key.clone(), value.as_str().unwrap_or_default().to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    tags.sort();
    tags
}

/// The authorizer AWS IoT holds under `name`, or `None` where it holds none.
async fn described_authorizer(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, document) = iot(router, "GET", &format!("/authorizer/{name}"), None).await;
    if status != 200 {
        return None;
    }
    document.get("authorizerDescription").cloned()
}

/// Every output a `DescribeStacks` response reports, as `(OutputKey, OutputValue)` pairs read
/// inside each `<member>` block.
fn output(xml: &str, key: &str) -> Option<String> {
    let block = xml_blocks(xml, "Outputs").into_iter().next()?;
    xml_blocks(&block, "member").into_iter().find_map(|entry| {
        (xml_text(&entry, "OutputKey").as_deref() == Some(key))
            .then(|| xml_text(&entry, "OutputValue"))
            .flatten()
    })
}

/// The function ARN every row's authorizer names: the authorizer calls a Lambda function, and
/// what the type requires is the ARN of one.
fn authorizer_function(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:{name}")
}

/// Every resource failure the stack recorded, as the reason each one carries: what a case
/// whose stack did not complete needs in order to say WHY.
async fn failure_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    xml_blocks(&body, "member")
        .into_iter()
        .filter_map(|event| xml_text(&event, "ResourceStatusReason"))
        .collect()
}

/// One row of the table: a template, and what the rule says must then be in the two owning
/// services.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The log group name the template states, or `None` where CloudFormation names it (the
    /// row then reads the name out of the stack's own output).
    group: Option<&'static str>,
    /// The stream names the template states, in declaration order.
    streams: &'static [&'static str],
    /// The metric filter's name and the transformation CloudWatch Logs must report for it.
    filter: Option<(&'static str, Value)>,
    /// The retention and the class CloudWatch Logs must report for the group.
    retention: Option<i64>,
    class: &'static str,
    /// The authorizer's name and the members AWS IoT must report for it.
    authorizer: (&'static str, Value),
    /// The tags the group must carry: the declaration's own plus the stack's, and never a key
    /// in AWS's reserved namespace.
    tags: &'static [(&'static str, &'static str)],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "every resource named by the template, in JSON, with one stream and a \
                   filter over a plain transformation",
            stack: "logs-iot-named",
            template: json!({
                "Resources": {
                    "Group": {
                        "Type": "AWS::Logs::LogGroup",
                        "Properties": {
                            "LogGroupName": "named-group",
                            "RetentionInDays": 14,
                            "Tags": [{"Key": "Owner", "Value": "platform"}]
                        }
                    },
                    "Stream": {
                        "Type": "AWS::Logs::LogStream",
                        "Properties": {
                            "LogGroupName": {"Ref": "Group"},
                            "LogStreamName": "named-stream"
                        }
                    },
                    "Filter": {
                        "Type": "AWS::Logs::MetricFilter",
                        "Properties": {
                            "FilterName": "named-filter",
                            "LogGroupName": {"Ref": "Group"},
                            "FilterPattern": "ERROR",
                            "MetricTransformations": [{
                                "MetricName": "Errors",
                                "MetricNamespace": "Platform/Logs",
                                "MetricValue": "1",
                                "DefaultValue": 0.5,
                                "Unit": "Count"
                            }]
                        }
                    },
                    "Authorizer": {
                        "Type": "AWS::IoT::Authorizer",
                        "Properties": {
                            "AuthorizerName": "named-authorizer",
                            "AuthorizerFunctionArn": authorizer_function("authorize-named"),
                            "TokenKeyName": "token",
                            "Status": "ACTIVE"
                        }
                    }
                },
                "Outputs": {
                    "GroupName": {"Value": {"Ref": "Group"}},
                    "GroupArn": {"Value": {"Fn::GetAtt": ["Group", "Arn"]}},
                    "StreamName": {"Value": {"Ref": "Stream"}},
                    "FilterName": {"Value": {"Ref": "Filter"}},
                    "AuthorizerName": {"Value": {"Ref": "Authorizer"}},
                    "AuthorizerArn": {"Value": {"Fn::GetAtt": ["Authorizer", "Arn"]}}
                }
            })
            .to_string(),
            group: Some("named-group"),
            streams: &["named-stream"],
            filter: Some((
                "named-filter",
                json!([{
                    "metricName": "Errors",
                    "metricNamespace": "Platform/Logs",
                    "metricValue": "1",
                    "defaultValue": 0.5,
                    "unit": "Count"
                }]),
            )),
            retention: Some(14),
            class: "STANDARD",
            authorizer: (
                "named-authorizer",
                json!({
                    "authorizerFunctionArn": authorizer_function("authorize-named"),
                    "tokenKeyName": "token",
                    "status": "ACTIVE"
                }),
            ),
            tags: &[("Owner", "platform"), ("Stack", "logs-iot-suite")],
        },
        Row {
            case: "nothing named by the template, in YAML, with two streams, a \
                   transformation carrying dimensions and an inactive authorizer",
            stack: "logs-iot-generated",
            template: [
                "Resources:",
                "  Group:",
                "    Type: AWS::Logs::LogGroup",
                "    Properties:",
                "      LogGroupClass: INFREQUENT_ACCESS",
                "  First:",
                "    Type: AWS::Logs::LogStream",
                "    Properties:",
                "      LogGroupName: !Ref Group",
                "  Second:",
                "    Type: AWS::Logs::LogStream",
                "    Properties:",
                "      LogGroupName: !Ref Group",
                "      LogStreamName: second-stream",
                "  Filter:",
                "    Type: AWS::Logs::MetricFilter",
                "    Properties:",
                "      LogGroupName: !Ref Group",
                "      FilterPattern: '{ $.level = \"WARN\" }'",
                "      ApplyOnTransformedLogs: true",
                "      MetricTransformations:",
                "        - MetricName: Warnings",
                "          MetricNamespace: Platform/Logs",
                "          MetricValue: '1'",
                "          Unit: Count",
                "          Dimensions:",
                "            - Key: Stage",
                "              Value: $.stage",
                "  Authorizer:",
                "    Type: AWS::IoT::Authorizer",
                "    Properties:",
                "      AuthorizerFunctionArn: !Sub",
                "        - arn:aws:lambda:${Region}:${Account}:function:authorize-generated",
                "        - Region: !Ref AWS::Region",
                "          Account: !Ref AWS::AccountId",
                "      Status: INACTIVE",
                "      SigningDisabled: true",
                "Outputs:",
                "  GroupName:",
                "    Value: !Ref Group",
                "  GroupArn:",
                "    Value: !GetAtt Group.Arn",
                "  StreamName:",
                "    Value: !Ref First",
                "  FilterName:",
                "    Value: !Ref Filter",
                "  AuthorizerName:",
                "    Value: !Ref Authorizer",
                "  AuthorizerArn:",
                "    Value: !GetAtt Authorizer.Arn",
                "",
            ]
            .join("\n"),
            group: None,
            streams: &["second-stream"],
            filter: Some((
                "",
                json!([{
                    "metricName": "Warnings",
                    "metricNamespace": "Platform/Logs",
                    "metricValue": "1",
                    "unit": "Count",
                    "dimensions": {"Stage": "$.stage"}
                }]),
            )),
            retention: None,
            class: "INFREQUENT_ACCESS",
            authorizer: (
                "",
                json!({
                    "authorizerFunctionArn": authorizer_function("authorize-generated"),
                    "status": "INACTIVE",
                    "signingDisabled": true
                }),
            ),
            tags: &[("Stack", "logs-iot-suite")],
        },
    ]
}

/// The stack-level tag every row supplies, which CloudFormation propagates to the resources
/// it creates that are taggable — of these four, the log group and the authorizer.
const STACK_TAG: (&str, &str) = ("Stack", "logs-iot-suite");

/// A declared log group, stream, metric filter and IoT authorizer are really created in the
/// services that own them, under the physical ids the stack publishes.
#[tokio::test]
async fn a_stack_provisions_the_logs_and_iot_resources_it_declares() {
    for row in rows() {
        let case = row.case;
        let router = router();
        let (status, body) = create_stack(
            &router,
            row.stack,
            &row.template,
            &[
                member("Tags.member.1.Key", STACK_TAG.0),
                member("Tags.member.1.Value", STACK_TAG.1),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        wait_for_the_transition().await;

        let (status, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body} with the resource \
             failures {:?}",
            failure_reasons(&router, row.stack).await
        );

        // The group the stack published: the name the declaration asked for, or the one
        // CloudFormation generated for a declaration that named none.
        let group_name = output(&body, "GroupName")
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no group name"));
        if let Some(declared) = row.group {
            assert_eq!(
                group_name, declared,
                "case {case:?}: expected the group's published name to be the declared one"
            );
        }
        let group = described_group(&router, &group_name)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: CloudWatch Logs holds no log group under the name the \
                     stack published ({group_name:?})"
                )
            });
        assert_eq!(
            group.get("logGroupClass").and_then(Value::as_str),
            Some(row.class),
            "case {case:?}: expected the declared log group class, got {group}"
        );
        assert_eq!(
            group.get("retentionInDays").and_then(Value::as_i64),
            row.retention,
            "case {case:?}: expected the declared retention, got {group}"
        );

        // The group's tags, read through its own ARN. The type's published `Arn` carries the
        // trailing `:*` the `LogGroup$arn` member does, which the tagging member's modelled
        // pattern (`AmazonResourceName`, `^[\w+=/:,.@-]*$`) does not admit, so the read uses
        // the plain form of the same ARN: the tags it answers are the declaration's own plus
        // the stack's, and nothing in AWS's reserved namespace.
        let group_arn = output(&body, "GroupArn")
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no group ARN"));
        assert_eq!(
            group_arn,
            format!("arn:aws:logs:{REGION}:{ACCOUNT_ID}:log-group:{group_name}:*"),
            "case {case:?}: expected the published ARN to be the group's documented one"
        );
        let tag_arn = format!("arn:aws:logs:{REGION}:{ACCOUNT_ID}:log-group:{group_name}");
        let tags = group_tags(&router, case, &tag_arn).await;
        let expected: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tags, expected,
            "case {case:?}: expected the group to carry {expected:?} through the ARN \
             {tag_arn:?}, got {tags:?}"
        );

        // Every declared stream is in that group, under the name the stack published for it.
        let held = stream_names(&router, &group_name).await;
        let published = output(&body, "StreamName")
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no stream name"));
        assert!(
            held.contains(&published),
            "case {case:?}: expected the stream the stack published ({published:?}) to be in \
             the group, got {held:?}"
        );
        for declared in row.streams {
            assert!(
                held.iter().any(|name| name == declared),
                "case {case:?}: expected the declared stream {declared:?} to be in the \
                 group, got {held:?}"
            );
        }

        // The filter is on that group, with the transformation spelled the way CloudWatch
        // Logs's own API spells one.
        if let Some((_, transformations)) = &row.filter {
            let filter_name = output(&body, "FilterName")
                .unwrap_or_else(|| panic!("case {case:?}: the stack published no filter name"));
            let filter = described_filter(&router, &group_name, &filter_name)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: CloudWatch Logs holds no metric filter under the name \
                         the stack published ({filter_name:?})"
                    )
                });
            assert_eq!(
                filter.get("metricTransformations"),
                Some(transformations),
                "case {case:?}: expected the declared transformation, got {filter}"
            );
            assert_eq!(
                filter.get("logGroupName").and_then(Value::as_str),
                Some(group_name.as_str()),
                "case {case:?}: expected the filter to be on the declared group, got {filter}"
            );
        }

        // The authorizer is in AWS IoT, under the name the stack published, and the ARN the
        // stack publishes for it is the one that service minted.
        let authorizer_name = output(&body, "AuthorizerName")
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no authorizer name"));
        let authorizer = described_authorizer(&router, &authorizer_name)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: AWS IoT holds no authorizer under the name the stack \
                     published ({authorizer_name:?})"
                )
            });
        for (member, expected) in row
            .authorizer
            .1
            .as_object()
            .expect("a row's expected authorizer members are an object")
        {
            assert_eq!(
                authorizer.get(member),
                Some(expected),
                "case {case:?}: expected the authorizer's {member} to be {expected}, got \
                 {authorizer}"
            );
        }
        let authorizer_arn = output(&body, "AuthorizerArn")
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no authorizer ARN"));
        assert_eq!(
            authorizer.get("authorizerArn").and_then(Value::as_str),
            Some(authorizer_arn.as_str()),
            "case {case:?}: expected the published ARN to be the one AWS IoT minted, got \
             {authorizer}"
        );
        if !row.authorizer.0.is_empty() {
            assert_eq!(
                authorizer_name, row.authorizer.0,
                "case {case:?}: expected the authorizer's published name to be the declared one"
            );
        }

        // Deleting the stack releases all four in their owning services.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {body}"
        );
        wait_for_the_transition().await;
        assert!(
            described_group(&router, &group_name).await.is_none(),
            "case {case:?}: expected the deleted stack to have released the log group \
             {group_name:?}"
        );
        assert!(
            described_authorizer(&router, &authorizer_name)
                .await
                .is_none(),
            "case {case:?}: expected the deleted stack to have released the authorizer \
             {authorizer_name:?}"
        );
    }
}

/// A second stack declaring an authorizer name AWS IoT already holds is refused BY THAT
/// SERVICE: the resource fails, the stack rolls back naming it, and the authorizer the first
/// stack created is left exactly as it was.
#[tokio::test]
async fn a_duplicate_authorizer_name_is_refused_by_the_owning_service() {
    let router = router();
    let name = "duplicate-authorizer";
    let template = |function: &str| {
        json!({
            "Resources": {
                "Authorizer": {
                    "Type": "AWS::IoT::Authorizer",
                    "Properties": {
                        "AuthorizerName": name,
                        "AuthorizerFunctionArn": authorizer_function(function),
                        "Status": "ACTIVE"
                    }
                }
            }
        })
        .to_string()
    };

    let case = "the first stack, which creates the authorizer";
    let (status, body) = create_stack(&router, "iot-first", &template("authorize-first"), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "iot-first").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the first stack to complete, got {body}"
    );

    let case = "a second stack declaring the same authorizer name";
    let (status, body) =
        create_stack(&router, "iot-second", &template("authorize-second"), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "iot-second").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the refused create to be rolled back rather than report an \
         authorizer it did not create, got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Authorizer"),
        "case {case:?}: expected the reason to name the resource that failed, got {reason:?}"
    );

    // The authorizer the FIRST stack created is untouched: the refusal left it alone, and the
    // rollback of the second stack did not delete another stack's resource.
    let held = described_authorizer(&router, name)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: AWS IoT no longer holds the authorizer"));
    assert_eq!(
        held.get("authorizerFunctionArn").and_then(Value::as_str),
        Some(authorizer_function("authorize-first").as_str()),
        "case {case:?}: expected the first stack's authorizer to be untouched, got {held}"
    );
}

/// A changed declaration reaches the owning service's own update: the group keeps its
/// identity and the retention, the filter's pattern and the authorizer's configuration are
/// the ones the new revision states.
#[tokio::test]
async fn a_changed_declaration_is_applied_through_the_owning_services() {
    let router = router();
    let stack = "logs-iot-updated";
    let template = |retention: i64, pattern: &str, status: &str| {
        json!({
            "Resources": {
                "Group": {
                    "Type": "AWS::Logs::LogGroup",
                    "Properties": {
                        "LogGroupName": "updated-group",
                        "RetentionInDays": retention
                    }
                },
                "Filter": {
                    "Type": "AWS::Logs::MetricFilter",
                    "Properties": {
                        "FilterName": "updated-filter",
                        "LogGroupName": {"Ref": "Group"},
                        "FilterPattern": pattern,
                        "MetricTransformations": [{
                            "MetricName": "Hits",
                            "MetricNamespace": "Platform/Logs",
                            "MetricValue": "1"
                        }]
                    }
                },
                "Authorizer": {
                    "Type": "AWS::IoT::Authorizer",
                    "Properties": {
                        "AuthorizerName": "updated-authorizer",
                        "AuthorizerFunctionArn": authorizer_function("authorize-updated"),
                        "Status": status
                    }
                }
            }
        })
        .to_string()
    };

    let case = "the first revision";
    let (status, body) = create_stack(&router, stack, &template(7, "ERROR", "ACTIVE"), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;

    let case = "the revision that changes the retention, the pattern and the status";
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template(30, "WARN", "INACTIVE")),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {body}"
    );

    let group = described_group(&router, "updated-group")
        .await
        .unwrap_or_else(|| panic!("case {case:?}: CloudWatch Logs holds no updated group"));
    assert_eq!(
        group.get("retentionInDays").and_then(Value::as_i64),
        Some(30),
        "case {case:?}: expected the revised retention to have reached CloudWatch Logs, got \
         {group}"
    );
    let filter = described_filter(&router, "updated-group", "updated-filter")
        .await
        .unwrap_or_else(|| panic!("case {case:?}: CloudWatch Logs holds no updated filter"));
    assert_eq!(
        filter.get("filterPattern").and_then(Value::as_str),
        Some("WARN"),
        "case {case:?}: expected the revised pattern to have reached CloudWatch Logs, got \
         {filter}"
    );
    let authorizer = described_authorizer(&router, "updated-authorizer")
        .await
        .unwrap_or_else(|| panic!("case {case:?}: AWS IoT holds no updated authorizer"));
    assert_eq!(
        authorizer.get("status").and_then(Value::as_str),
        Some("INACTIVE"),
        "case {case:?}: expected the revised status to have reached AWS IoT, got {authorizer}"
    );
}
