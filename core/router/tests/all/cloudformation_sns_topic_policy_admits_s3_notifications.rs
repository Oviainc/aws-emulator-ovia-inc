// SPDX-License-Identifier: CC0-1.0

//! A stack's topic-policy resource is really the topic's policy, and that document is what
//! decides whether Amazon S3 may publish a bucket's notifications to the topic.
//!
//! The rule under test: **a template resource whose subject is another resource's POLICY is
//! deployed by writing that policy through the owning service's own call, so once the stack
//! reports `CREATE_COMPLETE` that service reports the DECLARED document in place of the
//! default the resource was created with — and every decision the document governs is made
//! from it.** For `AWS::SNS::TopicPolicy` (and its single-topic sibling
//! `AWS::SNS::TopicInlinePolicy`) the subject is a topic's `Policy` attribute, which Amazon
//! SNS reports through `GetTopicAttributes`; the decision it governs here is Amazon S3's,
//! which validates every destination of a `NotificationConfiguration` against the
//! destination's own resource policy and refuses the whole put when it grants S3 nothing.
//!
//! Three crates have to agree for that to hold — CloudFormation makes the attribute write,
//! SNS serves it and holds the document, S3 reads it to decide its own call — so the suite
//! lives here: a crate-local suite can only ever see one side of it. The CloudFormation
//! crate's own stack suites pin the provisioning of a topic, and the S3 crate's
//! `notification_destination_validation` suite pins the validation over the destination
//! kinds and grants; what only this suite can reach is the two halves meeting.
//!
//! The rows differ along what the rule generalises over: WHICH property carries the topics
//! (the list `Topics` of `AWS::SNS::TopicPolicy`, the single `TopicArn` of
//! `AWS::SNS::TopicInlinePolicy`), HOW MANY topics one policy resource names, how the
//! document SCOPES the grant (conditioned on the publishing bucket's ARN, or unscoped), and
//! whether the document is declared as an inline object or as its JSON text. Each row also
//! carries a control topic the same stack creates with NO policy resource, so what is being
//! read is the deployed document and not a default that happens to admit everything.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode};

/// The Region every request here is signed for, and so the Region the stack's topics, their
/// ARNs and the bucket belong to.
const REGION: &str = "us-west-2";

/// Long enough for the stack's clock-owed transition to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// The XML namespace a client's S3 payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The service principal Amazon S3 publishes notifications under, and the action it
/// performs on a topic destination — what a topic policy has to grant for a
/// `TopicConfiguration` naming that topic to be accepted.
const S3_PRINCIPAL: &str = "s3.amazonaws.com";
const PUBLISH_ACTION: &str = "sns:Publish";

/// The condition key a grant narrowed to ONE publishing bucket names it by.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The code and message S3 answers a container whose destination it could not validate
/// with, and the per-destination detail it carries for a topic whose policy grants S3
/// nothing.
const INVALID_ARGUMENT: &str = "InvalidArgument";
const UNABLE_TO_VALIDATE: &str = "Unable to validate the following destination configurations";
const TOPIC_UNPERMITTED: &str = "Permissions on the destination topic do not allow S3 to \
                                 publish notifications from this bucket";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
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
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// Call one awsQuery operation of `service`, answered with the HTTP status and the XML body.
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

/// One CloudFormation call (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// One Amazon SNS call (API version 2010-03-31).
async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "sns", "2010-03-31", action, members).await
}

/// One response of S3's REST surface: the status and the body as text.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The text of the first `<tag>` element the body carries, or the empty string.
    fn element(&self, tag: &str) -> String {
        elements(&self.body, tag).into_iter().next().unwrap_or_default()
    }
}

/// Dispatch one request against S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// `PUT /<bucket>` — S3's `CreateBucket`, in the Region every call here is signed for.
async fn create_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let answer = s3(
        router,
        "PUT",
        &format!("/{bucket}"),
        &format!(
            "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        ),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `PUT /<bucket>?notification` with one `TopicConfiguration` naming `topic_arn`.
async fn put_topic_notification(
    router: &MultiServiceRouter,
    bucket: &str,
    topic_arn: &str,
) -> Answer {
    let body = format!(
        "<NotificationConfiguration xmlns=\"{S3_XMLNS}\"><TopicConfiguration>\
         <Topic>{topic_arn}</Topic><Event>s3:ObjectCreated:*</Event>\
         </TopicConfiguration></NotificationConfiguration>"
    );
    s3(router, "PUT", &format!("/{bucket}?notification"), &body).await
}

/// `GET /<bucket>?notification` — `GetBucketNotificationConfiguration`.
async fn get_notification(router: &MultiServiceRouter, bucket: &str) -> Answer {
    s3(router, "GET", &format!("/{bucket}?notification"), "").await
}

/// The `Policy` attribute the topic `arn` names carries now, read from Amazon SNS itself,
/// parsed as the document it is — or `None` when the topic carries none.
async fn topic_policy(router: &MultiServiceRouter, case: &str, arn: &str) -> Option<Value> {
    let (status, body) = sns(router, "GetTopicAttributes", &[("TopicArn", arn)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SNS GetTopicAttributes, got {status} with \
         {body}"
    );
    let keys = elements(&body, "key");
    let values = elements(&body, "value");
    let document = keys
        .into_iter()
        .zip(values)
        .find(|(key, _)| key == "Policy")
        .map(|(_, value)| value)?;
    if document.is_empty() {
        return None;
    }
    Some(
        serde_json::from_str(&document).unwrap_or_else(|error| {
            panic!("case {case:?}: the reported policy must be a document ({error}): {document}")
        }),
    )
}

/// The statement a row's policy resource declares: the grant S3 needs on the topic, scoped
/// to the publishing bucket when `scoped` asks for it.
///
/// `aws:SourceArn` is the key such a grant is narrowed by, and an S3 bucket's ARN is
/// `arn:aws:s3:::<bucket>` — no Region or account, because a bucket name is globally unique.
fn declared_statement(topics: &[String], bucket: &str, scoped: bool) -> Value {
    let mut statement = json!({
        "Sid": "AllowS3Publish",
        "Effect": "Allow",
        "Principal": {"Service": S3_PRINCIPAL},
        "Action": PUBLISH_ACTION,
        "Resource": topics,
    });
    if scoped {
        statement["Condition"] =
            json!({"ArnLike": {SOURCE_ARN_KEY: format!("arn:aws:s3:::{bucket}")}});
    }
    statement
}

/// The whole document those statements make.
fn declared_document(topics: &[String], bucket: &str, scoped: bool) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [declared_statement(topics, bucket, scoped)],
    })
}

/// Which property of the declaration carries the topics, which is the one thing the two
/// policy resource types differ in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// `AWS::SNS::TopicPolicy`, whose `Topics` is a list.
    TopicsList,
    /// `AWS::SNS::TopicInlinePolicy`, whose `TopicArn` is one topic.
    OneTopicArn,
}

impl Carrier {
    fn resource_type(self) -> &'static str {
        match self {
            Self::TopicsList => "AWS::SNS::TopicPolicy",
            Self::OneTopicArn => "AWS::SNS::TopicInlinePolicy",
        }
    }
}

/// One row of the table.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    bucket: &'static str,
    /// The topics the policy resource applies its document to, by name.
    granted: &'static [&'static str],
    /// The topic the same stack creates with NO policy resource.
    ungranted: &'static str,
    carrier: Carrier,
    /// Whether the declared grant is conditioned on the publishing bucket.
    scoped: bool,
    /// Whether the document is declared as its JSON TEXT rather than as an inline object.
    as_text: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "one topic granted by a Topics list, scoped to the bucket",
        stack_name: "topic-policy-one",
        bucket: "topic-policy-one-bucket",
        granted: &["topic-policy-one-events"],
        ungranted: "topic-policy-one-control",
        carrier: Carrier::TopicsList,
        scoped: true,
        as_text: false,
    },
    Row {
        case: "two topics granted by one unscoped Topics list",
        stack_name: "topic-policy-two",
        bucket: "topic-policy-two-bucket",
        granted: &["topic-policy-two-events", "topic-policy-two-audit"],
        ungranted: "topic-policy-two-control",
        carrier: Carrier::TopicsList,
        scoped: false,
        as_text: false,
    },
    Row {
        case: "one topic granted by the inline policy type, declared as JSON text",
        stack_name: "topic-policy-inline",
        bucket: "topic-policy-inline-bucket",
        granted: &["topic-policy-inline-events"],
        ungranted: "topic-policy-inline-control",
        carrier: Carrier::OneTopicArn,
        scoped: true,
        as_text: true,
    },
];

/// The ARN a topic named `name` carries in this emulator: the account id every ARN it
/// mints carries ([`engine::emulator_account_id`]) in the Region these calls are signed
/// for.
fn topic_arn(name: &str) -> String {
    format!(
        "arn:aws:sns:{REGION}:{}:{name}",
        engine::emulator_account_id()
    )
}

/// The template a row declares: its topics, the policy resource over the granted ones, and
/// the control topic no policy resource names.
fn template(row: &Row) -> String {
    let arns: Vec<String> = row.granted.iter().map(|name| topic_arn(name)).collect();
    let document = declared_document(&arns, row.bucket, row.scoped);
    let declared = if row.as_text {
        Value::String(document.to_string())
    } else {
        document
    };
    let mut resources = serde_json::Map::new();
    for (index, name) in row.granted.iter().enumerate() {
        resources.insert(
            format!("Granted{index}"),
            json!({"Type": "AWS::SNS::Topic", "Properties": {"TopicName": name}}),
        );
    }
    resources.insert(
        "Control".to_string(),
        json!({"Type": "AWS::SNS::Topic", "Properties": {"TopicName": row.ungranted}}),
    );
    let mut properties = serde_json::Map::new();
    properties.insert("PolicyDocument".to_string(), declared);
    match row.carrier {
        Carrier::TopicsList => {
            properties.insert("Topics".to_string(), json!(arns));
        }
        Carrier::OneTopicArn => {
            properties.insert("TopicArn".to_string(), json!(arns[0]));
        }
    }
    resources.insert(
        "Policy".to_string(),
        json!({"Type": row.carrier.resource_type(), "Properties": Value::Object(properties)}),
    );
    json!({"Resources": Value::Object(resources)}).to_string()
}

/// Create the stack and wait past its transition, answering the `DescribeStacks` body.
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
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
    );
    body
}

#[tokio::test]
async fn a_deployed_topic_policy_is_the_topics_policy_and_admits_s3_notifications() {
    for row in ROWS {
        let case = row.case;
        let router = build_router();
        create_bucket(&router, case, row.bucket).await;

        let body = create_and_settle(&router, case, row.stack_name, &template(row)).await;
        assert_eq!(
            elements(&body, "StackStatus").into_iter().next().as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );

        // Every topic the policy resource names carries the DECLARED document, which is
        // what the stack deployed rather than the default a topic is created with.
        let arns: Vec<String> = row.granted.iter().map(|name| topic_arn(name)).collect();
        let expected = declared_document(&arns, row.bucket, row.scoped);
        let named: Vec<&String> = match row.carrier {
            Carrier::TopicsList => arns.iter().collect(),
            Carrier::OneTopicArn => vec![&arns[0]],
        };
        for arn in &named {
            let reported = topic_policy(&router, case, arn).await;
            assert_eq!(
                reported.as_ref(),
                Some(&expected),
                "case {case:?}: expected {arn} to carry the deployed policy {expected}, got \
                 {reported:?}"
            );
        }

        // And that document is what decides S3's own call: the configuration naming the
        // granted topic is accepted and reads back, …
        let granted = &arns[0];
        let accepted = put_topic_notification(&router, row.bucket, granted).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: a TopicConfiguration naming the granted topic must be accepted, \
             got {} with {}",
            accepted.status, accepted.body
        );
        let read = get_notification(&router, row.bucket).await;
        assert_eq!(
            read.element("Topic"),
            *granted,
            "case {case:?}: expected the stored configuration to name {granted}, got {}",
            read.body
        );

        // … while the control topic the same stack created, which no policy resource
        // names, carries the policy it was created with and grants Amazon S3 nothing: the
        // identical configuration is refused whole, naming that destination and the reason.
        let control = topic_arn(row.ungranted);
        let refused = put_topic_notification(&router, row.bucket, &control).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a TopicConfiguration naming the ungranted topic must be refused, \
             got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.element("Code"),
            INVALID_ARGUMENT,
            "case {case:?}: expected the refusal to carry {INVALID_ARGUMENT}, got {}",
            refused.body
        );
        assert_eq!(
            refused.element("Message"),
            UNABLE_TO_VALIDATE,
            "case {case:?}: expected the refusal's message to be {UNABLE_TO_VALIDATE:?}, got {}",
            refused.body
        );
        assert_eq!(
            (refused.element("ArgumentName1"), refused.element("ArgumentValue1")),
            (control.clone(), TOPIC_UNPERMITTED.to_string()),
            "case {case:?}: expected the refusal to name the destination and its reason, got {}",
            refused.body
        );
        // Refused WHOLE: the configuration installed before it is untouched.
        let read = get_notification(&router, row.bucket).await;
        assert_eq!(
            read.element("Topic"),
            *granted,
            "case {case:?}: a refused put records nothing, so the accepted configuration must \
             still be reported, got {}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_changed_topic_policy_declaration_reaches_the_topic() {
    let case = "a deployed grant narrowed to another bucket by an update";
    let router = build_router();
    let bucket = "topic-policy-updated-bucket";
    let other_bucket = "topic-policy-other-bucket";
    let stack_name = "topic-policy-updated";
    let topic = "topic-policy-updated-events";
    let arns = vec![topic_arn(topic)];

    create_bucket(&router, case, bucket).await;

    let declared = |scoped_to: &str| {
        json!({
            "Resources": {
                "Granted": {
                    "Type": "AWS::SNS::Topic",
                    "Properties": {"TopicName": topic},
                },
                "Policy": {
                    "Type": "AWS::SNS::TopicPolicy",
                    "Properties": {
                        "Topics": json!(arns),
                        "PolicyDocument": declared_document(&arns, scoped_to, true),
                    },
                },
            }
        })
        .to_string()
    };

    let body = create_and_settle(&router, case, stack_name, &declared(bucket)).await;
    assert_eq!(
        elements(&body, "StackStatus").into_iter().next().as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let accepted = put_topic_notification(&router, bucket, &arns[0]).await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: the configuration must be accepted while the grant names this \
         bucket, got {} with {}",
        accepted.status, accepted.body
    );

    // The update REVALUES the policy, so the topic reports the new document …
    let (status, body) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &declared(other_bucket)),
        ],
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
        elements(&body, "StackStatus").into_iter().next().as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the in-place update to complete, got {body}"
    );
    let expected = declared_document(&arns, other_bucket, true);
    let reported = topic_policy(&router, case, &arns[0]).await;
    assert_eq!(
        reported.as_ref(),
        Some(&expected),
        "case {case:?}: expected the updated policy {expected}, got {reported:?}"
    );

    // … and the decision it governs moves with it: the same configuration, on the bucket
    // the grant no longer names, is refused.
    create_bucket(&router, case, "topic-policy-updated-second").await;
    let refused = put_topic_notification(&router, "topic-policy-updated-second", &arns[0]).await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: the configuration must be refused for a bucket the deployed grant \
         does not name, got {} with {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.element("ArgumentValue1"),
        TOPIC_UNPERMITTED,
        "case {case:?}: expected the refusal to state why the destination failed, got {}",
        refused.body
    );
}
