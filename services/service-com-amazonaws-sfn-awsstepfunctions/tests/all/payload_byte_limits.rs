// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members whose `@length` bounds a PAYLOAD SIZE, driven over
//! this service's own wire protocol through the production `MultiServiceRouter`.
//!
//! The rule under test: a member whose own documentation states its bound as a payload
//! size — "Length constraints apply to the payload size, and are expressed as bytes in
//! UTF-8 encoding" (`smithy_query(query="doc",shape="StartExecutionInput",member="input")`,
//! and the same sentence on `StartSyncExecution$input` and `SendTaskSuccess$output`) —
//! is measured in UTF-8 BYTES, and a value past that bound is refused with the
//! requirement about that size. The value itself never appears in the refusal: these
//! members are `@sensitive`, so the clause names the member and the requirement and
//! states no value at all.
//!
//! The rows differ along what the rule generalises over: the operation carrying the
//! payload, which member it is, and HOW the value sits against the bound — inside it
//! in bytes, exactly on it, one byte past it, and the two cases the constraint walk's
//! own character count cannot tell apart from those (a multi-byte document of fewer
//! CHARACTERS than the bound whose UTF-8 encoding is past it, and one whose encoding
//! is inside it).
//!
//! The harness is `state_machine_lifecycle`'s; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    error_code, string, Emulator, HELLO_DEFINITION, REGION, ROLE_ARN,
};

/// The modelled maximum of the `SensitiveData` members (`@length` 0..262144), which is
/// the number of UTF-8 bytes a payload may hold.
const MAX_PAYLOAD_BYTES: usize = 262_144;

/// The envelope and clause a refusal of a payload past that bound states: the member,
/// the requirement about its SIZE, and no value at all.
fn expected_message(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: Member \
         must have size less than or equal to {MAX_PAYLOAD_BYTES} bytes in UTF-8 encoding"
    )
}

/// A JSON document of exactly `bytes` UTF-8 bytes whose padding is ASCII.
fn ascii_payload(bytes: usize) -> String {
    let wrapper = r#"{"p":""}"#;
    let padding = bytes.saturating_sub(wrapper.len());
    format!(r#"{{"p":"{}"}}"#, "x".repeat(padding))
}

/// A JSON document of exactly `bytes` UTF-8 bytes whose padding is a two-byte
/// character, so it carries about half as many CHARACTERS as bytes.
fn multi_byte_payload(bytes: usize) -> String {
    // `ä` is two bytes in UTF-8 and one character, which is what makes a document of
    // this shape pass a character count and fail a byte count.
    let wrapper = r#"{"p":""}"#;
    let padding = bytes.saturating_sub(wrapper.len()) / 2;
    let document = format!(r#"{{"p":"{}"}}"#, "ä".repeat(padding));
    assert_eq!(
        document.len(),
        bytes,
        "the padding fills the document to {bytes} bytes, got {}",
        document.len()
    );
    document
}

/// What one payload-carrying request looks like.
struct Carrier {
    case: &'static str,
    operation: &'static str,
    /// The member the payload is carried in, which a refusal names.
    member: &'static str,
    /// The request, built from the payload and the state machine the row addresses.
    request: fn(&str, &str) -> Value,
    /// Whether a payload INSIDE the bound is served by this operation as the row sends
    /// it. `SendTaskSuccess` names a task token too, and an accepted payload then
    /// fails on the token rather than on the payload, so only its refusals are
    /// asserted — which is also what shows the payload bound is enforced before
    /// anything else the request names is resolved.
    serves_an_accepted_payload: bool,
}

/// Every operation that carries a payload member, and the member each carries.
fn carriers() -> Vec<Carrier> {
    vec![
        Carrier {
            case: "StartExecution carries the execution's input",
            operation: "StartExecution",
            member: "input",
            request: |payload, machine| {
                json!({ "stateMachineArn": machine, "input": payload })
            },
            serves_an_accepted_payload: true,
        },
        Carrier {
            case: "SendTaskSuccess carries the task's output",
            operation: "SendTaskSuccess",
            member: "output",
            request: |payload, _machine| {
                json!({
                    "taskToken": "a-token-this-service-did-not-mint",
                    "output": payload,
                })
            },
            serves_an_accepted_payload: false,
        },
    ]
}

/// A payload is measured in UTF-8 bytes, and one past its member's bound is refused
/// with the requirement about that size and with no value echoed.
#[tokio::test]
async fn a_payload_is_bounded_by_its_size_in_utf8_bytes() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "payload-bytes",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    let machine = string(&created, "stateMachineArn");

    // (case, the payload, accepted?)
    let payloads: Vec<(String, String, bool)> = vec![
        (
            "an ASCII payload well inside the bound".to_string(),
            ascii_payload(1_024),
            true,
        ),
        (
            "an ASCII payload exactly on the bound".to_string(),
            ascii_payload(MAX_PAYLOAD_BYTES),
            true,
        ),
        (
            "an ASCII payload one byte past the bound".to_string(),
            ascii_payload(MAX_PAYLOAD_BYTES + 1),
            false,
        ),
        (
            // Fewer CHARACTERS than the bound, more BYTES: the case a character count
            // cannot tell from an accepted payload.
            "a two-byte-character payload past the bound in bytes".to_string(),
            multi_byte_payload(MAX_PAYLOAD_BYTES + 2),
            false,
        ),
        (
            "a two-byte-character payload inside the bound in bytes".to_string(),
            multi_byte_payload(MAX_PAYLOAD_BYTES),
            true,
        ),
    ];

    for carrier in carriers() {
        for (index, (payload_case, payload, accepted)) in payloads.iter().enumerate() {
            let case = format!("{} / {payload_case}", carrier.case);
            let body = (carrier.request)(payload, &machine);
            let (status, answer) = emulator.call(carrier.operation, &body).await;
            if *accepted {
                if !carrier.serves_an_accepted_payload {
                    continue;
                }
                assert_eq!(
                    status, 200,
                    "case {case:?}: a payload inside its bound is served, got {status} with \
                     {answer:?}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {case:?}: a payload past its bound is refused, got {status} with {answer:?}"
            );
            let message = answer
                .get("message")
                .or_else(|| answer.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                message,
                expected_message(carrier.member),
                "case {case:?}: expected the requirement about the payload SIZE, got {message:?}"
            );
            // The payload itself is withheld, however long it was: the whole message is
            // the clause above, and nothing of a `@sensitive` member travels back out.
            assert!(
                !message.contains(&payload[..64.min(payload.len())]),
                "case {case:?}: expected the payload to be withheld, got a message of {} \
                 characters",
                message.len()
            );
            let _ = index;
        }
    }
}

/// The same rule holds for a synchronous Express start, whose `input` carries the same
/// documented bound: the refusal is the same clause, and a payload inside the bound
/// runs.
#[tokio::test]
async fn a_synchronous_start_bounds_its_input_the_same_way() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "payload-bytes-sync",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
            }),
        )
        .await;
    let machine = string(&created, "stateMachineArn");
    assert!(
        machine.contains(REGION),
        "the state machine is created in the signed region, got {machine:?}"
    );

    // (case, the payload, accepted?)
    for (case, payload, accepted) in [
        ("inside the bound", ascii_payload(2_048), true),
        (
            "past the bound in bytes alone",
            multi_byte_payload(MAX_PAYLOAD_BYTES + 2),
            false,
        ),
        (
            "past the bound in characters too",
            ascii_payload(MAX_PAYLOAD_BYTES + 1),
            false,
        ),
    ] {
        let (status, answer) = emulator
            .call(
                "StartSyncExecution",
                &json!({ "stateMachineArn": machine, "input": payload }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a payload inside its bound runs, got {status} with {answer:?}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a payload past its bound is refused, got {status} with {answer:?}"
        );
        let message = answer
            .get("message")
            .or_else(|| answer.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message,
            expected_message("input"),
            "case {case:?}: expected the requirement about the payload SIZE, got {message:?}"
        );
    }
}
