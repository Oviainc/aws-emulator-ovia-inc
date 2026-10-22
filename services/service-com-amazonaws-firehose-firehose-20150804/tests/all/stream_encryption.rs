// SPDX-License-Identifier: CC0-1.0

//! `StopDeliveryStreamEncryption` over the `awsJson1_1` wire: what it writes, what
//! `DescribeDeliveryStream` then reports, every state it refuses the change in, and the
//! call budget it shares with `StartDeliveryStreamEncryption`.
//!
//! The rule under test: the call takes an existing, settled stream that has SSE in force
//! and leaves its encryption configuration in the transient `DISABLING` status, the
//! settled `DISABLED` owed to the clock; a stream with no encryption in force has nothing
//! to disable and is refused, as is one whose own state has not settled; and nothing is
//! written for a refused call.
//!
//! The emulator and its request helpers come from the lifecycle suite beside this one;
//! `StartDeliveryStreamEncryption`'s own rules are covered by `delivery_stream_encryption`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_firehose_firehose_20150804::ComAmazonawsFirehoseFirehose20150804State;

// The CMK fixture and the describe-the-encryption helper are the start suite's: one copy
// for both halves of the pair, so they cannot drift apart.
use crate::delivery_stream_encryption::{described_encryption, key_arn};
use crate::delivery_stream_lifecycle::{
    error_code, s3_configuration, Emulator, PAST_THE_TRANSITION,
};

/// How SSE came to be in force on the stream a stop is then asked for. The two paths are
/// the only ways this service turns SSE on, and a stop has to work for either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnabledBy {
    /// `CreateDeliveryStream` carried a `DeliveryStreamEncryptionConfigurationInput`.
    Create,
    /// `StartDeliveryStreamEncryption` turned it on after the create.
    Start,
}

/// Create `name` with SSE in force under `configuration`, by the named path, and wait
/// until both the stream and its encryption have settled.
async fn encrypted_stream(
    emulator: &Emulator,
    name: &str,
    enabled_by: EnabledBy,
    configuration: &Value,
) {
    match enabled_by {
        EnabledBy::Create => {
            let (status, created) = emulator
                .call(
                    "CreateDeliveryStream",
                    json!({
                        "DeliveryStreamName": name,
                        "S3DestinationConfiguration": s3_configuration(),
                        "DeliveryStreamEncryptionConfigurationInput": configuration,
                    }),
                )
                .await;
            assert_eq!(status, 200, "create {name} with SSE: got {created:?}");
            tokio::time::sleep(PAST_THE_TRANSITION).await;
        }
        EnabledBy::Start => {
            emulator.create_stream(name, None).await;
            tokio::time::sleep(PAST_THE_TRANSITION).await;
            let (status, started) = emulator
                .call(
                    "StartDeliveryStreamEncryption",
                    json!({
                        "DeliveryStreamName": name,
                        "DeliveryStreamEncryptionConfigurationInput": configuration,
                    }),
                )
                .await;
            assert_eq!(status, 200, "start SSE on {name}: got {started:?}");
            tokio::time::sleep(PAST_THE_TRANSITION).await;
        }
    }
}

#[tokio::test]
async fn stop_encryption_leaves_disabling_and_the_clock_settles_it_to_disabled() {
    // (case, how SSE was turned on, the configuration it was turned on with). The
    // `KeyType` rows are the whole catalog the model publishes —
    // `smithy_query(query="shape",shape="KeyType")` -> ["AWS_OWNED_CMK",
    // "CUSTOMER_MANAGED_CMK"] — and a customer managed CMK carries a `KeyARN` an Amazon
    // owned one must not, so the rows differ in that member too.
    let cases: Vec<(&str, EnabledBy, Value)> = vec![
        (
            "a service-owned CMK asked for at create",
            EnabledBy::Create,
            json!({ "KeyType": "AWS_OWNED_CMK" }),
        ),
        (
            "a customer managed CMK asked for at create",
            EnabledBy::Create,
            json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn("created-key") }),
        ),
        (
            "a service-owned CMK started after the create",
            EnabledBy::Start,
            json!({ "KeyType": "AWS_OWNED_CMK" }),
        ),
        (
            "a customer managed CMK started after the create",
            EnabledBy::Start,
            json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn("started-key") }),
        ),
    ];

    for (index, (case, enabled_by, configuration)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("stop-encryption-{index}");
        encrypted_stream(&emulator, &name, enabled_by, &configuration).await;

        let enabled = described_encryption(&emulator, &name).await;
        assert_eq!(
            enabled["Status"],
            json!("ENABLED"),
            "case {case:?}: SSE is in force before the stop, got {enabled:?}"
        );

        let (status, stopped) = emulator
            .call(
                "StopDeliveryStreamEncryption",
                json!({ "DeliveryStreamName": name }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: stop got {status} {stopped:?}");
        assert!(
            stopped.is_null() || stopped == json!({}),
            "case {case:?}: the operation models no output members, got {stopped:?}"
        );

        // "When you invoke it, Firehose first sets the encryption status of the stream to
        // DISABLING" — the call returns immediately, so a caller acting straight after it
        // sees DISABLING. The CMK stays named while the disable is in flight: Firehose
        // "schedules the related KMS grant for retirement and then retires it after it
        // ensures that it is finished delivering records to the destination".
        let disabling = described_encryption(&emulator, &name).await;
        assert_eq!(
            disabling["Status"],
            json!("DISABLING"),
            "case {case:?}: right after the call, got {disabling:?}"
        );
        assert_eq!(
            disabling["KeyType"], enabled["KeyType"],
            "case {case:?}: the stop changes only the status, got {disabling:?}"
        );
        assert_eq!(
            disabling["KeyARN"], enabled["KeyARN"],
            "case {case:?}: the stop changes only the status, got {disabling:?}"
        );
        // "You can continue to read and write data to your stream while its status is
        // DISABLING."
        let (status, put) = emulator
            .call(
                "PutRecord",
                json!({
                    "DeliveryStreamName": name,
                    "Record": { "Data": aws_smithy_types::base64::encode("while disabling") },
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: put while disabling got {put:?}");

        // "and then to DISABLED" — owed to the clock, not to the next call.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let disabled = described_encryption(&emulator, &name).await;
        assert_eq!(
            disabled["Status"],
            json!("DISABLED"),
            "case {case:?}: after the disabling span, got {disabled:?}"
        );
        // Once SSE is off a put is no longer answered as encrypted.
        let (status, put) = emulator
            .call(
                "PutRecord",
                json!({
                    "DeliveryStreamName": name,
                    "Record": { "Data": aws_smithy_types::base64::encode("once disabled") },
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: put once disabled got {put:?}");
        assert_eq!(
            put["Encrypted"],
            json!(false),
            "case {case:?}: put once disabled got {put:?}"
        );

        // With nothing left in force to disable, the same call is refused.
        let (status, refused) = emulator
            .call(
                "StopDeliveryStreamEncryption",
                json!({ "DeliveryStreamName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second stop got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {case:?}: a second stop got {refused:?}"
        );
    }
}

#[tokio::test]
async fn encryption_can_be_turned_back_on_after_it_was_stopped() {
    let emulator = Emulator::new();
    emulator.create_stream("toggled-stream", None).await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // Two full rounds, with a different CMK each time: the stop leaves the stream in a
    // state a later start can act on, so the pair can be cycled.
    for (round, configuration) in [
        (1, json!({ "KeyType": "AWS_OWNED_CMK" })),
        (
            2,
            json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn("round-two-key") }),
        ),
    ] {
        let (status, started) = emulator
            .call(
                "StartDeliveryStreamEncryption",
                json!({
                    "DeliveryStreamName": "toggled-stream",
                    "DeliveryStreamEncryptionConfigurationInput": configuration,
                }),
            )
            .await;
        assert_eq!(status, 200, "round {round}: start got {started:?}");
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let enabled = described_encryption(&emulator, "toggled-stream").await;
        assert_eq!(
            enabled["Status"],
            json!("ENABLED"),
            "round {round}: after the enabling span, got {enabled:?}"
        );
        assert_eq!(
            enabled["KeyType"], configuration["KeyType"],
            "round {round}: the stream reports the CMK asked for, got {enabled:?}"
        );

        let (status, stopped) = emulator
            .call(
                "StopDeliveryStreamEncryption",
                json!({ "DeliveryStreamName": "toggled-stream" }),
            )
            .await;
        assert_eq!(status, 200, "round {round}: stop got {stopped:?}");
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let disabled = described_encryption(&emulator, "toggled-stream").await;
        assert_eq!(
            disabled["Status"],
            json!("DISABLED"),
            "round {round}: after the disabling span, got {disabled:?}"
        );
    }
}

/// How a test sets a stream up so a `StopDeliveryStreamEncryption` on it is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Nothing is created at all.
    NoStream,
    /// Created with SSE asked for and left in `CREATING` — the call lands inside the
    /// creation span.
    LeftCreating,
    /// Created, settled, then deleted and left in `DELETING`.
    LeftDeleting,
    /// Created, settled, and encryption started — the call lands inside the enabling span.
    LeftEnabling,
    /// Encrypted, then stopped — the call lands inside the disabling span.
    LeftDisabling,
    /// Created and settled, with SSE never asked for.
    NeverEncrypted,
    /// Encrypted, stopped, and left past the disabling span.
    AlreadyDisabled,
}

#[tokio::test]
async fn stop_encryption_is_refused_with_the_modelled_error_for_the_reason() {
    // (case, how the stream is set up, the expected error code)
    let cases: Vec<(&str, Setup, &str)> = vec![
        (
            "no stream of that name",
            Setup::NoStream,
            "ResourceNotFoundException",
        ),
        (
            "a stream still being created",
            Setup::LeftCreating,
            "ResourceInUseException",
        ),
        (
            "a stream being deleted",
            Setup::LeftDeleting,
            "ResourceInUseException",
        ),
        (
            "a stream whose own enabling has not settled",
            Setup::LeftEnabling,
            "ResourceInUseException",
        ),
        (
            "a stream whose own disabling has not settled",
            Setup::LeftDisabling,
            "ResourceInUseException",
        ),
        (
            "a stream that was never encrypted",
            Setup::NeverEncrypted,
            "InvalidArgumentException",
        ),
        (
            "a stream whose encryption is already disabled",
            Setup::AlreadyDisabled,
            "InvalidArgumentException",
        ),
    ];

    for (index, (case, setup, expected_code)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("stop-refused-{index}");

        match setup {
            Setup::NoStream => {}
            Setup::LeftCreating => {
                let (status, created) = emulator
                    .call(
                        "CreateDeliveryStream",
                        json!({
                            "DeliveryStreamName": name,
                            "S3DestinationConfiguration": s3_configuration(),
                            "DeliveryStreamEncryptionConfigurationInput": {
                                "KeyType": "AWS_OWNED_CMK",
                            },
                        }),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: create got {created:?}");
            }
            Setup::LeftDeleting => {
                emulator.create_stream(&name, None).await;
                tokio::time::sleep(PAST_THE_TRANSITION).await;
                let (status, deleted) = emulator
                    .call(
                        "DeleteDeliveryStream",
                        json!({ "DeliveryStreamName": name }),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: delete got {deleted:?}");
            }
            Setup::LeftEnabling => {
                emulator.create_stream(&name, None).await;
                tokio::time::sleep(PAST_THE_TRANSITION).await;
                let (status, started) = emulator
                    .call(
                        "StartDeliveryStreamEncryption",
                        json!({ "DeliveryStreamName": name }),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: start got {started:?}");
            }
            Setup::LeftDisabling => {
                encrypted_stream(
                    &emulator,
                    &name,
                    EnabledBy::Start,
                    &json!({ "KeyType": "AWS_OWNED_CMK" }),
                )
                .await;
                let (status, stopped) = emulator
                    .call(
                        "StopDeliveryStreamEncryption",
                        json!({ "DeliveryStreamName": name }),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: first stop got {stopped:?}");
            }
            Setup::NeverEncrypted => {
                emulator.create_stream(&name, None).await;
                tokio::time::sleep(PAST_THE_TRANSITION).await;
            }
            Setup::AlreadyDisabled => {
                encrypted_stream(
                    &emulator,
                    &name,
                    EnabledBy::Start,
                    &json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn("gone-key") }),
                )
                .await;
                let (status, stopped) = emulator
                    .call(
                        "StopDeliveryStreamEncryption",
                        json!({ "DeliveryStreamName": name }),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: first stop got {stopped:?}");
                tokio::time::sleep(PAST_THE_TRANSITION).await;
            }
        }
        // What the stream reported before the refused call, for the streams there are one
        // of: a refused call must leave exactly this behind.
        let before = if setup == Setup::NoStream {
            None
        } else {
            Some(described_encryption(&emulator, &name).await)
        };

        let (status, refused) = emulator
            .call(
                "StopDeliveryStreamEncryption",
                json!({ "DeliveryStreamName": name }),
            )
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
async fn a_stop_is_charged_to_the_same_window_as_a_start() {
    // "The StartDeliveryStreamEncryption and StopDeliveryStreamEncryption operations have
    // a combined limit of 25 calls per Firehose stream per 24 hours."
    const LIMIT: usize = 25;
    const WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

    // (case, how many calls already landed and how long ago, whether the stop is taken).
    // Seeding the earlier calls is what keeps the window testable without waiting 24
    // hours for it.
    let cases: Vec<(&str, usize, Duration, bool)> = vec![
        ("no earlier call", 0, Duration::ZERO, true),
        ("one short of the limit", LIMIT - 1, Duration::ZERO, true),
        ("exactly the limit", LIMIT, Duration::ZERO, false),
        ("more than the limit", LIMIT + 3, Duration::ZERO, false),
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
        let name = format!("stop-budgeted-{index}");
        encrypted_stream(
            &emulator,
            &name,
            EnabledBy::Create,
            &json!({ "KeyType": "AWS_OWNED_CMK" }),
        )
        .await;

        // No modelled operation reports the budget, so the earlier calls are seeded into
        // the state this one is counted from. A create does not spend the budget, which is
        // why SSE here was asked for at create time.
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
                "StopDeliveryStreamEncryption",
                json!({ "DeliveryStreamName": name }),
            )
            .await;
        if taken {
            assert_eq!(status, 200, "case {case:?}: got {status} {answered:?}");
            assert_eq!(
                described_encryption(&emulator, &name).await["Status"],
                json!("DISABLING"),
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
                described_encryption(&emulator, &name).await["Status"],
                json!("ENABLED"),
                "case {case:?}: a refused call must not start the change"
            );
        }
    }
}

#[tokio::test]
async fn a_start_and_a_stop_spend_one_budget_between_them() {
    // The documented limit is COMBINED, so a start spends from the same 25 a stop does:
    // with the window one short of the limit, the start is taken and the stop that
    // follows it is not.
    const LIMIT: usize = 25;

    let emulator = Emulator::new();
    emulator.create_stream("shared-budget-stream", None).await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    {
        let state = emulator
            .engine()
            .state::<ComAmazonawsFirehoseFirehose20150804State>();
        let mut store = state.lock();
        let stream = store
            .streams
            .get_mut("shared-budget-stream")
            .expect("the stream was created");
        stream.encryption_calls = vec![SystemTime::now(); LIMIT - 1];
    }

    let (status, started) = emulator
        .call(
            "StartDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": "shared-budget-stream" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the last call of the window is taken, got {started:?}"
    );
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let (status, refused) = emulator
        .call(
            "StopDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": "shared-budget-stream" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "the start spent the window's last call, got {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "the start spent the window's last call, got {refused:?}"
    );
    assert_eq!(
        described_encryption(&emulator, "shared-budget-stream").await["Status"],
        json!("ENABLED"),
        "the refused stop left the stream encrypted"
    );
}

#[tokio::test]
async fn describing_a_stream_does_not_move_its_encryption_status_or_spend_its_budget() {
    let emulator = Emulator::new();
    encrypted_stream(
        &emulator,
        "polled-stream",
        EnabledBy::Create,
        &json!({ "KeyType": "AWS_OWNED_CMK" }),
    )
    .await;

    // Reads must not change what a later caller sees: polling an encrypted stream neither
    // moves its encryption status nor consumes the budget the stop is counted against.
    for poll in 0..40 {
        let described = described_encryption(&emulator, "polled-stream").await;
        assert_eq!(
            described["Status"],
            json!("ENABLED"),
            "poll {poll}: a read must not move the encryption status, got {described:?}"
        );
    }

    let (status, stopped) = emulator
        .call(
            "StopDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": "polled-stream" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a stop after 40 reads is still inside the budget, got {stopped:?}"
    );
}
