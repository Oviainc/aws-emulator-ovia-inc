// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING configuration recorder produces, end to end: the resources it
//! observes, the verdicts the rules in whose scope they fall place them at, and the
//! compliance-change events those verdicts publish.
//!
//! The rule under test: while a configuration recorder is recording, Config records a
//! configuration item for each resource of the types it records, evaluates every
//! `ACTIVE` rule whose scope covers such a resource, and — whenever a rule's verdict
//! for a resource CHANGES — publishes one `Config Rules Compliance Change` event onto
//! the account's default EventBridge bus, carrying the verdict that now holds and the
//! one it replaced. A configuration change that does not change a verdict publishes
//! nothing, and a rule whose implementation this emulator does not have places no
//! resource at all.
//!
//! Four services are mounted on ONE engine, because the behaviour spans them: the
//! recorder READS the resources it records from S3's own API, the compliance change is
//! published to EventBridge, and the bus delivers it to the SQS queue a rule targets.
//! S3 is driven through the AWS SDK over the same in-process callback a handler uses;
//! Config, EventBridge and SQS are driven over the wire, the way a client drives them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every request below is signed for, and the Region the buckets are
/// created in: a configuration recorder records the resources of its own Region.
pub(crate) const REGION: &str = "us-west-2";

/// The account every emulated resource belongs to.
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// The `source` Config publishes its events under, and the `detail-type` of a
/// compliance change (the EventBridge reference for Config).
const CONFIG_EVENT_SOURCE: &str = "aws.config";
/// See [`CONFIG_EVENT_SOURCE`].
const COMPLIANCE_CHANGE_DETAIL_TYPE: &str = "Config Rules Compliance Change";
/// The `detail.messageType` a compliance-change notification carries.
const COMPLIANCE_CHANGE_MESSAGE_TYPE: &str = "ComplianceChangeNotification";

/// The resource type Config records an S3 bucket under.
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// `ComplianceType` values.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The bucket versioning statuses `PutBucketVersioning` takes
/// (`com.amazonaws.s3#BucketVersioningStatus`).
const ENABLED: &str = "Enabled";
/// See [`ENABLED`].
const SUSPENDED: &str = "Suspended";

/// How long a drain waits for the events a pass owes before giving up. A pass falls
/// due a fraction of a second after the call that made it due, so this is many passes'
/// worth of slack and not a span the behaviour depends on.
const DRAIN_BUDGET: Duration = Duration::from_secs(10);

/// How long a drain waits before asserting that NO further event is owed: long enough
/// for several passes to have run.
const QUIET_BUDGET: Duration = Duration::from_millis(2_500);

/// Config, S3, EventBridge and SQS over one shared engine.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// One RPC call: `POST /` carrying `X-Amz-Target`, signed for `signing_name`.
    pub(crate) async fn call(
        &self,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A Config call (`awsJson1_1`), required to answer 200.
    pub(crate) async fn config(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .call("config", "StarlingDoveService", "1.1", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// An EventBridge call (`awsJson1_1`), required to answer 200.
    async fn events(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .call("events", "AWSEvents", "1.1", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// An SQS call (`awsJson1_0`), required to answer 200.
    async fn sqs(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call("sqs", "AmazonSQS", "1.0", operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// An S3 client over the in-process callback, addressing path style exactly as the
    /// recorder's own client does.
    pub(crate) fn s3(&self) -> aws_sdk_s3::Client {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(REGION.to_string()))
            .endpoint_url("https://s3.us-east-1.amazonaws.com")
            .force_path_style(true)
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_s3::Client::from_conf(config)
    }

    /// Create `bucket` in [`REGION`], with `tags` on it when it carries any.
    pub(crate) async fn create_bucket(&self, bucket: &str, tags: &[(&str, &str)]) {
        self.s3()
            .create_bucket()
            .bucket(bucket)
            .create_bucket_configuration(
                aws_sdk_s3::types::CreateBucketConfiguration::builder()
                    .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                    .build(),
            )
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket} must succeed, got {error:?}"));
        if tags.is_empty() {
            return;
        }
        let tag_set: Vec<aws_sdk_s3::types::Tag> = tags
            .iter()
            .map(|(key, value)| {
                aws_sdk_s3::types::Tag::builder()
                    .key(*key)
                    .value(*value)
                    .build()
                    .expect("a tag with a key and a value is well formed")
            })
            .collect();
        self.s3()
            .put_bucket_tagging()
            .bucket(bucket)
            .tagging(
                aws_sdk_s3::types::Tagging::builder()
                    .set_tag_set(Some(tag_set))
                    .build()
                    .expect("a tagging with a tag set is well formed"),
            )
            .send()
            .await
            .unwrap_or_else(|error| panic!("tagging {bucket} must succeed, got {error:?}"));
    }

    /// Set `bucket`'s versioning status.
    pub(crate) async fn set_versioning(&self, bucket: &str, status: &str) {
        self.s3()
            .put_bucket_versioning()
            .bucket(bucket)
            .versioning_configuration(
                aws_sdk_s3::types::VersioningConfiguration::builder()
                    .status(aws_sdk_s3::types::BucketVersioningStatus::from(status))
                    .build(),
            )
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("setting {bucket} versioning to {status} must succeed, got {error:?}")
            });
    }

    /// Put the recorder and the delivery channel a rule needs, and start recording.
    pub(crate) async fn recording(&self) {
        self.config(
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
        self.config(
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": { "s3BucketName": "config-delivery" } }),
        )
        .await;
        self.config(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "the-recorder" }),
        )
        .await;
    }

    /// Create an SQS queue and an EventBridge rule on the default bus that targets it,
    /// matching on `source` and `detail-type` alone, and hand back the queue's url.
    async fn queue_fed_by_compliance_changes(&self, queue_name: &str) -> String {
        let created = self
            .sqs("CreateQueue", &json!({ "QueueName": queue_name }))
            .await;
        let queue_url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a QueueUrl, got {created}"))
            .to_string();
        let attributes = self
            .sqs(
                "GetQueueAttributes",
                &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
            )
            .await;
        let queue_arn = attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a QueueArn, got {attributes}"))
            .to_string();
        // The grant a rule's queue target needs: EventBridge makes that target's
        // `sqs:SendMessage` as its own service principal and "relies on resource-based
        // policies" for a queue (`PutTargets`), so a queue that has granted it nothing
        // receives nothing. The helper asserts the write's own 200.
        self.sqs(
            "SetQueueAttributes",
            &json!({
                "QueueUrl": queue_url,
                "Attributes": {
                    "Policy": json!({
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "events.amazonaws.com"},
                            "Action": "sqs:SendMessage",
                            "Resource": queue_arn,
                        }],
                    })
                    .to_string(),
                },
            }),
        )
        .await;
        self.events(
            "PutRule",
            &json!({
                "Name": "compliance-changes",
                "EventPattern": json!({
                    "source": [CONFIG_EVENT_SOURCE],
                    "detail-type": [COMPLIANCE_CHANGE_DETAIL_TYPE],
                })
                .to_string(),
            }),
        )
        .await;
        self.events(
            "PutTargets",
            &json!({
                "Rule": "compliance-changes",
                "Targets": [{ "Id": "the-queue", "Arn": queue_arn }],
            }),
        )
        .await;
        queue_url
    }

    /// Every message on the queue right now, as the event each carries, deleting each
    /// so a later drain reports only what arrived after it.
    async fn drain(&self, queue_url: &str) -> Vec<Value> {
        let mut drained = Vec::new();
        loop {
            let received = self
                .sqs(
                    "ReceiveMessage",
                    &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
                )
                .await;
            let messages = received["Messages"].as_array().cloned().unwrap_or_default();
            if messages.is_empty() {
                return drained;
            }
            for message in messages {
                let body = message["Body"].as_str().unwrap_or_default();
                drained.push(serde_json::from_str(body).unwrap_or(Value::Null));
                if let Some(handle) = message["ReceiptHandle"].as_str() {
                    self.sqs(
                        "DeleteMessage",
                        &json!({ "QueueUrl": queue_url, "ReceiptHandle": handle }),
                    )
                    .await;
                }
            }
        }
    }

    /// The next `want` events the queue receives, waiting up to [`DRAIN_BUDGET`] for
    /// the passes that publish them. Fewer than `want` means they never arrived, which
    /// the caller asserts on.
    async fn drain_at_least(&self, queue_url: &str, want: usize) -> Vec<Value> {
        let deadline = Instant::now() + DRAIN_BUDGET;
        let mut drained = Vec::new();
        while drained.len() < want && Instant::now() < deadline {
            drained.extend(self.drain(queue_url).await);
            if drained.len() < want {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        drained
    }

    /// Whatever the queue receives over [`QUIET_BUDGET`] — several passes' worth of
    /// time — which is how a row asserts that nothing FURTHER is owed.
    async fn drain_while_quiet(&self, queue_url: &str) -> Vec<Value> {
        let deadline = Instant::now() + QUIET_BUDGET;
        let mut drained = Vec::new();
        while Instant::now() < deadline {
            drained.extend(self.drain(queue_url).await);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drained
    }

    /// The verdict the rule `rule` holds for each resource, as
    /// `GetComplianceDetailsByConfigRule` reports it: `(resourceType, resourceId,
    /// complianceType)` per recorded result.
    async fn recorded_verdicts(&self, rule: &str) -> Vec<(String, String, String)> {
        let body = self
            .config(
                "GetComplianceDetailsByConfigRule",
                &json!({ "ConfigRuleName": rule }),
            )
            .await;
        body["EvaluationResults"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|result| {
                let qualifier = &result["EvaluationResultIdentifier"]["EvaluationResultQualifier"];
                let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
                (
                    text(&qualifier["ResourceType"]),
                    text(&qualifier["ResourceId"]),
                    text(&result["ComplianceType"]),
                )
            })
            .collect()
    }

    /// The `ComplianceType` `DescribeComplianceByConfigRule` rolls the rule up to.
    async fn rule_compliance(&self, rule: &str) -> String {
        let body = self
            .config(
                "DescribeComplianceByConfigRule",
                &json!({ "ConfigRuleNames": [rule] }),
            )
            .await;
        body["ComplianceByConfigRules"][0]["Compliance"]["ComplianceType"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The resource ids `ListDiscoveredResources` reports for `resource_type`.
    async fn discovered(&self, resource_type: &str) -> Vec<String> {
        let body = self
            .config(
                "ListDiscoveredResources",
                &json!({ "resourceType": resource_type }),
            )
            .await;
        body["resourceIdentifiers"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|identifier| {
                identifier["resourceId"]
                    .as_str()
                    .map(ToString::to_string)
            })
            .collect()
    }

    /// The configuration items `GetResourceConfigHistory` reports for `bucket`, with
    /// `extra` members (an order, a limit, a token, a time bound) added to the request.
    async fn configuration_history(&self, bucket: &str, extra: Value) -> Value {
        let mut request = json!({
            "resourceType": BUCKET_RESOURCE_TYPE,
            "resourceId": bucket,
        });
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        self.config("GetResourceConfigHistory", &request).await
    }

    /// The versioning status each configuration item of `bucket` records, in the order
    /// the response carried them, waiting until at least `want` items are recorded. A
    /// resource the recorder has not observed yet is refused, which is one of the
    /// states this poll waits out.
    async fn recorded_versioning_states(&self, bucket: &str, want: usize) -> Vec<Option<String>> {
        let deadline = Instant::now() + DRAIN_BUDGET;
        loop {
            let (status, body) = self
                .call(
                    "config",
                    "StarlingDoveService",
                    "1.1",
                    "GetResourceConfigHistory",
                    &json!({
                        "resourceType": BUCKET_RESOURCE_TYPE,
                        "resourceId": bucket,
                    }),
                )
                .await;
            let states = if status == 200 {
                versioning_states(&body)
            } else {
                Vec::new()
            };
            if states.len() >= want || Instant::now() >= deadline {
                return states;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Call `operation` and require a refusal, returning its status and error code.
    pub(crate) async fn refused(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, parsed) = self
            .call("config", "StarlingDoveService", "1.1", operation, body)
            .await;
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
            .unwrap_or_else(|| format!("<no __type in {parsed}>"));
        (status, code)
    }
}

/// The versioning status each configuration item in a `GetResourceConfigHistory`
/// response records, read out of the `supplementaryConfiguration` block Config records
/// it in — which "needs to be deserialized using `json.loads()`"
/// (`ConfigurationItem$supplementaryConfiguration`).
fn versioning_states(history: &Value) -> Vec<Option<String>> {
    history["configurationItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let block = item["supplementaryConfiguration"]["BucketVersioningConfiguration"]
                .as_str()
                .unwrap_or("{}");
            serde_json::from_str::<Value>(block)
                .ok()
                .and_then(|parsed| parsed["status"].as_str().map(ToString::to_string))
        })
        .collect()
}

/// A rule whose managed implementation is `identifier`, scoped to S3 buckets and, when
/// `bucket` is given, to that one bucket.
fn bucket_rule(name: &str, identifier: &str, bucket: Option<&str>) -> Value {
    let mut scope = json!({ "ComplianceResourceTypes": [BUCKET_RESOURCE_TYPE] });
    if let Some(bucket) = bucket {
        scope["ComplianceResourceId"] = Value::String(bucket.to_string());
    }
    json!({
        "ConfigRule": {
            "ConfigRuleName": name,
            "Source": { "Owner": "AWS", "SourceIdentifier": identifier },
            "Scope": scope,
        }
    })
}

/// The verdict an event reports as holding now, the one it reports it replaced, and the
/// resource it is about.
fn reported(event: &Value) -> (Option<&str>, Option<&str>, Option<&str>) {
    (
        event["detail"]["newEvaluationResult"]["complianceType"].as_str(),
        event["detail"]["oldEvaluationResult"]["complianceType"].as_str(),
        event["detail"]["resourceId"].as_str(),
    )
}

/// Assert the envelope every compliance-change event carries, whatever the verdict.
fn assert_envelope(case: &str, position: usize, event: &Value, rule: &str) {
    assert_eq!(
        (
            event["source"].as_str(),
            event["detail-type"].as_str(),
            event["region"].as_str(),
            event["account"].as_str(),
        ),
        (
            Some(CONFIG_EVENT_SOURCE),
            Some(COMPLIANCE_CHANGE_DETAIL_TYPE),
            Some(REGION),
            Some(ACCOUNT_ID),
        ),
        "case {case:?}: event {position} must be a Config compliance change in this \
         Region, got {event}"
    );
    assert_eq!(
        (
            event["detail"]["messageType"].as_str(),
            event["detail"]["newEvaluationResult"]["evaluationResultIdentifier"]
                ["evaluationResultQualifier"]["configRuleName"]
                .as_str(),
            event["detail"]["resourceType"].as_str(),
        ),
        (
            Some(COMPLIANCE_CHANGE_MESSAGE_TYPE),
            Some(rule),
            Some(BUCKET_RESOURCE_TYPE),
        ),
        "case {case:?}: event {position} must name the rule whose verdict changed and \
         the resource it is about, got {event}"
    );
}

/// One row of the verdict table: the versioning status the bucket is created with and
/// the ones applied to it afterwards (`None` meaning never set), and the (new, old)
/// verdict pair of each event the queue must then receive, in order.
struct Case {
    case: &'static str,
    versioning: &'static [Option<&'static str>],
    expected: &'static [(&'static str, Option<&'static str>)],
}

#[tokio::test]
async fn a_recorded_bucket_is_evaluated_and_every_verdict_change_reaches_the_queue() {
    // The rows differ along what the rule generalises over: the recorded configuration
    // the verdict is computed from (versioning never set, suspended, enabled), whether
    // it CHANGES after the first verdict, and whether a change moves the verdict at all
    // — a configuration change that leaves the verdict where it was is not a compliance
    // change and publishes nothing.
    let cases: [Case; 6] = [
        Case {
            case: "versioning never set",
            versioning: &[None],
            expected: &[(NON_COMPLIANT, None)],
        },
        Case {
            case: "versioning enabled before the rule existed",
            versioning: &[Some(ENABLED)],
            expected: &[(COMPLIANT, None)],
        },
        Case {
            case: "versioning suspended before the rule existed",
            versioning: &[Some(SUSPENDED)],
            expected: &[(NON_COMPLIANT, None)],
        },
        Case {
            case: "versioning enabled after the first verdict",
            versioning: &[None, Some(ENABLED)],
            expected: &[(NON_COMPLIANT, None), (COMPLIANT, Some(NON_COMPLIANT))],
        },
        Case {
            case: "versioning suspended after being enabled",
            versioning: &[Some(ENABLED), Some(SUSPENDED)],
            expected: &[(COMPLIANT, None), (NON_COMPLIANT, Some(COMPLIANT))],
        },
        Case {
            case: "a configuration change that does not move the verdict",
            versioning: &[None, Some(SUSPENDED)],
            expected: &[(NON_COMPLIANT, None)],
        },
    ];

    for Case {
        case,
        versioning,
        expected,
    } in cases
    {
        let emulator = Emulator::new();
        let bucket = "evaluated-bucket";
        let rule = "versioning-rule";
        let queue_url = emulator
            .queue_fed_by_compliance_changes("compliance-events")
            .await;
        emulator.recording().await;
        emulator.create_bucket(bucket, &[]).await;
        let (first, rest) = versioning
            .split_first()
            .expect("each row names the bucket's initial versioning state");
        if let Some(status) = first {
            emulator.set_versioning(bucket, status).await;
        }

        // Nothing has been evaluated yet, so nothing has been published.
        let before = emulator.drain(&queue_url).await;
        assert!(
            before.is_empty(),
            "case {case:?}: no event is owed before a rule exists, got {before:?}"
        );

        emulator
            .config("PutConfigRule", &bucket_rule(rule, VERSIONING_RULE, None))
            .await;

        // The first verdict, which no call delivered: Config observed the bucket and
        // evaluated the rule against the configuration item it recorded.
        let mut received = emulator.drain_at_least(&queue_url, 1).await;
        for status in rest {
            if let Some(status) = status {
                emulator.set_versioning(bucket, status).await;
            }
            // Only a verdict CHANGE publishes, so a row expecting no further event
            // waits out several passes instead of waiting for one that never comes.
            received.extend(if expected.len() > received.len() {
                emulator
                    .drain_at_least(&queue_url, expected.len() - received.len())
                    .await
            } else {
                emulator.drain_while_quiet(&queue_url).await
            });
        }

        assert_eq!(
            received.len(),
            expected.len(),
            "case {case:?}: expected one event per verdict change over the versioning \
             states {versioning:?}, got {received:?}"
        );
        for (position, (new_verdict, old_verdict)) in expected.iter().enumerate() {
            let event = &received[position];
            assert_envelope(case, position, event, rule);
            assert_eq!(
                reported(event),
                (Some(*new_verdict), *old_verdict, Some(bucket)),
                "case {case:?}: event {position} must report the new verdict, the one it \
                 replaced and the bucket it is about, got {event}"
            );
        }

        // The verdict that now holds is the one the rule READS BACK, and the rule rolls
        // up to it.
        let (_, last_verdict) = expected
            .last()
            .copied()
            .expect("each row expects at least one event");
        let _ = last_verdict;
        let now_holds = expected
            .last()
            .map(|(verdict, _)| *verdict)
            .expect("each row expects at least one event");
        assert_eq!(
            emulator.recorded_verdicts(rule).await,
            vec![(
                BUCKET_RESOURCE_TYPE.to_string(),
                bucket.to_string(),
                now_holds.to_string(),
            )],
            "case {case:?}: the rule must hold ONE result, for the bucket, carrying \
             {now_holds}"
        );
        let rolled_up = emulator.rule_compliance(rule).await;
        assert_eq!(
            rolled_up, now_holds,
            "case {case:?}: the rule must roll up to {now_holds}, got {rolled_up}"
        );

        // And the resource the verdict is about is in the inventory the recorder wrote.
        let discovered = emulator.discovered(BUCKET_RESOURCE_TYPE).await;
        assert!(
            discovered.contains(&bucket.to_string()),
            "case {case:?}: the recorder must have discovered {bucket}, got {discovered:?}"
        );
    }
}

/// The managed rule this issue's recorded check uses.
const VERSIONING_RULE: &str = "S3_BUCKET_VERSIONING_ENABLED";
/// The catalogue entry this issue did NOT cite: every bucket has a default encryption
/// configuration, so this rule places a bucket COMPLIANT.
const ENCRYPTION_RULE: &str = "S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED";
/// A managed rule whose implementation this emulator does not have, which therefore
/// places no resource at all.
const UNIMPLEMENTED_RULE: &str = "IAM_PASSWORD_POLICY";

#[tokio::test]
async fn a_rule_places_exactly_the_recorded_resources_its_scope_covers() {
    // The rows differ along which rule is written and what its scope narrows to: the
    // managed implementation (two entries of the catalogue, and one the catalogue does
    // not hold), the resource type, one resource id, and a tag.
    let emulator = Emulator::new();
    let queue_url = emulator
        .queue_fed_by_compliance_changes("compliance-events")
        .await;
    emulator.recording().await;
    emulator.create_bucket("scoped-bucket", &[("stage", "prod")]).await;
    emulator.create_bucket("other-bucket", &[]).await;

    // Four rules, written together so one pass evaluates them all.
    let rules: [(&str, &str, &str, Option<&str>, Value); 4] = [
        (
            "the versioning rule, narrowed to one bucket",
            "one-bucket",
            VERSIONING_RULE,
            None,
            bucket_rule("one-bucket", VERSIONING_RULE, Some("scoped-bucket")),
        ),
        (
            "the versioning rule over every recorded bucket",
            "every-bucket",
            VERSIONING_RULE,
            None,
            bucket_rule("every-bucket", VERSIONING_RULE, None),
        ),
        (
            "the encryption rule over every recorded bucket",
            "encryption",
            ENCRYPTION_RULE,
            None,
            bucket_rule("encryption", ENCRYPTION_RULE, None),
        ),
        (
            "a rule narrowed by a tag only one bucket carries",
            "tagged",
            VERSIONING_RULE,
            None,
            json!({
                "ConfigRule": {
                    "ConfigRuleName": "tagged",
                    "Source": { "Owner": "AWS", "SourceIdentifier": VERSIONING_RULE },
                    "Scope": { "TagKey": "stage", "TagValue": "prod" },
                }
            }),
        ),
    ];
    for (case, _, _, _, request) in &rules {
        let answered = emulator.config("PutConfigRule", request).await;
        assert!(
            answered.is_null() || answered.is_object(),
            "case {case:?}: writing the rule must answer the modelled empty response, \
             got {answered}"
        );
    }
    emulator
        .config(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": "unimplemented",
                    "Source": { "Owner": "AWS", "SourceIdentifier": UNIMPLEMENTED_RULE },
                }
            }),
        )
        .await;

    // Five events in all: `one-bucket` and `tagged` place the one bucket their scope
    // covers, `every-bucket` places both, `encryption` places both, and
    // `unimplemented` places none. Waiting for the five the rules above owe is what
    // proves the passes ran.
    let received = emulator.drain_at_least(&queue_url, 6).await;

    let expected: [(&str, &str, &str); 6] = [
        ("one-bucket", "scoped-bucket", NON_COMPLIANT),
        ("every-bucket", "scoped-bucket", NON_COMPLIANT),
        ("every-bucket", "other-bucket", NON_COMPLIANT),
        ("encryption", "scoped-bucket", COMPLIANT),
        ("encryption", "other-bucket", COMPLIANT),
        ("tagged", "scoped-bucket", NON_COMPLIANT),
    ];
    let mut reported_changes: Vec<(String, String, String)> = received
        .iter()
        .map(|event| {
            let detail = &event["detail"];
            (
                detail["configRuleName"].as_str().unwrap_or_default().to_string(),
                detail["resourceId"].as_str().unwrap_or_default().to_string(),
                detail["newEvaluationResult"]["complianceType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    reported_changes.sort();
    let mut wanted: Vec<(String, String, String)> = expected
        .iter()
        .map(|(rule, resource, verdict)| {
            (rule.to_string(), resource.to_string(), verdict.to_string())
        })
        .collect();
    wanted.sort();
    assert_eq!(
        reported_changes, wanted,
        "every verdict each rule's scope covers must be published exactly once, got \
         {received:?}"
    );

    // The same verdicts, read back through the rule's own read path.
    for (case, rule, _, _, _) in &rules {
        let mut verdicts = emulator.recorded_verdicts(rule).await;
        verdicts.sort();
        let mut wanted: Vec<(String, String, String)> = expected
            .iter()
            .filter(|(named, _, _)| named == rule)
            .map(|(_, resource, verdict)| {
                (
                    BUCKET_RESOURCE_TYPE.to_string(),
                    resource.to_string(),
                    verdict.to_string(),
                )
            })
            .collect();
        wanted.sort();
        assert_eq!(
            verdicts, wanted,
            "case {case:?}: {rule} must hold exactly the results its scope covers"
        );
    }

    // A rule whose implementation this emulator does not have places nothing, so it
    // reports the value Config uses for a rule with no results rather than a
    // synthesised verdict.
    let unplaced = emulator.recorded_verdicts("unimplemented").await;
    assert!(
        unplaced.is_empty(),
        "a rule this emulator cannot evaluate must record no result, got {unplaced:?}"
    );
    let unplaced_compliance = emulator.rule_compliance("unimplemented").await;
    assert_eq!(
        unplaced_compliance, INSUFFICIENT_DATA,
        "a rule with no results must report {INSUFFICIENT_DATA}, got {unplaced_compliance}"
    );

    // Every verdict has been published once: the passes that keep running re-evaluate
    // the same resources and find nothing changed.
    let quiet = emulator.drain_while_quiet(&queue_url).await;
    assert!(
        quiet.is_empty(),
        "a re-evaluation that changes no verdict must publish nothing, got {quiet:?}"
    );
}

#[tokio::test]
async fn the_configuration_history_reports_each_state_the_recorder_captured() {
    // A recorder captures a configuration item per STATE of a resource, so the history
    // grows by one item per change it observes and keeps the earlier states. The rows
    // below differ along what the request asks of that history: its order, its page
    // size, its time window, and a resource the recorder never observed.
    let emulator = Emulator::new();
    let bucket = "historied-bucket";
    emulator.recording().await;
    emulator.create_bucket(bucket, &[]).await;

    // The first item: the bucket as created, with versioning never set.
    let first = emulator.recorded_versioning_states(bucket, 1).await;
    assert_eq!(
        first,
        vec![None],
        "the recorder must capture the bucket as created, with no versioning status"
    );

    emulator.set_versioning(bucket, ENABLED).await;

    // The second item: the state the change produced, reported FIRST because the
    // default order is reverse chronological.
    let states = emulator.recorded_versioning_states(bucket, 2).await;
    assert_eq!(
        states,
        vec![Some(ENABLED.to_string()), None],
        "the history must report the state the change produced first and the state it \
         replaced after it"
    );

    // `Forward` lists the same items oldest first.
    let forward = emulator
        .configuration_history(bucket, json!({ "chronologicalOrder": "Forward" }))
        .await;
    assert_eq!(
        versioning_states(&forward),
        vec![None, Some(ENABLED.to_string())],
        "Forward must list the same items oldest first, got {forward}"
    );

    // A page carries at most `limit` items and hands back the token for the next one.
    let page = emulator
        .configuration_history(bucket, json!({ "limit": 1 }))
        .await;
    assert_eq!(
        versioning_states(&page),
        vec![Some(ENABLED.to_string())],
        "a limit of one must carry one item, got {page}"
    );
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a nextToken while items remain, got {page}"))
        .to_string();
    let rest = emulator
        .configuration_history(bucket, json!({ "limit": 1, "nextToken": token }))
        .await;
    assert_eq!(
        (versioning_states(&rest), rest["nextToken"].as_str()),
        (vec![None], None),
        "the next page must carry the remaining item and no further token, got {rest}"
    );

    // Each item names the resource it is about, this account and this Region.
    let items = emulator.configuration_history(bucket, json!({})).await;
    for (position, item) in items["configurationItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        assert_eq!(
            (
                item["resourceType"].as_str(),
                item["resourceId"].as_str(),
                item["awsRegion"].as_str(),
                item["accountId"].as_str(),
                item["configurationItemStatus"].as_str(),
            ),
            (
                Some(BUCKET_RESOURCE_TYPE),
                Some(bucket),
                Some(REGION),
                Some(ACCOUNT_ID),
                Some("OK"),
            ),
            "item {position} must name the recorded resource, got {item}"
        );
    }

    // A time window that ends before anything was recorded contains no state of the
    // resource.
    let before_everything = emulator
        .configuration_history(bucket, json!({ "laterTime": 1.0 }))
        .await;
    assert_eq!(
        versioning_states(&before_everything),
        Vec::<Option<String>>::new(),
        "a window that ends before the first item was captured must report none, got \
         {before_everything}"
    );

    // Two refusals a client branches on: a window whose ends are the wrong way round,
    // and a resource the recorder never observed.
    let (status, code) = emulator
        .refused(
            "GetResourceConfigHistory",
            &json!({
                "resourceType": BUCKET_RESOURCE_TYPE,
                "resourceId": bucket,
                "earlierTime": 2_000_000_000.0,
                "laterTime": 1_000_000_000.0,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidTimeRangeException"),
        "an earlier time after the later time must be refused"
    );
    let (status, code) = emulator
        .refused(
            "GetResourceConfigHistory",
            &json!({
                "resourceType": BUCKET_RESOURCE_TYPE,
                "resourceId": "never-created",
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourceNotDiscoveredException"),
        "a resource the recorder never observed must be refused"
    );
    let (status, code) = emulator
        .refused(
            "GetResourceConfigHistory",
            &json!({
                "resourceType": BUCKET_RESOURCE_TYPE,
                "resourceId": bucket,
                "nextToken": "not-a-token-this-operation-issued",
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidNextTokenException"),
        "a token this operation did not issue must be refused"
    );
}

#[tokio::test]
async fn a_stopped_recorder_observes_nothing_further() {
    // Recording is what makes Config observe a resource at all: a bucket created after
    // the recorder was stopped is not recorded, so no rule places it and no event is
    // published for it.
    let emulator = Emulator::new();
    let queue_url = emulator
        .queue_fed_by_compliance_changes("compliance-events")
        .await;
    emulator.recording().await;
    emulator.create_bucket("recorded-bucket", &[]).await;
    emulator
        .config(
            "PutConfigRule",
            &bucket_rule("versioning-rule", VERSIONING_RULE, None),
        )
        .await;
    let first = emulator.drain_at_least(&queue_url, 1).await;
    assert_eq!(
        first.len(),
        1,
        "the recorded bucket must be evaluated once, got {first:?}"
    );

    emulator
        .config(
            "StopConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "the-recorder" }),
        )
        .await;
    emulator.create_bucket("unrecorded-bucket", &[]).await;

    let quiet = emulator.drain_while_quiet(&queue_url).await;
    assert!(
        quiet.is_empty(),
        "a bucket created while no recorder is recording must be evaluated by nothing, \
         got {quiet:?}"
    );
    let discovered = emulator.discovered(BUCKET_RESOURCE_TYPE).await;
    assert!(
        !discovered.contains(&"unrecorded-bucket".to_string()),
        "a bucket created while no recorder is recording must not be discovered, got \
         {discovered:?}"
    );
}
