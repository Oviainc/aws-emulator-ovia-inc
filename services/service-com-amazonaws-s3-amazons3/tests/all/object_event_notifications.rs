// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for S3's EVENT NOTIFICATIONS: what a bucket's stored notification
//! configuration DOES when an object changes.
//!
//! `PutBucketNotificationConfiguration` records "the event types that you want Amazon S3
//! to publish and the destination where you want Amazon S3 to publish an event
//! notification when it detects an event of the specified type"
//! (`PutBucketNotificationConfiguration`'s own documentation). The rule under test is
//! that half of it:
//!
//! * a destination configuration is notified of an object change when its `Events` list
//!   names that change's event value — either literally or through the `s3:<Type>:*`
//!   form, which stands for every event of that type — and when the object's key passes
//!   every `prefix` and `suffix` rule of the configuration's `Filter`;
//! * with an `EventBridgeConfiguration` on the bucket, which "enables delivery of events
//!   to Amazon EventBridge" (`NotificationConfiguration$EventBridgeConfiguration`), the
//!   same change is published onto the account's default event bus under the `aws.s3`
//!   source and that event class's detail-type, so a rule matching it reaches its own
//!   targets;
//! * every notification NAMES THE CALL it came of, by the `x-amz-request-id` /
//!   `x-amz-id-2` pair that call's own response carried, so a consumer can tell which API
//!   call produced which event;
//! * and a change no configuration names is delivered nowhere.
//!
//! Every assertion drives real HTTP requests through a router carrying S3 beside the
//! three services a notification is delivered to, so what is pinned is the wire
//! behaviour — the message a consumer receives — and not any handler's internals. The
//! queue destination is the observable one (`ReceiveMessage` reports what was delivered);
//! the Lambda destination is read back out of the Lambda emulator's own state, because
//! no modelled Lambda operation reports an accepted asynchronous invocation.
//!
//! The table spans the class the rule generalises over: the event (a write, a delete, a
//! tag set), how the configuration names it (the value itself, the type wildcard, a list
//! that names another type), the key filter (a prefix, a suffix, a key that fails one),
//! the key's own form (plain, prefixed, carrying a space) and whether EventBridge
//! delivery is enabled.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and so the Region the buckets live in
/// and the one every destination ARN names.
///
/// `pub(crate)`, like the harness below it: the multipart half of this same rule
/// ([`super::multipart_upload_event_notifications`]) drives its requests and reads its
/// deliveries through these rather than standing up a second router of its own.
pub(crate) const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The XML namespace a client puts on an S3 request payload.
pub(crate) const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The `Id` every queue configuration below carries, which a delivered record reports as
/// its `configurationId`.
pub(crate) const CONFIGURATION_ID: &str = "delivery";

/// How long a wait for a delivery lasts before giving up. A delivery is made on a task of
/// its own as soon as the operation that produced it is answered, so this is many
/// deliveries' worth of slack and not a span the behaviour depends on.
const DELIVERY_BUDGET: Duration = Duration::from_secs(10);

/// How long a wait lasts before concluding that NOTHING was delivered.
const QUIET_BUDGET: Duration = Duration::from_millis(1_500);

/// A well-formed Lambda execution role ARN, satisfying that model's `RoleArn` pattern.
///
/// `pub(crate)`: the destination-validation suite
/// ([`super::notification_destination_validation`]) creates its functions with the same
/// role and package rather than spelling a second pair of its own.
pub(crate) const LAMBDA_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A deployment package's bytes, base64-encoded as `FunctionCode$ZipFile` carries them.
/// Nothing runs it: what this suite asserts is the invocation S3 made, which carries the
/// event, not the package.
pub(crate) const PACKAGE_ZIP_FILE: &str = "ZGVwbG95bWVudC1wYWNrYWdl";

/// S3, EventBridge, SQS and Lambda over one shared engine, reached exactly the way the
/// production router reaches them.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// What a dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as text, for a failure message.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// A response header's value — among them the entity tag an uploaded part is
    /// recorded with.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The `x-amz-request-id` / `x-amz-id-2` pair this response named its request by —
    /// "a value created by Amazon S3 that uniquely identifies the request" and "a special
    /// token that helps Amazon Web Services troubleshoot problems" (Amazon S3 API
    /// Reference, *Common Response Headers*), which every S3 response carries.
    pub(crate) fn correlation(&self, case: &str) -> (String, String) {
        let request_id = self
            .header("x-amz-request-id")
            .unwrap_or_else(|| panic!("case {case:?}: the response carried no x-amz-request-id"));
        let extended = self
            .header("x-amz-id-2")
            .unwrap_or_else(|| panic!("case {case:?}: the response carried no x-amz-id-2"));
        (request_id, extended)
    }
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// Dispatch one request, signed for `signing_name` so the router resolves the service
    /// the way it does in production. Neither the key id nor the signature is a
    /// credential: this harness turns request authentication off, exactly as the emulator
    /// runs it unless started with authentication on.
    pub(crate) async fn send(
        &self,
        signing_name: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host, Signature=not-verified"
        );
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("authorization", authorization);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }

    /// One S3 request carrying an XML (or empty) payload.
    pub(crate) async fn s3_xml(&self, method: &str, uri: &str, body: &str) -> Answer {
        self.send(
            "s3",
            method,
            uri,
            &[("content-type", "application/xml")],
            body.as_bytes().to_vec(),
        )
        .await
    }

    /// One RPC call: `POST /` carrying `X-Amz-Target`, required to answer 200.
    async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> Value {
        let content_type = format!("application/x-amz-json-{json_version}");
        let target = format!("{target_prefix}.{operation}");
        let answer = self
            .send(
                signing_name,
                "POST",
                "/",
                &[
                    ("content-type", content_type.as_str()),
                    ("x-amz-target", target.as_str()),
                ],
                body.to_string().into_bytes(),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "{operation}: expected HTTP 200, got {} with {}",
            answer.status,
            answer.text()
        );
        if answer.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&answer.body).unwrap_or(Value::Null)
    }

    /// An EventBridge call (`awsJson1_1`).
    ///
    /// `pub(crate)`: the tagging sub-type suite
    /// ([`super::object_tagging_event_sub_types`]) puts its own rules on the default bus
    /// through this one rather than building a second EventBridge request.
    pub(crate) async fn events(&self, operation: &str, body: &Value) -> Value {
        self.rpc("events", "AWSEvents", "1.1", operation, body).await
    }

    /// An SQS call (`awsJson1_0`).
    ///
    /// `pub(crate)`: the sequencer suite ([`super::object_event_sequencer_order`]) takes a
    /// whole fan-out off a queue in one receive rather than one message at a time.
    pub(crate) async fn sqs(&self, operation: &str, body: &Value) -> Value {
        self.rpc("sqs", "AmazonSQS", "1.0", operation, body).await
    }

    /// `PUT /<bucket>` — `CreateBucket`, with the Object Lock header when the row's
    /// change needs a bucket that has an Object Lock configuration
    /// (`CreateBucketRequest$ObjectLockEnabledForBucket`).
    async fn create_bucket_of(&self, bucket: &str, kind: BucketKind) {
        let headers: &[(&str, &str)] = match kind {
            BucketKind::ObjectLock => &[("x-amz-bucket-object-lock-enabled", "true")],
            BucketKind::Plain | BucketKind::Versioned => &[],
        };
        let answer = self
            .send("s3", "PUT", &format!("/{bucket}"), headers, Vec::new())
            .await;
        assert_eq!(
            answer.status,
            200,
            "creating {bucket} must succeed, got {} with {}",
            answer.status,
            answer.text()
        );
        if kind == BucketKind::Versioned {
            let body = format!(
                "<VersioningConfiguration xmlns=\"{XMLNS}\"><Status>Enabled</Status>\
                 </VersioningConfiguration>"
            );
            let enabled = self
                .s3_xml("PUT", &format!("/{bucket}?versioning"), &body)
                .await;
            assert_eq!(
                enabled.status,
                200,
                "enabling versioning on {bucket} must succeed, got {} with {}",
                enabled.status,
                enabled.text()
            );
        }
    }

    /// `PUT /<bucket>` — `CreateBucket`.
    pub(crate) async fn create_bucket(&self, bucket: &str) {
        self.create_bucket_of(bucket, BucketKind::Plain).await;
    }

    /// `PUT /<bucket>/<key>` — `PutObject`, with `body` as the object's bytes. Answers
    /// what the write reported, so a row can compare the notification against the call.
    pub(crate) async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) -> Answer {
        let answer = self
            .send(
                "s3",
                "PUT",
                &format!("/{bucket}/{}", request_key(key)),
                &[("content-type", "text/plain")],
                body.to_vec(),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "writing {key:?} must succeed, got {} with {}",
            answer.status,
            answer.text()
        );
        answer
    }

    /// `DELETE /<bucket>/<key>` — `DeleteObject`.
    pub(crate) async fn delete_object(&self, bucket: &str, key: &str) -> Answer {
        let answer = self
            .s3_xml("DELETE", &format!("/{bucket}/{}", request_key(key)), "")
            .await;
        assert_eq!(
            answer.status,
            204,
            "deleting {key:?} must succeed, got {} with {}",
            answer.status,
            answer.text()
        );
        answer
    }

    /// `POST /<bucket>?delete` — `DeleteObjects`, naming the one key.
    async fn delete_objects(&self, bucket: &str, key: &str) -> Answer {
        let body =
            format!("<Delete xmlns=\"{XMLNS}\"><Object><Key>{key}</Key></Object></Delete>");
        let answer = self.s3_xml("POST", &format!("/{bucket}?delete"), &body).await;
        assert_eq!(
            answer.status,
            200,
            "batch-deleting {key:?} must succeed, got {} with {}",
            answer.status,
            answer.text()
        );
        answer
    }

    /// `PUT /<bucket>/<key>?retention` — `PutObjectRetention`, placing a governance-mode
    /// period an hour out, which the bucket's Object Lock configuration admits.
    async fn put_object_retention(&self, bucket: &str, key: &str) -> Answer {
        let retain_until =
            ::aws_smithy_types::DateTime::from(SystemTime::now() + Duration::from_secs(3_600))
                .fmt(::aws_smithy_types::date_time::Format::DateTime)
                .expect("the test's own retention date must render as an ISO8601 date-time");
        let body = format!(
            "<Retention xmlns=\"{XMLNS}\"><Mode>GOVERNANCE</Mode>\
             <RetainUntilDate>{retain_until}</RetainUntilDate></Retention>"
        );
        let answer = self
            .s3_xml(
                "PUT",
                &format!("/{bucket}/{}?retention", request_key(key)),
                &body,
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "placing a retention on {key:?} must succeed, got {} with {}",
            answer.status,
            answer.text()
        );
        answer
    }

    /// `PUT /<bucket>/<key>?tagging` — `PutObjectTagging`, with the one tag the rows that
    /// do not care which tags were written use.
    pub(crate) async fn put_object_tagging(&self, bucket: &str, key: &str) -> Answer {
        self.put_object_tagging_of(bucket, key, &[("reviewed", "yes")])
            .await
    }

    /// `PUT /<bucket>/<key>?tagging` — `PutObjectTagging` carrying exactly `tags`, which
    /// "sets the supplied tag-set to an object": the container is the whole tag set, so an
    /// empty slice writes an empty one.
    ///
    /// `pub(crate)`: the tagging sub-type suite
    /// ([`super::object_tagging_event_sub_types`]) varies the tag set across its rows
    /// through this one rather than writing a second tagging payload builder.
    pub(crate) async fn put_object_tagging_of(
        &self,
        bucket: &str,
        key: &str,
        tags: &[(&str, &str)],
    ) -> Answer {
        let mut body = format!("<Tagging xmlns=\"{XMLNS}\"><TagSet>");
        for (tag_key, value) in tags {
            body.push_str(&format!(
                "<Tag><Key>{tag_key}</Key><Value>{value}</Value></Tag>"
            ));
        }
        body.push_str("</TagSet></Tagging>");
        let answer = self
            .s3_xml(
                "PUT",
                &format!("/{bucket}/{}?tagging", request_key(key)),
                &body,
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "tagging {key:?} must succeed, got {} with {}",
            answer.status,
            answer.text()
        );
        answer
    }

    /// `PUT /<bucket>?notification` — `PutBucketNotificationConfiguration` with one
    /// destination configuration and, when `event_bridge` is on, the
    /// `EventBridgeConfiguration` element.
    pub(crate) async fn put_notification(
        &self,
        bucket: &str,
        destination: &Destination<'_>,
        events: &[&str],
        filter_rules: &[(&str, &str)],
        event_bridge: bool,
    ) {
        let element = destination.element;
        let mut body = format!("<NotificationConfiguration xmlns=\"{XMLNS}\"><{element}>");
        body.push_str(&format!("<Id>{CONFIGURATION_ID}</Id>"));
        body.push_str(&format!(
            "<{arn_element}>{arn}</{arn_element}>",
            arn_element = destination.arn_element,
            arn = destination.arn
        ));
        for event in events {
            body.push_str(&format!("<Event>{event}</Event>"));
        }
        if !filter_rules.is_empty() {
            body.push_str("<Filter><S3Key>");
            for (name, value) in filter_rules {
                body.push_str(&format!(
                    "<FilterRule><Name>{name}</Name><Value>{value}</Value></FilterRule>"
                ));
            }
            body.push_str("</S3Key></Filter>");
        }
        body.push_str(&format!("</{element}>"));
        if event_bridge {
            body.push_str("<EventBridgeConfiguration></EventBridgeConfiguration>");
        }
        body.push_str("</NotificationConfiguration>");
        let answer = self
            .s3_xml("PUT", &format!("/{bucket}?notification"), &body)
            .await;
        assert_eq!(
            answer.status,
            200,
            "configuring notifications on {bucket} must succeed, got {} with {}",
            answer.status,
            answer.text()
        );
        // "After Amazon S3 receives this request, it first verifies that any … Amazon
        // Simple Queue Service (Amazon SQS) destination exists … by sending a test
        // notification" (`PutBucketNotificationConfiguration`'s own documentation): a
        // queue destination that validated carries that `s3:TestEvent` already, so it is
        // taken here — and asserted, because it is the observable half of the validation —
        // leaving the rows below reading only what their own object change delivered.
        if destination.element == QUEUE_CONFIGURATION_ELEMENT {
            let url = self.queue_url_of(destination.arn).await;
            let test_event = self.delivered(&url).await.unwrap_or_else(|| {
                panic!(
                    "configuring notifications on {bucket} must publish an s3:TestEvent to \
                     the queue it validated"
                )
            });
            assert_eq!(
                test_event["Event"].as_str(),
                Some("s3:TestEvent"),
                "the first message a validated queue destination receives must be the test \
                 notification, got {test_event}"
            );
            assert_eq!(
                test_event["Bucket"].as_str(),
                Some(bucket),
                "the test notification must name the bucket it was configured on, got \
                 {test_event}"
            );
        }
    }

    /// Create the queue named `name` and answer its url.
    pub(crate) async fn create_queue(&self, name: &str) -> String {
        let created = self.sqs("CreateQueue", &json!({ "QueueName": name })).await;
        created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateQueue must answer a QueueUrl, got {created}"))
            .to_string()
    }

    /// The url SQS addresses the queue `arn` names, read from SQS itself rather than
    /// rebuilt here — a queue ARN names the queue by its bare resource field.
    async fn queue_url_of(&self, arn: &str) -> String {
        let name = arn.rsplit_once(':').map_or(arn, |(_, name)| name);
        let answered = self.sqs("GetQueueUrl", &json!({ "QueueName": name })).await;
        answered["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("GetQueueUrl must answer a QueueUrl for {name:?}, got {answered}")
            })
            .to_string()
    }

    /// An EventBridge rule on the default bus matching every `aws.s3` event about
    /// `bucket`, delivering to the queue `queue_arn`.
    async fn rule_for_bucket(&self, name: &str, bucket: &str, queue_arn: &str) {
        self.events(
            "PutRule",
            &json!({
                "Name": name,
                "EventPattern": json!({
                    "source": ["aws.s3"],
                    "detail": { "bucket": { "name": [bucket] } },
                })
                .to_string(),
            }),
        )
        .await;
        self.events(
            "PutTargets",
            &json!({
                "Rule": name,
                "Targets": [{ "Id": "the-queue", "Arn": queue_arn }],
            }),
        )
        .await;
    }

    /// Every message on the queue right now, each parsed as the JSON it carries.
    async fn drain(&self, queue_url: &str) -> Vec<Value> {
        let received = self
            .sqs(
                "ReceiveMessage",
                &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
            )
            .await;
        received["Messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| {
                let body = message["Body"].as_str().unwrap_or_default();
                serde_json::from_str(body).unwrap_or(Value::Null)
            })
            .collect()
    }

    /// The first message the queue receives, waiting up to [`DELIVERY_BUDGET`] for the
    /// delivery, or `None` when none arrives in that time.
    pub(crate) async fn delivered(&self, queue_url: &str) -> Option<Value> {
        let deadline = Instant::now() + DELIVERY_BUDGET;
        loop {
            let mut messages = self.drain(queue_url).await;
            if !messages.is_empty() {
                return Some(messages.remove(0));
            }
            if Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Whatever the queue receives over [`QUIET_BUDGET`], which is how a row asserts that
    /// NOTHING was delivered.
    pub(crate) async fn nothing_delivered(&self, queue_url: &str) -> Vec<Value> {
        let deadline = Instant::now() + QUIET_BUDGET;
        let mut drained = Vec::new();
        while Instant::now() < deadline {
            drained.extend(self.drain(queue_url).await);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        drained
    }
}

/// One destination a notification configuration names: the element it is carried in, the
/// element its ARN is carried in, and that ARN.
///
/// The element names are the `xmlName`s the model puts on `NotificationConfiguration` and
/// on each configuration's ARN member
/// (`smithy_query(query="shape",shape="NotificationConfiguration")`): the Lambda list is
/// `CloudFunctionConfiguration` and its ARN member is `CloudFunction`.
pub(crate) struct Destination<'a> {
    element: &'a str,
    arn_element: &'a str,
    arn: &'a str,
}

/// The element one SQS queue destination configuration is carried in — the one destination
/// kind whose test notification a test can read back.
const QUEUE_CONFIGURATION_ELEMENT: &str = "QueueConfiguration";

impl<'a> Destination<'a> {
    /// The SQS queue `arn` names, as a `QueueConfiguration`.
    pub(crate) fn queue(arn: &'a str) -> Self {
        Self {
            element: QUEUE_CONFIGURATION_ELEMENT,
            arn_element: "Queue",
            arn,
        }
    }

    /// The Lambda function `arn` names, as a `CloudFunctionConfiguration`.
    fn function(arn: &'a str) -> Self {
        Self {
            element: "CloudFunctionConfiguration",
            arn_element: "CloudFunction",
            arn,
        }
    }
}

/// `key` as a REQUEST addresses it: the path segment a client sends, with the one
/// character the rows carry that a URI cannot hold literally percent-encoded. What S3
/// then stores is the decoded key, which is what the assertions compare against.
pub(crate) fn request_key(key: &str) -> String {
    key.replace(' ', "%20")
}

/// The ARN of the queue named `name` in this account.
pub(crate) fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}")
}

/// Which bucket the row's change needs: a plain one, one whose versioning status is
/// `Enabled` (where a delete lays a delete marker instead of removing anything), or one
/// carrying an Object Lock configuration (which is what a retention needs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BucketKind {
    Plain,
    Versioned,
    ObjectLock,
}

/// Which call the row makes on the object once the bucket is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// `PutObject` — `s3:ObjectCreated:Put`.
    Write,
    /// `DeleteObject` on a plain bucket — `s3:ObjectRemoved:Delete`.
    Delete,
    /// `DeleteObject` on a versioning-enabled bucket, where the delete "inserts a delete
    /// marker, which becomes the current version of the object" —
    /// `s3:ObjectRemoved:DeleteMarkerCreated`.
    DeleteVersioned,
    /// `DeleteObjects` — the same removal, performed per identifier of a batch.
    DeleteBatch,
    /// `PutObjectTagging` — `s3:ObjectTagging:Put`.
    Tag,
    /// `PutObjectRetention` — `s3:ObjectRetention:Put`, an event class EventBridge
    /// publishes no detail-type for.
    Retain,
}

impl Change {
    /// Whether the key has to hold an object BEFORE the configuration is installed, so
    /// that seeding it does not itself notify.
    fn needs_a_seeded_object(self) -> bool {
        !matches!(self, Self::Write)
    }

    /// The bucket this change needs.
    fn bucket_kind(self) -> BucketKind {
        match self {
            Self::DeleteVersioned => BucketKind::Versioned,
            Self::Retain => BucketKind::ObjectLock,
            Self::Write | Self::Delete | Self::DeleteBatch | Self::Tag => BucketKind::Plain,
        }
    }

    /// Whether the change leaves an object at the key, which is what decides whether the
    /// delivered record reports a size and an entity tag.
    fn leaves_an_object(self) -> bool {
        !matches!(self, Self::Delete | Self::DeleteVersioned | Self::DeleteBatch)
    }
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The `Events` the queue configuration names.
    configured_events: &'static [&'static str],
    /// The `Filter`'s key-name rules, as `(Name, Value)` pairs.
    filter_rules: &'static [(&'static str, &'static str)],
    /// Whether the bucket also carries an `EventBridgeConfiguration`.
    event_bridge: bool,
    change: Change,
    key: &'static str,
    /// The key as a delivered RECORD spells it — "URL encoded" (Amazon S3 User Guide,
    /// *Event message structure*).
    published_key: &'static str,
    /// The `eventName` the queue configuration must be delivered, or `None` when this
    /// change is not one it asked for.
    expected_event_name: Option<&'static str>,
    /// The `detail-type` the event bus must carry, or `None` when EventBridge delivery is
    /// off for the bucket.
    expected_detail_type: Option<&'static str>,
}

/// The bytes every write in the table stores, so a record's `size` has one expected
/// value.
const OBJECT_BODY: &[u8] = b"notification-payload";

/// The rule, over the class it generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a write, named by its own event value",
            configured_events: &["s3:ObjectCreated:Put"],
            filter_rules: &[],
            event_bridge: true,
            change: Change::Write,
            key: "report.json",
            published_key: "report.json",
            expected_event_name: Some("ObjectCreated:Put"),
            expected_detail_type: Some("Object Created"),
        },
        Row {
            case: "a write, named by the type wildcard, with EventBridge off",
            configured_events: &["s3:ObjectCreated:*"],
            filter_rules: &[],
            event_bridge: false,
            change: Change::Write,
            key: "input/2026/report.json",
            published_key: "input/2026/report.json",
            expected_event_name: Some("ObjectCreated:Put"),
            expected_detail_type: None,
        },
        Row {
            case: "a write whose configuration names removals only",
            configured_events: &["s3:ObjectRemoved:*"],
            filter_rules: &[],
            event_bridge: true,
            change: Change::Write,
            key: "report.json",
            published_key: "report.json",
            expected_event_name: None,
            expected_detail_type: Some("Object Created"),
        },
        Row {
            case: "a write whose key passes both filter rules",
            configured_events: &["s3:ObjectCreated:*"],
            filter_rules: &[("prefix", "input/"), ("suffix", ".json")],
            event_bridge: false,
            change: Change::Write,
            key: "input/a.json",
            published_key: "input/a.json",
            expected_event_name: Some("ObjectCreated:Put"),
            expected_detail_type: None,
        },
        Row {
            case: "a write whose key fails the suffix rule",
            configured_events: &["s3:ObjectCreated:*"],
            filter_rules: &[("prefix", "input/"), ("suffix", ".json")],
            event_bridge: true,
            change: Change::Write,
            key: "input/a.csv",
            published_key: "input/a.csv",
            expected_event_name: None,
            expected_detail_type: Some("Object Created"),
        },
        Row {
            case: "a write whose key carries a space",
            configured_events: &["s3:ObjectCreated:*"],
            filter_rules: &[],
            event_bridge: false,
            change: Change::Write,
            key: "red flower.jpg",
            published_key: "red+flower.jpg",
            expected_event_name: Some("ObjectCreated:Put"),
            expected_detail_type: None,
        },
        Row {
            case: "a delete of an object the bucket held",
            configured_events: &["s3:ObjectRemoved:*"],
            filter_rules: &[],
            event_bridge: true,
            change: Change::Delete,
            key: "report.json",
            published_key: "report.json",
            expected_event_name: Some("ObjectRemoved:Delete"),
            expected_detail_type: Some("Object Deleted"),
        },
        Row {
            case: "a tag set placed on an object",
            configured_events: &["s3:ObjectTagging:Put"],
            filter_rules: &[],
            event_bridge: true,
            change: Change::Tag,
            key: "report.json",
            published_key: "report.json",
            expected_event_name: Some("ObjectTagging:Put"),
            expected_detail_type: Some("Object Tags Added"),
        },
        Row {
            case: "a delete on a versioning-enabled bucket, which lays a delete marker",
            configured_events: &["s3:ObjectRemoved:*"],
            filter_rules: &[],
            event_bridge: true,
            change: Change::DeleteVersioned,
            key: "report.json",
            published_key: "report.json",
            expected_event_name: Some("ObjectRemoved:DeleteMarkerCreated"),
            expected_detail_type: Some("Object Deleted"),
        },
        Row {
            case: "a removal performed as one identifier of a batch delete",
            configured_events: &["s3:ObjectRemoved:Delete"],
            filter_rules: &[],
            event_bridge: true,
            change: Change::DeleteBatch,
            key: "input/report.json",
            published_key: "input/report.json",
            expected_event_name: Some("ObjectRemoved:Delete"),
            expected_detail_type: Some("Object Deleted"),
        },
        Row {
            case: "a retention, an event class EventBridge publishes no detail-type for",
            configured_events: &["s3:ObjectRetention:Put"],
            filter_rules: &[],
            // EventBridge delivery is ENABLED here and the bus must still stay empty: an
            // Object Lock retention is a bucket event a configuration can name and not
            // an event type EventBridge publishes.
            event_bridge: true,
            change: Change::Retain,
            key: "report.json",
            published_key: "report.json",
            expected_event_name: Some("ObjectRetention:Put"),
            expected_detail_type: None,
        },
    ]
}

#[tokio::test]
async fn an_object_change_reaches_every_destination_its_configuration_names() {
    let emulator = Emulator::new();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let bucket = format!("notify-{index}");
        let direct = format!("direct-{index}");
        let bus = format!("bus-{index}");

        emulator
            .create_bucket_of(&bucket, row.change.bucket_kind())
            .await;
        let direct_url = emulator.create_queue(&direct).await;
        let bus_url = emulator.create_queue(&bus).await;
        emulator
            .rule_for_bucket(&format!("rule-{index}"), &bucket, &queue_arn(&bus))
            .await;

        // Seeded BEFORE the configuration is installed, so the write that seeds a key a
        // delete or a tagging needs notifies nothing and only the row's own change does.
        if row.change.needs_a_seeded_object() {
            emulator.put_object(&bucket, row.key, OBJECT_BODY).await;
        }
        emulator
            .put_notification(
                &bucket,
                &Destination::queue(&queue_arn(&direct)),
                row.configured_events,
                row.filter_rules,
                row.event_bridge,
            )
            .await;

        // The call the row is about, and the pair ITS OWN response named the request by:
        // every notification below has to name the same one.
        let answer = match row.change {
            Change::Write => emulator.put_object(&bucket, row.key, OBJECT_BODY).await,
            Change::Delete | Change::DeleteVersioned => {
                emulator.delete_object(&bucket, row.key).await
            }
            Change::DeleteBatch => emulator.delete_objects(&bucket, row.key).await,
            Change::Tag => emulator.put_object_tagging(&bucket, row.key).await,
            Change::Retain => emulator.put_object_retention(&bucket, row.key).await,
        };
        let (request_id, extended_request_id) = answer.correlation(case);

        // The event bus first: a row that expects nothing on the queue configuration
        // still waits for the bus delivery, so "nothing arrived" is a wait that the
        // delivery path had time to finish.
        if let Some(detail_type) = row.expected_detail_type {
            let event = emulator.delivered(&bus_url).await.unwrap_or_else(|| {
                panic!("case {case:?}: no aws.s3 event reached the rule's target queue")
            });
            assert_eq!(
                event["source"].as_str(),
                Some("aws.s3"),
                "case {case:?}: expected the event's source to be aws.s3, got {event}"
            );
            assert_eq!(
                event["detail-type"].as_str(),
                Some(detail_type),
                "case {case:?}: expected detail-type {detail_type:?}, got {event}"
            );
            assert_eq!(
                event["detail"]["bucket"]["name"].as_str(),
                Some(bucket.as_str()),
                "case {case:?}: expected detail.bucket.name {bucket:?}, got {event}"
            );
            assert_eq!(
                event["detail"]["object"]["key"].as_str(),
                Some(row.key),
                "case {case:?}: expected detail.object.key {:?}, got {event}",
                row.key
            );
            // The detail names the CALL it came of, by that call's own request id.
            assert_eq!(
                event["detail"]["request-id"].as_str(),
                Some(request_id.as_str()),
                "case {case:?}: expected detail.request-id {request_id:?}, got {event}"
            );
        } else {
            let delivered = emulator.nothing_delivered(&bus_url).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: EventBridge delivery is off for this bucket, so the \
                 rule's target queue must stay empty, got {delivered:?}"
            );
        }

        match row.expected_event_name {
            Some(event_name) => {
                let message = emulator.delivered(&direct_url).await.unwrap_or_else(|| {
                    panic!("case {case:?}: no notification reached the configured queue")
                });
                let record = &message["Records"][0];
                assert_eq!(
                    record["eventName"].as_str(),
                    Some(event_name),
                    "case {case:?}: expected eventName {event_name:?}, got {message}"
                );
                assert_eq!(
                    record["eventSource"].as_str(),
                    Some("aws:s3"),
                    "case {case:?}: expected eventSource aws:s3, got {message}"
                );
                assert_eq!(
                    record["awsRegion"].as_str(),
                    Some(REGION),
                    "case {case:?}: expected awsRegion {REGION:?}, got {message}"
                );
                assert_eq!(
                    record["s3"]["configurationId"].as_str(),
                    Some(CONFIGURATION_ID),
                    "case {case:?}: expected configurationId {CONFIGURATION_ID:?}, got \
                     {message}"
                );
                assert_eq!(
                    record["s3"]["bucket"]["name"].as_str(),
                    Some(bucket.as_str()),
                    "case {case:?}: expected the record's bucket to be {bucket:?}, got \
                     {message}"
                );
                assert_eq!(
                    record["s3"]["bucket"]["arn"].as_str(),
                    Some(format!("arn:aws:s3:::{bucket}").as_str()),
                    "case {case:?}: expected the record's bucket ARN, got {message}"
                );
                assert_eq!(
                    record["s3"]["object"]["key"].as_str(),
                    Some(row.published_key),
                    "case {case:?}: expected the record's key {:?}, got {message}",
                    row.published_key
                );
                assert!(
                    record["s3"]["object"]["sequencer"]
                        .as_str()
                        .is_some_and(|sequencer| !sequencer.is_empty()),
                    "case {case:?}: expected a sequencer on the record, got {message}"
                );
                // "The response elements … useful if you want to trace a request": the
                // record names the CALL it came of, by the two correlation headers that
                // call's own response carried.
                assert_eq!(
                    record["responseElements"]["x-amz-request-id"].as_str(),
                    Some(request_id.as_str()),
                    "case {case:?}: expected the call's own x-amz-request-id \
                     {request_id:?}, got {message}"
                );
                assert_eq!(
                    record["responseElements"]["x-amz-id-2"].as_str(),
                    Some(extended_request_id.as_str()),
                    "case {case:?}: expected the call's own x-amz-id-2 \
                     {extended_request_id:?}, got {message}"
                );
                // A change that left an object at the key reports that object's size and
                // entity tag; a removal reports neither, because there is nothing there
                // to report them from.
                if row.change.leaves_an_object() {
                    assert_eq!(
                        record["s3"]["object"]["size"].as_i64(),
                        Some(OBJECT_BODY.len() as i64),
                        "case {case:?}: expected the stored size, got {message}"
                    );
                    // "The object etag … does not include the surrounding quotes": the
                    // digest alone, as the ETag the write itself reported without them.
                    let reported = record["s3"]["object"]["eTag"].as_str().unwrap_or_default();
                    assert!(
                        !reported.is_empty() && !reported.contains('"'),
                        "case {case:?}: expected an unquoted eTag, got {message}"
                    );
                } else {
                    assert!(
                        record["s3"]["object"]["size"].is_null()
                            && record["s3"]["object"]["eTag"].is_null(),
                        "case {case:?}: a removal's record must carry no size or eTag, \
                         got {message}"
                    );
                }
            }
            None => {
                let delivered = emulator.nothing_delivered(&direct_url).await;
                assert!(
                    delivered.is_empty(),
                    "case {case:?}: this configuration does not name the change, so the \
                     queue must stay empty, got {delivered:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_write_invokes_the_function_a_lambda_configuration_names() {
    let emulator = Emulator::new();
    let bucket = "notify-lambda";
    let function = "object-events";
    let key = "input/report.json";

    emulator.create_bucket(bucket).await;

    // The function the configuration names, brought to `Active` — the state an
    // invocation needs — by moving the activation instant the store holds into the past,
    // which is what waiting would do.
    let created = emulator
        .send(
            "lambda",
            "POST",
            "/2015-03-31/functions",
            &[("content-type", "application/json")],
            json!({
                "FunctionName": function,
                "Role": LAMBDA_ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": PACKAGE_ZIP_FILE },
            })
            .to_string()
            .into_bytes(),
        )
        .await;
    assert_eq!(
        created.status,
        201,
        "creating the function must succeed, got {} with {}",
        created.status,
        created.text()
    );
    {
        let state = emulator
            .engine
            .state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for record in store.functions.values_mut() {
            record.active_due = past;
        }
    }

    // The grant S3 reads before it records the configuration: "in the case of Lambda
    // destinations, Amazon S3 verifies that the Lambda function permissions grant Amazon S3
    // permission to invoke the function from the Amazon S3 bucket"
    // (`PutBucketNotificationConfiguration`'s own documentation).
    let granted = emulator
        .send(
            "lambda",
            "POST",
            &format!("/2015-03-31/functions/{function}/policy"),
            &[("content-type", "application/json")],
            json!({
                "StatementId": "s3-invoke",
                "Action": "lambda:InvokeFunction",
                "Principal": "s3.amazonaws.com",
                "SourceArn": format!("arn:aws:s3:::{bucket}"),
            })
            .to_string()
            .into_bytes(),
        )
        .await;
    assert_eq!(
        granted.status,
        201,
        "granting S3 the invoke on the function must succeed, got {} with {}",
        granted.status,
        granted.text()
    );

    emulator
        .put_notification(
            bucket,
            &Destination::function(&format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}"
            )),
            &["s3:ObjectCreated:*"],
            &[("prefix", "input/")],
            false,
        )
        .await;

    let written = emulator.put_object(bucket, key, OBJECT_BODY).await;
    let (request_id, _) = written.correlation("lambda destination");

    // No modelled Lambda operation reports an accepted asynchronous invocation, so the
    // round trip is through Lambda's own state, which is where S3's invocation lands.
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let payload = loop {
        let queued = {
            let state = emulator
                .engine
                .state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
            let store = state.lock();
            store
                .functions
                .get(function)
                .and_then(|record| record.async_invocations.front().cloned())
        };
        if let Some(invocation) = queued {
            break invocation.payload;
        }
        assert!(
            Instant::now() < deadline,
            "case \"lambda destination\": S3 invoked no function for the write"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    let event: Value = serde_json::from_slice(&payload).unwrap_or_else(|error| {
        panic!(
            "case \"lambda destination\": the invocation's payload must be the event \
             envelope, got {error} for {:?}",
            String::from_utf8_lossy(&payload)
        )
    });
    let record = &event["Records"][0];
    assert_eq!(
        record["eventName"].as_str(),
        Some("ObjectCreated:Put"),
        "case \"lambda destination\": expected eventName ObjectCreated:Put, got {event}"
    );
    assert_eq!(
        record["s3"]["bucket"]["name"].as_str(),
        Some(bucket),
        "case \"lambda destination\": expected the record's bucket {bucket:?}, got {event}"
    );
    assert_eq!(
        record["s3"]["object"]["key"].as_str(),
        Some(key),
        "case \"lambda destination\": expected the record's key {key:?}, got {event}"
    );
    assert_eq!(
        record["s3"]["object"]["size"].as_i64(),
        Some(OBJECT_BODY.len() as i64),
        "case \"lambda destination\": expected the stored size, got {event}"
    );
    assert_eq!(
        record["responseElements"]["x-amz-request-id"].as_str(),
        Some(request_id.as_str()),
        "case \"lambda destination\": expected the write's own x-amz-request-id \
         {request_id:?}, got {event}"
    );
}

#[tokio::test]
async fn a_bucket_with_no_notifications_configured_delivers_nothing() {
    let emulator = Emulator::new();
    let bucket = "notify-none";
    let bus = "bus-unconfigured";

    emulator.create_bucket(bucket).await;
    let bus_url = emulator.create_queue(bus).await;
    emulator
        .rule_for_bucket("rule-unconfigured", bucket, &queue_arn(bus))
        .await;

    // Every object change this suite publishes, on a bucket whose notification
    // configuration is the empty one every bucket is created with.
    emulator.put_object(bucket, "report.json", OBJECT_BODY).await;
    emulator.put_object_tagging(bucket, "report.json").await;
    emulator.delete_object(bucket, "report.json").await;

    let delivered = emulator.nothing_delivered(&bus_url).await;
    assert!(
        delivered.is_empty(),
        "case \"no configuration\": a bucket with no event notifications configured must \
         publish nothing, got {delivered:?}"
    );
}

/// One row of the request-correlation table: the case, the method, the uri, the request
/// headers, the request body, the status the call answers, and whether the answer is an
/// error envelope — which names its request in the BODY as well, so the two have to agree.
type CorrelationRow<'a> = (
    &'a str,
    &'a str,
    String,
    Vec<(&'a str, &'a str)>,
    Vec<u8>,
    u16,
    bool,
);

/// The text of the first `<name>` element of `body`, when it carries one.
fn body_element(body: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(body).into_owned();
    text.split_once(&format!("<{name}>"))
        .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
        .map(|(value, _)| value.to_string())
}

#[tokio::test]
async fn every_response_names_the_one_request_it_answers() {
    let emulator = Emulator::new();
    let bucket = "notify-correlated";
    let key = "reports/one.txt";

    emulator.create_bucket(bucket).await;

    // The rows differ along the kind of answer: a write and a subresource write (both of
    // which publish an object event and so record the pair their notification names), a
    // read, a 204 with no body at all, and a REFUSAL — every one of which carries the
    // pair, because "a value created by Amazon S3 that uniquely identifies the request" is
    // a common RESPONSE header rather than a success-only one.
    let rows: Vec<CorrelationRow> = vec![
        (
            "a write",
            "PUT",
            format!("/{bucket}/{key}"),
            vec![("content-type", "text/plain")],
            OBJECT_BODY.to_vec(),
            200,
            false,
        ),
        (
            "a read of what it wrote",
            "GET",
            format!("/{bucket}/{key}"),
            vec![],
            Vec::new(),
            200,
            false,
        ),
        (
            "a subresource write",
            "PUT",
            format!("/{bucket}/{key}?tagging"),
            vec![("content-type", "application/xml")],
            b"<Tagging><TagSet><Tag><Key>a</Key><Value>b</Value></Tag></TagSet></Tagging>".to_vec(),
            200,
            false,
        ),
        (
            "a 204 with no body",
            "DELETE",
            format!("/{bucket}/{key}?tagging"),
            vec![],
            Vec::new(),
            204,
            false,
        ),
        (
            "a refusal, whose envelope names its own request",
            "GET",
            "/notify-no-such-bucket/absent.txt".to_string(),
            vec![],
            Vec::new(),
            404,
            true,
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, method, uri, headers, body, expected_status, names_in_body) in rows {
        let answer = emulator.send("s3", method, &uri, &headers, body).await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with {}",
            answer.status,
            answer.text()
        );

        let (request_id, extended) = answer.correlation(case);
        if names_in_body {
            // An error envelope carries the pair in its `<RequestId>` / `<HostId>`
            // elements too, and a client reads the id from there while an operator reads
            // it from the header: ONE refusal may not report two ids.
            assert_eq!(
                body_element(&answer.body, "RequestId").as_deref(),
                Some(request_id.as_str()),
                "case {case:?}: the body's RequestId must be the header's {request_id:?}, \
                 got {}",
                answer.text()
            );
            assert_eq!(
                body_element(&answer.body, "HostId").as_deref(),
                Some(extended.as_str()),
                "case {case:?}: the body's HostId must be the header's {extended:?}, got {}",
                answer.text()
            );
        } else {
            // An opaque token, not a modelled value: what matters is that the response
            // names SOMETHING and that no two responses name the same thing (asserted
            // below), so the rendering itself is not pinned here.
            assert!(
                !request_id.is_empty() && !request_id.contains(char::is_whitespace),
                "case {case:?}: expected a non-empty request id, got {request_id:?}"
            );
            assert!(
                !extended.is_empty() && !extended.contains(char::is_whitespace),
                "case {case:?}: expected a non-empty extended request id, got {extended:?}"
            );
        }
        assert!(
            !seen.contains(&request_id),
            "case {case:?}: a request id must name ONE request, but {request_id:?} was \
             already answered with (seen {seen:?})"
        );
        seen.push(request_id);
    }
}
