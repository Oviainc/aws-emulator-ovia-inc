// SPDX-License-Identifier: CC0-1.0

//! Which tag keys and values this service admits into its store, on EVERY operation
//! that writes tags.
//!
//! The rule under test: a tag a caller submits must satisfy the AWS resource-tagging
//! contract — a key of 1 to 128 characters that does not begin with the prefix reserved
//! for AWS-managed tags, and a value of at most 256 characters — so a request carrying
//! any other key or value is refused with HTTP 400 `ValidationException` and writes
//! nothing, while a key or value at either published bound is stored and reported back
//! unchanged. The rule holds wherever tags arrive: `TagResource` and the create of each
//! of the four readiness resources.
//!
//! The tables range over what the rule generalises over: key length at and one past each
//! bound, value length at and one past the bound, the empty key, the spelling and case of
//! the reserved prefix, keys that merely contain those letters elsewhere, multi-byte
//! keys and values (lengths are counted in characters, not bytes), and a map mixing a
//! well-formed tag with a rejected one. The 200-character key, the empty key and the
//! `aws:`-prefixed key the issue recorded are three rows among them.
//!
//! Everything goes over the wire through the shared harness, so "nothing written" is
//! checked by reading the tags back rather than by inspecting the state struct.

use serde_json::{json, Value};

use crate::account_gate_and_not_found::{arn, create, label, read, Kind};
use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The lengths the AWS resource-tagging contract publishes for a tag key and value, and
/// the key prefix it reserves for AWS-managed tags. Written out here rather than
/// imported from the implementation, so the test pins the contract.
const KEY_MAX_LENGTH: usize = 128;
const VALUE_MAX_LENGTH: usize = 256;
const RESERVED_PREFIX: &str = "aws:";

/// The HTTP status and wire error code a refused tag write carries. The sentence is the
/// service's own report of the member and the constraint it broke, so it is asserted
/// only for being present and bounded, not for its wording.
const REFUSAL_STATUS: u16 = 400;
const REFUSAL_CODE: &str = "ValidationException";

/// The constant sentence a read of a name this account has no record of answers with
/// (pinned in `account_gate_and_not_found.rs`), used here to show a refused create
/// stored nothing.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The name every resource these tests tag is created under.
const SUBJECT: &str = "tag_rule_subject";

/// A tag map as the wire carries it.
fn tags(pairs: &[(&str, &str)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_string(), Value::String((*value).to_string()));
    }
    Value::Object(map)
}

/// Tag maps every tag-writing operation must REFUSE, each with the reason the contract
/// does not admit it.
fn rejected() -> Vec<(String, Value)> {
    let over_key = "k".repeat(KEY_MAX_LENGTH + 1);
    let issue_key = "k".repeat(200);
    let over_key_multibyte = "é".repeat(KEY_MAX_LENGTH + 1);
    let over_value = "v".repeat(VALUE_MAX_LENGTH + 1);
    let over_value_multibyte = "é".repeat(VALUE_MAX_LENGTH + 1);
    vec![
        ("an empty key".to_string(), tags(&[("", "v")])),
        (
            "a key one character past the published bound".to_string(),
            tags(&[(over_key.as_str(), "v")]),
        ),
        (
            "the 200-character key the issue recorded".to_string(),
            tags(&[(issue_key.as_str(), "v")]),
        ),
        (
            "a multi-byte key one CHARACTER past the bound".to_string(),
            tags(&[(over_key_multibyte.as_str(), "v")]),
        ),
        (
            "a key that is exactly the reserved prefix".to_string(),
            tags(&[(RESERVED_PREFIX, "v")]),
        ),
        (
            "a reserved key the issue recorded".to_string(),
            tags(&[("aws:reserved", "v")]),
        ),
        (
            "a reserved key of the form AWS itself writes".to_string(),
            tags(&[("aws:cloudformation:stack-name", "v")]),
        ),
        (
            "the reserved prefix in upper case".to_string(),
            tags(&[("AWS:Reserved", "v")]),
        ),
        (
            "the reserved prefix in mixed case".to_string(),
            tags(&[("Aws:reserved", "v")]),
        ),
        (
            "a value one character past the published bound".to_string(),
            tags(&[("bounded_key", over_value.as_str())]),
        ),
        (
            "a multi-byte value one CHARACTER past the bound".to_string(),
            tags(&[("bounded_key", over_value_multibyte.as_str())]),
        ),
        (
            "a well-formed tag alongside a rejected one".to_string(),
            tags(&[("env", "prod"), ("aws:reserved", "v")]),
        ),
    ]
}

/// Tag maps every tag-writing operation must ACCEPT, including both published bounds.
fn accepted() -> Vec<(String, Value)> {
    let bound_key = "k".repeat(KEY_MAX_LENGTH);
    let bound_value = "v".repeat(VALUE_MAX_LENGTH);
    let bound_key_multibyte = "é".repeat(KEY_MAX_LENGTH);
    let bound_value_multibyte = "é".repeat(VALUE_MAX_LENGTH);
    vec![
        (
            "the well-formed tag the issue's own coverage uses".to_string(),
            tags(&[("env", "prod")]),
        ),
        (
            "the shortest key the contract admits".to_string(),
            tags(&[("k", "v")]),
        ),
        (
            "a key at the published length bound".to_string(),
            tags(&[(bound_key.as_str(), "v")]),
        ),
        (
            "an empty value, which the contract gives no lower bound".to_string(),
            tags(&[("empty_value", "")]),
        ),
        (
            "a value at the published length bound".to_string(),
            tags(&[("bounded_value", bound_value.as_str())]),
        ),
        (
            "a multi-byte key and value at the bounds, counted in characters".to_string(),
            tags(&[(bound_key_multibyte.as_str(), bound_value_multibyte.as_str())]),
        ),
        (
            "a key carrying the reserved letters without the colon".to_string(),
            tags(&[("awsome", "v")]),
        ),
        (
            "a key carrying the reserved prefix somewhere other than the start".to_string(),
            tags(&[("not:aws:reserved", "v")]),
        ),
        (
            "several well-formed tags at once".to_string(),
            tags(&[("env", "prod"), ("team", "arc"), ("k", "")]),
        ),
    ]
}

/// Assert `reply` is the refusal the contract calls for, and that its sentence does not
/// amplify the rejected request: the message is present and no longer than the tag map
/// the caller sent.
fn assert_tag_refusal(case: &str, reply: &Reply, submitted: &Value) {
    assert_eq!(
        reply.status, REFUSAL_STATUS,
        "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        REFUSAL_CODE,
        "case {case:?}: expected error code {REFUSAL_CODE}, got {:?} / {}",
        reply.error_code(),
        reply.body
    );
    let message = reply.string("message").unwrap_or_default();
    assert!(
        !message.is_empty(),
        "case {case:?}: expected a sentence naming the broken constraint, got {:?}",
        reply.body
    );
    let submitted_length = submitted.to_string().chars().count();
    assert!(
        message.chars().count() <= submitted_length + 200,
        "case {case:?}: expected a bounded message for a {submitted_length}-character \
         request, got one of {} characters",
        message.chars().count()
    );
}

#[tokio::test]
async fn tag_resource_refuses_a_key_or_value_outside_the_tagging_limits_and_writes_nothing() {
    for kind in Kind::ALL {
        let emulator = Emulator::new();
        create(&emulator, kind, SUBJECT).await;
        let tags_uri = format!("/tags/{}", label(&arn(kind.arn_type(), SUBJECT)));

        // One well-formed tag first, so a refused call is shown to leave the resource's
        // existing tags exactly as they were.
        let existing = tags(&[("keep", "me")]);
        let written = emulator
            .send("POST", &tags_uri, Some(json!({"tags": existing})))
            .await;
        assert_eq!(
            written.status, 200,
            "case {kind:?}: expected the baseline tag write to answer HTTP 200, got {} / {}",
            written.status, written.body
        );

        for (row, submitted) in rejected() {
            let case = format!("{kind:?} / TagResource / {row}");
            let reply = emulator
                .send("POST", &tags_uri, Some(json!({"tags": submitted})))
                .await;
            assert_tag_refusal(&case, &reply, &submitted);

            let listed = emulator.send("GET", &tags_uri, None).await;
            assert_eq!(
                listed.field("tags"),
                Some(&existing),
                "case {case:?}: expected the refused call to write nothing, got {}",
                listed.body
            );
        }
    }
}

#[tokio::test]
async fn a_create_refuses_a_tag_key_or_value_outside_the_tagging_limits_and_stores_nothing() {
    for kind in Kind::ALL {
        let emulator = Emulator::new();

        for (row, submitted) in rejected() {
            let case = format!("{kind:?} / create / {row}");
            let mut body = kind.create_body(SUBJECT);
            body["tags"] = submitted.clone();
            let reply = emulator.send("POST", kind.collection(), Some(body)).await;
            assert_tag_refusal(&case, &reply, &submitted);

            // Nothing was stored: the reader still answers the modelled not-found.
            let read_back = read(&emulator, kind, SUBJECT).await;
            assert_eq!(
                read_back.status, 404,
                "case {case:?}: expected the refused create to store nothing, got {} / {}",
                read_back.status, read_back.body
            );
            assert_eq!(
                read_back.string("message").unwrap_or_default(),
                NOT_FOUND_MESSAGE,
                "case {case:?}: expected the constant not-found sentence, got {}",
                read_back.body
            );
        }
    }
}

#[tokio::test]
async fn tags_at_the_published_bounds_are_stored_and_reported_by_every_reader() {
    for kind in Kind::ALL {
        for (row, submitted) in accepted() {
            // A fresh store per row, so each row's tag set is exactly what it submitted.
            let emulator = Emulator::new();
            let case = format!("{kind:?} / TagResource / {row}");
            create(&emulator, kind, SUBJECT).await;
            let tags_uri = format!("/tags/{}", label(&arn(kind.arn_type(), SUBJECT)));

            let written = emulator
                .send("POST", &tags_uri, Some(json!({"tags": submitted})))
                .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected HTTP 200 for a tag the contract admits, got {} / {}",
                written.status, written.body
            );
            let listed = emulator.send("GET", &tags_uri, None).await;
            assert_eq!(
                listed.field("tags"),
                Some(&submitted),
                "case {case:?}: expected the stored tags {submitted}, got {}",
                listed.body
            );
            let read_back = read(&emulator, kind, SUBJECT).await;
            assert_eq!(
                read_back.field("tags"),
                Some(&submitted),
                "case {case:?}: expected the resource's own reader to report {submitted}, got {}",
                read_back.body
            );

            // The same tags on the CREATE path, which applies the identical rule.
            let created_emulator = Emulator::new();
            let create_case = format!("{kind:?} / create / {row}");
            let mut body = kind.create_body(SUBJECT);
            body["tags"] = submitted.clone();
            let created = created_emulator
                .send("POST", kind.collection(), Some(body))
                .await;
            assert_eq!(
                created.status, 200,
                "case {create_case:?}: expected HTTP 200 for a tag the contract admits, \
                 got {} / {}",
                created.status, created.body
            );
            assert_eq!(
                created.field("tags"),
                Some(&submitted),
                "case {create_case:?}: expected the create to report {submitted}, got {}",
                created.body
            );
            let created_read_back = read(&created_emulator, kind, SUBJECT).await;
            assert_eq!(
                created_read_back.field("tags"),
                Some(&submitted),
                "case {create_case:?}: expected the reader to report {submitted}, got {}",
                created_read_back.body
            );
        }
    }
}
