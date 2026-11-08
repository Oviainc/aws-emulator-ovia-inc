// SPDX-License-Identifier: CC0-1.0

//! The NOTIFICATION leg of a delivery channel: what a consumer subscribed to the channel's
//! Amazon SNS topic receives when Config records a configuration item.
//!
//! The rule under test: whenever Config records a configuration item for a resource — the
//! resource's first one, one recording a state it changed into, or the one recording that
//! it is gone — the item is published to the `snsTopicARN` of this account's delivery
//! channel as a `ConfigurationItemChangeNotification`, whose `configurationItemDiff`
//! carries the `changeType` of the item and, for a changed one, NAMES each property that
//! changed with the value it held and the value it now holds. An observation that records
//! no configuration item — the resource is still in the state the held item records —
//! publishes nothing.
//!
//! Config, Amazon SNS and Amazon SQS are mounted on ONE engine (the harness
//! `config_recorded_resource_compliance` builds): the notification is published to a real
//! SNS topic and arrives at the SQS queue subscribed to it, so what is asserted is what a
//! subscriber actually received, envelope and all.
//!
//! Two ways a configuration item gets recorded are driven here, because the rule is about
//! the RECORDING and not about which call did it: `PutResourceConfig` and
//! `DeleteResourceConfig`, which record the configuration state of one resource, and the
//! running configuration recorder, which records what it OBSERVES of the resources the
//! account holds in their own services.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config_recorded_resource_compliance::{Emulator, ACCOUNT_ID, REGION};

/// The `messageType` every configuration-item change notification carries.
const CHANGE_MESSAGE_TYPE: &str = "ConfigurationItemChangeNotification";

/// `configurationItemDiff$changeType` values.
const CREATE: &str = "CREATE";
/// See [`CREATE`].
const UPDATE: &str = "UPDATE";
/// See [`CREATE`].
const DELETE: &str = "DELETE";

/// The resource type the `PutResourceConfig` rows record, an entry of
/// `com.amazonaws.configservice#ResourceType` the recorder does not observe for itself —
/// so the only configuration items recorded for it are the ones these rows record.
const VOLUME_RESOURCE_TYPE: &str = "AWS::EC2::Volume";

/// The resource type Config records an Amazon SQS queue under, the type the recorder
/// OBSERVES below.
const QUEUE_RESOURCE_TYPE: &str = "AWS::SQS::Queue";

/// The resource the `PutResourceConfig` rows record configuration items for.
const VOLUME_ID: &str = "vol-0123456789abcdef0";

/// The `SchemaVersionId` those rows record under.
const SCHEMA_VERSION_ID: &str = "00000000-0000-0000-0000-000000000000";

/// How long a poll waits for a notification the clock owes. A recording pass falls due a
/// fraction of a second after the call that makes it due, so this is many passes' worth of
/// slack and not a span the behaviour depends on.
const ARRIVAL_BUDGET: Duration = Duration::from_secs(20);

/// How long a row waits while asserting that NO notification is owed — several passes'
/// worth of time, so one that was going to be published would have been.
const QUIET_BUDGET: Duration = Duration::from_millis(2_500);

/// How long a poll waits between two receives.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The queue the notifications are consumed from, and the topic they are published to.
struct Subscriber {
    /// The url the subscribed queue is received from.
    queue_url: String,
    /// The topic's ARN, which is the channel's `snsTopicARN`.
    topic_arn: String,
    /// The notifications received so far and not yet asserted on, in arrival order.
    received: Vec<Value>,
}

impl Subscriber {
    /// A topic, a queue subscribed to it, and this account's recorder and delivery channel
    /// streaming to that topic.
    async fn new(emulator: &Emulator, queue_name: &str, topic_name: &str) -> Self {
        let created = emulator
            .sqs("CreateQueue", &json!({ "QueueName": queue_name }))
            .await;
        let queue_url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateQueue: expected a QueueUrl, got {created}"))
            .to_string();
        let attributes = emulator
            .sqs(
                "GetQueueAttributes",
                &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
            )
            .await;
        let queue_arn = attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("GetQueueAttributes: expected a QueueArn, got {attributes}"))
            .to_string();
        // The grant AWS documents for an SQS queue subscribed to an SNS topic: without it
        // the topic's deliveries reach nothing.
        emulator
            .sqs(
                "SetQueueAttributes",
                &json!({
                    "QueueUrl": queue_url,
                    "Attributes": {
                        "Policy": json!({
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Principal": { "Service": "sns.amazonaws.com" },
                                "Action": "sqs:SendMessage",
                                "Resource": queue_arn,
                            }],
                        })
                        .to_string(),
                    },
                }),
            )
            .await;

        let created = emulator.sns("CreateTopic", &[("Name", topic_name)]).await;
        let topic_arn = element(&created, "TopicArn");
        emulator
            .sns(
                "Subscribe",
                &[
                    ("TopicArn", &topic_arn),
                    ("Protocol", "sqs"),
                    ("Endpoint", &queue_arn),
                ],
            )
            .await;

        emulator
            .config(
                "PutConfigurationRecorder",
                &json!({
                    "ConfigurationRecorder": {
                        "name": "the-recorder",
                        "roleARN": format!("arn:aws:iam::{ACCOUNT_ID}:role/config-role"),
                        "recordingGroup": { "allSupported": true },
                    }
                }),
            )
            .await;
        emulator
            .config(
                "PutDeliveryChannel",
                &json!({
                    "DeliveryChannel": {
                        "s3BucketName": "config-delivery",
                        "snsTopicARN": topic_arn,
                    }
                }),
            )
            .await;
        emulator
            .config(
                "StartConfigurationRecorder",
                &json!({ "ConfigurationRecorderName": "the-recorder" }),
            )
            .await;

        Self {
            queue_url,
            topic_arn,
            received: Vec::new(),
        }
    }

    /// Receive whatever is waiting on the queue, as the notification each message carries.
    ///
    /// Every message is asserted to be an SNS NOTIFICATION envelope naming the channel's
    /// topic, with the notification itself as its `Message` string — which is the shape a
    /// subscriber reads.
    async fn receive(&mut self, emulator: &Emulator, case: &str) {
        for envelope in emulator.drain(&self.queue_url).await {
            assert_eq!(
                envelope["Type"],
                json!("Notification"),
                "case {case:?}: a subscriber receives an SNS notification envelope, got \
                 {envelope}"
            );
            assert_eq!(
                envelope["TopicArn"],
                json!(self.topic_arn),
                "case {case:?}: the envelope names the channel's topic, got {envelope}"
            );
            let message = envelope["Message"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {envelope}"));
            let notification: Value = serde_json::from_str(message).unwrap_or_else(|error| {
                panic!("case {case:?}: the Message must be a JSON document, got {error}: {message}")
            });
            self.received.push(notification);
        }
    }

    /// The next notification about `resource_id`, waiting up to [`ARRIVAL_BUDGET`] for it.
    ///
    /// Notifications about other resources — the subscribed queue is a recorded resource
    /// too — are kept, so one row's assertions never consume another's.
    async fn next_about(&mut self, emulator: &Emulator, case: &str, resource_id: &str) -> Value {
        let deadline = Instant::now() + ARRIVAL_BUDGET;
        loop {
            self.receive(emulator, case).await;
            if let Some(at) = self
                .received
                .iter()
                .position(|notification| about(notification, resource_id))
            {
                return self.received.remove(at);
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected a notification about {resource_id:?}, got {:?}",
                self.received
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Assert that NO notification about `resource_id` arrives for as long as
    /// [`QUIET_BUDGET`].
    async fn quiet_about(&mut self, emulator: &Emulator, case: &str, resource_id: &str) {
        let deadline = Instant::now() + QUIET_BUDGET;
        loop {
            self.receive(emulator, case).await;
            let about_it: Vec<&Value> = self
                .received
                .iter()
                .filter(|notification| about(notification, resource_id))
                .collect();
            assert!(
                about_it.is_empty(),
                "case {case:?}: no configuration item was recorded for {resource_id:?}, so \
                 nothing is streamed about it, got {about_it:?}"
            );
            if Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// Whether `notification` is about the resource `resource_id`.
fn about(notification: &Value, resource_id: &str) -> bool {
    notification["configurationItem"]["resourceId"] == json!(resource_id)
}

/// The text of the first `<tag>…</tag>` element of an awsQuery response.
fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no <{tag}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("no </{tag}> in {body}"));
    body[start..start + end].to_string()
}

/// Assert the diff of `notification`: its `changeType` and the properties it names.
fn assert_diff(
    case: &str,
    notification: &Value,
    change_type: &str,
    changed: &[(&str, Value, Value, &str)],
) {
    assert_eq!(
        notification["messageType"],
        json!(CHANGE_MESSAGE_TYPE),
        "case {case:?}: the notification names the message type it is, got {notification}"
    );
    let diff = &notification["configurationItemDiff"];
    assert_eq!(
        diff["changeType"],
        json!(change_type),
        "case {case:?}: expected the diff to report {change_type}, got {notification}"
    );
    let properties = diff["changedProperties"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected changedProperties, got {notification}"));
    assert_eq!(
        properties.len(),
        changed.len(),
        "case {case:?}: expected {} changed properties, got {properties:?}",
        changed.len()
    );
    for (property, previous, updated, property_change) in changed {
        let named = properties.get(*property).unwrap_or_else(|| {
            panic!("case {case:?}: expected {property:?} to be named, got {properties:?}")
        });
        let expected = json!({
            "previousValue": previous,
            "updatedValue": updated,
            "changeType": property_change,
        });
        assert_eq!(
            named, &expected,
            "case {case:?}: expected {property:?} to be {expected}, got {named}"
        );
    }
}

/// Record the configuration state of the volume: `configuration` as its configuration
/// document and `tags` as the tags recorded with it.
async fn record_volume(emulator: &Emulator, configuration: &Value, tags: &[(&str, &str)]) {
    let mut request = json!({
        "ResourceType": VOLUME_RESOURCE_TYPE,
        "SchemaVersionId": SCHEMA_VERSION_ID,
        "ResourceId": VOLUME_ID,
        "ResourceName": "the-volume",
        "Configuration": configuration.to_string(),
    });
    if !tags.is_empty() {
        request["Tags"] = json!(tags
            .iter()
            .map(|(key, value)| (key.to_string(), json!(value)))
            .collect::<serde_json::Map<String, Value>>());
    }
    emulator.config("PutResourceConfig", &request).await;
}

#[tokio::test]
async fn every_recorded_configuration_item_is_streamed_with_the_diff_that_names_it() {
    let emulator = Emulator::new();
    let mut subscriber = Subscriber::new(&emulator, "config-stream-items", "config-items").await;

    // The rows differ along what the rule generalises over: which KIND of configuration
    // item was recorded (the first one, a changed one, the one recording the deletion, and
    // an observation that records none at all) and WHAT changed in a changed one (a
    // top-level member, a nested member, a member that appeared, a member that went away, a
    // tag).
    type Row = (
        &'static str,
        Value,
        &'static [(&'static str, &'static str)],
        &'static str,
        Vec<(&'static str, Value, Value, &'static str)>,
    );
    let rows: Vec<Row> = vec![
        (
            "the resource's first configuration item",
            json!({ "size": 8, "encrypted": false, "attachment": { "device": "/dev/sdf" } }),
            &[("stage", "prod")],
            CREATE,
            vec![],
        ),
        (
            "the same configuration recorded again",
            json!({ "size": 8, "encrypted": false, "attachment": { "device": "/dev/sdf" } }),
            &[("stage", "prod")],
            // Nothing was recorded, so nothing is streamed: this row asserts the quiet.
            "",
            vec![],
        ),
        (
            "one top-level member changed",
            json!({ "size": 16, "encrypted": false, "attachment": { "device": "/dev/sdf" } }),
            &[("stage", "prod")],
            UPDATE,
            vec![("Configuration.size", json!(8), json!(16), UPDATE)],
        ),
        (
            "a nested member changed",
            json!({ "size": 16, "encrypted": false, "attachment": { "device": "/dev/sdg" } }),
            &[("stage", "prod")],
            UPDATE,
            vec![(
                "Configuration.attachment.device",
                json!("/dev/sdf"),
                json!("/dev/sdg"),
                UPDATE,
            )],
        ),
        (
            "a member the new item adds",
            json!({
                "size": 16,
                "encrypted": false,
                "attachment": { "device": "/dev/sdg" },
                "volumeType": "gp3",
            }),
            &[("stage", "prod")],
            UPDATE,
            vec![(
                "Configuration.volumeType",
                Value::Null,
                json!("gp3"),
                CREATE,
            )],
        ),
        (
            "a member the new item drops",
            json!({
                "size": 16,
                "attachment": { "device": "/dev/sdg" },
                "volumeType": "gp3",
            }),
            &[("stage", "prod")],
            UPDATE,
            vec![(
                "Configuration.encrypted",
                json!(false),
                Value::Null,
                DELETE,
            )],
        ),
        (
            "a tag changed",
            json!({
                "size": 16,
                "attachment": { "device": "/dev/sdg" },
                "volumeType": "gp3",
            }),
            &[("stage", "staging")],
            UPDATE,
            vec![("Tags.stage", json!("prod"), json!("staging"), UPDATE)],
        ),
    ];

    let mut last_configuration = Value::Null;
    for (case, configuration, tags, change_type, changed) in rows {
        record_volume(&emulator, &configuration, tags).await;
        if change_type.is_empty() {
            subscriber.quiet_about(&emulator, case, VOLUME_ID).await;
            continue;
        }
        let notification = subscriber.next_about(&emulator, case, VOLUME_ID).await;
        assert_diff(case, &notification, change_type, &changed);

        // The item the notification carries is the resource as Config recorded it, and its
        // `configuration` is the document it encodes rather than a JSON-encoded string.
        // The status is the one the item was recorded with: the resource's FIRST item
        // discovered it, and every later one records a configuration that was updated.
        let item = &notification["configurationItem"];
        let expected_status = if change_type == CREATE {
            "ResourceDiscovered"
        } else {
            "OK"
        };
        for (member, expected) in [
            ("resourceType", json!(VOLUME_RESOURCE_TYPE)),
            ("resourceId", json!(VOLUME_ID)),
            ("resourceName", json!("the-volume")),
            ("awsAccountId", json!(ACCOUNT_ID)),
            ("awsRegion", json!(REGION)),
            ("configurationItemStatus", json!(expected_status)),
            (
                "ARN",
                json!(format!("arn:aws:ec2:{REGION}:{ACCOUNT_ID}:volume/{VOLUME_ID}")),
            ),
            ("configuration", configuration.clone()),
        ] {
            assert_eq!(
                item[member], expected,
                "case {case:?}: expected the item's {member} to be {expected}, got {item}"
            );
        }
        last_configuration = configuration.clone();
    }

    // ROUND TRIP: the item the notification carried is the one the resource's history
    // reports, read back through the API.
    let history = emulator
        .config(
            "GetResourceConfigHistory",
            &json!({ "resourceType": VOLUME_RESOURCE_TYPE, "resourceId": VOLUME_ID }),
        )
        .await;
    let recorded: Value = serde_json::from_str(
        history["configurationItems"][0]["configuration"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a recorded configuration, got {history}")),
    )
    .unwrap_or_else(|error| panic!("the recorded configuration must be JSON, got {error}"));
    assert_eq!(
        recorded, last_configuration,
        "the item streamed last is the resource's current one, got {history}"
    );

    // The deletion of the resource is a configuration item like any other, so it is
    // streamed too — and reports the resource is gone rather than which properties went
    // with it.
    emulator
        .config(
            "DeleteResourceConfig",
            &json!({ "ResourceType": VOLUME_RESOURCE_TYPE, "ResourceId": VOLUME_ID }),
        )
        .await;
    let case = "the deletion of the resource";
    let notification = subscriber.next_about(&emulator, case, VOLUME_ID).await;
    assert_diff(case, &notification, DELETE, &[]);
    assert_eq!(
        notification["configurationItem"]["configurationItemStatus"],
        json!("ResourceDeleted"),
        "case {case:?}: the item records that the resource is gone, got {notification}"
    );
}

#[tokio::test]
async fn a_resource_the_recorder_observes_is_streamed_as_its_configuration_changes() {
    let emulator = Emulator::new();
    let mut subscriber = Subscriber::new(&emulator, "config-stream-queues", "config-queues").await;

    // The recorded resource lives in its OWN service: Config is told nothing about it, and
    // every configuration item below is one the running recorder observed.
    let queue = "recorded-queue";
    let created = emulator
        .sqs(
            "CreateQueue",
            &json!({
                "QueueName": queue,
                "Attributes": { "VisibilityTimeout": "31", "DelaySeconds": "5" },
            }),
        )
        .await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue: expected a QueueUrl, got {created}"))
        .to_string();

    let case = "a queue the recorder discovered";
    let notification = subscriber.next_about(&emulator, case, &queue_url).await;
    assert_diff(case, &notification, CREATE, &[]);
    let item = &notification["configurationItem"];
    for (member, expected) in [
        ("resourceType", json!(QUEUE_RESOURCE_TYPE)),
        // Amazon SQS addresses a queue by its url, which is what identifies it; the
        // queue's own name is its custom name.
        ("resourceId", json!(queue_url)),
        ("resourceName", json!(queue)),
        ("awsRegion", json!(REGION)),
        // The queue's FIRST configuration item is the one that discovered it.
        ("configurationItemStatus", json!("ResourceDiscovered")),
        (
            "ARN",
            json!(format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{queue}")),
        ),
    ] {
        assert_eq!(
            item[member], expected,
            "case {case:?}: expected the item's {member} to be {expected}, got {item}"
        );
    }
    // The configuration is the queue's own configuration as Amazon SQS reports it.
    for (attribute, expected) in [
        ("VisibilityTimeout", json!("31")),
        ("DelaySeconds", json!("5")),
        (
            "QueueArn",
            json!(format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{queue}")),
        ),
    ] {
        assert_eq!(
            item["configuration"][attribute], expected,
            "case {case:?}: expected the recorded {attribute} to be {expected}, got {item}"
        );
    }

    // Changing ONE attribute of the resource in its own service is what the diff names.
    // The rows differ along which attribute changed and the values it moved between.
    let changes: [(&str, &str, &str, &str); 3] = [
        ("the visibility timeout", "VisibilityTimeout", "31", "41"),
        ("the delay", "DelaySeconds", "5", "9"),
        (
            "the message retention period",
            "MessageRetentionPeriod",
            "345600",
            "600",
        ),
    ];
    for (case, attribute, previous, updated) in changes {
        emulator
            .sqs(
                "SetQueueAttributes",
                &json!({
                    "QueueUrl": queue_url,
                    "Attributes": { attribute: updated },
                }),
            )
            .await;
        let notification = subscriber.next_about(&emulator, case, &queue_url).await;
        assert_diff(
            case,
            &notification,
            UPDATE,
            &[(
                &format!("Configuration.{attribute}"),
                json!(previous),
                json!(updated),
                UPDATE,
            )],
        );
    }

    // ROUND TRIP: the states the notifications reported are the items the resource's
    // history holds, newest first.
    let history = emulator
        .config(
            "GetResourceConfigHistory",
            &json!({ "resourceType": QUEUE_RESOURCE_TYPE, "resourceId": queue_url }),
        )
        .await;
    let items = history["configurationItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        items.len() >= 4,
        "one item was recorded per state the queue went through, got {history}"
    );
    let current: Value = serde_json::from_str(
        items[0]["configuration"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a recorded configuration, got {history}")),
    )
    .unwrap_or_else(|error| panic!("the recorded configuration must be JSON, got {error}"));
    assert_eq!(
        current["MessageRetentionPeriod"],
        json!("600"),
        "the current item records the state the last change left the queue in, got {current}"
    );

    // A resource that goes away in its own service has its DELETION recorded, and that
    // item is streamed too.
    emulator
        .sqs("DeleteQueue", &json!({ "QueueUrl": queue_url }))
        .await;
    let case = "a queue deleted in its own service";
    let notification = subscriber.next_about(&emulator, case, &queue_url).await;
    assert_diff(case, &notification, DELETE, &[]);
    assert_eq!(
        notification["configurationItem"]["configurationItemStatus"],
        json!("ResourceDeleted"),
        "case {case:?}: the item records that the resource is gone, got {notification}"
    );
}

#[tokio::test]
async fn a_channel_with_no_topic_streams_nothing() {
    let emulator = Emulator::new();
    // A queue subscribed to a topic the channel does NOT name: the channel this account
    // holds carries no `snsTopicARN`, so SNS delivery is turned off for it.
    let created = emulator
        .sqs("CreateQueue", &json!({ "QueueName": "unsubscribed" }))
        .await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue: expected a QueueUrl, got {created}"))
        .to_string();
    let attributes = emulator
        .sqs(
            "GetQueueAttributes",
            &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes: expected a QueueArn, got {attributes}"))
        .to_string();
    emulator
        .sqs(
            "SetQueueAttributes",
            &json!({
                "QueueUrl": queue_url,
                "Attributes": {
                    "Policy": json!({
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "sns.amazonaws.com" },
                            "Action": "sqs:SendMessage",
                            "Resource": queue_arn,
                        }],
                    })
                    .to_string(),
                },
            }),
        )
        .await;
    let topic_arn = element(
        &emulator.sns("CreateTopic", &[("Name", "not-the-channel")]).await,
        "TopicArn",
    );
    emulator
        .sns(
            "Subscribe",
            &[
                ("TopicArn", &topic_arn),
                ("Protocol", "sqs"),
                ("Endpoint", &queue_arn),
            ],
        )
        .await;
    emulator.recording().await;

    // The recorder records what it observes, and `PutResourceConfig` records what it is
    // handed; neither streams anything, because the channel names no topic.
    record_volume(&emulator, &json!({ "size": 8 }), &[]).await;
    let deadline = Instant::now() + QUIET_BUDGET;
    loop {
        let received = emulator.drain(&queue_url).await;
        assert!(
            received.is_empty(),
            "a channel carrying no snsTopicARN streams nothing, got {received:?}"
        );
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
