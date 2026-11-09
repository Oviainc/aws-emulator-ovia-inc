// SPDX-License-Identifier: CC0-1.0

//! The response envelope this data plane writes: `{message, traceId}`.
//!
//! The rules these cases assert:
//!
//! * EVERY response this service composes itself — each modelled failure, and the
//!   `Publish` acknowledgement — carries both envelope members: the service's own text
//!   under `message`, and a freshly minted per-request correlation id under `traceId`.
//!   `traceId` is never absent, is UUID-shaped, and is new on every call (it identifies
//!   the request, not the resource).
//! * `message` is the service's own text for the condition — a fixed string, or, where
//!   the service renders a template from the request, that template. It is never prose
//!   rebuilt from the emulator's internals: not a stored version, not the JSON
//!   library's parser diagnostic, not a byte count the service does not report.
//! * The one message this service builds from the request is the over-size rejection,
//!   `Json document too large.  Size is <size>, max allowed: <max>`, whose `<size>` is
//!   the STATE the document carries — its `desired` and `reported` nodes as canonical
//!   JSON — so the framing the client wrapped that state in, and the whitespace it
//!   spaced the request out with, change neither the size reported nor the verdict. The
//!   verdict on a document's size is the SERVICE's at every size: no ceiling of the
//!   emulator's own answers it first, however far past the quota the entity is.
//!
//! Each table row varies what the rule generalises over — operation, status, condition,
//! shadow naming, topic form, payload form, document size, whitespace — and no row is
//! privileged: the recorded case that filed the issue is one row among them.

use serde_json::{json, Map, Value};
use crate::shadow_and_retained_message::{Answer, Harness};

/// This service's state-document size limit, as `state::MAX_SHADOW_DOCUMENT_BYTES`
/// declares it from the IoT Core Device Shadow quota "Maximum size of a JSON state
/// document" (8 KB).
const MAX_DOCUMENT_BYTES: usize = 8 * 1024;

/// This emulator reads a state document of ANY size the transport delivers: there is no
/// emulator-chosen ceiling in front of the service's own quota, so a document far past
/// the quota still gets the service's `Json document too large.  …` verdict rather than
/// a cap of the emulator's. This is the size the OLD ceiling sat at, kept as a row of the
/// table so the rule is asserted across it.
const PAST_ANY_EMULATOR_CEILING: usize = 8 * MAX_DOCUMENT_BYTES;

/// Whether `value` has the 8-4-4-4-12 hexadecimal shape a `traceId` carries.
fn is_uuid_shaped(value: &str) -> bool {
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == widths.len()
        && groups
            .iter()
            .zip(widths)
            .all(|(group, width)| {
                group.len() == width && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

/// The members of a response body, as a map.
fn members(case: &str, answer: &Answer) -> Map<String, Value> {
    match answer.json() {
        Value::Object(members) => members,
        other => panic!("case {case:?}: expected a JSON object body, got {other:?}"),
    }
}

/// Assert the envelope on one FAILURE: the status, the modelled error code, the exact
/// `message`, and a `traceId` that is present and UUID-shaped. Returns the `traceId`,
/// so a caller can assert the ids are distinct across calls.
fn assert_failure_envelope(
    case: &str,
    answer: &Answer,
    status: u16,
    code: &str,
    message: &str,
) -> String {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected error code {code}, got {:?}",
        answer.error_code()
    );
    let body = members(case, answer);
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(message),
        "case {case:?}: expected message {message:?}, got {:?}",
        body.get("message")
    );
    let trace_id = body
        .get("traceId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the body carries no traceId: {body:?}"))
        .to_string();
    assert!(
        is_uuid_shaped(&trace_id),
        "case {case:?}: expected a UUID-shaped traceId, got {trace_id:?}"
    );
    // The envelope is exactly `__type` (restJson1's error discriminator) beside the two
    // envelope members: no third member, and nothing missing.
    let mut keys: Vec<&str> = body.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["__type", "message", "traceId"],
        "case {case:?}: expected the error envelope's members, got {keys:?}"
    );
    trace_id
}

/// One failure whose `message` is the service's own text for the condition.
struct FailureCase {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Vec<u8>,
    status: u16,
    code: &'static str,
    message: String,
}

/// A state document carrying `reported`, as a client writes one.
fn reported(state: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"state": {"reported": state}})).expect("a JSON document")
}

#[tokio::test]
async fn every_response_body_this_service_writes_is_reported_as_json() {
    let harness = Harness::new();

    // The shadow operations' body IS a JSON document, so the response says so — and
    // so does every other body this service writes, success or failure.
    let mut cases: Vec<(&str, Answer)> = Vec::new();
    cases.push((
        "an update of a classic shadow",
        harness
            .update("ct-thing", None, reported(json!({"a": 1})))
            .await,
    ));
    cases.push((
        "an update of a named shadow",
        harness
            .update("ct-thing", Some("sensor"), reported(json!({"b": 2})))
            .await,
    ));
    cases.push((
        "a read of a classic shadow",
        harness.get_shadow("ct-thing", None).await,
    ));
    cases.push((
        "a read of a named shadow",
        harness.get_shadow("ct-thing", Some("sensor")).await,
    ));
    cases.push((
        "a listing of a thing's named shadows",
        harness.list_named_shadows("ct-thing", "").await,
    ));
    cases.push((
        "a publish acknowledgement",
        harness
            .publish("ct-topic", 0, true, None, b"body".to_vec())
            .await,
    ));
    cases.push((
        "a retained-message read",
        harness.get_retained_message("ct-topic").await,
    ));
    cases.push((
        "a delete of a named shadow",
        harness.delete_shadow("ct-thing", Some("sensor")).await,
    ));
    cases.push((
        "a delete of a classic shadow",
        harness.delete_shadow("ct-thing", None).await,
    ));
    cases.push((
        "a read of a shadow that does not exist",
        harness.get_shadow("ct-missing", None).await,
    ));

    for (name, answer) in &cases {
        assert_eq!(
            answer.content_type.as_deref(),
            Some("application/json"),
            "case {name:?}: expected the JSON content type, got {:?} for body {}",
            answer.content_type,
            String::from_utf8_lossy(&answer.body)
        );
        // A body reported as JSON has to BE JSON.
        let body = members(name, answer);
        assert!(
            !body.is_empty(),
            "case {name:?}: expected a non-empty JSON object body, got {body:?}"
        );
    }
}

#[tokio::test]
async fn every_modelled_failure_answers_in_the_service_message_and_trace_id_envelope() {
    let harness = Harness::new();

    // Two shadows to fail against: a classic shadow taken to version 2, and a named
    // one at version 1.
    for update in [json!({"state": {"reported": {"a": 1}}}), json!({"state": {"reported": {"b": 2}}})] {
        let accepted = harness
            .update("stored-thing", None, serde_json::to_vec(&update).expect("a JSON document"))
            .await;
        assert_eq!(accepted.status, 200, "the setup update must be accepted");
    }
    let accepted = harness
        .update("stored-thing", Some("sensor-a"), reported(json!({"c": 3})))
        .await;
    assert_eq!(accepted.status, 200, "the named-shadow setup update must be accepted");

    let not_found = |name: &'static str, method: &'static str, uri: String, shadow: &str| FailureCase {
        name,
        method,
        uri,
        body: Vec::new(),
        status: 404,
        code: "ResourceNotFoundException",
        // The shadow-not-found message names the shadow the request addressed.
        message: format!("No shadow exists with name: '{shadow}'"),
    };
    let retained_missing = |name: &'static str, topic: &str| FailureCase {
        name,
        method: "GET",
        uri: format!("/retainedMessage/{topic}"),
        body: Vec::new(),
        status: 404,
        code: "ResourceNotFoundException",
        message: "Retained message not found".to_string(),
    };
    // `topic` is sent percent-encoded, the way an SDK sends a topic holding a
    // wildcard or a level separator.
    let wildcard_topic = |name: &'static str, topic: &str| FailureCase {
        name,
        method: "POST",
        uri: format!("/topics/{topic}?qos=0&retain=false"),
        body: b"payload".to_vec(),
        status: 400,
        code: "InvalidRequestException",
        message: "Invalid topic filter".to_string(),
    };
    let invalid_json = |name: &'static str, body: &[u8]| FailureCase {
        name,
        method: "POST",
        uri: "/things/parse-thing/shadow".to_string(),
        body: body.to_vec(),
        status: 400,
        code: "InvalidRequestException",
        message: "Payload contains invalid json".to_string(),
    };
    let reserved_topic = |name: &'static str, topic: &str| FailureCase {
        name,
        method: "POST",
        uri: format!("/topics/{topic}?qos=0&retain=false"),
        body: b"payload".to_vec(),
        status: 400,
        code: "InvalidRequestException",
        message: "Topic can't start with $".to_string(),
    };
    let conflict = |name: &'static str, thing: &str, shadow: Option<&str>, stated: i64| FailureCase {
        name,
        method: "POST",
        uri: Harness::shadow_uri(thing, shadow),
        body: serde_json::to_vec(&json!({"state": {"reported": {"x": 1}}, "version": stated}))
            .expect("a JSON document"),
        status: 409,
        code: "ConflictException",
        message: "Version conflict".to_string(),
    };
    // A document that parses but is not a state document: one fixed sentence per
    // malformed node, naming the node at fault.
    let malformed = |name: &'static str, document: Value, message: &str| FailureCase {
        name,
        method: "POST",
        uri: "/things/shape-thing/shadow".to_string(),
        body: serde_json::to_vec(&document).expect("a JSON document"),
        status: 400,
        code: "InvalidRequestException",
        message: message.to_string(),
    };

    let cases = vec![
        not_found(
            "the classic shadow of a thing that has none",
            "GET",
            Harness::shadow_uri("probe-alpha", None),
            "probe-alpha",
        ),
        not_found(
            "a named shadow of a thing that has none",
            "GET",
            Harness::shadow_uri("probe-alpha", Some("sensor-z")),
            "sensor-z",
        ),
        not_found(
            "a named shadow of a thing whose other shadows exist",
            "GET",
            Harness::shadow_uri("stored-thing", Some("sensor-missing")),
            "sensor-missing",
        ),
        not_found(
            "deleting a classic shadow that does not exist",
            "DELETE",
            Harness::shadow_uri("probe-beta", None),
            "probe-beta",
        ),
        not_found(
            "deleting a named shadow that does not exist",
            "DELETE",
            Harness::shadow_uri("stored-thing", Some("sensor-gone")),
            "sensor-gone",
        ),
        retained_missing("a topic that never held a retained message", "metrics"),
        retained_missing("a single-character topic", "a"),
        retained_missing("a long dashed topic", "fleet-1-telemetry-aggregated"),
        wildcard_topic("a trailing multi-level wildcard", "telemetry%23"),
        wildcard_topic("a lone single-level wildcard", "%2B"),
        wildcard_topic("a single-level wildcard inside a level", "a%2Bb"),
        wildcard_topic("a multi-level wildcard after a separator", "probe%2F%23"),
        wildcard_topic("a single-level wildcard between levels", "probe%2F%2B%2Ftemp"),
        reserved_topic("a reserved shadow topic", "%24aws%2Fthings%2Ft%2Fshadow%2Fupdate"),
        reserved_topic("a bare reserved marker", "%24"),
        reserved_topic("a reserved single-level topic", "%24reserved"),
        FailureCase {
            name: "a topic of nine levels",
            method: "POST",
            uri: "/topics/a%2Fb%2Fc%2Fd%2Fe%2Ff%2Fg%2Fh%2Fi?qos=0&retain=false".to_string(),
            body: b"payload".to_vec(),
            status: 400,
            code: "InvalidRequestException",
            message: "Topic can't contain more than 8 parts".to_string(),
        },
        FailureCase {
            name: "a topic of twelve levels",
            method: "POST",
            uri: "/topics/a%2Fb%2Fc%2Fd%2Fe%2Ff%2Fg%2Fh%2Fi%2Fj%2Fk%2Fl?qos=0&retain=false"
                .to_string(),
            body: b"payload".to_vec(),
            status: 400,
            code: "InvalidRequestException",
            message: "Topic can't contain more than 8 parts".to_string(),
        },
        invalid_json("a payload that is not JSON at all", b"not json"),
        invalid_json("a payload that is only whitespace", b"   "),
        invalid_json("a truncated JSON document", b"{\"state\": {\"reported\": "),
        invalid_json("a document with a trailing comma", b"{\"state\": {\"reported\": {\"a\": 1,}}}"),
        FailureCase {
            name: "a payload that is not UTF-8",
            method: "POST",
            uri: "/things/encoding-thing/shadow".to_string(),
            body: vec![0xff, 0xfe, 0x7b, 0x7d],
            status: 415,
            code: "UnsupportedDocumentEncodingException",
            message: "The document encoding is not supported".to_string(),
        },
        conflict("an update one version behind", "stored-thing", None, 1),
        conflict("an update at the initial version", "stored-thing", None, 0),
        conflict("an update ahead of the stored version", "stored-thing", None, 7),
        conflict("an update of a named shadow one version ahead", "stored-thing", Some("sensor-a"), 2),
        conflict("an update of a shadow that does not exist yet", "probe-delta", None, 3),
        malformed(
            "a document that names no state node",
            json!({"version": 1}),
            "Missing required node: state",
        ),
        malformed(
            "a document that is not an object at all",
            json!([{"state": {"reported": {"a": 1}}}]),
            "Missing required node: state",
        ),
        malformed(
            "a state node that is a string",
            json!({"state": "reported"}),
            "State node must be an object",
        ),
        malformed(
            "a state node that is a list",
            json!({"state": [{"reported": {"a": 1}}]}),
            "State node must be an object",
        ),
        malformed(
            "a node under state the service does not merge",
            json!({"state": {"reported": {"a": 1}, "bogus": {}}}),
            "State contains an invalid node: 'bogus'",
        ),
        malformed(
            "the derived delta node written by a client",
            json!({"state": {"delta": {"a": 1}}}),
            "State contains an invalid node: 'delta'",
        ),
        malformed(
            "a desired node that is a number",
            json!({"state": {"desired": 7}}),
            "Desired node must be an object",
        ),
        malformed(
            "a reported node that is a list",
            json!({"state": {"reported": [1, 2]}}),
            "Reported node must be an object",
        ),
        malformed(
            "a version that is a string",
            json!({"state": {"reported": {"a": 1}}, "version": "1"}),
            "Invalid version",
        ),
        malformed(
            "a version that is an object",
            json!({"state": {"reported": {"a": 1}}, "version": {"is": 1}}),
            "Invalid version",
        ),
    ];

    let mut trace_ids: Vec<String> = Vec::new();
    for case in &cases {
        let answer = harness
            .send(case.method, &case.uri, &[], case.body.clone())
            .await;
        let trace_id = assert_failure_envelope(
            case.name,
            &answer,
            case.status,
            case.code,
            &case.message,
        );
        // Nothing the emulator knows internally leaks into the service's text: no
        // parser diagnostic, no stored version, no store capacity.
        for leak in ["line 1", "column", "expects version", "this emulator"] {
            assert!(
                !case.message.contains(leak) && !answer.json().to_string().contains(leak),
                "case {:?}: the message leaks {leak:?}: {}",
                case.name,
                String::from_utf8_lossy(&answer.body)
            );
        }
        trace_ids.push(trace_id);
    }

    // The correlation id identifies the CALL, so no two responses share one.
    let mut distinct = trace_ids.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        trace_ids.len(),
        "expected one distinct traceId per call, got {} for {} calls",
        distinct.len(),
        trace_ids.len()
    );

    // Repeating ONE request must also mint a new id each time: the id is not a
    // property of the condition it reports.
    let mut repeated: Vec<String> = Vec::new();
    for _ in 0..8 {
        let answer = harness.get_retained_message("metrics").await;
        repeated.push(assert_failure_envelope(
            "the same missing retained message, read again",
            &answer,
            404,
            "ResourceNotFoundException",
            "Retained message not found",
        ));
    }
    let mut unique = repeated.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        repeated.len(),
        "expected a fresh traceId per repeat, got {unique:?}"
    );
}

/// One accepted publish.
struct PublishCase {
    name: &'static str,
    topic: &'static str,
    qos: i32,
    retain: bool,
    payload: Vec<u8>,
}

#[tokio::test]
async fn an_accepted_publish_answers_with_the_acknowledgement_envelope() {
    let harness = Harness::new();

    let cases = vec![
        PublishCase {
            name: "a plain publish at QoS 0",
            topic: "ack-plain",
            qos: 0,
            retain: false,
            payload: b"hello".to_vec(),
        },
        PublishCase {
            name: "a plain publish at QoS 1",
            topic: "ack-plain-qos1",
            qos: 1,
            retain: false,
            payload: b"hello".to_vec(),
        },
        PublishCase {
            name: "an empty payload",
            topic: "ack-empty",
            qos: 0,
            retain: false,
            payload: Vec::new(),
        },
        PublishCase {
            name: "a retained publish",
            topic: "ack-retained",
            qos: 1,
            retain: true,
            payload: b"retained body".to_vec(),
        },
        PublishCase {
            name: "a binary payload",
            topic: "ack-binary",
            qos: 0,
            retain: false,
            payload: vec![0x00, 0xff, 0x10, 0x7f],
        },
        PublishCase {
            name: "a kilobyte payload",
            topic: "ack-large",
            qos: 0,
            retain: true,
            payload: vec![b'z'; 1024],
        },
        PublishCase {
            name: "a topic at the eight-part bound",
            topic: "a%2Fb%2Fc%2Fd%2Fe%2Ff%2Fg%2Fh",
            qos: 0,
            retain: false,
            payload: b"hello".to_vec(),
        },
        PublishCase {
            name: "a retained publish that clears the topic",
            topic: "ack-retained",
            qos: 0,
            retain: true,
            payload: Vec::new(),
        },
    ];

    let mut trace_ids: Vec<String> = Vec::new();
    for case in &cases {
        let answer = harness
            .publish(case.topic, case.qos, case.retain, None, case.payload.clone())
            .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {} with body {}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = members(case.name, &answer);
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some("OK"),
            "case {:?}: expected the acknowledgement message, got {:?}",
            case.name,
            body.get("message")
        );
        let trace_id = body
            .get("traceId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the acknowledgement carries no traceId: {body:?}",
                    case.name
                )
            })
            .to_string();
        assert!(
            is_uuid_shaped(&trace_id),
            "case {:?}: expected a UUID-shaped traceId, got {trace_id:?}",
            case.name
        );
        let mut keys: Vec<&str> = body.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["message", "traceId"],
            "case {:?}: expected the acknowledgement envelope's members, got {keys:?}",
            case.name
        );
        trace_ids.push(trace_id);
    }

    let mut distinct = trace_ids.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        trace_ids.len(),
        "expected one distinct traceId per publish, got {distinct:?}"
    );

    // The acknowledgement is not decoupled from what the publish did: the retained
    // publish above is readable back, and the clearing publish removed it.
    let stored = harness.get_retained_message("ack-large").await;
    assert_eq!(
        stored.status, 200,
        "expected the retained publish to be readable back, got {} with body {}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );
    let cleared = harness.get_retained_message("ack-retained").await;
    assert_eq!(
        cleared.status, 404,
        "expected the cleared topic to hold no retained message, got {} with body {}",
        cleared.status,
        String::from_utf8_lossy(&cleared.body)
    );
}

/// One form a client may send a state document in: `render` builds the document with a
/// padding string of the given length, and `serialize` puts it on the wire the way that
/// client does — compact or spaced out.
struct DocumentForm {
    name: &'static str,
    render: fn(usize) -> Value,
    serialize: fn(&Value) -> Vec<u8>,
}

fn compact(document: &Value) -> Vec<u8> {
    serde_json::to_vec(document).expect("a JSON document")
}

fn pretty(document: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(document).expect("a JSON document")
}

fn one_section(pad: usize) -> Value {
    json!({"state": {"reported": {"pad": "x".repeat(pad)}}})
}

fn both_sections(pad: usize) -> Value {
    let half = pad / 2;
    json!({
        "state": {
            "desired": {"pad": "x".repeat(pad - half)},
            "reported": {"pad": "y".repeat(half)},
        }
    })
}

fn nested_section(pad: usize) -> Value {
    json!({"state": {"desired": {"a": {"b": {"c": {"pad": "x".repeat(pad)}}}}}})
}

/// A document carrying framing BESIDE its state — a client token the service does not
/// store as state.
fn framed_section(pad: usize) -> Value {
    json!({
        "state": {"reported": {"pad": "x".repeat(pad)}},
        "clientToken": "a-client-token-the-service-does-not-store-as-state",
    })
}

/// The state a document carries, as the service weighs it against the limit: the
/// canonical JSON of its `desired` and `reported` nodes.
fn state_bytes(document: &Value) -> usize {
    ["desired", "reported"]
        .into_iter()
        .filter_map(|section| document.get("state").and_then(|state| state.get(section)))
        .map(|section| section.to_string().len())
        .sum()
}

/// The document `form` renders whose STATE is exactly `bytes` long.
fn document_with_state_bytes(form: &DocumentForm, bytes: usize) -> Value {
    let base = state_bytes(&(form.render)(0));
    assert!(
        bytes >= base,
        "form {:?}: a document's state cannot be smaller than its own frame ({base} bytes)",
        form.name
    );
    let document = (form.render)(bytes - base);
    assert_eq!(
        state_bytes(&document),
        bytes,
        "form {:?}: the test document's state must be {bytes} bytes",
        form.name
    );
    document
}

#[tokio::test]
async fn the_reported_document_size_is_the_state_the_document_carries() {
    let harness = Harness::new();

    let forms = [
        DocumentForm {
            name: "one state section",
            render: one_section,
            serialize: compact,
        },
        DocumentForm {
            name: "both state sections",
            render: both_sections,
            serialize: compact,
        },
        DocumentForm {
            name: "a nested state section",
            render: nested_section,
            serialize: compact,
        },
        DocumentForm {
            name: "framing beside the state",
            render: framed_section,
            serialize: compact,
        },
        DocumentForm {
            name: "a pretty-printed document",
            render: one_section,
            serialize: pretty,
        },
    ];

    for (form_index, form) in forms.iter().enumerate() {
        // The published bound, one byte either side of it, and well past it. Each form
        // puts a DIFFERENT number of bytes on the wire for the same state, and the
        // size reported — and the verdict — is the same across all of them.
        for (size_index, (state_size, accepted)) in [
            (MAX_DOCUMENT_BYTES - 1, true),
            (MAX_DOCUMENT_BYTES, true),
            (MAX_DOCUMENT_BYTES + 1, false),
            (MAX_DOCUMENT_BYTES + 4_096, false),
            // Past where an emulator-chosen read ceiling used to sit: the verdict and
            // the message are still the SERVICE's, built from the measured state.
            (PAST_ANY_EMULATOR_CEILING + 1, false),
            (2 * PAST_ANY_EMULATOR_CEILING, false),
        ]
        .into_iter()
        .enumerate()
        {
            let case = format!("{} carrying {state_size} bytes of state", form.name);
            let document = document_with_state_bytes(form, state_size);
            let body = (form.serialize)(&document);
            let entity_bytes = body.len();
            let thing = format!("sized-{form_index}-{size_index}");
            let answer = harness.update(&thing, None, body).await;
            if accepted {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the document to be accepted, got {} with body {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                // Round-trip: what the update accepted is what the reader answers with.
                let read = harness.get_shadow(&thing, None).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected the stored shadow to be readable, got {} with body {}",
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                assert_eq!(
                    read.json().get("version").and_then(Value::as_i64),
                    Some(1),
                    "case {case:?}: expected the stored shadow at version 1, got {}",
                    read.json()
                );
            } else {
                assert_failure_envelope(
                    &case,
                    &answer,
                    413,
                    "RequestEntityTooLargeException",
                    &format!(
                        "Json document too large.  Size is {state_size}, max allowed: \
                         {MAX_DOCUMENT_BYTES}"
                    ),
                );
                // The framing the document was sent in is not part of the size.
                assert!(
                    entity_bytes == state_size
                        || !answer
                            .json()
                            .to_string()
                            .contains(&entity_bytes.to_string()),
                    "case {case:?}: the reported size must not be the entity's {entity_bytes} \
                     bytes"
                );
            }
        }
    }

    // A payload the service cannot read as a document is not weighed against the
    // state quota at all: it has no state. That holds at any size — one past the quota,
    // and one past where an emulator read ceiling used to refuse unparsed entities — so
    // both are the invalid-json rejection.
    for (case, length) in [
        (
            "a payload past the quota that is not JSON",
            MAX_DOCUMENT_BYTES + 11,
        ),
        (
            "a payload past any emulator ceiling that is not JSON",
            PAST_ANY_EMULATOR_CEILING + 1,
        ),
    ] {
        let not_json = vec![b'q'; length];
        let thing = format!("sized-not-json-{length}");
        let answer = harness.update(&thing, None, not_json).await;
        assert_failure_envelope(
            case,
            &answer,
            400,
            "InvalidRequestException",
            "Payload contains invalid json",
        );
    }

    // An update whose MERGED state would exceed the limit is refused the same way,
    // reporting the state that merge would leave stored, and leaves the stored
    // document as it was: the limit bounds the accumulated state, not one request.
    let pad_a = "x".repeat(4_100);
    let pad_b = "y".repeat(4_100);
    let first = compact(&json!({"state": {"reported": {"a": pad_a}}}));
    let second = compact(&json!({"state": {"reported": {"b": pad_b}}}));
    let accepted = harness.update("sized-accumulated", None, first).await;
    assert_eq!(
        accepted.status, 200,
        "expected the first half-sized update to be accepted, got {} with body {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    let accumulated = json!({"a": pad_a, "b": pad_b}).to_string().len();
    let refused = harness.update("sized-accumulated", None, second).await;
    assert_failure_envelope(
        "an update whose merge would exceed the limit",
        &refused,
        413,
        "RequestEntityTooLargeException",
        &format!(
            "Json document too large.  Size is {accumulated}, max allowed: {MAX_DOCUMENT_BYTES}"
        ),
    );
    let read = harness.get_shadow("sized-accumulated", None).await;
    assert_eq!(
        read.json().get("version").and_then(Value::as_i64),
        Some(1),
        "expected the refused update to have changed nothing, got {}",
        read.json()
    );
}
