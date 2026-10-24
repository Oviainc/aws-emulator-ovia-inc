// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::SNS::Topic` resources its template declares — in Amazon SNS.
//!
//! The rule under test: **a stack reports a resource as created only where the service
//! that owns it created it, so the handle the stack publishes for that resource
//! (`Ref`/`Fn::GetAtt`) addresses a resource that service really holds.** For a topic the
//! handle is its ARN, so after the create waiter returns, `GetTopicAttributes` and the
//! out-of-band write every client changes a topic with (`SetTopicAttributes`) answer for
//! the stack's own output — rather than SNS answering `NotFound` for a topic its own stack
//! just published. Two crates have to agree for that to hold (CloudFormation makes the
//! call, SNS serves it), so the suite lives here: a crate-local test can only ever see one
//! side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! topic or leaves the name to CloudFormation, how many topics one stack declares, which
//! attributes the properties set (and whether they are written as strings, numbers,
//! booleans or an object), whether the topic is standard or FIFO, whether the properties
//! carry the two that are NOT attributes (an inline subscription list, a per-protocol
//! delivery-status logging config), and which attribute an out-of-band write then changes.
//! Every expected value is computed from what the row declares, never read back out of the
//! answer being checked.
//!
//! The deletion half and the refusal half are rows of the same rule: deleting the stack
//! deletes the topics it created, and a topic SNS REFUSES to create leaves the stack
//! `CREATE_FAILED` naming the resource rather than publishing a dangling ARN. Drift
//! detection is the same rule read the other way: it asks SNS what the topic IS now, so a
//! topic changed out of band is `MODIFIED` and an untouched one `IN_SYNC`.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode};

/// The region every request here is signed for, and so the region the stack's topics and
/// their ARNs belong to. Deliberately NOT the emulator's default region: the ARN a stack
/// publishes carries the Region the stack was created in, and the call that creates the
/// topic has to be made in that same Region for SNS to mint the same ARN.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms)
/// to come due. A topic has no transient status of its own: SNS answers `CreateTopic` with
/// a topic that is usable at once.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// How many characters CloudFormation's generated-name suffix carries.
const GENERATED_SUFFIX_LEN: usize = 12;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
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
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one awsQuery operation of `service`, answered with the HTTP status and the XML
/// body. The form components are encoded by the test crate's one awsQuery encoder
/// ([`crate::query_wire::form_encode`]), so a template body full of `{`, `"` and `&`
/// survives the body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon SNS operation (API version 2010-03-31).
async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "sns", "2010-03-31", action, members).await
}

/// One SNS call that must be answered.
async fn sns_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = sns(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SNS {action}, got {status} with {body}"
    );
    body
}

/// Every `<tag>` text of an XML body, in document order, with the serializer's escaping
/// undone: the test crate's one repeated-element reader
/// ([`crate::query_wire::elements`]).
fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    elements(xml, tag)
}

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    elements(xml, tag).into_iter().next()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// A `GetTopicAttributes` answer's `Attributes`, as the map a client reads it as: each
/// `<entry>`'s `<key>` paired with its own `<value>`.
fn attributes_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "key")
        .into_iter()
        .zip(xml_texts(xml, "value"))
        .collect()
}

/// A `ListTagsForResource` answer's tags, as the map a client reads it as.
fn tags_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "Key")
        .into_iter()
        .zip(xml_texts(xml, "Value"))
        .collect()
}

/// The awsQuery error `<Code>` of a refusal, or `""` on a success envelope.
fn error_code(xml: &str) -> String {
    xml_text(xml, "Code").unwrap_or_default()
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
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
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// The ARN a topic named `name` carries in this emulator.
fn topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}")
}

/// Whether `name` is the `<stack>-<logical id>-<suffix>` name CloudFormation generates for
/// a resource the template does not name.
fn is_generated_name(name: &str, stack_name: &str, logical_id: &str) -> bool {
    let prefix = format!("{stack_name}-{logical_id}-");
    name.strip_prefix(&prefix).is_some_and(|suffix| {
        suffix.len() == GENERATED_SUFFIX_LEN
            && suffix
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    })
}

/// What the row says about one topic the template declares.
struct DeclaredTopic {
    /// The output key whose value is the topic's ARN (`{"Ref": <logical id>}`).
    arn_output: &'static str,
    /// The resource's logical id, which the generated name is built from.
    logical_id: &'static str,
    /// The name the template declares, or `None` where CloudFormation generates it.
    declared_name: Option<&'static str>,
    /// Attributes `GetTopicAttributes` must report for the created topic, computed from
    /// what the row's template declares.
    expected_attributes: &'static [(&'static str, &'static str)],
    /// Tags `ListTagsForResource` must report, computed from the declared `Tags`.
    expected_tags: &'static [(&'static str, &'static str)],
    /// `(Protocol, Endpoint)` pairs `ListSubscriptionsByTopic` must report, computed from
    /// the declared inline `Subscription` list.
    expected_subscriptions: &'static [(&'static str, &'static str)],
    /// The attribute an out-of-band `SetTopicAttributes` then writes on the created topic,
    /// and the value it writes — the ordinary way a client changes a topic a stack owns.
    out_of_band_write: (&'static str, &'static str),
}

/// One row: a template and the topics it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    topics: &'static [DeclaredTopic],
}

const ROWS: &[Row] = &[
    Row {
        case: "an unnamed topic with no properties, whose name CloudFormation generates",
        stack_name: "unnamed-topic-stack",
        template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},
             "Outputs":{"TopicArn":{"Value":{"Ref":"Topic"}}}}"#,
        topics: &[DeclaredTopic {
            arn_output: "TopicArn",
            logical_id: "Topic",
            declared_name: None,
            expected_attributes: &[],
            expected_tags: &[],
            expected_subscriptions: &[],
            out_of_band_write: ("DisplayName", "renamed out of band"),
        }],
    },
    Row {
        case: "a named topic whose properties are attributes written as strings, and tags",
        stack_name: "named-topic-stack",
        template: r#"{"Resources":{"Events":{"Type":"AWS::SNS::Topic",
               "Properties":{"TopicName":"stack-events-topic","DisplayName":"Stack events",
                 "TracingConfig":"Active","SignatureVersion":"2",
                 "KmsMasterKeyId":"alias/aws/sns",
                 "Tags":[{"Key":"team","Value":"platform"}]}}},
             "Outputs":{"EventsArn":{"Value":{"Ref":"Events"}},
               "EventsTopicArn":{"Value":{"Fn::GetAtt":["Events","TopicArn"]}},
               "EventsName":{"Value":{"Fn::GetAtt":["Events","TopicName"]}}}}"#,
        topics: &[DeclaredTopic {
            arn_output: "EventsArn",
            logical_id: "Events",
            declared_name: Some("stack-events-topic"),
            expected_attributes: &[
                ("DisplayName", "Stack events"),
                ("TracingConfig", "Active"),
                ("SignatureVersion", "2"),
                ("KmsMasterKeyId", "alias/aws/sns"),
            ],
            expected_tags: &[("team", "platform")],
            expected_subscriptions: &[],
            out_of_band_write: ("DisplayName", "Stack events, drifted"),
        }],
    },
    Row {
        case: "a FIFO topic, whose properties are booleans, a value set and an object",
        stack_name: "fifo-topic-stack",
        template: r#"{"Resources":{"Ordered":{"Type":"AWS::SNS::Topic",
               "Properties":{"TopicName":"stack-ordered-topic.fifo","FifoTopic":true,
                 "ContentBasedDeduplication":true,"FifoThroughputScope":"Topic",
                 "ArchivePolicy":{"MessageRetentionPeriod":30}}}},
             "Outputs":{"OrderedArn":{"Value":{"Ref":"Ordered"}}}}"#,
        topics: &[DeclaredTopic {
            arn_output: "OrderedArn",
            logical_id: "Ordered",
            declared_name: Some("stack-ordered-topic.fifo"),
            expected_attributes: &[
                ("FifoTopic", "true"),
                ("ContentBasedDeduplication", "true"),
                ("FifoThroughputScope", "Topic"),
                // The object the template declared, as the compact JSON string the
                // attribute is documented to carry.
                ("ArchivePolicy", r#"{"MessageRetentionPeriod":30}"#),
            ],
            expected_tags: &[],
            expected_subscriptions: &[],
            out_of_band_write: ("DisplayName", "ordered, drifted"),
        }],
    },
    Row {
        case: "two topics, one carrying an inline subscription list and the other a \
               per-protocol delivery-status logging config",
        stack_name: "wired-topic-stack",
        template: r#"{"Resources":{
               "Notified":{"Type":"AWS::SNS::Topic",
                 "Properties":{"TopicName":"stack-notified-topic","DisplayName":"Notified",
                   "Subscription":[{"Protocol":"https","Endpoint":"https://example.com/hook"},
                     {"Protocol":"email","Endpoint":"ops@example.com"}]}},
               "Logged":{"Type":"AWS::SNS::Topic",
                 "Properties":{"TopicName":"stack-logged-topic",
                   "DeliveryStatusLogging":[{"Protocol":"sqs",
                     "SuccessFeedbackRoleArn":"arn:aws:iam::000000000000:role/sns-logs",
                     "SuccessFeedbackSampleRate":50,
                     "FailureFeedbackRoleArn":"arn:aws:iam::000000000000:role/sns-logs"}]}}},
             "Outputs":{"NotifiedArn":{"Value":{"Ref":"Notified"}},
               "LoggedArn":{"Value":{"Ref":"Logged"}}}}"#,
        topics: &[
            DeclaredTopic {
                arn_output: "NotifiedArn",
                logical_id: "Notified",
                declared_name: Some("stack-notified-topic"),
                expected_attributes: &[("DisplayName", "Notified")],
                expected_tags: &[],
                expected_subscriptions: &[
                    ("https", "https://example.com/hook"),
                    ("email", "ops@example.com"),
                ],
                out_of_band_write: ("DisplayName", "Notified, drifted"),
            },
            DeclaredTopic {
                arn_output: "LoggedArn",
                logical_id: "Logged",
                declared_name: Some("stack-logged-topic"),
                // The `LoggingConfig` members under the protocol's own prefix, which is
                // how SNS names its delivery-status attributes; the sample rate was
                // written as a number and is the text it is.
                expected_attributes: &[
                    (
                        "SQSSuccessFeedbackRoleArn",
                        "arn:aws:iam::000000000000:role/sns-logs",
                    ),
                    ("SQSSuccessFeedbackSampleRate", "50"),
                    (
                        "SQSFailureFeedbackRoleArn",
                        "arn:aws:iam::000000000000:role/sns-logs",
                    ),
                ],
                expected_tags: &[],
                expected_subscriptions: &[],
                out_of_band_write: ("SQSSuccessFeedbackSampleRate", "100"),
            },
        ],
    },
];

#[tokio::test]
async fn a_completed_stack_has_created_every_topic_it_declares_in_sns() {
    let router = build_router();

    for row in ROWS {
        let body = create_and_settle(&router, row.case, row.stack_name, row.template).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to complete, got {body}",
            row.case
        );
        let outputs = outputs_of(&body);

        for topic in row.topics {
            let case = format!("{} [{}]", row.case, topic.logical_id);
            let arn = outputs
                .get(topic.arn_output)
                .unwrap_or_else(|| panic!("case {case:?}: no {} output in {body}", topic.arn_output))
                .clone();

            // The name the stack gave the topic: the declared one, or CloudFormation's
            // generated `<stack>-<logical id>-<suffix>`.
            let name = match topic.declared_name {
                Some(declared) => {
                    assert_eq!(
                        arn,
                        topic_arn(declared),
                        "case {case:?}: expected the Ref to be the declared topic's ARN"
                    );
                    declared.to_string()
                }
                None => {
                    let name = arn.rsplit(':').next().unwrap_or_default().to_string();
                    assert_eq!(
                        arn,
                        topic_arn(&name),
                        "case {case:?}: expected a topic ARN of this account and Region, \
                         got {arn}"
                    );
                    assert!(
                        is_generated_name(&name, row.stack_name, topic.logical_id),
                        "case {case:?}: expected a generated <stack>-<logical id>-<suffix> \
                         name, got {name:?}"
                    );
                    name
                }
            };

            // SNS itself holds a topic at the ARN the stack published, and reports that
            // same ARN back: the two halves agree about which topic exists.
            let read = sns_ok(&router, &case, "GetTopicAttributes", &[("TopicArn", &arn)]).await;
            let attributes = attributes_of(&read);
            assert_eq!(
                attributes.get("TopicArn").map(String::as_str),
                Some(arn.as_str()),
                "case {case:?}: expected the created topic's own ARN, got {attributes:?}"
            );
            assert_eq!(
                attributes.get("Owner").map(String::as_str),
                Some(ACCOUNT_ID),
                "case {case:?}: expected the topic to be owned by this account, got \
                 {attributes:?}"
            );

            // The configuration the template declared is the configuration the topic has.
            for (attribute, value) in topic.expected_attributes {
                assert_eq!(
                    attributes.get(*attribute).map(String::as_str),
                    Some(*value),
                    "case {case:?}: expected {attribute}={value}, got {attributes:?}"
                );
            }

            let listed = sns_ok(
                &router,
                &case,
                "ListTagsForResource",
                &[("ResourceArn", &arn)],
            )
            .await;
            let tags = tags_of(&listed);
            // The tags the template declared, PLUS the three CloudFormation records on
            // every resource it provisions: `aws:cloudformation:stack-name`,
            // `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id` (these rows
            // supply no stack-level `Tags`, which would be here too). The whole set is held
            // to by
            // `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/stack_tag_propagation.rs`.
            let mut expected_tags: BTreeMap<String, String> = topic
                .expected_tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            expected_tags.insert(
                "aws:cloudformation:stack-name".to_string(),
                row.stack_name.to_string(),
            );
            expected_tags.insert(
                "aws:cloudformation:stack-id".to_string(),
                xml_text(&body, "StackId").unwrap_or_default(),
            );
            expected_tags.insert(
                "aws:cloudformation:logical-id".to_string(),
                topic.logical_id.to_string(),
            );
            assert_eq!(
                tags, expected_tags,
                "case {case:?}: expected the declared tags beside the aws:cloudformation \
                 ones, got {tags:?}"
            );

            let listed = sns_ok(
                &router,
                &case,
                "ListSubscriptionsByTopic",
                &[("TopicArn", &arn)],
            )
            .await;
            let subscriptions: Vec<(String, String)> = xml_texts(&listed, "Protocol")
                .into_iter()
                .zip(xml_texts(&listed, "Endpoint"))
                .collect();
            let expected: Vec<(String, String)> = topic
                .expected_subscriptions
                .iter()
                .map(|(protocol, endpoint)| ((*protocol).to_string(), (*endpoint).to_string()))
                .collect();
            assert_eq!(
                subscriptions, expected,
                "case {case:?}: expected the subscriptions the template declared, got \
                 {subscriptions:?}"
            );

            // And the write a client changes a topic with lands on the topic the stack
            // published — the out-of-band mutation a CloudFormation drift scenario is
            // built on. A topic whose ARN SNS did not hold would refuse this as NotFound.
            let (attribute, value) = topic.out_of_band_write;
            sns_ok(
                &router,
                &case,
                "SetTopicAttributes",
                &[
                    ("TopicArn", &arn),
                    ("AttributeName", attribute),
                    ("AttributeValue", value),
                ],
            )
            .await;
            let read = sns_ok(&router, &case, "GetTopicAttributes", &[("TopicArn", &arn)]).await;
            assert_eq!(
                attributes_of(&read).get(attribute).map(String::as_str),
                Some(value),
                "case {case:?}: expected the out-of-band {attribute} to be readable as \
                 {value:?}, got {read}"
            );

            // The stack still reports the resource it created, under the same physical id:
            // a write through SNS changes the topic, not the stack's record of it.
            let (status, described) = cloudformation(
                &router,
                "DescribeStackResources",
                &[("StackName", row.stack_name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} \
                 with {described}"
            );
            assert!(
                xml_texts(&described, "PhysicalResourceId").contains(&arn),
                "case {case:?}: expected the stack to still report {name} at {arn}, got \
                 {described}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_topics_it_created() {
    let router = build_router();
    let case = "two topics deleted with their stack";
    let stack_name = "deleted-topic-stack";
    let template = r#"{"Resources":{
           "Kept":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"deleted-stack-kept"}},
           "Other":{"Type":"AWS::SNS::Topic"}},
         "Outputs":{"KeptArn":{"Value":{"Ref":"Kept"}},
           "OtherArn":{"Value":{"Ref":"Other"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    let arns: Vec<String> = ["KeptArn", "OtherArn"]
        .iter()
        .map(|key| {
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                .clone()
        })
        .collect();
    for arn in &arns {
        sns_ok(&router, case, "GetTopicAttributes", &[("TopicArn", arn)]).await;
    }

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    // Every topic the stack created is gone from SNS, which is what makes the stack's
    // resources the stack's own rather than leftovers.
    for arn in &arns {
        let (status, answer) = sns(&router, "GetTopicAttributes", &[("TopicArn", arn)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the topic at {arn} to be gone, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "NotFound",
            "case {case:?}: expected NotFound for {arn}, got {answer}"
        );
    }
}

/// Drift detection reads the topic back off Amazon SNS, so a topic whose declared
/// configuration was changed OUT OF BAND — the ordinary way a client changes a topic, and
/// the way drift is induced — is reported `MODIFIED`, while an untouched one of the same
/// stack is `IN_SYNC`. This is the whole scenario this issue's fixture builds: the write
/// lands in SNS, and CloudFormation's detection sees it there.
#[tokio::test]
async fn a_topic_changed_out_of_band_is_detected_as_drifted() {
    let router = build_router();
    let case = "a declared DisplayName changed through SetTopicAttributes";
    let stack_name = "drifted-topic-stack";
    let template = r#"{"Resources":{
           "Drifted":{"Type":"AWS::SNS::Topic",
             "Properties":{"TopicName":"drifted-stack-topic","DisplayName":"as declared"}},
           "Untouched":{"Type":"AWS::SNS::Topic",
             "Properties":{"TopicName":"untouched-stack-topic","DisplayName":"as declared"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );

    sns_ok(
        &router,
        case,
        "SetTopicAttributes",
        &[
            ("TopicArn", &topic_arn("drifted-stack-topic")),
            ("AttributeName", "DisplayName"),
            ("AttributeValue", "changed out of band"),
        ],
    )
    .await;

    let (status, body) =
        cloudformation(&router, "DetectStackDrift", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DetectStackDrift, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    let (status, body) =
        cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "StackDriftStatus").as_deref(),
        Some("DRIFTED"),
        "case {case:?}: expected the stack to report the drift its topic carries, got {body}"
    );

    // Per resource: the one that was written to is MODIFIED, the one nothing touched is
    // IN_SYNC — read off SNS, not off CloudFormation's record of what it applied.
    let (status, body) = cloudformation(
        &router,
        "DescribeStackResources",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with {body}"
    );
    let reported: BTreeMap<String, String> = xml_texts(&body, "LogicalResourceId")
        .into_iter()
        .zip(xml_texts(&body, "StackResourceDriftStatus"))
        .collect();
    for (logical_id, expected) in [("Drifted", "MODIFIED"), ("Untouched", "IN_SYNC")] {
        assert_eq!(
            reported.get(logical_id).map(String::as_str),
            Some(expected),
            "case {case:?}: expected {logical_id} to be {expected}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_topic_sns_refuses_to_create_fails_the_stack() {
    let router = build_router();
    let case = "a FIFO topic whose name does not carry the documented .fifo suffix";
    let stack_name = "refused-topic-stack";
    // "For a FIFO (first-in-first-out) topic, the name must end with the .fifo suffix" —
    // a request SNS's own CreateTopic refuses, which is what the stack must report rather
    // than completing.
    let template = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
           "Properties":{"TopicName":"refused-stack-topic","FifoTopic":true}}},
         "Outputs":{"TopicArn":{"Value":{"Ref":"Topic"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_FAILED"),
        "case {case:?}: expected the stack to fail rather than publish a topic ARN nothing \
         answers to, got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Topic") && reason.contains("InvalidParameter"),
        "case {case:?}: expected the reason to name the resource and SNS's own refusal, got \
         {reason:?}"
    );

    // And nothing was created: the ARN the template asked for resolves to no topic.
    let (status, answer) = sns(
        &router,
        "GetTopicAttributes",
        &[("TopicArn", &topic_arn("refused-stack-topic"))],
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected no topic of that name, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "NotFound",
        "case {case:?}: expected NotFound, got {answer}"
    );
}

/// A row of the refusal half: the resource an `AWS::SNS::Topic` declares is the topic AND
/// the subscriptions declared on it, so a subscription SNS refuses fails the resource — and
/// leaves nothing behind, since the stack records no physical id it could later delete.
#[tokio::test]
async fn a_subscription_sns_refuses_fails_the_stack_and_leaves_no_topic() {
    let router = build_router();
    let case = "an https subscription whose endpoint is not a url";
    let stack_name = "refused-subscription-stack";
    // "The endpoint that you want to receive notifications" — for the `https` protocol
    // "the endpoint is a URL beginning with https://", which this is not, so SNS's own
    // Subscribe refuses it.
    let template = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
           "Properties":{"TopicName":"refused-subscription-topic",
             "Subscription":[{"Protocol":"https","Endpoint":"not-a-url"}]}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_FAILED"),
        "case {case:?}: expected the stack to fail rather than report a topic whose \
         declared subscription does not exist, got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Topic") && reason.contains("InvalidParameter"),
        "case {case:?}: expected the reason to name the resource and SNS's own refusal, got \
         {reason:?}"
    );

    // And the topic the create had already made is gone: a resource the stack reports as
    // failed carries no physical id, so nothing else would ever delete it.
    let (status, answer) = sns(
        &router,
        "GetTopicAttributes",
        &[("TopicArn", &topic_arn("refused-subscription-topic"))],
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the half-created topic to have been deleted, got {status} \
         with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "NotFound",
        "case {case:?}: expected NotFound, got {answer}"
    );
}

/// The other side of the same rule: a resource this emulator really created is never
/// reported as reconfigured by something that did not reconfigure it. An update that
/// changes a created topic's properties IN PLACE leaves the stack `UPDATE_FAILED` naming
/// the resource, and the topic keeps the configuration it was created with.
#[tokio::test]
async fn an_update_that_would_change_a_created_topic_in_place_fails_the_stack() {
    let router = build_router();
    let case = "a declared DisplayName changed by an update";
    let stack_name = "updated-topic-stack";
    let template = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
           "Properties":{"TopicName":"updated-stack-topic","DisplayName":"before"}}}}"#;
    let next = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
           "Properties":{"TopicName":"updated-stack-topic","DisplayName":"after"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );

    let (status, body) = cloudformation(
        &router,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", next)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_FAILED"),
        "case {case:?}: expected the update to fail rather than report a topic nothing \
         reconfigured, got {body}"
    );

    // The topic is still the one the create made, with the display name it was created
    // with: a failed update changed nothing in SNS.
    let read = sns_ok(
        &router,
        case,
        "GetTopicAttributes",
        &[("TopicArn", &topic_arn("updated-stack-topic"))],
    )
    .await;
    assert_eq!(
        attributes_of(&read).get("DisplayName").map(String::as_str),
        Some("before"),
        "case {case:?}: expected the created topic's own DisplayName, got {read}"
    );
}
