// SPDX-License-Identifier: CC0-1.0

//! The thing-shadow lifecycle and the retained-message publish/read loop, end to end
//! over HTTP through the router this service mounts in production.
//!
//! The rules these cases assert:
//!
//! * A shadow document is the MERGE of every update applied to it — objects merge key
//!   by key, a `null` leaf deletes that key, any other value replaces what was there —
//!   carrying a version that increases by one per accepted update, a per-leaf
//!   `metadata` mirror, and a `state.delta` DERIVED from `desired` vs `reported`. Every
//!   reader answers from that one stored document, so no response is an echo of the
//!   request that asked for it.
//! * An update that states a `version` is accepted only when it is the version stored.
//! * `ListNamedShadowsForThing` lists the thing's NAMED shadows in a stable order and
//!   never the unnamed (classic) one.
//! * A `Publish` with `retain=true` stores the message as the topic's retained
//!   message, byte for byte; an empty retained payload clears it; a publish without
//!   `retain` stores nothing.
//!
//! Each table row varies what the rule generalises over — patch shape, nesting depth,
//! value type, section, update count, page size, payload bytes, QoS — and no row is
//! privileged.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service
/// by its SigV4 credential scope (signing name `iotdata`), the way an SDK request
/// does. Signature verification is off in this harness, so the value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/iotdata/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// One HTTP answer, reduced to what these tests assert on. Shared with the sibling
/// suites in this target, which drive the same service over the same router.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
    /// The response's `content-type`, which the shadow operations report as
    /// `application/json` because their body IS a JSON document.
    pub(crate) content_type: Option<String>,
}

impl Answer {
    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The modelled error code, which restJson1 carries in `__type`.
    pub(crate) fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine
/// behind one mounted service.
pub(crate) struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iotdataplane_iotmoonrakerservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    pub(crate) async fn send(
        &self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Answer {
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "data-ats.iot.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/octet-stream");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request"),
            )
            .await;
        Answer {
            status: response.status().as_u16(),
            content_type: response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            body: response.into_body().to_vec(),
        }
    }

    /// The shadow route, with the `name` query parameter only when a shadow is named.
    pub(crate) fn shadow_uri(thing: &str, shadow: Option<&str>) -> String {
        match shadow {
            Some(name) => format!("/things/{thing}/shadow?name={name}"),
            None => format!("/things/{thing}/shadow"),
        }
    }

    pub(crate) async fn update(
        &self,
        thing: &str,
        shadow: Option<&str>,
        payload: Vec<u8>,
    ) -> Answer {
        self.send("POST", &Self::shadow_uri(thing, shadow), &[], payload)
            .await
    }

    pub(crate) async fn get_shadow(&self, thing: &str, shadow: Option<&str>) -> Answer {
        self.send("GET", &Self::shadow_uri(thing, shadow), &[], Vec::new())
            .await
    }

    pub(crate) async fn delete_shadow(&self, thing: &str, shadow: Option<&str>) -> Answer {
        self.send("DELETE", &Self::shadow_uri(thing, shadow), &[], Vec::new())
            .await
    }

    pub(crate) async fn list_named_shadows(&self, thing: &str, query: &str) -> Answer {
        self.send(
            "GET",
            &format!("/api/things/shadow/ListNamedShadowsForThing/{thing}{query}"),
            &[],
            Vec::new(),
        )
        .await
    }

    pub(crate) async fn publish(
        &self,
        topic: &str,
        qos: i32,
        retain: bool,
        user_properties: Option<&str>,
        payload: Vec<u8>,
    ) -> Answer {
        let headers: Vec<(&str, &str)> = user_properties
            .map(|value| vec![("x-amz-mqtt5-user-properties", value)])
            .unwrap_or_default();
        self.send(
            "POST",
            &format!("/topics/{topic}?qos={qos}&retain={retain}"),
            &headers,
            payload,
        )
        .await
    }

    pub(crate) async fn get_retained_message(&self, topic: &str) -> Answer {
        self.send("GET", &format!("/retainedMessage/{topic}"), &[], Vec::new())
            .await
    }
}

/// `metadata` must mirror `state`: the same tree, with every leaf replaced by a
/// positive `timestamp`. That is what makes metadata derived from the stored document
/// rather than canned.
fn assert_metadata_mirrors(case: &str, path: &str, state: &Value, metadata: &Value) {
    match state {
        Value::Object(members) => {
            let mirror = metadata.as_object().unwrap_or_else(|| {
                panic!("case {case:?} at {path}: expected a metadata object, got {metadata:?}")
            });
            assert_eq!(
                members.len(),
                mirror.len(),
                "case {case:?} at {path}: expected {} metadata members, got {mirror:?}",
                members.len()
            );
            for (key, member) in members {
                let sub = mirror.get(key).unwrap_or_else(|| {
                    panic!("case {case:?} at {path}: metadata has no {key:?}, got {mirror:?}")
                });
                assert_metadata_mirrors(case, &format!("{path}.{key}"), member, sub);
            }
        }
        Value::Array(items) => {
            let mirror = metadata.as_array().unwrap_or_else(|| {
                panic!("case {case:?} at {path}: expected a metadata array, got {metadata:?}")
            });
            assert_eq!(
                items.len(),
                mirror.len(),
                "case {case:?} at {path}: expected {} metadata entries, got {mirror:?}",
                items.len()
            );
            for (index, item) in items.iter().enumerate() {
                assert_metadata_mirrors(case, &format!("{path}[{index}]"), item, &mirror[index]);
            }
        }
        _ => {
            let timestamp = metadata.get("timestamp").and_then(Value::as_i64);
            assert!(
                timestamp.is_some_and(|seconds| seconds > 0),
                "case {case:?} at {path}: expected a positive metadata timestamp, got {metadata:?}"
            );
        }
    }
}

/// One merge case: a sequence of update payloads, and the document the merge of all of
/// them must produce.
struct MergeCase {
    name: &'static str,
    thing: &'static str,
    updates: Vec<Value>,
    desired: Option<Value>,
    reported: Option<Value>,
    delta: Option<Value>,
}

fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            name: "a single reported scalar",
            thing: "thing-reported",
            updates: vec![json!({"state": {"reported": {"temp": 20}}})],
            desired: None,
            reported: Some(json!({"temp": 20})),
            // Nothing is desired, so nothing is outstanding.
            delta: None,
        },
        MergeCase {
            name: "desired differs from reported",
            thing: "thing-desired-differs",
            updates: vec![
                json!({"state": {"reported": {"temp": 20}}}),
                json!({"state": {"desired": {"temp": 25}}}),
            ],
            desired: Some(json!({"temp": 25})),
            reported: Some(json!({"temp": 20})),
            delta: Some(json!({"temp": 25})),
        },
        MergeCase {
            name: "desired already reported",
            thing: "thing-desired-matches",
            updates: vec![
                json!({"state": {"reported": {"temp": 20}}}),
                json!({"state": {"desired": {"temp": 20}}}),
            ],
            desired: Some(json!({"temp": 20})),
            reported: Some(json!({"temp": 20})),
            delta: None,
        },
        MergeCase {
            name: "nested objects merge key by key",
            thing: "thing-nested-merge",
            updates: vec![
                json!({"state": {"reported": {"cfg": {"a": 1, "b": 2}}}}),
                json!({"state": {"reported": {"cfg": {"b": 3, "c": 4}}}}),
            ],
            desired: None,
            reported: Some(json!({"cfg": {"a": 1, "b": 3, "c": 4}})),
            delta: None,
        },
        MergeCase {
            name: "a null leaf deletes one key",
            thing: "thing-null-leaf",
            updates: vec![
                json!({"state": {"reported": {"a": 1, "b": 2}}}),
                json!({"state": {"reported": {"b": null}}}),
            ],
            desired: None,
            reported: Some(json!({"a": 1})),
            delta: None,
        },
        MergeCase {
            name: "a null leaf deep in the tree",
            thing: "thing-null-deep",
            updates: vec![
                json!({"state": {"reported": {"cfg": {"a": 1, "b": 2}}}}),
                json!({"state": {"reported": {"cfg": {"a": null}}}}),
            ],
            desired: None,
            reported: Some(json!({"cfg": {"b": 2}})),
            delta: None,
        },
        MergeCase {
            name: "nulling the last key drops the section",
            thing: "thing-section-emptied",
            updates: vec![
                json!({"state": {"reported": {"only": 1}}}),
                json!({"state": {"reported": {"only": null}}}),
            ],
            desired: None,
            reported: None,
            delta: None,
        },
        MergeCase {
            name: "an array replaces wholesale",
            thing: "thing-array",
            updates: vec![
                json!({"state": {"reported": {"xs": [1, 2, 3]}}}),
                json!({"state": {"reported": {"xs": [9]}}}),
            ],
            desired: None,
            reported: Some(json!({"xs": [9]})),
            delta: None,
        },
        MergeCase {
            name: "an object replaces a scalar",
            thing: "thing-widening",
            updates: vec![
                json!({"state": {"reported": {"n": 7}}}),
                json!({"state": {"reported": {"n": {"deep": true}}}}),
            ],
            desired: None,
            reported: Some(json!({"n": {"deep": true}})),
            delta: None,
        },
        MergeCase {
            name: "both sections in one update",
            thing: "thing-both-sections",
            updates: vec![json!({"state": {"desired": {"x": 1}, "reported": {"y": 2}}})],
            desired: Some(json!({"x": 1})),
            reported: Some(json!({"y": 2})),
            // `x` is not reported at all, so the whole desired subtree is outstanding.
            delta: Some(json!({"x": 1})),
        },
        MergeCase {
            name: "the delta is computed leaf by leaf",
            thing: "thing-nested-delta",
            updates: vec![
                json!({"state": {"reported": {"c": {"a": 1, "b": 2}, "spare": "kept"}}}),
                json!({"state": {"desired": {"c": {"a": 1, "b": 5}}}}),
            ],
            desired: Some(json!({"c": {"a": 1, "b": 5}})),
            reported: Some(json!({"c": {"a": 1, "b": 2}, "spare": "kept"})),
            delta: Some(json!({"c": {"b": 5}})),
        },
        MergeCase {
            name: "desired with nothing reported is all outstanding",
            thing: "thing-desired-only",
            updates: vec![json!({"state": {"desired": {"k": "v", "mixed": [true, null]}}})],
            desired: Some(json!({"k": "v", "mixed": [true, null]})),
            reported: None,
            delta: Some(json!({"k": "v", "mixed": [true, null]})),
        },
        MergeCase {
            name: "seven updates to the same leaf",
            thing: "thing-many-updates",
            updates: (0..7)
                .map(|step| json!({"state": {"reported": {"step": step}}}))
                .collect(),
            desired: None,
            reported: Some(json!({"step": 6})),
            delta: None,
        },
    ]
}

/// The expected `state` node of a merge case.
fn expected_state(case: &MergeCase) -> Value {
    let mut state = Map::new();
    if let Some(desired) = &case.desired {
        state.insert("desired".to_string(), desired.clone());
    }
    if let Some(reported) = &case.reported {
        state.insert("reported".to_string(), reported.clone());
    }
    if let Some(delta) = &case.delta {
        state.insert("delta".to_string(), delta.clone());
    }
    Value::Object(state)
}

#[tokio::test]
async fn a_shadow_document_is_the_merge_of_every_update_applied_to_it() {
    let harness = Harness::new();

    for case in merge_cases() {
        let mut last = Value::Null;
        for (index, update) in case.updates.iter().enumerate() {
            let answer = harness
                .update(
                    case.thing,
                    None,
                    serde_json::to_vec(update).expect("a serializable update"),
                )
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {:?} update {index}: expected 200, got {} with body {}",
                case.name,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            last = answer.json();
        }

        let expected = expected_state(&case);
        assert_eq!(
            last.get("state"),
            Some(&expected),
            "case {:?}: expected the accepted state {expected}, got {last}",
            case.name
        );
        assert_eq!(
            last.get("version").and_then(Value::as_i64),
            Some(case.updates.len() as i64),
            "case {:?}: expected version {}, got {last}",
            case.name,
            case.updates.len()
        );
        assert!(
            last.get("timestamp")
                .and_then(Value::as_i64)
                .is_some_and(|seconds| seconds > 0),
            "case {:?}: expected a positive timestamp, got {last}",
            case.name
        );

        // The metadata mirrors the sections the document actually carries — `delta` is
        // derived, so it has no metadata of its own.
        let metadata = last
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: no metadata in {last}", case.name));
        for section in ["desired", "reported"] {
            match last.get("state").and_then(|state| state.get(section)) {
                Some(state) => assert_metadata_mirrors(
                    case.name,
                    section,
                    state,
                    metadata
                        .get(section)
                        .unwrap_or_else(|| panic!("case {:?}: no {section} metadata", case.name)),
                ),
                None => assert!(
                    metadata.get(section).is_none(),
                    "case {:?}: expected no {section} metadata, got {metadata}",
                    case.name
                ),
            }
        }
        assert!(
            metadata.get("delta").is_none(),
            "case {:?}: delta is derived and carries no metadata, got {metadata}",
            case.name
        );

        // The round trip: what a reader gets back is the stored document, not the
        // request that wrote it.
        let read = harness.get_shadow(case.thing, None).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected the stored document to read back as 200, got {} with body {}",
            case.name,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let read = read.json();
        assert_eq!(
            read.get("state"),
            Some(&expected),
            "case {:?}: expected the persisted state {expected}, got {read}",
            case.name
        );
        assert_eq!(
            read.get("version"),
            last.get("version"),
            "case {:?}: expected the persisted version {:?}, got {read}",
            case.name,
            last.get("version")
        );
        assert_eq!(
            read.get("metadata"),
            Some(&metadata),
            "case {:?}: expected the persisted metadata {metadata}, got {read}",
            case.name
        );
    }
}

#[tokio::test]
async fn named_and_classic_shadows_of_one_thing_are_separate_documents() {
    let harness = Harness::new();

    // The same thing, three shadows, three different payloads: a per-shadow document,
    // not one document per thing. The third name exercises the pattern's `$` and `:` —
    // with the `$` after the first character, since a LEADING `$` names the service's
    // own reserved namespace, which no request may address.
    for (shadow, value) in [
        (None, 10),
        (Some("cfg"), 20),
        (Some("metrics$:v1_a-b"), 30),
    ] {
        let answer = harness
            .update(
                "sharedThing",
                shadow,
                serde_json::to_vec(&json!({"state": {"reported": {"slot": value}}}))
                    .expect("a serializable update"),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {shadow:?}: expected 200, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    for (shadow, value) in [
        (None, 10),
        (Some("cfg"), 20),
        (Some("metrics$:v1_a-b"), 30),
    ] {
        let read = harness.get_shadow("sharedThing", shadow).await;
        assert_eq!(
            read.status, 200,
            "case {shadow:?}: expected 200, got {}",
            read.status
        );
        let document = read.json();
        assert_eq!(
            document.pointer("/state/reported/slot"),
            Some(&json!(value)),
            "case {shadow:?}: expected slot {value}, got {document}"
        );
        assert_eq!(
            document.get("version").and_then(Value::as_i64),
            Some(1),
            "case {shadow:?}: each shadow versions independently, got {document}"
        );
    }
}

#[tokio::test]
async fn an_update_that_states_a_version_is_accepted_only_at_that_version() {
    let harness = Harness::new();

    // A shadow that does not exist yet is at version 0.
    for (case, version, expected_status) in [
        ("a fresh shadow at the wrong version", 1_i64, 409_u16),
        ("a fresh shadow at the wrong version, far off", 99, 409),
        ("a fresh shadow at version 0", 0, 200),
    ] {
        let answer = harness
            .update(
                "versionedThing",
                None,
                serde_json::to_vec(&json!({"version": version, "state": {"reported": {"v": version}}}))
                    .expect("a serializable update"),
            )
            .await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected {expected_status}, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected_status == 409 {
            assert_eq!(
                answer.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }

    // The document is now at version 1, so only 1 is accepted — and accepting it moves
    // the document to 2.
    for (case, version, expected_status) in [
        ("behind the stored version", 0_i64, 409_u16),
        ("ahead of the stored version", 2, 409),
        ("the stored version", 1, 200),
        ("the version just superseded", 1, 409),
        ("the new stored version", 2, 200),
    ] {
        let answer = harness
            .update(
                "versionedThing",
                None,
                serde_json::to_vec(&json!({"version": version, "state": {"reported": {"v": version}}}))
                    .expect("a serializable update"),
            )
            .await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected {expected_status}, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A conflict must not have written anything: three updates were accepted in total.
    let read = harness.get_shadow("versionedThing", None).await;
    let document = read.json();
    assert_eq!(
        document.get("version").and_then(Value::as_i64),
        Some(3),
        "expected exactly the accepted updates to have moved the version, got {document}"
    );
    assert_eq!(
        document.pointer("/state/reported/v"),
        Some(&json!(2)),
        "expected the last accepted update to be the stored one, got {document}"
    );
}

#[tokio::test]
async fn an_update_whose_document_is_not_a_state_document_is_refused() {
    let harness = Harness::new();

    // UTF-8 JSON that is not a well-formed state document: 400 InvalidRequestException.
    let invalid: Vec<(&str, Vec<u8>)> = vec![
        ("truncated JSON", b"{\"state\":".to_vec()),
        ("not JSON at all", b"hello".to_vec()),
        ("a JSON array", b"[]".to_vec()),
        ("a JSON scalar", b"42".to_vec()),
        ("no state node", b"{\"desired\":{\"a\":1}}".to_vec()),
        ("a scalar state node", b"{\"state\":7}".to_vec()),
        ("an empty state node", b"{\"state\":{}}".to_vec()),
        (
            "a client-written delta",
            b"{\"state\":{\"delta\":{\"a\":1}}}".to_vec(),
        ),
        (
            "an unknown state member",
            b"{\"state\":{\"reported\":{},\"wanted\":{}}}".to_vec(),
        ),
        (
            "a scalar section",
            b"{\"state\":{\"reported\":5}}".to_vec(),
        ),
        (
            "a non-numeric version",
            b"{\"version\":\"one\",\"state\":{\"reported\":{\"a\":1}}}".to_vec(),
        ),
    ];
    for (case, payload) in invalid {
        let answer = harness.update("badThing", None, payload).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // Bytes that are not UTF-8 are an encoding the service does not support: 415.
    for (case, payload) in [
        ("a lone continuation byte", vec![0x80_u8]),
        ("a truncated multi-byte sequence", vec![b'{', 0xc3]),
        ("UTF-16 bytes", vec![0xff, 0xfe, b'{', 0x00]),
    ] {
        let answer = harness.update("badThing", None, payload).await;
        assert_eq!(
            answer.status,
            415,
            "case {case:?}: expected 415, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "UnsupportedDocumentEncodingException",
            "case {case:?}: expected UnsupportedDocumentEncodingException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The documented state-document size quota is 8 KB: at the bound it is accepted,
    // past it the payload is too large.
    for (case, filler_len, expected_status) in [
        ("just inside the 8 KB quota", 8 * 1024 - 40, 200_u16),
        ("just past the 8 KB quota", 8 * 1024, 413),
        ("far past the 8 KB quota", 64 * 1024, 413),
    ] {
        let payload = serde_json::to_vec(&json!({
            "state": {"reported": {"blob": "x".repeat(filler_len)}}
        }))
        .expect("a serializable update");
        let answer = harness.update("sizedThing", None, payload).await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected {expected_status}, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected_status == 413 {
            assert_eq!(
                answer.error_code(),
                "RequestEntityTooLargeException",
                "case {case:?}: expected RequestEntityTooLargeException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }

    // Nothing above the refusals wrote anything: the thing that only saw refusals has
    // no shadow at all.
    let read = harness.get_shadow("badThing", None).await;
    assert_eq!(
        read.status,
        404,
        "a refused update must store nothing, got {} with body {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
}

#[tokio::test]
async fn reading_a_shadow_that_was_never_written_is_a_not_found() {
    let harness = Harness::new();

    harness
        .update(
            "presentThing",
            Some("cfg"),
            serde_json::to_vec(&json!({"state": {"reported": {"a": 1}}})).expect("serializable"),
        )
        .await;

    for (case, thing, shadow) in [
        ("an unknown thing", "absentThing", None),
        ("an unknown thing, named shadow", "absentThing", Some("cfg")),
        ("a known thing with no classic shadow", "presentThing", None),
        (
            "a known thing with another shadow name",
            "presentThing",
            Some("other"),
        ),
    ] {
        let answer = harness.get_shadow(thing, shadow).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}

#[tokio::test]
async fn listing_named_shadows_pages_over_the_stored_names_and_never_the_classic_one() {
    let harness = Harness::new();

    // Written out of order, so the listing's order comes from the store and not from
    // the order the shadows were created in.
    let names = ["zz", "cfg", "a", "d", "b"];
    for name in names {
        harness
            .update(
                "listThing",
                Some(name),
                serde_json::to_vec(&json!({"state": {"reported": {"n": name}}}))
                    .expect("serializable"),
            )
            .await;
    }
    // ...plus the classic shadow, which is not a named shadow.
    harness
        .update(
            "listThing",
            None,
            serde_json::to_vec(&json!({"state": {"reported": {"classic": true}}}))
                .expect("serializable"),
        )
        .await;

    let mut sorted: Vec<&str> = names.to_vec();
    sorted.sort_unstable();

    // Every page size — including each published bound of the modelled 1..100 range —
    // walks the same names in the same order.
    for (case, page_size) in [
        ("no page size", None),
        ("the low bound of the pageSize range", Some(1)),
        ("a page size that splits the names", Some(2)),
        ("a page size one short of the whole set", Some(4)),
        ("a page size equal to the whole set", Some(5)),
        ("the high bound of the pageSize range", Some(100)),
    ] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = String::new();
            if let Some(size) = page_size {
                query.push_str(&format!("?pageSize={size}"));
            }
            if let Some(token) = &token {
                query.push_str(if query.is_empty() { "?" } else { "&" });
                query.push_str(&format!("nextToken={token}"));
            }
            let answer = harness.list_named_shadows("listThing", &query).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected 200, got {} with body {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let body = answer.json();
            assert!(
                body.get("timestamp")
                    .and_then(Value::as_i64)
                    .is_some_and(|seconds| seconds > 0),
                "case {case:?}: expected a positive timestamp, got {body}"
            );
            let page: Vec<String> = body
                .get("results")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no results in {body}"))
                .iter()
                .map(|name| name.as_str().unwrap_or_default().to_string())
                .collect();
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case {case:?}: expected at most {size} results, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: paging did not terminate after {pages} pages ({seen:?})"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, sorted,
            "case {case:?}: expected the named shadows {sorted:?}, got {seen:?}"
        );
        if let Some(size) = page_size {
            let expected_pages = sorted.len().div_ceil(size);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages, got {pages}"
            );
        }
    }

    // One past each published bound of the modelled range is refused at the boundary.
    for (case, page_size) in [
        ("one below the low bound", "0"),
        ("one above the high bound", "101"),
    ] {
        let answer = harness
            .list_named_shadows("listThing", &format!("?pageSize={page_size}"))
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A token this service did not mint is an invalid request, not an empty page.
    for (case, token) in [
        ("a word", "abc"),
        ("a negative offset", "-1"),
        ("a fractional offset", "1.5"),
    ] {
        let answer = harness
            .list_named_shadows("listThing", &format!("?nextToken={token}"))
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A thing with only a classic shadow exists but has no NAMED shadow.
    harness
        .update(
            "classicOnlyThing",
            None,
            serde_json::to_vec(&json!({"state": {"reported": {"a": 1}}})).expect("serializable"),
        )
        .await;
    let answer = harness.list_named_shadows("classicOnlyThing", "").await;
    assert_eq!(
        answer.status,
        200,
        "a thing with a classic shadow exists, got {} with body {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    assert_eq!(
        body.get("results").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "the classic shadow is not a named shadow, got {body}"
    );

    // A thing with no shadow at all is not found.
    let answer = harness.list_named_shadows("absentThing", "").await;
    assert_eq!(
        answer.status,
        404,
        "expected 404 for a thing with no shadows, got {} with body {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {}",
        String::from_utf8_lossy(&answer.body)
    );
}

#[tokio::test]
async fn deleting_a_shadow_removes_it_from_every_reader() {
    let harness = Harness::new();

    for (case, shadow) in [
        ("the classic shadow", None),
        ("a named shadow", Some("cfg")),
        ("another named shadow", Some("telemetry-1")),
    ] {
        // Two updates, so the deletion acknowledgement reports a version the store
        // actually reached rather than a constant.
        for step in 0..2 {
            harness
                .update(
                    "deleteThing",
                    shadow,
                    serde_json::to_vec(&json!({"state": {"reported": {"step": step}}}))
                        .expect("serializable"),
                )
                .await;
        }

        let deleted = harness.delete_shadow("deleteThing", shadow).await;
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: expected 200, got {} with body {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        let acknowledgement = deleted.json();
        assert_eq!(
            acknowledgement.get("version").and_then(Value::as_i64),
            Some(2),
            "case {case:?}: expected the deleted version, got {acknowledgement}"
        );
        assert!(
            acknowledgement
                .get("timestamp")
                .and_then(Value::as_i64)
                .is_some_and(|seconds| seconds > 0),
            "case {case:?}: expected a positive timestamp, got {acknowledgement}"
        );

        // Gone for every reader.
        let read = harness.get_shadow("deleteThing", shadow).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the deleted shadow to read back as 404, got {}",
            read.status
        );
        let again = harness.delete_shadow("deleteThing", shadow).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: expected a second delete to be 404, got {}",
            again.status
        );
        assert_eq!(
            again.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&again.body)
        );
        if let Some(name) = shadow {
            let listed = harness.list_named_shadows("deleteThing", "").await;
            let names: Vec<String> = listed
                .json()
                .get("results")
                .and_then(Value::as_array)
                .map(|results| {
                    results
                        .iter()
                        .map(|value| value.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                !names.contains(&name.to_string()),
                "case {case:?}: expected {name} to be gone from the listing, got {names:?}"
            );
        }

        // Recreating resumes the shadow name's version counter: the two updates took it
        // to 2, the delete consumed 3, so the recreated document is version 4.
        let recreated = harness
            .update(
                "deleteThing",
                shadow,
                serde_json::to_vec(&json!({"state": {"reported": {"fresh": true}}}))
                    .expect("serializable"),
            )
            .await;
        assert_eq!(
            recreated.json().get("version").and_then(Value::as_i64),
            Some(4),
            "case {case:?}: expected the recreated shadow to resume above the delete, got {}",
            String::from_utf8_lossy(&recreated.body)
        );
    }
}

#[tokio::test]
async fn the_accumulated_shadow_document_is_bounded_by_the_state_document_quota() {
    let harness = Harness::new();

    // Every one of these updates is well inside the PER-REQUEST limit; what the 8 KB
    // state-document quota bounds is the document they accumulate into, so the
    // sequence must stop being accepted at some point.
    let chunk = "y".repeat(900);
    let mut accepted = 0_i64;
    let mut refused_at = None;
    for step in 0..32 {
        let mut reported = Map::new();
        reported.insert(format!("k{step}"), Value::from(chunk.clone()));
        let payload = serde_json::to_vec(&json!({"state": {"reported": reported}}))
            .expect("a serializable update");
        let answer = harness.update("growThing", None, payload).await;
        if answer.status == 200 {
            accepted += 1;
            continue;
        }
        assert_eq!(
            answer.status,
            413,
            "case step {step}: expected 200 or 413, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "RequestEntityTooLargeException",
            "case step {step}: expected RequestEntityTooLargeException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        refused_at = Some(step);
        break;
    }
    assert!(
        accepted > 0,
        "expected the first updates to be accepted, got {accepted}"
    );
    assert!(
        refused_at.is_some(),
        "expected accumulation past the 8 KB quota to be refused, but 32 updates of {} bytes \
         each were all accepted",
        chunk.len()
    );

    // The refusal changed nothing: the stored document is the one the accepted updates
    // left, and it is inside the quota.
    let read = harness.get_shadow("growThing", None).await;
    let document = read.json();
    assert_eq!(
        document.get("version").and_then(Value::as_i64),
        Some(accepted),
        "case {refused_at:?}: expected the refused update to have changed nothing, got {document}"
    );
    let state_bytes = document
        .get("state")
        .map(|state| state.to_string().len())
        .unwrap_or_default();
    assert!(
        state_bytes <= 8 * 1024,
        "expected the stored state document to stay inside the 8 KB quota, got {state_bytes} bytes"
    );

    // An update that does not grow the document is still accepted after the refusal.
    let mut reported = Map::new();
    reported.insert("k0".to_string(), Value::from("z"));
    let shrunk = harness
        .update(
            "growThing",
            None,
            serde_json::to_vec(&json!({"state": {"reported": reported}})).expect("serializable"),
        )
        .await;
    assert_eq!(
        shrunk.status,
        200,
        "expected an update that does not grow the document to be accepted, got {} with body {}",
        shrunk.status,
        String::from_utf8_lossy(&shrunk.body)
    );
    assert_eq!(
        shrunk.json().get("version").and_then(Value::as_i64),
        Some(accepted + 1),
        "expected the accepted update to move the version, got {}",
        String::from_utf8_lossy(&shrunk.body)
    );
}

#[tokio::test]
async fn the_shadow_store_holds_at_most_its_maximum_number_of_documents() {
    /// Named by the crate, so the test cannot drift from the bound the handler
    /// enforces.
    const MAX: usize = service_com_amazonaws_iotdataplane_iotmoonrakerservice::state::MAX_SHADOWS;

    let harness = Harness::new();
    let payload =
        serde_json::to_vec(&json!({"state": {"reported": {"a": 1}}})).expect("serializable");

    for index in 0..MAX {
        let answer = harness
            .update("capThing", Some(&format!("s{index}")), payload.clone())
            .await;
        assert_eq!(
            answer.status,
            200,
            "case shadow {index} of {MAX}: expected 200, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A key that is not stored yet is refused, whichever half of the key is new.
    for (case, thing, shadow) in [
        ("a new shadow name on the same thing", "capThing", "overflow"),
        ("a new thing entirely", "otherCapThing", "overflow"),
        ("a new thing's classic shadow", "thirdCapThing", ""),
    ] {
        let answer = harness
            .update(
                thing,
                if shadow.is_empty() { None } else { Some(shadow) },
                payload.clone(),
            )
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 at the {MAX}-document cap, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A shadow that IS stored is still updatable at the cap.
    let answer = harness.update("capThing", Some("s0"), payload.clone()).await;
    assert_eq!(
        answer.status,
        200,
        "expected an update of a stored shadow to be accepted at the cap, got {} with body {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.json().get("version").and_then(Value::as_i64),
        Some(2),
        "expected the stored shadow to have moved to version 2, got {}",
        String::from_utf8_lossy(&answer.body)
    );

    // Deleting a document does NOT free a slot: a shadow name's version counter outlives
    // its document, so the slot stays and a new name is still refused…
    let deleted = harness.delete_shadow("capThing", Some("s0")).await;
    assert_eq!(
        deleted.status, 200,
        "expected the delete to succeed, got {}",
        deleted.status
    );
    let answer = harness
        .update("otherCapThing", Some("overflow"), payload.clone())
        .await;
    assert_eq!(
        answer.status,
        400,
        "expected the cap to still refuse a new shadow name after a delete, got {} with body {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    // …while the deleted name, whose slot is still held, is recreatable and resumes its
    // counter: two updates took it to 2 and the delete consumed 3.
    let answer = harness.update("capThing", Some("s0"), payload).await;
    assert_eq!(
        answer.status,
        200,
        "expected a recreate of a slot the store already holds, got {} with body {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.json().get("version").and_then(Value::as_i64),
        Some(4),
        "expected the recreated shadow to resume above the delete, got {}",
        String::from_utf8_lossy(&answer.body)
    );
}

struct RetainedCase {
    name: &'static str,
    topic: &'static str,
    qos: i32,
    payload: Vec<u8>,
    user_properties: Option<&'static str>,
}

fn retained_cases() -> Vec<RetainedCase> {
    vec![
        RetainedCase {
            name: "ascii text at QoS 0",
            topic: "sensors-alpha",
            qos: 0,
            payload: b"hello".to_vec(),
            user_properties: None,
        },
        RetainedCase {
            name: "ascii text at QoS 1 with user properties",
            topic: "sensors-beta",
            qos: 1,
            payload: b"hello".to_vec(),
            user_properties: Some("[{\"deviceName\": \"alpha\"}, {\"deviceCnt\": \"45\"}]"),
        },
        RetainedCase {
            name: "a single byte",
            topic: "sensors.gamma",
            qos: 1,
            payload: vec![0x00],
            user_properties: None,
        },
        RetainedCase {
            name: "every byte value, so nothing can be text-mangled",
            topic: "sensors_delta",
            qos: 0,
            payload: (0u8..=255).collect(),
            user_properties: Some("[{\"kind\": \"binary\"}]"),
        },
        RetainedCase {
            name: "multi-byte UTF-8",
            topic: "sensors-epsilon",
            qos: 1,
            payload: "temperatur-mäßig ✓ 温度".as_bytes().to_vec(),
            user_properties: None,
        },
        RetainedCase {
            name: "a four-kilobyte payload",
            topic: "sensors-zeta",
            qos: 0,
            payload: (0..4096u32).map(|index| (index % 251) as u8).collect(),
            user_properties: None,
        },
    ]
}

#[tokio::test]
async fn a_retained_publish_is_what_the_retained_message_reader_answers_with() {
    let harness = Harness::new();

    for case in retained_cases() {
        let published = harness
            .publish(
                case.topic,
                case.qos,
                true,
                case.user_properties,
                case.payload.clone(),
            )
            .await;
        assert_eq!(
            published.status,
            200,
            "case {:?}: expected 200, got {} with body {}",
            case.name,
            published.status,
            String::from_utf8_lossy(&published.body)
        );
        // The acknowledgement envelope itself is asserted by the `response_envelope`
        // suite; here it only has to say the publish was accepted.
        assert_eq!(
            published.json().get("message").and_then(Value::as_str),
            Some("OK"),
            "case {:?}: expected the publish acknowledgement, got body {}",
            case.name,
            String::from_utf8_lossy(&published.body)
        );

        let read = harness.get_retained_message(case.topic).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected 200, got {} with body {}",
            case.name,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let body = read.json();
        assert_eq!(
            body.get("topic").and_then(Value::as_str),
            Some(case.topic),
            "case {:?}: expected the topic back, got {body}",
            case.name
        );
        let payload = body
            .get("payload")
            .and_then(Value::as_str)
            .map(|encoded| {
                aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                    panic!("case {:?}: payload is not base64: {error}", case.name)
                })
            })
            .unwrap_or_else(|| panic!("case {:?}: no payload in {body}", case.name));
        assert_eq!(
            payload, case.payload,
            "case {:?}: expected the published bytes back, got {} bytes",
            case.name,
            payload.len()
        );
        assert_eq!(
            body.get("qos").and_then(Value::as_i64),
            Some(i64::from(case.qos)),
            "case {:?}: expected qos {}, got {body}",
            case.name,
            case.qos
        );
        // Milliseconds since the epoch, so far larger than the same instant in seconds.
        assert!(
            body.get("lastModifiedTime")
                .and_then(Value::as_i64)
                .is_some_and(|millis| millis > 1_700_000_000_000),
            "case {:?}: expected an epoch-millisecond lastModifiedTime, got {body}",
            case.name
        );
        let properties = body
            .get("userProperties")
            .and_then(Value::as_str)
            .map(|encoded| {
                String::from_utf8(
                    aws_smithy_types::base64::decode(encoded)
                        .unwrap_or_else(|error| panic!("case {:?}: {error}", case.name)),
                )
                .unwrap_or_else(|error| panic!("case {:?}: {error}", case.name))
            });
        assert_eq!(
            properties.as_deref(),
            case.user_properties,
            "case {:?}: expected the published user properties back, got {body}",
            case.name
        );

        // Reading a retained message does not consume it.
        let again = harness.get_retained_message(case.topic).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: expected a second read to still find it, got {}",
            case.name, again.status
        );
        assert_eq!(
            again.json().get("payload"),
            body.get("payload"),
            "case {:?}: expected a second read to answer the same payload",
            case.name
        );
    }
}

#[tokio::test]
async fn only_a_retained_publish_changes_the_retained_message_of_a_topic() {
    let harness = Harness::new();

    // A plain publish stores nothing: there is no retained message to read.
    for (case, topic, qos) in [
        ("QoS 0, not retained", "plain-zero", 0),
        ("QoS 1, not retained", "plain-one", 1),
    ] {
        let published = harness.publish(topic, qos, false, None, b"fire-and-forget".to_vec()).await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: expected 200, got {}",
            published.status
        );
        let read = harness.get_retained_message(topic).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: a publish without retain stores nothing, got {} with body {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&read.body)
        );
    }

    // A later retained publish replaces the stored message; a plain publish over it
    // leaves it alone; an empty retained payload clears it.
    harness
        .publish("lifecycle-topic", 0, true, Some("[{\"v\":\"1\"}]"), b"first".to_vec())
        .await;
    let first = harness.get_retained_message("lifecycle-topic").await.json();
    assert_eq!(
        first.get("qos").and_then(Value::as_i64),
        Some(0),
        "expected the first retained publish to be stored, got {first}"
    );

    harness
        .publish("lifecycle-topic", 1, true, None, b"second".to_vec())
        .await;
    let second = harness.get_retained_message("lifecycle-topic").await.json();
    assert_eq!(
        second
            .get("payload")
            .and_then(Value::as_str)
            .map(|encoded| aws_smithy_types::base64::decode(encoded).expect("base64")),
        Some(b"second".to_vec()),
        "expected the later retained publish to replace the stored one, got {second}"
    );
    assert_eq!(
        second.get("qos").and_then(Value::as_i64),
        Some(1),
        "expected the later publish's qos, got {second}"
    );
    assert_eq!(
        second.get("userProperties"),
        None,
        "expected the later publish's (absent) user properties, got {second}"
    );

    harness
        .publish("lifecycle-topic", 0, false, None, b"not retained".to_vec())
        .await;
    let unchanged = harness.get_retained_message("lifecycle-topic").await.json();
    assert_eq!(
        unchanged.get("payload"),
        second.get("payload"),
        "expected a plain publish to leave the retained message alone, got {unchanged}"
    );

    // An empty retained payload is how MQTT clears a retained message.
    let cleared = harness
        .publish("lifecycle-topic", 0, true, None, Vec::new())
        .await;
    assert_eq!(
        cleared.status, 200,
        "expected clearing to be accepted, got {}",
        cleared.status
    );
    let gone = harness.get_retained_message("lifecycle-topic").await;
    assert_eq!(
        gone.status,
        404,
        "expected the retained message to be cleared, got {} with body {}",
        gone.status,
        String::from_utf8_lossy(&gone.body)
    );
}

#[tokio::test]
async fn a_publish_outside_the_modelled_and_mqtt_rules_is_refused() {
    let harness = Harness::new();

    // Each published bound of the modelled qos range (0..1) is accepted; one past
    // either end is refused at the boundary.
    for (case, qos, expected_status) in [
        ("the low bound of the qos range", 0, 200_u16),
        ("the high bound of the qos range", 1, 200),
        ("one below the low bound", -1, 400),
        ("one above the high bound", 2, 400),
    ] {
        let answer = harness
            .publish("qos-topic", qos, true, None, b"x".to_vec())
            .await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected {expected_status}, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // MQTT wildcards are subscription syntax and may not appear in a publish topic.
    for (case, topic) in [
        ("the multi-level wildcard", "%23"),
        ("the single-level wildcard", "%2B"),
        ("a wildcard inside a name", "sensors%2Ball"),
    ] {
        let answer = harness.publish(topic, 0, true, None, b"x".to_vec()).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The documented message-payload quota is 128 KB: at the bound it is accepted, past
    // it refused — and refused as an entity too large (413), not as a malformed request.
    for (case, size, expected_status, expected_code) in [
        ("at the 128 KB quota", 128 * 1024, 200_u16, ""),
        (
            "one byte past the quota",
            128 * 1024 + 1,
            413,
            "RequestEntityTooLargeException",
        ),
    ] {
        let answer = harness
            .publish("sized-topic", 0, true, None, vec![7u8; size])
            .await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected {expected_status}, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if !expected_code.is_empty() {
            assert_eq!(
                answer.error_code(),
                expected_code,
                "case {case:?}: expected {expected_code}, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
    // The refused publish did not replace the accepted one.
    let stored = harness.get_retained_message("sized-topic").await.json();
    let payload = stored
        .get("payload")
        .and_then(Value::as_str)
        .map(|encoded| aws_smithy_types::base64::decode(encoded).expect("base64"))
        .unwrap_or_default();
    assert_eq!(
        payload.len(),
        128 * 1024,
        "expected the refused publish to change nothing, got {} bytes",
        payload.len()
    );

    // A topic nobody retained anything for.
    for (case, topic) in [
        ("never published", "quiet-topic"),
        ("published without retain", "qos-topic-other"),
    ] {
        if topic == "qos-topic-other" {
            harness.publish(topic, 0, false, None, b"x".to_vec()).await;
        }
        let answer = harness.get_retained_message(topic).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}
