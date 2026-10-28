// SPDX-License-Identifier: CC0-1.0

//! How the two encryption operations WORD a refusal, over the wire, for the blocking
//! states `control_plane_refusal_templates` does not reach.
//!
//! The rule under test: an encryption change this service refuses because the stream is
//! in use is reported as `ResourceInUseException` (HTTP 400) whose message names the
//! change the call asked for, the stream the request named and the state blocking it —
//! `Cannot <enable|disable> encryption on firehose: <DeliveryStreamName> when encryption
//! is in <DeliveryStreamEncryptionStatus> status` when it is the stream's own unsettled
//! encryption that blocks it, and the same frame over the stream's
//! `DeliveryStreamStatus` when it is the stream's lifecycle.
//!
//! The sibling suite asserts that rule for an `ENABLING` change and for a `CREATING`
//! stream; the rows here span the rest of the class, so no reachable blocking state is
//! asserted for one direction only: `DISABLING` in both directions, `DELETING` in both,
//! and stream names differing in the characters `DeliveryStreamName` allows. The harness
//! (`Emulator`, `error_code`, `error_message`, …) is the lifecycle suite's.

use serde_json::{json, Value};

use crate::delivery_stream_encryption::{described_encryption, key_arn};
use crate::delivery_stream_lifecycle::{
    error_code, error_message, Emulator, PAST_THE_TRANSITION,
};

/// The operation each change direction is asked for with, and the verb the refusals are
/// worded after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Enable,
    Disable,
}

impl Change {
    fn operation(self) -> &'static str {
        match self {
            Self::Enable => "StartDeliveryStreamEncryption",
            Self::Disable => "StopDeliveryStreamEncryption",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
        }
    }
}

/// Ask for `change` on `name`, carrying no encryption configuration (a stop takes none).
async fn ask_for(emulator: &Emulator, change: Change, name: &str) -> (u16, Value) {
    emulator
        .call(change.operation(), json!({ "DeliveryStreamName": name }))
        .await
}

/// Leave `name` with a disable still in flight — SSE enabled under `configuration`,
/// settled, then stopped — so the next encryption call on it collides with that change.
///
/// `DISABLING` is the transient `DeliveryStreamEncryptionStatus` a stop leaves behind
/// ("Firehose first sets the encryption status of the stream to DISABLING"), and the one
/// blocking status the sibling suite does not reach.
async fn left_disabling(emulator: &Emulator, name: &str, configuration: &Value) {
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
    assert_eq!(status, 200, "start SSE on {name}: got {status} {started:?}");
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let (status, stopped) = emulator
        .call(
            "StopDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": name }),
        )
        .await;
    assert_eq!(status, 200, "stop SSE on {name}: got {status} {stopped:?}");

    let reported = described_encryption(emulator, name).await;
    assert_eq!(
        reported["Status"],
        json!("DISABLING"),
        "setup for {name}: the disable has to still be in flight, got {reported:?}"
    );
}

#[tokio::test]
async fn an_encryption_change_blocked_by_an_in_flight_disable_states_that_status() {
    // (case, the change asked for, the CMK the settled enable was asked for with, the
    // stream name). Both `KeyType` values the model publishes
    // (`smithy_query(query="shape",shape="KeyType")` -> ["AWS_OWNED_CMK",
    // "CUSTOMER_MANAGED_CMK"]) appear, and the names differ in the characters
    // `DeliveryStreamName` allows (`@pattern ^[a-zA-Z0-9_.-]+$`).
    let rows: Vec<(&str, Change, Value, &str)> = vec![
        (
            "an enable while a disable is still disabling",
            Change::Enable,
            json!({ "KeyType": "AWS_OWNED_CMK" }),
            "disabling.conflict-stream",
        ),
        (
            "a second disable while the first is still disabling",
            Change::Disable,
            json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn("conflict-key") }),
            "Disabling_Conflict2",
        ),
    ];

    for (case, change, configuration, name) in rows {
        let emulator = Emulator::new();
        left_disabling(&emulator, name, &configuration).await;

        let (status, refused) = ask_for(&emulator, change, name).await;
        assert_eq!(status, 400, "case {case:?}: got {status} {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceInUseException",
            "case {case:?}: got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            format!(
                "Cannot {} encryption on firehose: {name} when encryption is in DISABLING status",
                change.verb()
            ),
            "case {case:?}: got {refused:?}"
        );

        // A refused call starts nothing: the disable it collided with is still the change
        // in flight.
        let reported = described_encryption(&emulator, name).await;
        assert_eq!(
            reported["Status"],
            json!("DISABLING"),
            "case {case:?}: the refused call must change nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_encryption_change_on_a_stream_being_deleted_states_the_stream_status() {
    // (case, the change asked for, the stream name). `DELETING` is the stream's own
    // status, so the sentence states it rather than an encryption status — in both
    // directions, whether or not SSE was ever enabled on the stream.
    let rows: Vec<(&str, Change, &str)> = vec![
        (
            "an enable on a stream being deleted",
            Change::Enable,
            "deleting.stream",
        ),
        (
            "a disable on a stream being deleted",
            Change::Disable,
            "deleting-stream_2",
        ),
    ];

    for (case, change, name) in rows {
        let emulator = Emulator::new();
        emulator.create_stream(name, None).await;
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, deleted) = emulator
            .call("DeleteDeliveryStream", json!({ "DeliveryStreamName": name }))
            .await;
        assert_eq!(status, 200, "case {case:?}: delete got {status} {deleted:?}");

        let (status, refused) = ask_for(&emulator, change, name).await;
        assert_eq!(status, 400, "case {case:?}: got {status} {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceInUseException",
            "case {case:?}: got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            format!(
                "Cannot {} encryption on firehose: {name} when it is in DELETING status",
                change.verb()
            ),
            "case {case:?}: got {refused:?}"
        );
    }
}
