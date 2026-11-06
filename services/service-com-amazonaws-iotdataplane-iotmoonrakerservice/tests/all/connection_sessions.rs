// SPDX-License-Identifier: CC0-1.0

//! The MQTT connection family — `GetConnection`, `ListSubscriptions`,
//! `SendDirectMessage` and `DeleteConnection` — over the broker's session table, end to
//! end over HTTP through the router this service mounts in production.
//!
//! The rules these cases assert:
//!
//! * These four operations read and write the session the BROKER HOLDS for a client id.
//!   A client id the broker holds no session for is `ResourceNotFoundException` (HTTP
//!   404) from every one of them — the normal, expected outcome for a client that has
//!   never connected, and the one answer that must never be a fabricated 200: a
//!   `SendDirectMessage` answered 200 for a client nothing can deliver to is undetected
//!   message loss.
//! * `GetConnection` reports the stored session: its connection state, the instants it
//!   connected and disconnected, why it disconnected, and the session's own settings —
//!   with the socket information reported only to a request that asked for it.
//! * `ListSubscriptions` pages over the session's topic filters in stable order,
//!   including for an offline client with a persistent session.
//! * `SendDirectMessage` delivers into the session: a message to an offline client is
//!   queued on its persistent session, a message to a connected client is handed to the
//!   connection and queues nothing, a payload past the broker's quota is 413, a topic
//!   outside the broker's topic-name rules is 400, and a request asking for delivery
//!   confirmation — which no client in this emulator can send — is the modelled 504.
//! * `DeleteConnection` closes the connection and records that the SERVICE closed it,
//!   discarding the session state only when the caller asks for it (or when the client
//!   connected with a clean session, for which there is no persistent state to keep).
//! * A disconnected client's record is reported for exactly as long as the model
//!   documents, and is released once the clock has passed that window.
//!
//! A session is opened by an MQTT CONNECT, a transport this emulator does not serve, so
//! these cases put sessions in the table directly — which is what the broker's own
//! CONNECT handling would do — and then drive every operation over HTTP. Each table row
//! varies what the rule generalises over: client id form, connection state, session
//! settings, subscription count, page size, payload size, topic form, and how long ago
//! the client disconnected.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::publish_topic_and_payload_limits::encode_component;
use service_com_amazonaws_iotdataplane_iotmoonrakerservice::state::{
    now_millis, payload_too_large, too_many_topic_bytes, too_many_topic_parts, DirectMessage,
    MqttSession, CLIENT_NOT_FOUND, DEFAULT_SUBSCRIPTION_PAGE_SIZE,
    DISCONNECT_INFORMATION_RETENTION_MILLIS, INVALID_TOPIC_FILTER, MAX_PUBLISH_PAYLOAD_BYTES,
    MAX_QUEUED_SESSION_MESSAGES, MAX_TOPIC_BYTES, MAX_TOPIC_PARTS, NO_DELIVERY_CONFIRMATION,
    SERVER_INITIATED_DISCONNECT,
};
use service_com_amazonaws_iotdataplane_iotmoonrakerservice::ComAmazonawsIotdataplaneIotmoonrakerserviceState as IotDataState;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope (signing name `iotdata`), the way an SDK request does.
/// Signature verification is off in this harness, so the value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/iotdata/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// One HTTP answer, reduced to what these cases assert on.
struct Answer {
    status: u16,
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

    /// The modelled error code, which restJson1 carries in `__type`.
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

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service — and kept here, so a case can put a session in the broker's table
/// the way an MQTT CONNECT would.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iotdataplane_iotmoonrakerservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// Open `session` in the broker's session table, which is what an MQTT CONNECT does.
    fn connect(&self, session: MqttSession) {
        self.engine
            .state::<IotDataState>()
            .lock()
            .connections
            .insert(session.client_id.clone(), session);
    }

    /// The session the broker holds for `client_id`, as it stands now.
    fn stored_session(&self, client_id: &str) -> Option<MqttSession> {
        self.engine
            .state::<IotDataState>()
            .lock()
            .connections
            .get(client_id)
            .cloned()
    }

    /// How many client ids the table holds, reported without releasing anything.
    fn session_count(&self) -> usize {
        self.engine.state::<IotDataState>().lock().connections.len()
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
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    async fn get_connection(&self, client_id: &str, include_socket_information: bool) -> Answer {
        self.send(
            "GET",
            &format!(
                "/connections/{}?includeSocketInformation={include_socket_information}",
                encode_component(client_id)
            ),
            Vec::new(),
        )
        .await
    }

    async fn list_subscriptions(&self, client_id: &str, query: &str) -> Answer {
        self.send(
            "GET",
            &format!(
                "/connections/{}/subscriptions{query}",
                encode_component(client_id)
            ),
            Vec::new(),
        )
        .await
    }

    async fn send_direct_message(
        &self,
        client_id: &str,
        topic: &str,
        confirmation: bool,
        payload: Vec<u8>,
    ) -> Answer {
        self.send(
            "POST",
            &format!(
                "/connections/{}/messages?topic={}&confirmation={confirmation}&timeout=5",
                encode_component(client_id),
                encode_component(topic)
            ),
            payload,
        )
        .await
    }

    async fn delete_connection(
        &self,
        client_id: &str,
        clean_session: bool,
        prevent_will_message: bool,
    ) -> Answer {
        self.send(
            "DELETE",
            &format!(
                "/connections/{}?cleanSession={clean_session}&preventWillMessage={prevent_will_message}",
                encode_component(client_id)
            ),
            Vec::new(),
        )
        .await
    }
}

/// A connected session, with socket information and a thing behind it.
fn connected_session(client_id: &str) -> MqttSession {
    MqttSession {
        client_id: client_id.to_string(),
        connected: true,
        clean_session: false,
        thing_name: Some("sensor-1".to_string()),
        keep_alive_seconds: 60,
        session_expiry_seconds: 3600,
        connected_since_millis: now_millis() - 5_000,
        source_ip: Some("198.51.100.7".to_string()),
        source_port: 51_234,
        target_ip: Some("10.0.3.9".to_string()),
        target_port: 8883,
        vpc_endpoint_id: Some("vpce-0abc1234".to_string()),
        ..MqttSession::default()
    }
}

/// A session whose client went offline `ago_millis` ago, keeping its persistent state.
fn offline_session(client_id: &str, ago_millis: i64) -> MqttSession {
    let mut session = connected_session(client_id);
    session.connected = false;
    session.disconnected_since_millis = now_millis() - ago_millis;
    session.disconnect_reason = Some("CONNECTION_LOST".to_string());
    session
}

/// The four routes of the connection family, as `(name, method, uri, body)` around a
/// client id — so one case can exercise every one of them. Only `SendDirectMessage`
/// carries a body: its message payload.
fn connection_routes(client_id: &str) -> Vec<(&'static str, &'static str, String, Vec<u8>)> {
    let encoded = encode_component(client_id);
    vec![
        (
            "GetConnection",
            "GET",
            format!("/connections/{encoded}?includeSocketInformation=false"),
            Vec::new(),
        ),
        (
            "ListSubscriptions",
            "GET",
            format!("/connections/{encoded}/subscriptions"),
            Vec::new(),
        ),
        (
            "SendDirectMessage",
            "POST",
            format!("/connections/{encoded}/messages?topic=orders%2Fnew&confirmation=false&timeout=5"),
            b"direct".to_vec(),
        ),
        (
            "DeleteConnection",
            "DELETE",
            format!("/connections/{encoded}?cleanSession=false&preventWillMessage=false"),
            Vec::new(),
        ),
    ]
}

#[tokio::test]
async fn every_connection_operation_refuses_a_client_with_no_session() {
    let harness = Harness::new();
    // A session the broker DOES hold, so a 404 below cannot come from an empty table.
    harness.connect(connected_session("live-client"));

    for (case, client_id) in [
        ("a client id that has never connected", "never-connected-7f3a".to_string()),
        ("another client id entirely", "fleet/device-42".to_string()),
        ("a client id at the modelled maximum length", "c".repeat(128)),
        ("a client id carrying characters an SDK must encode", "device 9 ✓".to_string()),
    ] {
        for (operation, method, uri, body) in connection_routes(&client_id) {
            let answer = harness.send(method, &uri, body).await;
            assert_eq!(
                answer.status,
                404,
                "case {case:?} / {operation}: expected 404, got {} with body {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?} / {operation}: expected ResourceNotFoundException, got {}",
                answer.text()
            );
            assert_eq!(
                answer.message(),
                CLIENT_NOT_FOUND,
                "case {case:?} / {operation}: expected the service's not-found text, got {:?}",
                answer.message()
            );
        }
        assert!(
            harness.stored_session(&client_id).is_none(),
            "case {case:?}: a refused call must not have created a session"
        );
    }

    // The session that WAS held is untouched by every refusal above.
    let live = harness
        .stored_session("live-client")
        .expect("the live session is still held");
    assert!(
        live.connected && live.queued_messages.is_empty(),
        "expected the live session to be untouched, got {live:?}"
    );
}

#[tokio::test]
async fn get_connection_reports_the_stored_session() {
    let harness = Harness::new();

    let mut clean = connected_session("clean-client");
    clean.clean_session = true;
    clean.thing_name = None;
    clean.vpc_endpoint_id = None;
    clean.keep_alive_seconds = 30;
    clean.session_expiry_seconds = 0;

    for (case, session) in [
        ("a connected persistent session", connected_session("persistent-client")),
        ("a connected clean session", clean),
        ("an offline persistent session", offline_session("offline-client", 60_000)),
    ] {
        let client_id = session.client_id.clone();
        harness.connect(session.clone());

        for include_socket_information in [false, true] {
            let answer = harness
                .get_connection(&client_id, include_socket_information)
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?} (socket={include_socket_information}): expected 200, got {} with \
                 body {}",
                answer.status,
                answer.text()
            );
            let body = answer.json();
            let text = answer.text();
            let string_of = |key: &str| {
                body.get(key)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            // A member at its modelled default may travel as that default or be absent;
            // either way it MEANS the default, so each is read with that default.
            let bool_of = |key: &str| body.get(key).and_then(Value::as_bool).unwrap_or(false);
            let int_of = |key: &str| body.get(key).and_then(Value::as_i64).unwrap_or(0);

            assert_eq!(
                string_of("clientId").as_deref(),
                Some(client_id.as_str()),
                "case {case:?}: expected the session's own client id, got {text}"
            );
            assert_eq!(
                bool_of("connected"),
                session.connected,
                "case {case:?}: expected connected={}, got {text}",
                session.connected
            );
            assert_eq!(
                bool_of("cleanSession"),
                session.clean_session,
                "case {case:?}: expected cleanSession={}, got {text}",
                session.clean_session
            );
            assert_eq!(
                string_of("thingName"),
                session.thing_name,
                "case {case:?}: expected the session's thing name, got {text}"
            );
            assert_eq!(
                string_of("vpcEndpointId"),
                session.vpc_endpoint_id,
                "case {case:?}: expected the session's VPC endpoint, got {text}"
            );
            assert_eq!(
                int_of("keepAliveDuration"),
                i64::from(session.keep_alive_seconds),
                "case {case:?}: expected the session's keep-alive, got {text}"
            );
            assert_eq!(
                int_of("sessionExpiry"),
                session.session_expiry_seconds,
                "case {case:?}: expected the session's expiry interval, got {text}"
            );
            // `connectedSince` is reported while the connection is open and
            // `disconnectedSince` / `disconnectReason` once it has closed — never both.
            if session.connected {
                assert_eq!(
                    int_of("connectedSince"),
                    session.connected_since_millis,
                    "case {case:?}: expected the instant the client connected, got {text}"
                );
                assert_eq!(
                    int_of("disconnectedSince"),
                    0,
                    "case {case:?}: a connected client has no disconnect instant, got {text}"
                );
                assert_eq!(
                    string_of("disconnectReason"),
                    None,
                    "case {case:?}: a connected client has no disconnect reason, got {text}"
                );
            } else {
                assert_eq!(
                    int_of("disconnectedSince"),
                    session.disconnected_since_millis,
                    "case {case:?}: expected the instant the client disconnected, got {text}"
                );
                assert_eq!(
                    string_of("disconnectReason"),
                    session.disconnect_reason,
                    "case {case:?}: expected the recorded disconnect reason, got {text}"
                );
                assert_eq!(
                    int_of("connectedSince"),
                    0,
                    "case {case:?}: a disconnected client has no connect instant, got {text}"
                );
            }

            // Socket information is reported only to a request that asked for it.
            if include_socket_information {
                assert_eq!(
                    string_of("sourceIp"),
                    session.source_ip,
                    "case {case:?}: expected the session's source address, got {text}"
                );
                assert_eq!(
                    int_of("sourcePort"),
                    i64::from(session.source_port),
                    "case {case:?}: expected the session's source port, got {text}"
                );
                assert_eq!(
                    string_of("targetIp"),
                    session.target_ip,
                    "case {case:?}: expected the session's target address, got {text}"
                );
                assert_eq!(
                    int_of("targetPort"),
                    i64::from(session.target_port),
                    "case {case:?}: expected the session's target port, got {text}"
                );
            } else {
                assert_eq!(
                    string_of("sourceIp"),
                    None,
                    "case {case:?}: socket information was not asked for, got {text}"
                );
                assert_eq!(
                    int_of("sourcePort"),
                    0,
                    "case {case:?}: socket information was not asked for, got {text}"
                );
                assert_eq!(
                    string_of("targetIp"),
                    None,
                    "case {case:?}: socket information was not asked for, got {text}"
                );
                assert_eq!(
                    int_of("targetPort"),
                    0,
                    "case {case:?}: socket information was not asked for, got {text}"
                );
            }
        }

        // Reading a connection changes nothing about it.
        let after = harness
            .stored_session(&client_id)
            .expect("the session is still held");
        assert_eq!(
            (after.connected, after.disconnected_since_millis),
            (session.connected, session.disconnected_since_millis),
            "case {case:?}: reading a connection must not move its state"
        );
    }
}

#[tokio::test]
async fn list_subscriptions_pages_over_the_sessions_topic_filters() {
    let harness = Harness::new();

    // Rows differing in how many filters the session holds — including none, one, and
    // more than the default page — and in whether the client is still connected.
    for (case, count, connected) in [
        ("a session with no subscriptions", 0usize, true),
        ("a session with one subscription", 1, true),
        ("a session with three subscriptions", 3, true),
        (
            "an offline persistent session past the default page",
            DEFAULT_SUBSCRIPTION_PAGE_SIZE + 5,
            false,
        ),
    ] {
        let client_id = format!("sub-client-{count}-{connected}");
        let mut session = if connected {
            connected_session(&client_id)
        } else {
            offline_session(&client_id, 30_000)
        };
        let expected: Vec<String> = (0..count).map(|index| format!("filter/{index:03}/#")).collect();
        for (index, filter) in expected.iter().enumerate() {
            session
                .subscriptions
                .insert(filter.clone(), (index % 2) as i32);
        }
        harness.connect(session);

        // The default page is the documented one, so a session past it reports a token.
        let first = harness.list_subscriptions(&client_id, "").await;
        assert_eq!(
            first.status,
            200,
            "case {case:?}: expected 200, got {} with body {}",
            first.status,
            first.text()
        );
        let page = subscriptions_of(case, &first);
        assert_eq!(
            page.len(),
            count.min(DEFAULT_SUBSCRIPTION_PAGE_SIZE),
            "case {case:?}: expected the documented default page, got {}",
            first.text()
        );

        // Walking the pages, at several page sizes, yields exactly the stored filters in
        // stable order, each with the QoS it was stored at.
        for page_size in [1usize, 2, DEFAULT_SUBSCRIPTION_PAGE_SIZE, 200] {
            let mut seen: Vec<(String, i64)> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let query = match &token {
                    None => format!("?maxResults={page_size}"),
                    Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                };
                let answer = harness.list_subscriptions(&client_id, &query).await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?} at page size {page_size}: expected 200, got {} with body {}",
                    answer.status,
                    answer.text()
                );
                let returned = subscriptions_of(case, &answer);
                assert!(
                    returned.len() <= page_size,
                    "case {case:?} at page size {page_size}: expected at most {page_size} \
                     entries, got {}",
                    returned.len()
                );
                seen.extend(returned);
                match answer.json().get("nextToken").and_then(Value::as_str) {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            let filters: Vec<String> = seen.iter().map(|(filter, _)| filter.clone()).collect();
            assert_eq!(
                filters, expected,
                "case {case:?} at page size {page_size}: expected the stored filters in stable \
                 order, got {filters:?}"
            );
            for (index, (filter, qos)) in seen.iter().enumerate() {
                assert_eq!(
                    *qos,
                    (index % 2) as i64,
                    "case {case:?} at page size {page_size}: expected the QoS stored for \
                     {filter:?}, got {qos}"
                );
            }
        }

        // A token this service did not issue is an invalid request, not an empty page.
        let answer = harness
            .list_subscriptions(&client_id, "?nextToken=not-a-token")
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 for a token this service never issued, got {} with \
             body {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            answer.text()
        );
    }
}

/// The `(topicFilter, qos)` pairs one page reports.
fn subscriptions_of(case: &str, answer: &Answer) -> Vec<(String, i64)> {
    answer
        .json()
        .get("subscriptions")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|entry| {
            let filter = entry
                .get("topicFilter")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a summary with no topicFilter: {entry}"))
                .to_string();
            let qos = entry.get("qos").and_then(Value::as_i64).unwrap_or(0);
            (filter, qos)
        })
        .collect()
}

#[tokio::test]
async fn send_direct_message_delivers_into_the_session() {
    let harness = Harness::new();
    harness.connect(connected_session("online-client"));
    harness.connect(offline_session("queued-client", 10_000));

    // A message to a CONNECTED client goes over its connection and queues nothing; a
    // message to an offline client is queued on its persistent session, verbatim.
    for (case, client_id, queues) in [
        ("a connected client", "online-client", false),
        ("an offline persistent session", "queued-client", true),
    ] {
        let payload = format!("payload for {case}").into_bytes();
        let answer = harness
            .send_direct_message(client_id, "orders/new", false, payload.clone())
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200, got {} with body {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.json().get("message").and_then(Value::as_str),
            Some("OK"),
            "case {case:?}: expected the broker's acknowledgement, got {}",
            answer.text()
        );
        assert!(
            answer
                .json()
                .get("traceId")
                .and_then(Value::as_str)
                .is_some_and(|id| id.len() == 36),
            "case {case:?}: expected a correlation id in the envelope, got {}",
            answer.text()
        );

        let queue = harness
            .stored_session(client_id)
            .expect("the session is still held")
            .queued_messages;
        if !queues {
            assert!(
                queue.is_empty(),
                "case {case:?}: a message to a connected client queues nothing, got {} entries",
                queue.len()
            );
            continue;
        }
        assert_eq!(
            queue.len(),
            1,
            "case {case:?}: expected the message to be queued, got {} entries",
            queue.len()
        );
        let queued = &queue[0];
        assert_eq!(
            (queued.topic.as_str(), queued.payload.as_slice(), queued.qos),
            ("orders/new", payload.as_slice(), 0),
            "case {case:?}: expected the message queued verbatim, got {queued:?}"
        );
    }

    // The payload quota bounds a direct message as it bounds a publish: the bound itself
    // is delivered, one byte more is an entity too large, and a refused send queues
    // nothing.
    for (case, size) in [
        ("exactly at the quota", MAX_PUBLISH_PAYLOAD_BYTES),
        ("one byte past the quota", MAX_PUBLISH_PAYLOAD_BYTES + 1),
        ("far past the quota", 4 * MAX_PUBLISH_PAYLOAD_BYTES),
    ] {
        let before = harness
            .stored_session("queued-client")
            .expect("the session is still held")
            .queued_messages
            .len();
        let answer = harness
            .send_direct_message("queued-client", "orders/big", false, vec![3u8; size])
            .await;
        let after = harness
            .stored_session("queued-client")
            .expect("the session is still held")
            .queued_messages
            .len();
        if size <= MAX_PUBLISH_PAYLOAD_BYTES {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected {size} bytes to be delivered, got {} with body {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: expected the delivered message to be queued"
            );
            continue;
        }
        assert_eq!(
            answer.status,
            413,
            "case {case:?}: expected 413 for {size} bytes, got {} with body {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "RequestEntityTooLargeException",
            "case {case:?}: expected RequestEntityTooLargeException, got {}",
            answer.text()
        );
        assert_eq!(
            answer.message(),
            payload_too_large(MAX_PUBLISH_PAYLOAD_BYTES),
            "case {case:?}: expected the refusal naming the ceiling, got {:?}",
            answer.message()
        );
        assert!(
            !answer.message().contains(&size.to_string()),
            "case {case:?}: expected the refusal to carry nothing of the request's {size} \
             bytes, got {:?}",
            answer.message()
        );
        assert_eq!(
            after, before,
            "case {case:?}: a refused send must queue nothing"
        );
    }

    // The topic of an outbound message is a topic NAME, so it is weighed against the
    // broker's topic-name rules, each with its own message.
    let deep = (0..=MAX_TOPIC_PARTS)
        .map(|index| format!("p{index}"))
        .collect::<Vec<String>>()
        .join("/");
    for (case, topic, expected) in [
        (
            "a topic past the depth ceiling",
            deep,
            too_many_topic_parts(MAX_TOPIC_PARTS),
        ),
        (
            "a topic past the byte ceiling",
            "z".repeat(MAX_TOPIC_BYTES + 1),
            too_many_topic_bytes(MAX_TOPIC_BYTES),
        ),
        (
            "a topic carrying a wildcard",
            "orders/#".to_string(),
            INVALID_TOPIC_FILTER.to_string(),
        ),
    ] {
        let before = harness
            .stored_session("queued-client")
            .expect("the session is still held")
            .queued_messages
            .len();
        let answer = harness
            .send_direct_message("queued-client", &topic, false, b"x".to_vec())
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} with body {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            answer.text()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected the refusal naming its own limit, got {:?}",
            answer.message()
        );
        let after = harness
            .stored_session("queued-client")
            .expect("the session is still held")
            .queued_messages
            .len();
        assert_eq!(
            after, before,
            "case {case:?}: a refused send must queue nothing"
        );
    }
}

#[tokio::test]
async fn a_send_asking_for_delivery_confirmation_is_not_claimed_to_be_confirmed() {
    let harness = Harness::new();
    harness.connect(offline_session("confirm-client", 1_000));

    let answer = harness
        .send_direct_message("confirm-client", "orders/confirmed", true, b"needs ack".to_vec())
        .await;
    assert_eq!(
        answer.status,
        504,
        "expected the modelled gateway timeout for a confirmation nothing can send, got {} \
         with body {}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.error_code(),
        "GatewayTimeoutException",
        "expected GatewayTimeoutException, got {}",
        answer.text()
    );
    assert_eq!(
        answer.message(),
        NO_DELIVERY_CONFIRMATION,
        "expected the service's text for an unconfirmed delivery, got {:?}",
        answer.message()
    );

    // The message was still delivered — at QoS 1, which is what asking for confirmation
    // means — so the session holds it.
    let queue = harness
        .stored_session("confirm-client")
        .expect("the session is still held")
        .queued_messages;
    assert_eq!(
        queue.len(),
        1,
        "expected the unconfirmed message to have been delivered, got {} entries",
        queue.len()
    );
    assert_eq!(
        queue[0].qos, 1,
        "expected a confirmed delivery to use QoS 1, got {}",
        queue[0].qos
    );
}

#[tokio::test]
async fn a_sessions_offline_queue_is_bounded_and_keeps_the_newest() {
    let harness = Harness::new();
    harness.connect(offline_session("bounded-client", 1_000));

    let overflow = 5;
    for index in 0..MAX_QUEUED_SESSION_MESSAGES + overflow {
        let answer = harness
            .send_direct_message(
                "bounded-client",
                "orders/bulk",
                false,
                format!("message {index}").into_bytes(),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "message {index}: expected 200, got {} with body {}",
            answer.status,
            answer.text()
        );
    }

    let queue = harness
        .stored_session("bounded-client")
        .expect("the session is still held")
        .queued_messages;
    assert_eq!(
        queue.len(),
        MAX_QUEUED_SESSION_MESSAGES,
        "expected the queue to stop at its bound, got {} entries",
        queue.len()
    );
    let oldest = String::from_utf8_lossy(&queue[0].payload).to_string();
    assert_eq!(
        oldest,
        format!("message {overflow}"),
        "expected the oldest deliveries to have been dropped, got {oldest:?}"
    );
    let newest: DirectMessage = queue[queue.len() - 1].clone();
    assert_eq!(
        String::from_utf8_lossy(&newest.payload),
        format!("message {}", MAX_QUEUED_SESSION_MESSAGES + overflow - 1),
        "expected the newest delivery to be kept, got {newest:?}"
    );
}

#[tokio::test]
async fn delete_connection_closes_the_connection_and_optionally_the_session_state() {
    let harness = Harness::new();

    // Rows differing in what the caller asked for and in how the client connected: the
    // session state goes when the caller asks for it, and for a clean session there is no
    // persistent state to keep, so the parameter is ignored for one.
    for (case, clean_session_request, clean_session_client, keeps_state) in [
        ("a persistent session, state preserved", false, false, true),
        ("a persistent session, state discarded", true, false, false),
        ("a clean session, parameter ignored", false, true, false),
    ] {
        let client_id = format!("close-{clean_session_request}-{clean_session_client}");
        let mut session = connected_session(&client_id);
        session.clean_session = clean_session_client;
        session
            .subscriptions
            .insert("orders/+".to_string(), 1);
        session
            .subscriptions
            .insert("telemetry/#".to_string(), 0);
        harness.connect(session);

        let closed = harness
            .delete_connection(&client_id, clean_session_request, false)
            .await;
        assert_eq!(
            closed.status,
            200,
            "case {case:?}: expected 200, got {} with body {}",
            closed.status,
            closed.text()
        );

        // The connection is closed, and the record says the SERVICE closed it.
        let reported = harness.get_connection(&client_id, false).await;
        assert_eq!(
            reported.status,
            200,
            "case {case:?}: expected the disconnected client to still be reported, got {} with \
             body {}",
            reported.status,
            reported.text()
        );
        let body = reported.json();
        assert!(
            !body.get("connected").and_then(Value::as_bool).unwrap_or(false),
            "case {case:?}: expected connected=false after the close, got {}",
            reported.text()
        );
        assert_eq!(
            body.get("disconnectReason").and_then(Value::as_str),
            Some(SERVER_INITIATED_DISCONNECT),
            "case {case:?}: expected the service-initiated disconnect reason, got {}",
            reported.text()
        );
        assert!(
            body.get("disconnectedSince")
                .and_then(Value::as_i64)
                .is_some_and(|instant| instant > 0),
            "case {case:?}: expected the instant of the disconnect, got {}",
            reported.text()
        );

        // The session state is kept only where the rule says it is.
        let listed = harness.list_subscriptions(&client_id, "").await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected an offline session to still be listable, got {} with body {}",
            listed.status,
            listed.text()
        );
        let filters = subscriptions_of(case, &listed);
        if keeps_state {
            assert_eq!(
                filters.len(),
                2,
                "case {case:?}: expected the persistent session's subscriptions to be kept, got \
                 {filters:?}"
            );
        } else {
            assert!(
                filters.is_empty(),
                "case {case:?}: expected the session state to be discarded, got {filters:?}"
            );
        }

        // There is no connection left to close, so closing it again is not found.
        let again = harness
            .delete_connection(&client_id, clean_session_request, true)
            .await;
        assert_eq!(
            again.status,
            404,
            "case {case:?}: expected a second close to be 404, got {} with body {}",
            again.status,
            again.text()
        );
        assert_eq!(
            again.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            again.text()
        );
    }
}

#[tokio::test]
async fn a_disconnected_sessions_record_is_reported_for_the_documented_window() {
    let harness = Harness::new();

    // The window is the one `GetConnectionResponse$disconnectedSince` documents, so the
    // rows are its published bound and one past it. The "inside" row sits a full second
    // inside that bound rather than one millisecond: the session's age is measured from
    // the wall clock when the row is built, so a margin smaller than this test's own
    // run time would expire between `connect` and `get_connection` and make the row a
    // race rather than an assertion about the bound (the bound itself is pinned by the
    // `== window` row below, which must NOT be reported).
    for (case, ago_millis, reported) in [
        ("disconnected a moment ago", 1_000_i64, true),
        (
            "disconnected just inside the window",
            DISCONNECT_INFORMATION_RETENTION_MILLIS - 1_000,
            true,
        ),
        ("disconnected exactly at the window", DISCONNECT_INFORMATION_RETENTION_MILLIS, false),
        ("disconnected well past the window", 4 * DISCONNECT_INFORMATION_RETENTION_MILLIS, false),
    ] {
        let client_id = format!("retained-{ago_millis}");
        harness.connect(offline_session(&client_id, ago_millis));
        let answer = harness.get_connection(&client_id, true).await;
        if reported {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the record to still be reported, got {} with body {}",
                answer.status,
                answer.text()
            );
            assert!(
                harness.stored_session(&client_id).is_some(),
                "case {case:?}: a reported record must still be held"
            );
            continue;
        }
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected the record to be past the reporting window, got {} with \
             body {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.text()
        );
        // The release is what the clock already owed, so the record is gone from the
        // table too — not merely hidden from this one answer.
        assert!(
            harness.stored_session(&client_id).is_none(),
            "case {case:?}: expected the retired record to have been released"
        );
    }

    // A session whose client is still CONNECTED is reported however long ago it
    // connected: the window is the disconnect information's, not the session's.
    let mut old = connected_session("long-lived-client");
    old.connected_since_millis = now_millis() - 100 * DISCONNECT_INFORMATION_RETENTION_MILLIS;
    harness.connect(old);
    let answer = harness.get_connection("long-lived-client", false).await;
    assert_eq!(
        answer.status,
        200,
        "expected a long-connected client to be reported, got {} with body {}",
        answer.status,
        answer.text()
    );
    assert!(
        harness.stored_session("long-lived-client").is_some(),
        "expected the connected session to still be held, with {} session(s) in the table",
        harness.session_count()
    );
}
