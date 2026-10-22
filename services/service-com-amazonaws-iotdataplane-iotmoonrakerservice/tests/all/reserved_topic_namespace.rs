// SPDX-License-Identifier: CC0-1.0

//! `Publish` and the reserved `$` topic namespace, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule these cases assert: the topic namespace whose names begin with `$` belongs
//! to the service's own system topics, so a client publish to such a topic is refused
//! with `InvalidRequestException` (HTTP 400, restJson1 error identity on
//! `x-amzn-errortype`) and changes nothing; a `$` anywhere else in the topic name is an
//! ordinary application topic and is published normally.
//!
//! The table spans what the rule generalises over — how many segments the topic has,
//! whether the `$` arrives raw or percent-encoded, where in the name the `$` sits, and
//! whether the topic is otherwise well-formed — and the accepted rows are read back
//! through `GetRetainedMessage` so acceptance means stored, not merely answered 200.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// The reserved-topic wording and the prefix test, named by the crate so this suite
/// cannot drift from the guard the handler applies.
use service_com_amazonaws_iotdataplane_iotmoonrakerservice::ops::publish::{
    is_reserved_topic, RESERVED_TOPIC_MESSAGE, RESERVED_TOPIC_PREFIX,
};

/// A signed-looking request header so the router's `can_handle` resolves this service
/// by its SigV4 credential scope (signing name `iotdata`), the way an SDK request
/// does. Signature verification is off in this harness, so the value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/iotdata/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// One HTTP answer, reduced to what these cases assert on.
struct Answer {
    status: u16,
    /// The restJson1 error identity, from the `x-amzn-errortype` response header.
    error_type: Option<String>,
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

    /// The error shape named in the body, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
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
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Answer {
            status: response.status().as_u16(),
            error_type,
            body: response.into_body().to_vec(),
        }
    }

    async fn publish(&self, uri_topic: &str, retain: bool, payload: Vec<u8>) -> Answer {
        self.send(
            "POST",
            &format!("/topics/{uri_topic}?qos=0&retain={retain}"),
            payload,
        )
        .await
    }

    async fn get_retained_message(&self, uri_topic: &str) -> Answer {
        self.send("GET", &format!("/retainedMessage/{uri_topic}"), Vec::new())
            .await
    }
}

struct TopicCase {
    name: &'static str,
    /// The topic as it appears in the request path.
    uri_topic: &'static str,
    /// The same topic after the path label is decoded — what the handler sees.
    decoded: &'static str,
    /// Whether the topic lies in the reserved namespace and must be refused.
    reserved: bool,
}

/// Rows differing in segment count, in whether the `$` arrives raw or percent-encoded,
/// and in where in the name the `$` sits. The reserved rows include the system
/// namespaces AWS IoT Core publishes on itself (`$aws/…`, `$SYS/…`), none of which the
/// issue's own recording cited beyond the shadow topic.
fn topic_cases() -> Vec<TopicCase> {
    vec![
        TopicCase {
            name: "the device-shadow system topic",
            uri_topic: "$aws%2Fthings%2Fsensor-1%2Fshadow%2Fupdate",
            decoded: "$aws/things/sensor-1/shadow/update",
            reserved: true,
        },
        TopicCase {
            name: "a percent-encoded leading $",
            uri_topic: "%24aws%2Frules%2Fingest",
            decoded: "$aws/rules/ingest",
            reserved: true,
        },
        TopicCase {
            name: "the broker's $SYS namespace",
            uri_topic: "$SYS%2Fbroker%2Fuptime",
            decoded: "$SYS/broker/uptime",
            reserved: true,
        },
        TopicCase {
            name: "a single-segment reserved topic",
            uri_topic: "$internal",
            decoded: "$internal",
            reserved: true,
        },
        TopicCase {
            name: "the bare reserved prefix",
            uri_topic: "$",
            decoded: "$",
            reserved: true,
        },
        TopicCase {
            name: "a plain application topic",
            uri_topic: "sensors%2Fsensor-1%2Ftemperature",
            decoded: "sensors/sensor-1/temperature",
            reserved: false,
        },
        TopicCase {
            name: "a $ inside a later segment",
            uri_topic: "sensors%2F%24batch%2Ftemperature",
            decoded: "sensors/$batch/temperature",
            reserved: false,
        },
        TopicCase {
            name: "a $ at the end of the name",
            uri_topic: "sensors-total%24",
            decoded: "sensors-total$",
            reserved: false,
        },
        TopicCase {
            name: "a single-segment application topic",
            uri_topic: "heartbeat",
            decoded: "heartbeat",
            reserved: false,
        },
    ]
}

#[tokio::test]
async fn only_the_leading_dollar_marks_a_topic_as_reserved() {
    // The crate's predicate and the table agree on which rows are reserved, so a case
    // cannot be mislabelled.
    for case in topic_cases() {
        assert_eq!(
            is_reserved_topic(case.decoded),
            case.reserved,
            "case {:?}: expected reserved={}, got {}",
            case.name,
            case.reserved,
            is_reserved_topic(case.decoded)
        );
        assert_eq!(
            case.decoded.starts_with(RESERVED_TOPIC_PREFIX),
            case.reserved,
            "case {:?}: the reserved namespace is the one the prefix {RESERVED_TOPIC_PREFIX:?} \
             opens, got {:?}",
            case.name,
            case.decoded
        );
    }
}

#[tokio::test]
async fn a_publish_to_the_reserved_namespace_is_refused_and_stores_nothing() {
    let harness = Harness::new();

    for case in topic_cases().into_iter().filter(|case| case.reserved) {
        let answer = harness
            .publish(case.uri_topic, true, b"reserved-attempt".to_vec())
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {:?}: expected 400, got {} with body {}",
            case.name,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {:?}: expected the restJson1 error identity on x-amzn-errortype, got {:?}",
            case.name,
            answer.error_type
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {:?}: expected InvalidRequestException in the body, got {}",
            case.name,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            RESERVED_TOPIC_MESSAGE,
            "case {:?}: expected the service's reserved-topic message, got {}",
            case.name,
            answer.text()
        );

        // A refused publish reaches neither the broker nor the retained-message store.
        let read = harness.get_retained_message(case.uri_topic).await;
        assert_eq!(
            read.status,
            404,
            "case {:?}: a refused publish must retain nothing, got {} with body {}",
            case.name,
            read.status,
            read.text()
        );
    }
}

#[tokio::test]
async fn a_dollar_outside_the_first_position_is_an_ordinary_topic() {
    let harness = Harness::new();

    for case in topic_cases().into_iter().filter(|case| !case.reserved) {
        let payload = format!("payload for {}", case.decoded).into_bytes();
        let published = harness
            .publish(case.uri_topic, true, payload.clone())
            .await;
        assert_eq!(
            published.status,
            200,
            "case {:?}: expected 200, got {} with body {}",
            case.name,
            published.status,
            published.text()
        );

        // Accepted means STORED: the retained-message reader answers from the store.
        let read = harness.get_retained_message(case.uri_topic).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected the retained message to be readable, got {} with body {}",
            case.name,
            read.status,
            read.text()
        );
        let body = read.json();
        assert_eq!(
            body.get("topic").and_then(Value::as_str),
            Some(case.decoded),
            "case {:?}: expected the stored topic {:?}, got {}",
            case.name,
            case.decoded,
            read.text()
        );
        let stored = body
            .get("payload")
            .and_then(Value::as_str)
            .map(|encoded| aws_smithy_types::base64::decode(encoded).expect("base64"))
            .unwrap_or_default();
        assert_eq!(
            stored,
            payload,
            "case {:?}: expected the published payload back, got {} bytes",
            case.name,
            stored.len()
        );
    }
}
