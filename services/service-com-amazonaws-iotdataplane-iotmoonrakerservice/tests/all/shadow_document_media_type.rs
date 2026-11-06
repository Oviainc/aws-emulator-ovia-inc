// SPDX-License-Identifier: CC0-1.0

//! The media type this service announces for a shadow state document, end to end over
//! the router it is mounted in.
//!
//! The rule: a response whose body is a shadow state document — the `JsonDocument`
//! blob `UpdateThingShadow`, `GetThingShadow` and `DeleteThingShadow` bind as their
//! `@httpPayload` — is announced `content-type: application/json`, because the body IS
//! that JSON document and a client decides from this header whether to decode it.
//! Every other response is left exactly as the model's restJson1 rendering produces
//! it, so a blob carried as a BODY member (`GetRetainedMessage`'s `payload`) is still
//! base64 inside a JSON object and announced as that object's media type.
//!
//! The table rows differ along everything the rule generalises over — which of the
//! three operations answered, classic versus named shadow (including every character
//! class the modelled `ShadowName` pattern admits), the document's shape (scalar,
//! nested object, list, unicode), its size, which sections it carries, and how many
//! updates preceded the read — and no row is privileged. Each case also asserts the
//! BODY the media type describes: the document's own bytes, neither base64-encoded nor
//! wrapped in an outer member object.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service
/// by its SigV4 credential scope (signing name `iotdata`), the way an SDK request
/// does. Signature verification is off in this harness, so the value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/iotdata/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The media type a JSON document is announced with.
const JSON: &str = "application/json";

/// One HTTP answer, reduced to what these tests assert on. The sibling suite's
/// harness keeps only the status and body; this one keeps the response headers,
/// which are the subject here.
struct Answer {
    status: u16,
    content_type: Vec<String>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine
/// behind one mounted service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iotdataplane_iotmoonrakerservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Vec<u8>) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "data-ats.iot.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/octet-stream");
        let response = self
            .router
            .dispatch(
                request
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request"),
            )
            .await;
        let content_type = response
            .headers()
            .get_all("content-type")
            .iter()
            .map(|value| {
                value
                    .to_str()
                    .expect("a media type is ASCII")
                    .to_ascii_lowercase()
            })
            .collect();
        Answer {
            status: response.status().as_u16(),
            content_type,
            body: response.into_body().to_vec(),
        }
    }

    /// The shadow route, with the `name` query parameter only when a shadow is named.
    fn shadow_uri(thing: &str, shadow: Option<&str>) -> String {
        match shadow {
            Some(name) => format!("/things/{thing}/shadow?name={name}"),
            None => format!("/things/{thing}/shadow"),
        }
    }

    async fn update(&self, thing: &str, shadow: Option<&str>, payload: Vec<u8>) -> Answer {
        self.send("POST", &Self::shadow_uri(thing, shadow), payload)
            .await
    }

    async fn get_shadow(&self, thing: &str, shadow: Option<&str>) -> Answer {
        self.send("GET", &Self::shadow_uri(thing, shadow), Vec::new())
            .await
    }

    async fn delete_shadow(&self, thing: &str, shadow: Option<&str>) -> Answer {
        self.send("DELETE", &Self::shadow_uri(thing, shadow), Vec::new())
            .await
    }
}

/// Assert the one media type a JSON-document response announces: exactly one
/// `content-type`, and `application/json`.
fn assert_announced_as_json(case: &str, answer: &Answer) {
    assert_eq!(
        answer.content_type.len(),
        1,
        "case {case:?}: expected exactly one content-type header, got {:?}",
        answer.content_type
    );
    assert_eq!(
        answer.content_type.first().map(String::as_str),
        Some(JSON),
        "case {case:?}: expected the document to be announced as {JSON}, got {:?}",
        answer.content_type
    );
}

/// Assert the body the media type describes really is the document: UTF-8 JSON that
/// parses to an object, not base64 and not wrapped in an outer member object.
fn assert_body_is_the_document(case: &str, answer: &Answer) -> Map<String, Value> {
    let parsed = answer.json();
    let object = parsed.as_object().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the state document itself, got {}",
            String::from_utf8_lossy(&answer.body)
        )
    });
    assert!(
        !object.contains_key("payload"),
        "case {case:?}: the document must be the whole body, not wrapped in a \
         'payload' member: {}",
        String::from_utf8_lossy(&answer.body)
    );
    object.clone()
}

/// One shadow a case writes, reads and deletes.
struct DocumentCase {
    name: &'static str,
    thing: &'static str,
    /// `None` is the classic (unnamed) shadow.
    shadow: Option<&'static str>,
    /// The updates applied in order; the last one's answer is asserted alongside the
    /// read, so a case can vary how many updates precede the read.
    updates: Vec<Value>,
    /// The `state` node the document is expected to hold once every update is applied.
    expected_state: Value,
}

/// The rows: one per document class the rule generalises over. The shadow names cover
/// every character class the modelled `ShadowName` pattern admits
/// (`^[$a-zA-Z0-9:_-]+$`, len 1..64 — `smithy_query(query="operation",
/// operation="GetThingShadow")`), and the documents differ in shape, value type,
/// section, size and update count.
fn document_cases() -> Vec<DocumentCase> {
    // A document large enough to span several KB, well inside the 8 KB state-document
    // quota the handler enforces.
    let wide: Map<String, Value> = (0..60)
        .map(|index| (format!("sensor_{index:02}"), json!(index * 7)))
        .collect();

    vec![
        DocumentCase {
            name: "the classic shadow, one reported scalar",
            thing: "boiler-1",
            shadow: None,
            updates: vec![json!({"state": {"reported": {"temperature": 21}}})],
            expected_state: json!({"reported": {"temperature": 21}}),
        },
        DocumentCase {
            name: "a named shadow, a nested desired object",
            thing: "boiler-1",
            shadow: Some("firmware"),
            updates: vec![json!({
                "state": {"desired": {"update": {"channel": {"name": "beta", "retries": 3}}}}
            })],
            expected_state: json!({
                "desired": {"update": {"channel": {"name": "beta", "retries": 3}}},
                "delta": {"update": {"channel": {"name": "beta", "retries": 3}}}
            }),
        },
        DocumentCase {
            name: "a named shadow whose name uses the pattern's '$' and ':'",
            thing: "robot:arm_7",
            // The `$` sits after the first character: a LEADING `$` names the service's
            // own reserved namespace, which no request may address.
            shadow: Some("package$:v2"),
            updates: vec![json!({"state": {"reported": {"zones": [1, 2, 3], "ok": true}}})],
            expected_state: json!({"reported": {"zones": [1, 2, 3], "ok": true}}),
        },
        DocumentCase {
            name: "a named shadow carrying unicode values",
            thing: "thermostat_02",
            shadow: Some("locale-A1_b"),
            updates: vec![json!({"state": {"reported": {"label": "café — 温度", "unit": "°C"}}})],
            expected_state: json!({"reported": {"label": "café — 温度", "unit": "°C"}}),
        },
        DocumentCase {
            name: "both sections in one update",
            thing: "valve-9",
            shadow: None,
            updates: vec![json!({
                "state": {"desired": {"position": "open"}, "reported": {"position": "shut"}}
            })],
            expected_state: json!({
                "desired": {"position": "open"},
                "reported": {"position": "shut"},
                "delta": {"position": "open"}
            }),
        },
        DocumentCase {
            name: "three updates merged, including a null that deletes a key",
            thing: "pump-3",
            shadow: Some("telemetry"),
            updates: vec![
                json!({"state": {"reported": {"rpm": 1200, "stale": "yes"}}}),
                json!({"state": {"reported": {"rpm": 1350}}}),
                json!({"state": {"reported": {"stale": null}}}),
            ],
            expected_state: json!({"reported": {"rpm": 1350}}),
        },
        DocumentCase {
            name: "a multi-kilobyte document",
            thing: "gateway-fleet-01",
            shadow: Some("wide"),
            updates: vec![json!({"state": {"reported": wide.clone()}})],
            expected_state: json!({"reported": Value::Object(wide)}),
        },
    ]
}

#[tokio::test]
async fn every_shadow_document_response_is_announced_as_json() {
    let harness = Harness::new();

    for case in document_cases() {
        let name = case.name;

        // 1. UpdateThingShadow — the merged document it answers with.
        let mut update = None;
        for (index, patch) in case.updates.iter().enumerate() {
            let answer = harness
                .update(
                    case.thing,
                    case.shadow,
                    serde_json::to_vec(patch).expect("a patch serializes"),
                )
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {name:?}: update {index} expected 200, got {} with body {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            // Every update's answer is a state document, not only the last.
            assert_announced_as_json(name, &answer);
            update = Some(answer);
        }
        let update = update.expect("every case applies at least one update");
        let updated = assert_body_is_the_document(name, &update);
        assert_eq!(
            updated.get("state"),
            Some(&case.expected_state),
            "case {name:?}: expected the merged state document, got {}",
            String::from_utf8_lossy(&update.body)
        );

        // 2. GetThingShadow — the same document read back out of the store.
        let read = harness.get_shadow(case.thing, case.shadow).await;
        assert_eq!(
            read.status,
            200,
            "case {name:?}: read expected 200, got {} with body {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_announced_as_json(name, &read);
        let stored = assert_body_is_the_document(name, &read);
        assert_eq!(
            stored.get("state"),
            Some(&case.expected_state),
            "case {name:?}: expected the stored state document, got {}",
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            stored.get("version").and_then(Value::as_i64),
            Some(case.updates.len() as i64),
            "case {name:?}: expected one version per accepted update, got {}",
            String::from_utf8_lossy(&read.body)
        );

        // 3. DeleteThingShadow — the deletion acknowledgement document.
        let deleted = harness.delete_shadow(case.thing, case.shadow).await;
        assert_eq!(
            deleted.status,
            200,
            "case {name:?}: delete expected 200, got {} with body {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        assert_announced_as_json(name, &deleted);
        let acknowledgement = assert_body_is_the_document(name, &deleted);
        assert_eq!(
            acknowledgement.get("version").and_then(Value::as_i64),
            Some(case.updates.len() as i64),
            "case {name:?}: expected the deleted version in the acknowledgement, got {}",
            String::from_utf8_lossy(&deleted.body)
        );
    }
}

#[tokio::test]
async fn a_response_that_is_not_a_shadow_document_keeps_its_modelled_rendering() {
    let harness = Harness::new();

    // A retained message: the blob is a BODY member, so it stays base64 inside the
    // JSON object the protocol renders — the rule is keyed on the payload binding,
    // not on a response carrying bytes.
    let published = harness
        .send(
            "POST",
            "/topics/fleet%2Fstatus?qos=1&retain=true",
            b"not a json document\x00\x01".to_vec(),
        )
        .await;
    assert_eq!(
        published.status,
        200,
        "publish expected 200, got {} with body {}",
        published.status,
        String::from_utf8_lossy(&published.body)
    );
    // `Publish` binds no `@httpPayload`: its acknowledgement envelope is a JSON object
    // of members, announced as that object and untouched by the media-type rule.
    assert_eq!(
        published.content_type.first().map(String::as_str),
        Some(JSON),
        "the publish acknowledgement is announced as {JSON}, got {:?}",
        published.content_type
    );
    assert!(
        published
            .json()
            .get("message")
            .and_then(Value::as_str)
            .is_some(),
        "expected the publish acknowledgement envelope, got {}",
        String::from_utf8_lossy(&published.body)
    );

    let retained = harness
        .send("GET", "/retainedMessage/fleet%2Fstatus", Vec::new())
        .await;
    assert_eq!(
        retained.status,
        200,
        "retained read expected 200, got {} with body {}",
        retained.status,
        String::from_utf8_lossy(&retained.body)
    );
    assert_eq!(
        retained.content_type.first().map(String::as_str),
        Some(JSON),
        "the retained-message object is announced as {JSON}, got {:?}",
        retained.content_type
    );
    let body = retained.json();
    assert_eq!(
        body.get("payload")
            .and_then(Value::as_str)
            .map(|encoded| aws_smithy_types::base64::decode(encoded).expect("base64")),
        Some(b"not a json document\x00\x01".to_vec()),
        "a blob body member stays base64 inside the JSON object, got {body}"
    );

    // A listing: a JSON object of modelled members, announced as that object.
    harness
        .update(
            "listed-thing",
            Some("one"),
            br#"{"state":{"reported":{"a":1}}}"#.to_vec(),
        )
        .await;
    let listed = harness
        .send(
            "GET",
            "/api/things/shadow/ListNamedShadowsForThing/listed-thing",
            Vec::new(),
        )
        .await;
    assert_eq!(
        listed.status,
        200,
        "listing expected 200, got {} with body {}",
        listed.status,
        String::from_utf8_lossy(&listed.body)
    );
    assert_eq!(
        listed.content_type.first().map(String::as_str),
        Some(JSON),
        "the listing object is announced as {JSON}, got {:?}",
        listed.content_type
    );
    assert_eq!(
        listed.json().get("results"),
        Some(&json!(["one"])),
        "expected the stored named shadow in the listing, got {}",
        String::from_utf8_lossy(&listed.body)
    );

    // A modelled failure of each shadow operation: the protocol's JSON error
    // envelope, which the media-type rule does not touch.
    for (case, method, uri, body, status, code) in [
        (
            "reading a shadow nobody wrote",
            "GET",
            "/things/absent-thing/shadow",
            Vec::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "deleting a shadow nobody wrote",
            "DELETE",
            "/things/absent-thing/shadow?name=gone",
            Vec::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an update whose document is not a state document",
            "POST",
            "/things/absent-thing/shadow",
            b"{\"not-state\":true}".to_vec(),
            400,
            "InvalidRequestException",
        ),
    ] {
        let answer = harness.send(method, uri, body).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected {status}, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.first().map(String::as_str),
            Some(JSON),
            "case {case:?}: the error envelope is announced as {JSON}, got {:?}",
            answer.content_type
        );
        let rendered = answer.json();
        let kind = rendered
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            kind, code,
            "case {case:?}: expected the modelled error code, got {rendered}"
        );
    }
}
