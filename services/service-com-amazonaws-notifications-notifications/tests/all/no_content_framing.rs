// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service FRAMES an answer that carries no
//! content, driven over HTTP through the same multi-service router the emulator
//! boots in production, and once over a real socket.
//!
//! The rule under test: a successful call whose modelled output declares no
//! members answers `204 No Content` with no body bytes AND states the length
//! that body has — `content-length: 0` — so a client or intermediary that frames
//! a bodyless message by its stated length, rather than by the status alone, is
//! not left waiting for a body or determining the length from the connection
//! closing. An answer that DOES carry content keeps the body its renderer wrote
//! and the framing that body has, so the rule is about the answer's content and
//! not about which operation produced it.
//!
//! Which operations are in the no-content half is read off the model, never off
//! an operation's name or method:
//! `smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse",model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`
//! gives both structures an empty member set, and they are the only outputs of
//! this service that declare none.
//!
//! The rows differ along what the rule generalises over: both operations, both
//! HTTP methods they are bound to (`POST`, `DELETE`), what the call did to the
//! stored tag set (a write that adds keys, a write that changes nothing, a
//! removal of keys the resource holds and of keys it never held), how many keys
//! the call carries (none, one, many) and whether the same call is repeated.
//! The STATUS and the empty body of these answers are pinned by
//! `tests/all/tag_mutation_status.rs`; this suite is about the framing header
//! they carry, including what survives onto a real socket — where the HTTP/1.1
//! writer the vendored listener is built on drops `content-length` from a `204`,
//! so the socket case pins the status and the absent body and the in-process
//! cases pin the length the service states.

use serde_json::{json, Map, Value};

use crate::harness::{encode_path_label, Emulator, TAG_MUTATION_SUCCESS_STATUS};

/// The length an answer with no body states: it has no bytes.
const NO_CONTENT_LENGTH: &str = "0";

/// The status an answer that reports a document carries.
const DOCUMENT_STATUS: u16 = 200;

/// One row of the framing table: a tag mutation, in the shape its own binding
/// takes.
struct FramingCase {
    case: &'static str,
    /// The tags to write first, so a removal has something to remove.
    seeded: Vec<(String, String)>,
    /// The operation under test, as method and the keys it carries.
    mutation: Mutation,
}

/// Which mutation a row drives, and with what.
enum Mutation {
    /// `TagResource`: `POST /tags/{arn}` with a tag map in the body.
    Write(Vec<(String, String)>),
    /// `UntagResource`: `DELETE /tags/{arn}?tagKeys=…`, one repetition of the
    /// query parameter per key.
    Remove(Vec<String>),
}

/// The table.
fn framing_cases() -> Vec<FramingCase> {
    let long_key = "k".repeat(128);
    vec![
        FramingCase {
            case: "a write that adds one key",
            seeded: Vec::new(),
            mutation: Mutation::Write(vec![("env".to_string(), "prod".to_string())]),
        },
        FramingCase {
            case: "a write of the empty tag map, which changes nothing",
            seeded: vec![("env".to_string(), "prod".to_string())],
            mutation: Mutation::Write(Vec::new()),
        },
        FramingCase {
            case: "a write that overwrites a stored key",
            seeded: vec![("env".to_string(), "prod".to_string())],
            mutation: Mutation::Write(vec![("env".to_string(), "staging".to_string())]),
        },
        FramingCase {
            case: "a write of many keys at once",
            seeded: Vec::new(),
            mutation: Mutation::Write(
                (0..40)
                    .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
                    .collect(),
            ),
        },
        FramingCase {
            case: "a write carrying the longest modelled key",
            seeded: Vec::new(),
            mutation: Mutation::Write(vec![(long_key.clone(), "v".repeat(256))]),
        },
        FramingCase {
            case: "a removal of a stored key",
            seeded: vec![("env".to_string(), "prod".to_string())],
            mutation: Mutation::Remove(vec!["env".to_string()]),
        },
        FramingCase {
            case: "a removal of a key that was never stored",
            seeded: Vec::new(),
            mutation: Mutation::Remove(vec!["never-stored".to_string()]),
        },
        FramingCase {
            case: "a removal of several keys, some stored and some not",
            seeded: vec![
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "platform".to_string()),
            ],
            mutation: Mutation::Remove(vec![
                "env".to_string(),
                "team".to_string(),
                "absent".to_string(),
            ]),
        },
        FramingCase {
            case: "a removal naming the longest modelled key",
            seeded: vec![(long_key.clone(), String::new())],
            mutation: Mutation::Remove(vec![long_key]),
        },
    ]
}

impl Emulator {
    /// Create a configuration and return its arn, failing the test on anything
    /// other than the modelled 201.
    async fn create_configuration_for_framing(&self, name: &str) -> String {
        let body = json!({ "name": name, "description": "a configuration under test" });
        let (status, response) = self
            .send("POST", "/notification-configurations", Some(body))
            .await;
        assert_eq!(
            status, 201,
            "case {name}: create answers the status its `@http` code states, got {status} \
             {response:?}"
        );
        response["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: create must answer an arn, got {response:?}"))
            .to_string()
    }

    /// Drive one mutation and return the whole HTTP answer.
    async fn mutate(
        &self,
        arn: &str,
        mutation: &Mutation,
    ) -> ::http::Response<bytes::Bytes> {
        match mutation {
            Mutation::Write(tags) => {
                let uri = format!("/tags/{}", encode_path_label(arn));
                let mut map = Map::new();
                for (key, value) in tags {
                    map.insert(key.clone(), Value::String(value.clone()));
                }
                self.send_http("POST", &uri, Some(json!({ "tags": Value::Object(map) })))
                    .await
            }
            Mutation::Remove(keys) => {
                let query: Vec<String> = keys
                    .iter()
                    .map(|key| format!("tagKeys={}", encode_path_label(key)))
                    .collect();
                let uri = format!("/tags/{}?{}", encode_path_label(arn), query.join("&"));
                self.send_http("DELETE", &uri, None).await
            }
        }
    }
}

/// The `content-length` an answer stated, as text, or `None` when it stated none.
fn stated_length(response: &::http::Response<bytes::Bytes>) -> Option<String> {
    response
        .headers()
        .get("content-length")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
}

/// RULE: every answer that carries no content is framed the same way — `204`, no
/// body bytes, and `content-length: 0` stating the length of that nothing.
#[tokio::test]
async fn an_answer_with_no_content_states_the_length_it_has() {
    for (index, row) in framing_cases().into_iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let arn = emulator
            .create_configuration_for_framing(&format!("framing-case-{index}"))
            .await;
        if !row.seeded.is_empty() {
            let seeded = emulator
                .mutate(&arn, &Mutation::Write(row.seeded.clone()))
                .await;
            assert_eq!(
                seeded.status().as_u16(),
                TAG_MUTATION_SUCCESS_STATUS,
                "case {case:?}: seeding the tags must answer {TAG_MUTATION_SUCCESS_STATUS}, got {}",
                seeded.status()
            );
        }

        // The same call twice: the framing does not depend on whether the
        // mutation changed anything.
        for attempt in 0..2 {
            let answer = emulator.mutate(&arn, &row.mutation).await;
            let framing = stated_length(&answer);
            assert_eq!(
                (
                    answer.status().as_u16(),
                    answer.body().is_empty(),
                    framing.as_deref()
                ),
                (TAG_MUTATION_SUCCESS_STATUS, true, Some(NO_CONTENT_LENGTH)),
                "case {case:?} attempt {attempt}: expected \
                 {TAG_MUTATION_SUCCESS_STATUS} with no body and content-length \
                 {NO_CONTENT_LENGTH}, got {} body {:?} content-length {framing:?}",
                answer.status(),
                String::from_utf8_lossy(answer.body())
            );
        }
    }
}

/// RULE: the framing follows the answer's CONTENT, so an answer that reports a
/// document keeps its body and is not emptied or framed as if it had none.
#[tokio::test]
async fn an_answer_that_reports_a_document_keeps_its_body() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_configuration_for_framing("framing-contrast")
        .await;
    let written = emulator
        .mutate(&arn, &Mutation::Write(vec![("env".to_string(), "prod".to_string())]))
        .await;
    assert_eq!(
        written.status().as_u16(),
        TAG_MUTATION_SUCCESS_STATUS,
        "case seeding: expected {TAG_MUTATION_SUCCESS_STATUS}, got {}",
        written.status()
    );

    // Rows: the reader of the tag set the mutations above changed, a read of the
    // resource itself, a listing, and the published managed catalog — every kind
    // of answer this service gives that carries members.
    let cases: Vec<(&str, String)> = vec![
        ("ListTagsForResource", format!("/tags/{}", encode_path_label(&arn))),
        (
            "GetNotificationConfiguration",
            format!("/notification-configurations/{}", encode_path_label(&arn)),
        ),
        (
            "ListNotificationConfigurations",
            "/notification-configurations".to_string(),
        ),
        (
            "ListManagedNotificationConfigurations",
            "/managed-notification-configurations".to_string(),
        ),
    ];

    for (case, uri) in cases {
        let answer = emulator.send_http("GET", &uri, None).await;
        assert_eq!(
            answer.status().as_u16(),
            DOCUMENT_STATUS,
            "case {case}: expected {DOCUMENT_STATUS}, got {} {:?}",
            answer.status(),
            String::from_utf8_lossy(answer.body())
        );
        assert!(
            !answer.body().is_empty(),
            "case {case}: an answer carrying members must carry a body, got none"
        );
        let framing = stated_length(&answer);
        assert_ne!(
            framing.as_deref(),
            Some(NO_CONTENT_LENGTH),
            "case {case}: an answer with a body must not be framed as empty, got {framing:?}"
        );
    }
}

/// RULE, on a real socket: the same no-content answer reaches a client with the
/// status and no body, and with no chunked framing in place of a stated length.
///
/// The stated length does NOT survive this hop: the HTTP/1.1 writer the vendored
/// listener is built on drops `content-length` from a `204` (RFC 9110 §8.6
/// forbids it there), so this case pins what the socket really delivers, while
/// [`an_answer_with_no_content_states_the_length_it_has`] pins the length the
/// service itself states — which the in-process front door, the inter-service
/// callback path and the SDK connector all serve unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn the_no_content_answer_reaches_a_socket_client_framed() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_configuration_for_framing("framing-socket")
        .await;

    let (endpoint, _served) =
        emulator_server::serve_on_ephemeral_port(emulator.into_router()).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    // Rows: both mutations, each on its own method and binding.
    let cases: Vec<(&str, &str, String, Option<String>)> = vec![
        (
            "TagResource",
            "POST",
            format!("/tags/{}", encode_path_label(&arn)),
            Some(json!({ "tags": { "env": "prod" } }).to_string()),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{}?tagKeys=env", encode_path_label(&arn)),
            None,
        ),
    ];

    for (case, method, target, payload) in cases {
        let answered = tokio::task::spawn_blocking(move || {
            raw_request(port, method, &target, payload.as_deref())
        })
        .await
        .expect("the socket exchange runs to completion");

        assert!(
            answered.starts_with(&format!("HTTP/1.1 {TAG_MUTATION_SUCCESS_STATUS}")),
            "case {case} on a socket: expected a {TAG_MUTATION_SUCCESS_STATUS}, got {answered:?}"
        );
        let (head, body) = answered
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("case {case}: a response head and body, got {answered:?}"));
        assert!(
            body.is_empty(),
            "case {case} on a socket: expected no body, got {body:?}"
        );
        let lowered = head.to_ascii_lowercase();
        assert!(
            !lowered.contains("transfer-encoding"),
            "case {case} on a socket: expected the answer not to be chunked, got {head:?}"
        );
    }
}

/// One request written straight onto a socket, with everything the server
/// answered read back as text.
///
/// `connection: close` so the server closes the socket after answering and the
/// read below ends on EOF rather than on a parse of the very framing this case
/// is about.
fn raw_request(port: u16, method: &str, target: &str, payload: Option<&str>) -> String {
    use std::io::{Read, Write};

    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let body = payload.unwrap_or_default();
    let request = format!(
        "{method} {target} HTTP/1.1\r\n\
         host: notifications.us-east-1.amazonaws.com\r\n\
         authorization: AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/us-east-1/notifications/aws4_request, \
         SignedHeaders=host, Signature=0\r\n\
         content-type: application/json\r\n\
         content-length: {}\r\n\
         connection: close\r\n\
         \r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("write the request");
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .expect("read the whole answer");
    String::from_utf8_lossy(&answer).to_string()
}
