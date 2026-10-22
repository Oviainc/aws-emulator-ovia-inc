// SPDX-License-Identifier: CC0-1.0

//! `StartDeliveryStreamEncryption` over the `awsJson1_1` wire: what it writes, what
//! `DescribeDeliveryStream` then reports, and every state it refuses the change in.
//!
//! The rule under test: the call records the requested CMK against an existing, settled
//! Direct PUT stream with the transient `ENABLING` status and leaves the settled
//! `ENABLED` to the clock; a stream it cannot change is refused with the modelled error
//! for the reason, and nothing is written for a refused call.
//!
//! The emulator and its request helpers come from the lifecycle suite beside this one.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_firehose_firehose_20150804::ComAmazonawsFirehoseFirehose20150804State;

use crate::delivery_stream_lifecycle::{
    at, error_code, s3_configuration, Emulator, PAST_THE_TRANSITION, REGION,
};

/// A customer managed CMK in the Region the requests are signed for, satisfying
/// `DeliveryStreamEncryptionConfigurationInput$KeyARN`'s modelled pattern
/// (`^arn:.*:kms:[a-zA-Z0-9\-]+:\d{12}:key/[a-zA-Z_0-9+=,.@\-_/]+$`).
pub(crate) fn key_arn(key: &str) -> String {
    format!("arn:aws:kms:{REGION}:000000000000:key/{key}")
}

/// Create `name` and wait out the `CREATING` -> `ACTIVE` span, so it is settled before an
/// encryption call lands on it.
async fn active_stream(emulator: &Emulator, name: &str) {
    emulator.create_stream(name, None).await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;
}

/// `DeliveryStreamEncryptionConfiguration` as `DescribeDeliveryStream` reports it for
/// `name`.
pub(crate) async fn described_encryption(emulator: &Emulator, name: &str) -> Value {
    let (status, described) = emulator
        .call("DescribeDeliveryStream", json!({ "DeliveryStreamName": name }))
        .await;
    assert_eq!(status, 200, "describe {name}: got {described:?}");
    at(
        &described,
        "DeliveryStreamDescription/DeliveryStreamEncryptionConfiguration",
    )
    .clone()
}

/// The SSE state `DescribeDeliveryStream` reports for a stream no encryption call has
/// touched.
///
/// Firehose always carries `DeliveryStreamEncryptionConfiguration` with a status — the
/// member is readable without calling `StartDeliveryStreamEncryption` first — and with
/// SSE off that status is `DISABLED` and no CMK is named. So "nothing was written" is
/// exactly this block, not an absent member.
fn never_configured_encryption() -> Value {
    json!({ "Status": "DISABLED" })
}

#[tokio::test]
async fn start_encryption_records_the_requested_key_and_the_clock_settles_it_to_enabled() {
    // (case, the request's DeliveryStreamEncryptionConfigurationInput, the KeyType and
    // KeyARN the stream must then report). `KeyType`'s "default setting is
    // AWS_OWNED_CMK", so a request that carries no configuration asks for that.
    let cases: Vec<(&str, Option<Value>, &str, Value)> = vec![
        (
            "no configuration at all",
            None,
            "AWS_OWNED_CMK",
            Value::Null,
        ),
        (
            "an explicit service-owned CMK",
            Some(json!({ "KeyType": "AWS_OWNED_CMK" })),
            "AWS_OWNED_CMK",
            Value::Null,
        ),
        (
            "a customer managed CMK",
            Some(json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn("demo-key") })),
            "CUSTOMER_MANAGED_CMK",
            json!(key_arn("demo-key")),
        ),
        (
            "a customer managed CMK whose key id is a path",
            Some(json!({
                "KeyType": "CUSTOMER_MANAGED_CMK",
                "KeyARN": key_arn("tenants/a_b-1+2=3,4.5@6"),
            })),
            "CUSTOMER_MANAGED_CMK",
            json!(key_arn("tenants/a_b-1+2=3,4.5@6")),
        ),
    ];

    for (index, (case, configuration, expected_key_type, expected_key_arn)) in
        cases.into_iter().enumerate()
    {
        let emulator = Emulator::new();
        let name = format!("start-encryption-{index}");
        active_stream(&emulator, &name).await;

        // Before the call the stream reports the disabled SSE state.
        assert_eq!(
            described_encryption(&emulator, &name).await,
            never_configured_encryption(),
            "case {case:?}: an untouched stream reports the disabled SSE state"
        );

        let mut request = json!({ "DeliveryStreamName": name });
        if let Some(configuration) = configuration.clone() {
            request["DeliveryStreamEncryptionConfigurationInput"] = configuration;
        }
        let (status, started) = emulator
            .call("StartDeliveryStreamEncryption", request)
            .await;
        assert_eq!(status, 200, "case {case:?}: start got {started:?}");
        assert!(
            started.is_null() || started == json!({}),
            "case {case:?}: the operation models no output members, got {started:?}"
        );

        // "Firehose first sets the encryption status of the stream to ENABLING" — the
        // call returns immediately, so a caller acting straight after it sees ENABLING.
        let enabling = described_encryption(&emulator, &name).await;
        assert_eq!(
            enabling["Status"],
            json!("ENABLING"),
            "case {case:?}: right after the call, got {enabling:?}"
        );
        assert_eq!(
            enabling["KeyType"],
            json!(expected_key_type),
            "case {case:?}: right after the call, got {enabling:?}"
        );
        assert_eq!(
            enabling["KeyARN"],
            expected_key_arn,
            "case {case:?}: right after the call, got {enabling:?}"
        );
        // A record written while SSE is still ENABLING is not encrypted yet: "You can
        // continue to read and write data to your Firehose stream while the encryption
        // status is ENABLING, but the data is not encrypted."
        let (status, put) = emulator
            .call(
                "PutRecord",
                json!({
                    "DeliveryStreamName": name,
                    "Record": { "Data": aws_smithy_types::base64::encode("while enabling") },
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: put while enabling got {put:?}");
        assert_eq!(
            put["Encrypted"],
            json!(false),
            "case {case:?}: put while enabling got {put:?}"
        );

        // "and then to ENABLED" — owed to the clock, not to the next call.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let enabled = described_encryption(&emulator, &name).await;
        assert_eq!(
            enabled["Status"],
            json!("ENABLED"),
            "case {case:?}: after the enabling span, got {enabled:?}"
        );
        assert_eq!(
            enabled["KeyType"],
            json!(expected_key_type),
            "case {case:?}: after the enabling span, got {enabled:?}"
        );
        assert_eq!(
            enabled["KeyARN"],
            expected_key_arn,
            "case {case:?}: after the enabling span, got {enabled:?}"
        );

        // With SSE in force a put reports the record as encrypted.
        let (status, put) = emulator
            .call(
                "PutRecord",
                json!({
                    "DeliveryStreamName": name,
                    "Record": { "Data": aws_smithy_types::base64::encode("once enabled") },
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: put once enabled got {put:?}");
        assert_eq!(
            put["Encrypted"],
            json!(true),
            "case {case:?}: put once enabled got {put:?}"
        );
    }
}

#[tokio::test]
async fn start_encryption_on_an_encrypted_stream_changes_the_key_it_reports() {
    // "Even if encryption is currently enabled for a Firehose stream, you can still
    // invoke this operation on it to change the ARN of the CMK or both its type and ARN."
    // Each step is (the configuration asked for, the KeyType and KeyARN it must leave).
    let steps: Vec<(Value, &str, Value)> = vec![
        (
            json!({ "KeyType": "AWS_OWNED_CMK" }),
            "AWS_OWNED_CMK",
            Value::Null,
        ),
        (
            json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn("first-key") }),
            "CUSTOMER_MANAGED_CMK",
            json!(key_arn("first-key")),
        ),
        (
            json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn("second-key") }),
            "CUSTOMER_MANAGED_CMK",
            json!(key_arn("second-key")),
        ),
        (
            json!({ "KeyType": "AWS_OWNED_CMK" }),
            "AWS_OWNED_CMK",
            Value::Null,
        ),
    ];

    let emulator = Emulator::new();
    active_stream(&emulator, "rekeyed-stream").await;

    for (configuration, expected_key_type, expected_key_arn) in steps {
        let case = configuration.to_string();
        let (status, started) = emulator
            .call(
                "StartDeliveryStreamEncryption",
                json!({
                    "DeliveryStreamName": "rekeyed-stream",
                    "DeliveryStreamEncryptionConfigurationInput": configuration,
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: start got {started:?}");

        let enabling = described_encryption(&emulator, "rekeyed-stream").await;
        assert_eq!(
            enabling["Status"],
            json!("ENABLING"),
            "case {case:?}: a re-key starts ENABLING again, got {enabling:?}"
        );

        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let enabled = described_encryption(&emulator, "rekeyed-stream").await;
        assert_eq!(
            enabled["Status"],
            json!("ENABLED"),
            "case {case:?}: after the enabling span, got {enabled:?}"
        );
        assert_eq!(
            enabled["KeyType"],
            json!(expected_key_type),
            "case {case:?}: after the enabling span, got {enabled:?}"
        );
        assert_eq!(
            enabled["KeyARN"],
            expected_key_arn,
            "case {case:?}: after the enabling span, got {enabled:?}"
        );
    }
}

/// How a test sets a stream up so a `StartDeliveryStreamEncryption` on it is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Nothing is created at all.
    NoStream,
    /// Created and left in `CREATING` — the call lands inside the creation span.
    LeftCreating,
    /// Created, settled, then deleted and left in `DELETING`.
    LeftDeleting,
    /// Created, settled, and encryption started — the call lands inside the enabling span.
    LeftEnabling,
    /// Created with the named `DeliveryStreamType` and settled.
    OfType(&'static str),
    /// Created and settled: the refusal is about the request, not the stream.
    Settled,
}

#[tokio::test]
async fn start_encryption_is_refused_with_the_modelled_error_for_the_reason() {
    // (case, how the stream is set up, the configuration asked for, the expected code)
    let cases: Vec<(&str, Setup, Option<Value>, &str)> = vec![
        (
            "no stream of that name",
            Setup::NoStream,
            None,
            "ResourceNotFoundException",
        ),
        (
            "a stream still being created",
            Setup::LeftCreating,
            None,
            "ResourceInUseException",
        ),
        (
            "a stream being deleted",
            Setup::LeftDeleting,
            None,
            "ResourceInUseException",
        ),
        (
            "a stream whose own enabling has not settled",
            Setup::LeftEnabling,
            None,
            "ResourceInUseException",
        ),
        // "You can enable SSE for a Firehose stream only if it's a Firehose stream that
        // uses DirectPut as its source." `DeliveryStreamType`'s other values, from the
        // model's enum: KinesisStreamAsSource, MSKAsSource, DatabaseAsSource.
        (
            "a Kinesis-sourced stream",
            Setup::OfType("KinesisStreamAsSource"),
            None,
            "InvalidArgumentException",
        ),
        (
            "an MSK-sourced stream",
            Setup::OfType("MSKAsSource"),
            None,
            "InvalidArgumentException",
        ),
        (
            "a customer managed CMK without its key ARN",
            Setup::Settled,
            Some(json!({ "KeyType": "CUSTOMER_MANAGED_CMK" })),
            "InvalidArgumentException",
        ),
        (
            "a service-owned CMK with a key ARN",
            Setup::Settled,
            Some(json!({ "KeyType": "AWS_OWNED_CMK", "KeyARN": key_arn("demo-key") })),
            "InvalidArgumentException",
        ),
        (
            "a CMK in another Region",
            Setup::Settled,
            Some(json!({
                "KeyType": "CUSTOMER_MANAGED_CMK",
                "KeyARN": "arn:aws:kms:eu-west-1:000000000000:key/demo-key",
            })),
            "InvalidKMSResourceException",
        ),
    ];

    for (index, (case, setup, configuration, expected_code)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("refused-{index}");

        match setup {
            Setup::NoStream => {}
            Setup::LeftCreating => emulator.create_stream(&name, None).await,
            Setup::LeftDeleting => {
                active_stream(&emulator, &name).await;
                let (status, deleted) = emulator
                    .call("DeleteDeliveryStream", json!({ "DeliveryStreamName": name }))
                    .await;
                assert_eq!(status, 200, "case {case:?}: delete got {deleted:?}");
            }
            Setup::LeftEnabling => {
                active_stream(&emulator, &name).await;
                let (status, started) = emulator
                    .call(
                        "StartDeliveryStreamEncryption",
                        json!({ "DeliveryStreamName": name }),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: first start got {started:?}");
            }
            Setup::OfType(stream_type) => {
                emulator.create_stream(&name, Some(stream_type)).await;
                tokio::time::sleep(PAST_THE_TRANSITION).await;
            }
            Setup::Settled => active_stream(&emulator, &name).await,
        }
        // What the stream reported before the refused call, for the streams there are
        // one of: a refused call must leave exactly this behind.
        let before = if setup == Setup::NoStream {
            None
        } else {
            Some(described_encryption(&emulator, &name).await)
        };

        let mut request = json!({ "DeliveryStreamName": name });
        if let Some(configuration) = configuration {
            request["DeliveryStreamEncryptionConfigurationInput"] = configuration;
        }
        let (status, refused) = emulator
            .call("StartDeliveryStreamEncryption", request)
            .await;
        assert_eq!(status, 400, "case {case:?}: got {status} {refused:?}");
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: got {refused:?}"
        );

        // A refused call writes nothing: whatever the stream reported before it, it
        // reports after it too.
        if let Some(before) = before {
            assert_eq!(
                described_encryption(&emulator, &name).await,
                before,
                "case {case:?}: a refused call must not change the stream"
            );
        }
    }
}

#[tokio::test]
async fn the_combined_encryption_call_limit_is_counted_per_stream_over_its_window() {
    // "The StartDeliveryStreamEncryption and StopDeliveryStreamEncryption operations have
    // a combined limit of 25 calls per Firehose stream per 24 hours."
    const LIMIT: usize = 25;
    const WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

    // (case, how many calls already landed and how long ago, whether the next one is
    // taken). Seeding the earlier calls is what keeps the window testable without
    // waiting 24 hours for it.
    let cases: Vec<(&str, usize, Duration, bool)> = vec![
        ("no earlier call", 0, Duration::ZERO, true),
        ("one short of the limit", LIMIT - 1, Duration::ZERO, true),
        ("exactly the limit", LIMIT, Duration::ZERO, false),
        ("more than the limit", LIMIT + 7, Duration::ZERO, false),
        (
            "the limit, just inside the window",
            LIMIT,
            WINDOW - Duration::from_secs(60),
            false,
        ),
        (
            "the limit, just outside the window",
            LIMIT,
            WINDOW + Duration::from_secs(60),
            true,
        ),
    ];

    for (index, (case, earlier_calls, ago, taken)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("budgeted-{index}");
        active_stream(&emulator, &name).await;

        // No modelled operation reports the budget, so the earlier calls are seeded into
        // the state this one is counted from.
        let landed_at = SystemTime::now() - ago;
        {
            let state = emulator
                .engine()
                .state::<ComAmazonawsFirehoseFirehose20150804State>();
            let mut store = state.lock();
            let stream = store
                .streams
                .get_mut(&name)
                .unwrap_or_else(|| panic!("case {case:?}: the stream was created"));
            stream.encryption_calls = vec![landed_at; earlier_calls];
        }

        let (status, answered) = emulator
            .call(
                "StartDeliveryStreamEncryption",
                json!({ "DeliveryStreamName": name }),
            )
            .await;
        if taken {
            assert_eq!(status, 200, "case {case:?}: got {status} {answered:?}");
            assert_eq!(
                described_encryption(&emulator, &name).await["Status"],
                json!("ENABLING"),
                "case {case:?}: a call inside the budget starts the change"
            );
        } else {
            assert_eq!(status, 400, "case {case:?}: got {status} {answered:?}");
            assert_eq!(
                error_code(&answered),
                "LimitExceededException",
                "case {case:?}: got {answered:?}"
            );
            assert_eq!(
                described_encryption(&emulator, &name).await,
                never_configured_encryption(),
                "case {case:?}: a refused call must not start the change"
            );
        }
    }
}

#[tokio::test]
async fn describing_a_stream_does_not_spend_its_encryption_call_budget() {
    let emulator = Emulator::new();
    active_stream(&emulator, "polled-stream").await;

    // Reads must not change what a later caller sees: polling the stream cannot consume
    // the budget the encryption calls are counted against.
    for poll in 0..40 {
        let described = described_encryption(&emulator, "polled-stream").await;
        assert_eq!(
            described,
            never_configured_encryption(),
            "poll {poll}: no encryption has been asked for yet"
        );
        let (status, listed) = emulator.call("ListDeliveryStreams", json!({})).await;
        assert_eq!(status, 200, "poll {poll}: list got {listed:?}");
    }

    let (status, started) = emulator
        .call(
            "StartDeliveryStreamEncryption",
            json!({
                "DeliveryStreamName": "polled-stream",
                "DeliveryStreamEncryptionConfigurationInput": {
                    "KeyType": "CUSTOMER_MANAGED_CMK",
                    "KeyARN": key_arn("after-polling"),
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a start after 40 reads is still inside the budget, got {started:?}"
    );
}

#[tokio::test]
async fn the_encryption_call_budget_is_per_stream() {
    let emulator = Emulator::new();
    for name in ["stream-a", "stream-b"] {
        active_stream(&emulator, name).await;
    }

    // Spend stream-a's whole budget, leaving stream-b's untouched.
    {
        let state = emulator
            .engine()
            .state::<ComAmazonawsFirehoseFirehose20150804State>();
        let mut store = state.lock();
        let stream = store
            .streams
            .get_mut("stream-a")
            .expect("stream-a was created");
        stream.encryption_calls = vec![SystemTime::now(); 25];
    }

    let (status, refused) = emulator
        .call(
            "StartDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": "stream-a" }),
        )
        .await;
    assert_eq!(status, 400, "stream-a is out of budget, got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "stream-a is out of budget, got {refused:?}"
    );

    let (status, accepted) = emulator
        .call(
            "StartDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": "stream-b" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "stream-b has its own budget, got {accepted:?}"
    );
    assert_eq!(
        described_encryption(&emulator, "stream-b").await["Status"],
        json!("ENABLING"),
        "stream-b's own call is taken"
    );
    assert_eq!(
        described_encryption(&emulator, "stream-a").await,
        never_configured_encryption(),
        "the refused call left stream-a alone"
    );
}

#[tokio::test]
async fn an_unsettled_encryption_change_does_not_block_a_create_or_a_put() {
    let emulator = Emulator::new();
    active_stream(&emulator, "busy-stream").await;

    let (status, started) = emulator
        .call(
            "StartDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": "busy-stream" }),
        )
        .await;
    assert_eq!(status, 200, "start got {started:?}");

    // The stream itself stays ACTIVE while its encryption is ENABLING: only the
    // encryption status moves.
    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "busy-stream" }),
        )
        .await;
    assert_eq!(status, 200, "describe got {described:?}");
    assert_eq!(
        at(&described, "DeliveryStreamDescription/DeliveryStreamStatus"),
        &json!("ACTIVE"),
        "the stream stays ACTIVE while SSE is enabling, got {described:?}"
    );
    // "You can continue to read and write data to your Firehose stream while the
    // encryption status is ENABLING."
    let (status, put) = emulator
        .call(
            "PutRecordBatch",
            json!({
                "DeliveryStreamName": "busy-stream",
                "Records": [
                    { "Data": aws_smithy_types::base64::encode("one") },
                    { "Data": aws_smithy_types::base64::encode("two") },
                ],
            }),
        )
        .await;
    assert_eq!(status, 200, "batch while enabling got {put:?}");
    assert_eq!(
        put["FailedPutCount"],
        json!(0),
        "batch while enabling got {put:?}"
    );

    // Another stream can still be created with SSE asked for at create time.
    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "other-stream",
                "S3DestinationConfiguration": s3_configuration(),
                "DeliveryStreamEncryptionConfigurationInput": {
                    "KeyType": "CUSTOMER_MANAGED_CMK",
                    "KeyARN": key_arn("other-key"),
                },
            }),
        )
        .await;
    assert_eq!(status, 200, "create with SSE got {created:?}");
}
