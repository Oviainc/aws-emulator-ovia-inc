// SPDX-License-Identifier: CC0-1.0

//! Deregistering an enhanced fan-out consumer, end to end over the wire:
//! `RegisterStreamConsumer` -> `DeregisterStreamConsumer` -> `DescribeStreamConsumer` /
//! `DescribeStreamSummary` / `DeleteStream`.
//!
//! RULE: `DeregisterStreamConsumer` takes the consumer its members name OFF the stream it
//! was registered with, resolving it by `ConsumerARN`, by `StreamARN` and `ConsumerName`, or
//! by all three as long as they name one consumer — the rule the operation publishes ("To
//! deregister a consumer, provide its ARN. Alternatively, you can provide the ARN of the
//! data stream and the name you gave the consumer when you registered it. You may also
//! provide all three parameters, as long as they don't conflict with each other"), which is
//! the rule a read of a consumer resolves by. It answers 200 with no members, and from then
//! on that consumer is not registered: a read of it is `ResourceNotFoundException`, the
//! stream's `ConsumerCount` no longer counts it, every other consumer of the stream is
//! untouched, the name is free for a registration that mints a new consumer, and a stream
//! whose consumers have all been deregistered is deleted without
//! `EnforceConsumerDeletion`. A request naming no consumer, or whose members name different
//! consumers, is `InvalidArgumentException` and changes nothing; one naming a stream or a
//! consumer this account and region do not hold is `ResourceNotFoundException`.
//!
//! The recorded flow this was derived from deregisters ONE consumer of ONE stream by its
//! ARN and is one row of the table below. The rows differ along what the rule generalises
//! over: which identifiers the request resolves the consumer by, the form and length of
//! `ConsumerName` inside its `@pattern`/`@length`, the status the consumer is in when it is
//! deregistered (`CREATING` or `ACTIVE`), how many other consumers the stream carries, and
//! the capacity mode of the stream.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    consumer_not_found_message, stream_not_found_message, ACTIVATION_DELAY, CONSUMER_ACTIVATION_DELAY,
    CONSUMER_ACTIVE, CONSUMER_CREATING, CONSUMER_IDENTIFIER_CONFLICT_MESSAGE,
    CONSUMER_IDENTIFIER_REQUIRED_MESSAGE, STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region most cases sign for.
const REGION: &str = "us-east-1";

/// Another region, used where a case checks that a consumer is addressed inside the region
/// its stream lives in.
const OTHER_REGION: &str = "eu-west-1";

/// The `X-Amz-Target` prefix of this service (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "Kinesis_20131202";

/// The `@length` maximum of `ConsumerName`, in characters
/// (`smithy_query(query="operation",operation="DeregisterStreamConsumer")` -> `ConsumerName`
/// len 1..128).
const CONSUMER_NAME_MAX_LENGTH: usize = 128;

/// How long past a transition's due instant a case waits before reading the settled state.
const SETTLE_SLACK: Duration = Duration::from_millis(400);

/// A router carrying exactly this service, over one shared engine: the same assembly
/// `core/router` performs, with one service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` with `body`, signed for `region` so the service resolves the
    /// request's region the way it does in production (the SigV4 credential scope).
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kinesis/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// [`Self::call_in`] in the default test region.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Call `operation`, asserting a 200 and answering the body.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body \
             {response}"
        );
        response
    }

    /// Create a stream in the default region, wait out the transition the create armed and
    /// answer its ARN: a consumer is only registered with a stream whose create has settled.
    async fn active_stream(&self, case: &str, stream_name: &str, stream_mode: &str) -> String {
        let mut body = json!({
            "StreamName": stream_name,
            "StreamModeDetails": { "StreamMode": stream_mode },
        });
        if stream_mode == STREAM_MODE_PROVISIONED {
            body["ShardCount"] = json!(1);
        }
        self.ok(case, "CreateStream", &body).await;
        // The transition is owed at a stored instant, so waiting past it is all it takes.
        tokio::time::sleep(ACTIVATION_DELAY + SETTLE_SLACK).await;
        let summary = self
            .stream_summary(case, stream_name_body(stream_name))
            .await;
        text(&summary, "StreamARN")
    }

    /// Register `consumer_name` with the stream `stream_arn` addresses and answer its ARN.
    async fn register(&self, case: &str, stream_arn: &str, consumer_name: &str) -> String {
        let registered = self
            .ok(
                case,
                "RegisterStreamConsumer",
                &json!({ "StreamARN": stream_arn, "ConsumerName": consumer_name }),
            )
            .await;
        text(
            &registered.get("Consumer").cloned().unwrap_or(Value::Null),
            "ConsumerARN",
        )
    }

    /// The `StreamDescriptionSummary` of the stream `body` addresses.
    async fn stream_summary(&self, case: &str, body: Value) -> Value {
        self.ok(case, "DescribeStreamSummary", &body)
            .await
            .get("StreamDescriptionSummary")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// How many consumers the stream `stream_arn` addresses counts, which is what
    /// `StreamDescriptionSummary$ConsumerCount` reports.
    async fn consumer_count(&self, case: &str, stream_arn: &str) -> u64 {
        let summary = self
            .stream_summary(case, json!({ "StreamARN": stream_arn }))
            .await;
        summary
            .get("ConsumerCount")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    }

    /// The `ConsumerStatus` the read `body` addresses reports, or `None` when no consumer
    /// answers to it.
    async fn consumer_status(&self, body: &Value) -> Option<String> {
        let (status, response) = self.call("DescribeStreamConsumer", body).await;
        if status != 200 {
            return None;
        }
        Some(text(
            &response
                .get("ConsumerDescription")
                .cloned()
                .unwrap_or(Value::Null),
            "ConsumerStatus",
        ))
    }
}

/// A `DescribeStreamSummary` body addressing a stream by name.
fn stream_name_body(stream_name: &str) -> Value {
    json!({ "StreamName": stream_name })
}

/// The `__type` an awsJson1_1 error body carries, reduced to the error's shape name.
fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` an awsJson1_1 error body carries, under either casing the protocols use.
fn error_message(response: &Value) -> String {
    response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A string member of `value`, or the empty string when the body carries none.
fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `ConsumerName` of `length` characters inside the modelled `^[a-zA-Z0-9_.-]+$`.
fn name_of_length(length: usize) -> String {
    std::iter::repeat_n('c', length).collect()
}

/// Which identifiers a deregistration names the consumer by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolveBy {
    /// `ConsumerARN` alone.
    ConsumerArn,
    /// `StreamARN` and `ConsumerName`.
    StreamAndName,
    /// All three, which name one consumer.
    AllThree,
}

impl ResolveBy {
    /// The `DeregisterStreamConsumerInput` body this identifier set sends for the consumer
    /// `consumer_name` / `consumer_arn` names on `stream_arn`.
    fn body(self, stream_arn: &str, consumer_name: &str, consumer_arn: &str) -> Value {
        match self {
            Self::ConsumerArn => json!({ "ConsumerARN": consumer_arn }),
            Self::StreamAndName => {
                json!({ "StreamARN": stream_arn, "ConsumerName": consumer_name })
            }
            Self::AllThree => json!({
                "StreamARN": stream_arn,
                "ConsumerName": consumer_name,
                "ConsumerARN": consumer_arn,
            }),
        }
    }
}

/// One deregistration: the stream the consumer is registered with, the consumer, and how
/// the request names it.
struct DeregisterCase {
    /// What an assertion calls this row.
    label: &'static str,
    stream_name: &'static str,
    /// The capacity mode of the stream the consumer is registered with: a consumer is
    /// registered with, and deregistered from, a stream of either mode.
    stream_mode: &'static str,
    consumer_name: String,
    /// The other consumers of the same stream, which the deregistration must leave alone.
    bystanders: Vec<&'static str>,
    /// Whether the case waits for the consumer's `CREATING` -> `ACTIVE` transition before
    /// deregistering it: either status is deregistered.
    settle_first: bool,
    resolve_by: ResolveBy,
}

/// The table the deregistration rule is checked over. The rows differ along the identifiers
/// the request resolves the consumer by, the form and length of `ConsumerName` (both
/// published `@length` bounds and a punctuated name inside the pattern), the status the
/// consumer is in when it is deregistered, how many other consumers the stream carries, and
/// the stream's capacity mode.
fn deregister_cases() -> Vec<DeregisterCase> {
    vec![
        DeregisterCase {
            label: "by-consumer-arn-while-creating",
            stream_name: "sim2real-deregister-a",
            stream_mode: STREAM_MODE_PROVISIONED,
            // The modelled `@length` minimum of `ConsumerName`.
            consumer_name: name_of_length(1),
            bystanders: Vec::new(),
            settle_first: false,
            resolve_by: ResolveBy::ConsumerArn,
        },
        DeregisterCase {
            label: "by-stream-and-name-once-active",
            stream_name: "sim2real-deregister-b",
            stream_mode: STREAM_MODE_PROVISIONED,
            consumer_name: "sim.2real_consumer-a.1".to_string(),
            bystanders: vec!["sim2real-bystander-1", "sim2real-bystander-2"],
            settle_first: true,
            resolve_by: ResolveBy::StreamAndName,
        },
        DeregisterCase {
            label: "by-all-three-longest-name-on-demand-stream",
            stream_name: "sim2real-deregister-c",
            stream_mode: STREAM_MODE_ON_DEMAND,
            consumer_name: name_of_length(CONSUMER_NAME_MAX_LENGTH),
            bystanders: vec!["sim2real-bystander-3"],
            settle_first: false,
            resolve_by: ResolveBy::AllThree,
        },
    ]
}

#[tokio::test]
async fn a_deregistered_consumer_is_no_longer_registered_with_its_stream() {
    let emulator = Emulator::new();

    for case in deregister_cases() {
        let label = case.label;
        let stream_arn = emulator
            .active_stream(label, case.stream_name, case.stream_mode)
            .await;

        let consumer_arn = emulator
            .register(label, &stream_arn, &case.consumer_name)
            .await;
        let mut bystander_arns = Vec::new();
        for bystander in &case.bystanders {
            bystander_arns.push(emulator.register(label, &stream_arn, bystander).await);
        }
        let registered = 1 + case.bystanders.len() as u64;
        let counted = emulator.consumer_count(label, &stream_arn).await;
        assert_eq!(
            counted, registered,
            "case {label}: expected the stream to carry {registered} consumers before the \
             deregistration, got {counted}"
        );

        if case.settle_first {
            // The transition is owed at a stored instant, so waiting past it is all it takes.
            tokio::time::sleep(CONSUMER_ACTIVATION_DELAY + SETTLE_SLACK).await;
        }
        let expected_status = if case.settle_first {
            CONSUMER_ACTIVE
        } else {
            CONSUMER_CREATING
        };
        let status_before = emulator
            .consumer_status(&json!({ "ConsumerARN": consumer_arn }))
            .await;
        assert_eq!(
            status_before.as_deref(),
            Some(expected_status),
            "case {label}: expected the consumer to be {expected_status} when it is \
             deregistered, got {status_before:?}"
        );

        let body = case
            .resolve_by
            .body(&stream_arn, &case.consumer_name, &consumer_arn);
        let answer = emulator.ok(label, "DeregisterStreamConsumer", &body).await;
        let members = answer.as_object().map(serde_json::Map::len).unwrap_or(0);
        assert_eq!(
            members, 0,
            "case {label}: DeregisterStreamConsumer models no output members, got {answer}"
        );

        // The consumer is gone, by EVERY identifier a read resolves one by.
        let reads = vec![
            ("by-consumer-arn", json!({ "ConsumerARN": consumer_arn })),
            (
                "by-stream-and-name",
                json!({ "StreamARN": stream_arn, "ConsumerName": case.consumer_name }),
            ),
        ];
        for (how, read) in &reads {
            let read_label = format!("{label}/{how}");
            let (status, response) = emulator.call("DescribeStreamConsumer", read).await;
            assert_eq!(
                status, 400,
                "case {read_label}: a deregistered consumer must not be reported, got \
                 {status} with body {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {read_label}: expected ResourceNotFoundException, got body {response}"
            );
        }

        // A second deregistration of the same consumer names a consumer that is no longer
        // registered, so it is refused rather than served again.
        let (status, response) = emulator.call("DeregisterStreamConsumer", &body).await;
        assert_eq!(
            status, 400,
            "case {label}: deregistering an absent consumer must be refused, got {status} \
             with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {label}: expected ResourceNotFoundException, got body {response}"
        );

        // Every other consumer of the stream is untouched, and the count tracks exactly the
        // consumers still registered.
        for (bystander, arn) in case.bystanders.iter().zip(&bystander_arns) {
            let status = emulator
                .consumer_status(&json!({ "ConsumerARN": arn }))
                .await;
            assert!(
                status.is_some(),
                "case {label}/{bystander}: the other consumers of the stream must survive \
                 the deregistration, got {status:?}"
            );
        }
        let counted = emulator.consumer_count(label, &stream_arn).await;
        let remaining = case.bystanders.len() as u64;
        assert_eq!(
            counted, remaining,
            "case {label}: expected the stream to carry {remaining} consumers after the \
             deregistration, got {counted}"
        );

        // The name the deregistered consumer held is free again, and a registration under it
        // mints a consumer that starts over in CREATING.
        emulator
            .register(label, &stream_arn, &case.consumer_name)
            .await;
        let reborn = emulator
            .consumer_status(
                &json!({ "StreamARN": stream_arn, "ConsumerName": case.consumer_name }),
            )
            .await;
        assert_eq!(
            reborn.as_deref(),
            Some(CONSUMER_CREATING),
            "case {label}: a registration under a deregistered consumer's name must mint a \
             new consumer, got {reborn:?}"
        );
    }
}

#[tokio::test]
async fn a_deregistration_naming_no_one_consumer_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let case = "deregister-refusals";
    let stream_name = "sim2real-deregister-refused";
    let stream_arn = emulator
        .active_stream(case, stream_name, STREAM_MODE_PROVISIONED)
        .await;
    let consumer_name = "sim2real-kept-consumer";
    let consumer_arn = emulator.register(case, &stream_arn, consumer_name).await;

    // An ARN of a stream this account and region do not hold, inside the modelled
    // `StreamARN` pattern.
    let unknown_stream = format!(
        "arn:aws:kinesis:{REGION}:{}:stream/sim2real-deregister-absent",
        engine::EMULATOR_ACCOUNT_ID
    );
    let absent = "sim2real-absent-consumer";
    // Each row: the region the request is signed for, its body, the modelled error code it
    // must answer, and the message that code carries. The rows span every way a request
    // names no ONE registered consumer.
    let rows: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "no-identifier",
            REGION,
            json!({}),
            "InvalidArgumentException",
            CONSUMER_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        ),
        (
            "stream-without-a-name",
            REGION,
            json!({ "StreamARN": stream_arn }),
            "InvalidArgumentException",
            CONSUMER_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        ),
        (
            "conflicting-identifiers",
            REGION,
            json!({ "ConsumerARN": consumer_arn, "ConsumerName": absent }),
            "InvalidArgumentException",
            CONSUMER_IDENTIFIER_CONFLICT_MESSAGE.to_string(),
        ),
        (
            "unknown-stream",
            REGION,
            json!({ "StreamARN": unknown_stream, "ConsumerName": consumer_name }),
            "ResourceNotFoundException",
            stream_not_found_message(&unknown_stream),
        ),
        (
            "unknown-consumer-name",
            REGION,
            json!({ "StreamARN": stream_arn, "ConsumerName": absent }),
            "ResourceNotFoundException",
            consumer_not_found_message(absent, &stream_arn),
        ),
        (
            "unknown-consumer-arn",
            REGION,
            json!({ "ConsumerARN": format!("{stream_arn}/consumer/{absent}:1") }),
            "ResourceNotFoundException",
            consumer_not_found_message(&format!("{stream_arn}/consumer/{absent}:1"), ""),
        ),
        (
            // A consumer is addressed inside the region its stream lives in, so the same
            // ARN names no consumer of another region's endpoint.
            "another-region",
            OTHER_REGION,
            json!({ "ConsumerARN": consumer_arn }),
            "ResourceNotFoundException",
            consumer_not_found_message(&consumer_arn, ""),
        ),
    ];
    for (row, region, body, expected_code, expected_message) in &rows {
        let label = format!("{case}/{row}");
        let (status, response) = emulator
            .call_in(region, "DeregisterStreamConsumer", body)
            .await;
        assert_eq!(
            status, 400,
            "case {label}: expected the request to be refused, got {status} with body \
             {response}"
        );
        assert_eq!(
            &error_code(&response),
            expected_code,
            "case {label}: expected {expected_code}, got body {response}"
        );
        assert_eq!(
            &error_message(&response),
            expected_message,
            "case {label}: the refusal must be worded by the rule, got body {response}"
        );
    }

    // The refusal names the stream the consumer was looked for under by its NAME, which is
    // what the addressing ARN carries, rather than echoing the ARN the request sent.
    let (_, not_found) = emulator
        .call(
            "DeregisterStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": absent }),
        )
        .await;
    let message = error_message(&not_found);
    assert!(
        message.contains(stream_name) && !message.contains(stream_arn.as_str()),
        "case {case}: the refusal must name the stream by its name rather than its ARN, got \
         {message}"
    );
    // The wording of this error class, pinned literally once so a reworded refusal is
    // caught here rather than compared against the implementation's own helper.
    let expected = format!(
        "Consumer {absent} under stream: {stream_name}, account {} not found.",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert_eq!(
        message, expected,
        "case {case}: the refusal must be worded as this error class words it, got {message}"
    );

    // Every refusal left the consumer exactly as it was.
    let status = emulator
        .consumer_status(&json!({ "ConsumerARN": consumer_arn }))
        .await;
    assert!(
        status.is_some(),
        "case {case}: a refused deregistration must leave the consumer registered, got \
         {status:?}"
    );
    let counted = emulator.consumer_count(case, &stream_arn).await;
    assert_eq!(
        counted, 1,
        "case {case}: expected the stream to still carry its one consumer, got {counted}"
    );
}

#[tokio::test]
async fn a_stream_whose_consumers_were_all_deregistered_is_deleted_without_the_enforce_flag() {
    let emulator = Emulator::new();
    let case = "deregister-then-delete";
    let stream_name = "sim2real-deregister-then-delete";
    let stream_arn = emulator
        .active_stream(case, stream_name, STREAM_MODE_PROVISIONED)
        .await;
    let consumers = ["sim2real-first", "sim2real-second"];
    for consumer in &consumers {
        emulator.register(case, &stream_arn, consumer).await;
    }

    // "If this parameter is unset (null)... and the stream has registered consumers, the
    // call to DeleteStream fails with a ResourceInUseException"
    // (`DeleteStreamInput$EnforceConsumerDeletion`): while the stream carries consumers the
    // delete is refused, which is what makes this a check of the STORED record rather than
    // of what a read reports.
    let (status, response) = emulator
        .call("DeleteStream", &json!({ "StreamARN": stream_arn }))
        .await;
    assert_eq!(
        status, 400,
        "case {case}: a stream with registered consumers must not be deleted, got {status} \
         with body {response}"
    );

    for (index, consumer) in consumers.iter().enumerate() {
        emulator
            .ok(
                case,
                "DeregisterStreamConsumer",
                &json!({ "StreamARN": stream_arn, "ConsumerName": consumer }),
            )
            .await;
        let counted = emulator.consumer_count(case, &stream_arn).await;
        let remaining = (consumers.len() - index - 1) as u64;
        assert_eq!(
            counted, remaining,
            "case {case}/{consumer}: expected {remaining} consumers left, got {counted}"
        );
    }

    // Nothing stands in the way of the removal any more: the consumers are not registered
    // with the stream, so the delete that was refused is served.
    emulator
        .ok(case, "DeleteStream", &json!({ "StreamARN": stream_arn }))
        .await;
}
